//! Program equivalence over the symbolic mirror — Layer 1 of the proof
//! engine: `equiv(p1, p2, …)` asks "do these two programs behave
//! identically for EVERY initial register state and EVERY TX input
//! stream (contents AND occupancy) over a bounded horizon?".
//!
//! ## Verdict semantics (three-valued, shard-style)
//!
//! * [`EquivVerdict::Proven`] — the solver found the divergence formula
//!   UNSAT. Trusted modulo mirror fidelity (same trust rail as every
//!   UNSAT in this module: run `differential_fuzz` before believing a
//!   batch of these).
//! * [`EquivVerdict::Refuted`] — the solver produced a model, and the
//!   model was REPLAYED on the real emulator twin
//!   ([`crate::narrow::step`]) which confirmed the divergence. A
//!   `Refuted` never rests on the mirror alone.
//! * [`EquivVerdict::Unknown`] — out-of-subset word, unsupported
//!   config, or solver timeout. NEVER silently degraded into `Proven`.
//! * [`EquivVerdict::MirrorDivergence`] — the mirror's counterexample
//!   does NOT diverge on the real emulator (or a program's mirror
//!   trace disagrees with its emulator trace on the counterexample).
//!   That is a fidelity bug and is worth more than the query itself;
//!   it is reported loudly and never folded into the other verdicts.
//!
//! ## The quantified entry state
//!
//! Both programs run from ONE shared symbolic initial state: x, y,
//! osr, isr, osr_count (constrained to the machine invariant `<= 32`),
//! the pin value latch and the pin direction latch are free; pc = 0,
//! delay = 0, not stalled, FIFO cursor at the stream head are the
//! concrete "entry" contract (a program is entered at its first slot
//! with the machine at rest). The TX FIFO is `horizon` fresh 32-bit
//! words plus a free occupancy 0..=horizon shared by both programs —
//! ∀-inputs by default, because equivalence over a single seeded FIFO
//! has burned this project before, and PULL block/noblock only differ
//! on the occupancy axis.
//!
//! ## Tiers
//!
//! [`Tier`] deliberately matches the champion-fingerprint tiers in
//! `tests/narrow_engine.rs`: `TraceOnly` compares per-cycle (level,
//! oe, stalled) — the loose tier's (trace, oob) with the stall flag as
//! the reachable stall/OOB analog (out-of-footprint execution is
//! unreachable in the mirror subset: pc starts at 0, `legal_word`
//! confines JMP targets to the footprint, and wrap = (0, len-1)).
//! `TraceAndFinalState` adds full final-`SymState` field equality (the
//! strict tier's final-NState axis).

#![allow(deprecated)] // `_eq`, as in the parent module

use z3::ast::{Bool, BV};

use super::{
    bt, bvu, legal_word, supported_config, unroll_interned_from, SymFifo, SymProgram, SymState,
    SymTrace,
};
use crate::narrow::{self, NCfg, NState, Stall};
use crate::program::{Config, Program};

/// Solver budget per query (ms); hitting it yields `Unknown`, never a
/// silent verdict.
const TIMEOUT_MS: u32 = 60_000;

/// An entry-state constraint for conditional equivalence, as a small
/// representable enum (a conjunction is a slice of these) so rules can
/// later be serialized into a rule library. Each atom pins one field
/// of the shared symbolic initial state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatePred {
    XEq(u32),
    YEq(u32),
    OsrEq(u32),
    IsrEq(u32),
    /// 0..=32 (the machine invariant is asserted separately).
    OsrCountEq(u8),
    /// The pin VALUE latch bit.
    PinEq(bool),
    /// The pin DIRECTION latch bit.
    OeEq(bool),
}

impl StatePred {
    fn assert_on(&self, solver: &z3::Solver, st: &SymState) {
        let c = match *self {
            StatePred::XEq(v) => st.x._eq(&bvu(v as u64, 32)),
            StatePred::YEq(v) => st.y._eq(&bvu(v as u64, 32)),
            StatePred::OsrEq(v) => st.osr._eq(&bvu(v as u64, 32)),
            StatePred::IsrEq(v) => st.isr._eq(&bvu(v as u64, 32)),
            StatePred::OsrCountEq(v) => st.osr_cnt._eq(&bvu(v as u64, 8)),
            StatePred::PinEq(v) => st.pin.iff(&bt(v)),
            StatePred::OeEq(v) => st.oe.iff(&bt(v)),
        };
        solver.assert(&c);
    }
}

/// Which observables must match. See the module docs for the mapping
/// onto the loose/strict champion-fingerprint tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Per-cycle (level, oe, stalled).
    TraceOnly,
    /// `TraceOnly` plus full final-state field equality.
    TraceAndFinalState,
}

/// A model of the divergence formula, replay-confirmed on the real
/// emulator. Fields are the shared initial state plus the shared TX
/// stream (exactly `occupancy` words — the free-FIFO model truncated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counterexample {
    pub x: u32,
    pub y: u32,
    pub osr: u32,
    pub isr: u32,
    pub osr_count: u8,
    /// Initial pin VALUE latch bit.
    pub pin: bool,
    /// Initial pin DIRECTION latch bit.
    pub oe: bool,
    pub fifo: Vec<u32>,
    /// First cycle where the replayed (level, oe, stalled) triples
    /// differ; `None` = the divergence is final-state-only (only
    /// possible under [`Tier::TraceAndFinalState`]).
    pub diverge_cycle: Option<usize>,
}

/// Three-valued equivalence verdict plus the fidelity-bug channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EquivVerdict {
    /// Divergence UNSAT — equivalent over the whole quantified space
    /// (modulo mirror fidelity).
    Proven,
    /// Real, replay-confirmed divergence.
    Refuted(Counterexample),
    /// Out-of-subset word / unsupported config / solver timeout — no
    /// claim either way.
    Unknown(String),
    /// Mirror and real emulator disagree on the counterexample. A
    /// fidelity bug: report it, do not proceed as if it were a verdict.
    MirrorDivergence(String),
}

/// Ground (constant-folded) check of [`legal_word`] on a concrete word.
pub fn word_in_subset(w: u16, len: u8) -> bool {
    use z3::ast::Ast;
    legal_word(&bvu(w as u64, 16), len)
        .simplify()
        .as_bool()
        .expect("legal_word on a constant word must fold to a constant")
}

/// Bounded program equivalence under `cfg` (see the module docs for
/// exactly what is quantified). `pre` is a conjunction of entry-state
/// constraints (empty = unconditional). Both programs use wrap =
/// (0, len-1); lengths may differ.
pub fn equiv(
    p1: &[u16],
    p2: &[u16],
    cfg: &Config,
    horizon: usize,
    pre: &[StatePred],
    tier: Tier,
) -> EquivVerdict {
    if let Err(e) = supported_config(cfg) {
        return EquivVerdict::Unknown(format!("unsupported config: {e}"));
    }
    assert!((1..=254).contains(&horizon), "horizon out of range");
    for (label, p) in [("p1", p1), ("p2", p2)] {
        if !(1..=32).contains(&p.len()) {
            return EquivVerdict::Unknown(format!("{label} length {} out of 1..=32", p.len()));
        }
        for &w in p {
            if !word_in_subset(w, p.len() as u8) {
                return EquivVerdict::Unknown(format!(
                    "{label} word {w:#06x} outside the modeled subset"
                ));
            }
        }
    }

    let solver = z3::Solver::new_for_logic("QF_BV").unwrap_or_else(z3::Solver::new);
    let mut params = z3::Params::new();
    params.set_u32("timeout", TIMEOUT_MS);
    solver.set_params(&params);

    // Shared symbolic entry state + shared symbolic TX stream.
    let init = SymState {
        pc: bvu(0, 5),
        x: BV::new_const("eqv_x", 32),
        y: BV::new_const("eqv_y", 32),
        osr: BV::new_const("eqv_osr", 32),
        isr: BV::new_const("eqv_isr", 32),
        osr_cnt: BV::new_const("eqv_cnt", 8),
        delay: bvu(0, 5),
        stalled: bt(false),
        fifo_next: bvu(0, 8),
        pin: Bool::new_const("eqv_pin"),
        oe: Bool::new_const("eqv_oe"),
    };
    solver.assert(&init.osr_cnt.bvule(&bvu(32, 8))); // machine invariant
    let (fifo, fifo_side) = SymFifo::free(horizon, "eqv_fifo");
    solver.assert(&fifo_side);
    for p in pre {
        p.assert_on(&solver, &init);
    }

    let s1 = SymProgram::from_words(p1, &cfg.side);
    let s2 = SymProgram::from_words(p2, &cfg.side);
    let t1 = unroll_interned_from(&solver, &s1, cfg, &init, &fifo, horizon, 0);
    let t2 = unroll_interned_from(&solver, &s2, cfg, &init, &fifo, horizon, 1);

    // Divergence = OR over per-cycle observable inequality (per tier).
    let mut div: Vec<Bool> = Vec::new();
    for t in 0..horizon {
        div.push((&t1.levels[t]).xor(&t2.levels[t]));
        div.push((&t1.oes[t]).xor(&t2.oes[t]));
        div.push((&t1.states[t + 1].stalled).xor(&t2.states[t + 1].stalled));
    }
    if tier == Tier::TraceAndFinalState {
        let a = &t1.states[horizon];
        let b = &t2.states[horizon];
        div.push(!a.pc._eq(&b.pc));
        div.push(!a.x._eq(&b.x));
        div.push(!a.y._eq(&b.y));
        div.push(!a.osr._eq(&b.osr));
        div.push(!a.isr._eq(&b.isr));
        div.push(!a.osr_cnt._eq(&b.osr_cnt));
        div.push(!a.delay._eq(&b.delay));
        div.push(!a.fifo_next._eq(&b.fifo_next));
        div.push((&a.pin).xor(&b.pin));
        div.push((&a.oe).xor(&b.oe));
        // a.stalled/b.stalled already compared per-cycle above.
    }
    let any = div.into_iter().reduce(|a, b| a | b).expect("horizon >= 1");
    solver.assert(&any);

    match solver.check() {
        z3::SatResult::Unsat => EquivVerdict::Proven,
        z3::SatResult::Unknown => EquivVerdict::Unknown(format!(
            "solver: {}",
            solver.get_reason_unknown().unwrap_or_else(|| "unknown".into())
        )),
        z3::SatResult::Sat => {
            let model = solver.get_model().expect("sat without model");
            let ev32 = |bv: &BV| model.eval(bv, true).unwrap().as_u64().unwrap() as u32;
            let evb = |b: &Bool| model.eval(b, true).unwrap().as_bool().unwrap();
            let occupancy = (ev32(&fifo.len) as usize).min(fifo.words.len());
            let cex = Counterexample {
                x: ev32(&init.x),
                y: ev32(&init.y),
                osr: ev32(&init.osr),
                isr: ev32(&init.isr),
                osr_count: ev32(&init.osr_cnt) as u8,
                pin: evb(&init.pin),
                oe: evb(&init.oe),
                fifo: fifo.words[..occupancy].iter().map(|w| ev32(w)).collect(),
                diverge_cycle: None,
            };
            confirm_on_emulator(p1, p2, cfg, horizon, tier, cex, &model, &t1, &t2)
        }
    }
}

/// One replayed program: per-cycle (level, oe, stalled) plus the final
/// state and how many TX words were consumed.
struct Replay {
    trace: Vec<(bool, bool, bool)>,
    fin: NState,
    consumed: usize,
}

/// Run `words` on the REAL emulator twin from the counterexample's
/// initial state, streaming the counterexample FIFO (refill before
/// every cycle — the semantics `SymFifo` models).
fn replay_narrow(words: &[u16], cfg: &Config, cex: &Counterexample, horizon: usize) -> Replay {
    let mut p = Program::empty(*cfg);
    p.wrap_bottom = 0;
    p.wrap_top = (words.len() - 1) as u8;
    let mut ncfg = NCfg::from_program(&p, 0);
    for (i, &w) in words.iter().enumerate() {
        ncfg.code[i] = w;
    }
    let mut st = NState::new(&ncfg);
    st.x = cex.x;
    st.y = cex.y;
    st.osr = cex.osr;
    st.isr = cex.isr;
    st.osr_count = cex.osr_count;
    // The mirror's pin/oe are bit 0 of the block latches; all pin groups
    // are base 0 / count 1 under `supported_config`, so only bit 0 is
    // ever read or written.
    st.out_latch = (st.out_latch & !1) | cex.pin as u32;
    st.dir_latch = cex.oe as u32;

    let mut next = 0usize;
    let mut trace = Vec::with_capacity(horizon);
    for _ in 0..horizon {
        while next < cex.fifo.len() && !st.tx.is_full() {
            st.tx.push(cex.fifo[next]);
            next += 1;
        }
        let gpio = narrow::compose(&st, 0, 0);
        narrow::step(&mut st, &ncfg, gpio); // clkdiv is 1: every cycle ticks
        trace.push((
            (st.out_latch & st.dir_latch) & 1 != 0,
            st.dir_latch & 1 != 0,
            st.stall != Stall::None,
        ));
    }
    let consumed = next - st.tx.level() as usize;
    Replay { trace, fin: st, consumed }
}

/// Mirror-vs-emulator crosscheck for ONE program on the counterexample:
/// the model-completed mirror trace and final state must match the
/// replay exactly, field for field. Any mismatch is a fidelity bug.
fn crosscheck_mirror(
    label: &str,
    model: &z3::Model,
    tr: &SymTrace,
    rep: &Replay,
    horizon: usize,
) -> Result<(), String> {
    let ev32 = |bv: &BV| model.eval(bv, true).unwrap().as_u64().unwrap() as u32;
    let evb = |b: &Bool| model.eval(b, true).unwrap().as_bool().unwrap();
    for t in 0..horizon {
        let m = (evb(&tr.levels[t]), evb(&tr.oes[t]), evb(&tr.states[t + 1].stalled));
        if m != rep.trace[t] {
            return Err(format!(
                "{label} cycle {t}: mirror (lvl,oe,stl)={m:?} vs emulator {:?}",
                rep.trace[t]
            ));
        }
    }
    let f = &tr.states[horizon];
    let n = &rep.fin;
    let fields: [(&str, u64, u64); 8] = [
        ("pc", ev32(&f.pc) as u64, n.pc as u64),
        ("x", ev32(&f.x) as u64, n.x as u64),
        ("y", ev32(&f.y) as u64, n.y as u64),
        ("osr", ev32(&f.osr) as u64, n.osr as u64),
        ("isr", ev32(&f.isr) as u64, n.isr as u64),
        ("osr_cnt", ev32(&f.osr_cnt) as u64, n.osr_count as u64),
        ("delay", ev32(&f.delay) as u64, n.delay_count as u64),
        ("fifo_next", ev32(&f.fifo_next) as u64, rep.consumed as u64),
    ];
    for (name, m, e) in fields {
        if m != e {
            return Err(format!("{label} final {name}: mirror {m:#x} vs emulator {e:#x}"));
        }
    }
    let bools: [(&str, bool, bool); 3] = [
        ("pin", evb(&f.pin), n.out_latch & 1 != 0),
        ("oe", evb(&f.oe), n.dir_latch & 1 != 0),
        ("stalled", evb(&f.stalled), n.stall != Stall::None),
    ];
    for (name, m, e) in bools {
        if m != e {
            return Err(format!("{label} final {name}: mirror {m} vs emulator {e}"));
        }
    }
    Ok(())
}

/// The mandatory trust rail: replay the model on the real emulator,
/// crosscheck each program's mirror trajectory against its emulator
/// trajectory, and confirm the two programs actually diverge there
/// under `tier`'s observables. Only then is the verdict `Refuted`.
#[allow(clippy::too_many_arguments)]
fn confirm_on_emulator(
    p1: &[u16],
    p2: &[u16],
    cfg: &Config,
    horizon: usize,
    tier: Tier,
    mut cex: Counterexample,
    model: &z3::Model,
    t1: &SymTrace,
    t2: &SymTrace,
) -> EquivVerdict {
    let r1 = replay_narrow(p1, cfg, &cex, horizon);
    let r2 = replay_narrow(p2, cfg, &cex, horizon);

    for (label, tr, rep) in [("p1", t1, &r1), ("p2", t2, &r2)] {
        if let Err(e) = crosscheck_mirror(label, model, tr, rep, horizon) {
            let msg = format!(
                "MIRROR DIVERGENCE (fidelity bug — run differential_fuzz): {e}; cex={cex:?}"
            );
            eprintln!("[equiv] {msg}");
            return EquivVerdict::MirrorDivergence(msg);
        }
    }

    let trace_div = (0..horizon).find(|&t| r1.trace[t] != r2.trace[t]);
    let diverged = match tier {
        Tier::TraceOnly => trace_div.is_some(),
        Tier::TraceAndFinalState => trace_div.is_some() || r1.fin != r2.fin,
    };
    if !diverged {
        let msg = format!(
            "MIRROR DIVERGENCE (fidelity bug — run differential_fuzz): mirror model \
             diverges but the real emulator agrees under {tier:?}; cex={cex:?}"
        );
        eprintln!("[equiv] {msg}");
        return EquivVerdict::MirrorDivergence(msg);
    }
    cex.diverge_cycle = trace_div;
    EquivVerdict::Refuted(cex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::PinMap;

    /// The supported 1-pin config (same shape as the battery's
    /// `one_pin`): out/set windows both = pin 0, no autopull.
    fn one_pin_cfg() -> Config {
        Config {
            pins: PinMap { out_count: 1, set_count: 1, ..PinMap::default() },
            ..Config::default()
        }
    }

    const MOV_PINS_X: u16 = 0xA001;
    const MOV_PINS_NULL: u16 = 0xA003;

    /// CL1: `mov pins, x` ≡ `mov pins, null` under x == 0, strict tier.
    #[test]
    fn cl1_proven_under_precondition() {
        let v = equiv(
            &[MOV_PINS_X],
            &[MOV_PINS_NULL],
            &one_pin_cfg(),
            6,
            &[StatePred::XEq(0)],
            Tier::TraceAndFinalState,
        );
        assert_eq!(v, EquivVerdict::Proven, "CL1 must prove under x==0");
    }

    /// CL1 without the precondition must REFUTE, and the counterexample
    /// must have bit 0 of x set (the only bit the 1-pin window sees) —
    /// and it is emulator-replayed by construction.
    #[test]
    fn cl1_refuted_without_precondition() {
        let v = equiv(
            &[MOV_PINS_X],
            &[MOV_PINS_NULL],
            &one_pin_cfg(),
            6,
            &[],
            Tier::TraceOnly,
        );
        match v {
            EquivVerdict::Refuted(cex) => {
                assert_eq!(cex.x & 1, 1, "divergence needs x bit 0 set: {cex:?}");
                assert!(cex.diverge_cycle.is_some(), "trace tier ⇒ a diverging cycle");
            }
            other => panic!("expected Refuted, got {other:?}"),
        }
    }

    /// CL2: `out pins, N` ≡ `mov pins, null` for every N in 1..=32
    /// under osr == 0 && osr_count == 32, autopull off, strict tier.
    #[test]
    fn cl2_proven_for_all_bit_counts() {
        let cfg = one_pin_cfg();
        for n in 1..=32u16 {
            let out_pins_n = 0x6000 | (n & 31); // bc field: 0 encodes 32
            let v = equiv(
                &[out_pins_n],
                &[MOV_PINS_NULL],
                &cfg,
                6,
                &[StatePred::OsrEq(0), StatePred::OsrCountEq(32)],
                Tier::TraceAndFinalState,
            );
            assert_eq!(v, EquivVerdict::Proven, "CL2 failed at N={n}");
        }
    }

    /// CL2 without the precondition must refute (osr bit 0 reaches the
    /// pin through `out pins`).
    #[test]
    fn cl2_refuted_without_precondition() {
        let v = equiv(
            &[0x6001],
            &[MOV_PINS_NULL],
            &one_pin_cfg(),
            6,
            &[],
            Tier::TraceOnly,
        );
        assert!(matches!(v, EquivVerdict::Refuted(_)), "expected Refuted, got {v:?}");
    }

    /// Negative control: genuinely different programs refute with a
    /// replay-valid counterexample at the first cycle.
    #[test]
    fn negative_control_set_pins() {
        let v = equiv(&[0xE000], &[0xE001], &one_pin_cfg(), 4, &[], Tier::TraceOnly);
        match v {
            EquivVerdict::Refuted(cex) => {
                assert_eq!(cex.diverge_cycle, Some(0), "SET PINS diverges immediately");
            }
            other => panic!("expected Refuted, got {other:?}"),
        }
    }

    /// The tier gap, demonstrated: `set x, 5` vs `set y, 5` are
    /// trace-equal over the horizon (no pin writes) but final-state
    /// different — loose proves, strict refutes with a state-only
    /// counterexample.
    #[test]
    fn tier_gap_set_x_vs_set_y() {
        let cfg = one_pin_cfg();
        let (sx, sy) = (0xE025u16, 0xE045u16);
        assert_eq!(
            equiv(&[sx], &[sy], &cfg, 6, &[], Tier::TraceOnly),
            EquivVerdict::Proven,
            "loose tier must not see register-only differences"
        );
        match equiv(&[sx], &[sy], &cfg, 6, &[], Tier::TraceAndFinalState) {
            EquivVerdict::Refuted(cex) => {
                assert_eq!(cex.diverge_cycle, None, "divergence is final-state-only");
            }
            other => panic!("expected Refuted, got {other:?}"),
        }
    }

    /// PULL block vs noblock differ ONLY on the FIFO-occupancy axis —
    /// the ∀-occupancy quantification must find the empty-FIFO stall
    /// divergence (this is exactly the case a seeded-FIFO check misses).
    #[test]
    fn pull_block_vs_noblock_refuted_on_empty_fifo() {
        let v = equiv(&[0x80A0], &[0x8080], &one_pin_cfg(), 4, &[], Tier::TraceOnly);
        assert!(matches!(v, EquivVerdict::Refuted(_)), "expected Refuted, got {v:?}");
    }

    /// The X/Y self-move no-ops (word_canon lemma 3): `mov y, y` ≡
    /// `mov x, x` in every state, strict tier.
    #[test]
    fn nop_selfmoves_equivalent() {
        let v = equiv(&[0xA042], &[0xA021], &one_pin_cfg(), 6, &[], Tier::TraceAndFinalState);
        assert_eq!(v, EquivVerdict::Proven);
    }

    /// Out-of-subset words and unsupported configs yield `Unknown`,
    /// never a verdict.
    #[test]
    fn unknown_paths() {
        let cfg = one_pin_cfg();
        // WAIT is outside the modeled subset.
        let v = equiv(&[0x2000], &[0xA021], &cfg, 4, &[], Tier::TraceOnly);
        assert!(matches!(v, EquivVerdict::Unknown(_)), "WAIT must be Unknown, got {v:?}");
        // out_count == 0 is outside the modeled config contract.
        let mut c0 = cfg;
        c0.pins.out_count = 0;
        let v = equiv(&[0xA021], &[0xA021], &c0, 4, &[], Tier::TraceOnly);
        assert!(matches!(v, EquivVerdict::Unknown(_)), "cfg must be Unknown, got {v:?}");
    }
}
