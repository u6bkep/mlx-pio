//! Exhaustive loop-body enumeration (the second track of the 2026-07-05
//! strategy pivot): prove or refute the existence of a general DME TX loop
//! at small instruction counts, instead of asking an anneal to find one.
//!
//! ## What is enumerated
//!
//! Programs of exactly `len` slots (0..len), wrap = (0, len-1), under the
//! compression seed's config (`dme_spec_ref`: autopull ON, threshold 5,
//! shift right, clkdiv 1). The per-slot STRUCTURAL alphabet (delays handled
//! separately, see below):
//!
//!   * `jmp <cond> <target>` — all 8 conditions × targets in 0..len
//!   * `out {Pins,X,Y,Null,PinDirs}, 1..=5`
//!   * `mov {Pins,X,Y,PinDirs,Isr,Osr} <-, {None,Invert} {Pins,X,Y,Null,Isr,Osr}`
//!     (identity `mov r, r` excluded)
//!   * `set {Pins,X,Y,PinDirs}, {0..=7, 31}`
//!
//! **v1 scope exclusions (documented, deliberate):** `wait`/`irq` (stall on
//! external events that never fire single-SM), `pull`/`push` (autopull covers
//! refill; blocking-pull semantics add timing coupling), `in` (no RX path in
//! a TX encoder), `mov`/`out` to `Exec`/`Pc` (self-modifying / computed jumps
//! — a later tier), `MovOp::BitReverse`, side-set (a COST exclusion, not
//! fidelity — the emulator's opt-side-set bug was fixed in 368e499; but
//! side-set multiplies the per-slot alphabet ~3x, i.e. ~3^len ~ 243x at
//! len 5 — side-set variants need a scaffolded tier, or len <= 4), configs
//! other than the seed's. "Exhaustive" claims are relative to THIS
//! alphabet.
//!
//! ## Why delays factor out (the key cost collapse)
//!
//! None of the alphabet's instructions can stall mid-frame (`out` only
//! stalls once the FIFO is drained — after the data). So the *sequence* of
//! executed instructions, and therefore the *sequence* of pin-level changes,
//! is invariant under delay changes; delays only move edges in time. This
//! licenses a two-stage screen:
//!
//!   1. **Pattern screen** (per structure, delays all 0): run two L=4
//!      probes and check the transition COUNT (+ a quiet tail). A structure
//!      that cannot produce the DME edge pattern at delay 0 cannot at any
//!      delay. Delay-independence makes this exact, not heuristic — no
//!      false negatives. (`dme_spec_ref` with delays zeroed passes; pinned
//!      by `seed_passes_pattern_screen`.)
//!   2. **Timing stage** (pattern survivors only): brute-force delay tuples
//!      with `sum(d) <= DELAY_BUDGET` against real spec rows (early-break),
//!      then the full 2..=14 dataset, then the certifier (train + held-out).
//!
//! ## Sharding / resume / fleet
//!
//! Shard = first-slot op index. Each shard writes `shard-NNNN.json` into the
//! output dir when complete; existing files are skipped, so rerunning the
//! same command resumes, and `--shard-mod M --shard-rem R` splits the shard
//! space across machines with no coordination.

use crate::fixtures::{
    dme_corpus, dme_spec, dme_spec_multilength_dataset, dme_spec_ref, dme_validation_corpus,
    seq_words, spec_certify_corpus, SPEC_H, SPEC_PHI_MAX,
};
use crate::ir::{Insn, JmpCond, MovDst, MovOp, MovSrc, Op, OutDst, SetDst};
use crate::program::Program;
use crate::run::{run, RunSpec};
use crate::search::Target;

/// Max total delay cycles distributed over a structure's slots in the timing
/// stage. A 16-cycle cell with `len` executed instructions leaves at most
/// `16 - 1` spare cycles on any path; 15 covers every feasible assignment
/// for the loop shapes this alphabet can express.
const DELAY_BUDGET: u8 = 15;

/// The structural alphabet for `len`-slot bodies. Deterministic order — the
/// shard numbering and any resume depend on it, so treat changes as a NEW
/// enumeration (use a fresh --out dir).
pub fn alphabet(len: usize) -> Vec<Op> {
    let mut ops = Vec::new();
    for cond in [
        JmpCond::Always,
        JmpCond::NotX,
        JmpCond::XPostDec,
        JmpCond::NotY,
        JmpCond::YPostDec,
        JmpCond::XneY,
        JmpCond::Pin,
        JmpCond::NotOsrEmpty,
    ] {
        for target in 0..len as u8 {
            ops.push(Op::Jmp { cond, target });
        }
    }
    for dst in [OutDst::Pins, OutDst::X, OutDst::Y, OutDst::Null, OutDst::PinDirs] {
        for count in 1..=5u8 {
            ops.push(Op::Out { dst, count });
        }
    }
    let mdsts = [MovDst::Pins, MovDst::X, MovDst::Y, MovDst::PinDirs, MovDst::Isr, MovDst::Osr];
    let msrcs = [MovSrc::Pins, MovSrc::X, MovSrc::Y, MovSrc::Null, MovSrc::Isr, MovSrc::Osr];
    for dst in mdsts {
        for op in [MovOp::None, MovOp::Invert] {
            for src in msrcs {
                // Identity moves are structural no-ops the timing stage can't
                // distinguish from a pure delay slot — drop them.
                let identity = op == MovOp::None
                    && matches!(
                        (dst, src),
                        (MovDst::Pins, MovSrc::Pins)
                            | (MovDst::X, MovSrc::X)
                            | (MovDst::Y, MovSrc::Y)
                            | (MovDst::Isr, MovSrc::Isr)
                            | (MovDst::Osr, MovSrc::Osr)
                    );
                if !identity {
                    ops.push(Op::Mov { dst, op, src });
                }
            }
        }
    }
    for dst in [SetDst::Pins, SetDst::X, SetDst::Y, SetDst::PinDirs] {
        for data in [0, 1, 2, 3, 4, 5, 6, 7, 31u8] {
            ops.push(Op::Set { dst, data });
        }
    }
    // Canonical NOP (identity mov — what empty slots assemble to). A real
    // instruction-memory word whose only effect is +1 cycle ON ITS PATH: as
    // a branch-target landing pad it pads one branch outcome asymmetrically,
    // which no delay field can (delays apply to taken AND fall-through).
    // The 6-word compression champion's slot-5 trick (2026-07-05) — its
    // omission was an exhaustiveness hole in alphabet v1. Appended LAST so
    // v1 shard prefixes keep their meaning, but suffix spaces change: any
    // alphabet edit still requires a fresh --out dir.
    ops.push(Op::Mov { dst: MovDst::Y, op: MovOp::None, src: MovSrc::Y });
    ops
}

/// Does this op write the output pin (level or direction)? A body with no
/// pin write cannot encode anything — generation-time reject.
fn writes_pin(op: &Op) -> bool {
    matches!(
        op,
        Op::Out { dst: OutDst::Pins | OutDst::PinDirs, .. }
            | Op::Mov { dst: MovDst::Pins | MovDst::PinDirs, .. }
            | Op::Set { dst: SetDst::Pins | SetDst::PinDirs, .. }
    )
}

/// Does this op consume OSR bits? A DME encoder must read the data stream —
/// generation-time reject. (`mov` from Osr copies without consuming; only
/// OUT shifts.)
fn consumes_data(op: &Op) -> bool {
    matches!(op, Op::Out { .. })
}

fn build(ops: &[Op], delays: &[u8]) -> Program {
    let mut p = Program::empty(dme_spec_ref().config);
    for (i, op) in ops.iter().enumerate() {
        p.slots[i] = Some(Insn { op: op.clone(), delay: delays[i], sideset: None });
    }
    p.wrap_bottom = 0;
    p.wrap_top = (ops.len() - 1) as u8;
    p
}

/// One pattern probe: `(spec, min_edges, max_edges)` for a 4-data-bit word
/// (5 line bits incl. the fill 0). `max` = full DME edge count (one boundary
/// per line bit + one mid per 1-bit); `min` = that minus the possibly
/// invisible opening edge (a program whose first drive matches the pad's
/// idle level emits one fewer visible transition — delay-independent either
/// way).
fn pattern_probes() -> Vec<(RunSpec, usize)> {
    [0b0101u64, 0b1010u64]
        .into_iter()
        .map(|bits| {
            let word_bits = 5usize; // 4 data bits + fill 0 (threshold 5)
            let ones = (bits & 0x1F).count_ones() as usize;
            let edges = word_bits + ones;
            // Window: at delay 0 a cell is at most ~6 cycles, the whole
            // 5-bit frame < 40; 200 leaves a long mandatory-quiet tail.
            let sp = RunSpec { inputs: seq_words(bits, 4), cycles: 200, ..dme_spec(0) };
            (sp, edges)
        })
        .collect()
}

fn transition_times(wave: &[u32]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut prev = wave.first().copied().unwrap_or(0) & 1;
    for (i, w) in wave.iter().enumerate().skip(1) {
        let v = w & 1;
        if v != prev {
            out.push(i);
            prev = v;
        }
    }
    out
}

/// Delay-independent structure screen (see module doc): exact edge count
/// (modulo the invisible opening edge) and a quiet tail on both probes.
pub fn pattern_ok(p: &Program, probes: &[(RunSpec, usize)]) -> bool {
    for (sp, edges) in probes {
        let t = transition_times(&run(p, sp));
        if t.len() + 1 != *edges && t.len() != *edges {
            return false;
        }
        // Quiet tail: a correct encoder stalls on OUT once the FIFO drains.
        // At delay 0 the frame ends well before cycle 100; anything still
        // toggling later loops forever and would fail the strict-tail
        // certifier at every delay.
        if t.last().copied().unwrap_or(0) >= 100 {
            return false;
        }
    }
    true
}

/// A certified survivor.
#[derive(serde::Serialize)]
pub struct Survivor {
    pub brief: String,
    pub words: Vec<String>,
    pub wrap: (u8, u8),
    pub delays: Vec<u8>,
    pub size: u8,
}

/// Per-shard counters + survivors.
#[derive(serde::Serialize)]
pub struct ShardResult {
    pub shard: usize,
    pub len: usize,
    pub alphabet: usize,
    pub structures: u64,
    pub screened: u64,
    pub pattern_pass: u64,
    pub timing_evals: u64,
    pub survivors: Vec<Survivor>,
}

/// Enumerate every delay tuple with `sum <= DELAY_BUDGET` for one pattern-
/// passing structure; full-dataset + certifier check on the rare timing
/// hits. Returns survivors and the number of timing evals spent.
fn timing_stage(
    ops: &[Op],
    quick: &[(RunSpec, Target)],
    full: &[(RunSpec, Target)],
    survivors: &mut Vec<Survivor>,
) -> u64 {
    let len = ops.len();
    let mut delays = vec![0u8; len];
    let mut evals = 0u64;
    loop {
        let p = build(ops, &delays);
        // Quick rows first (short lengths), early-break on first error.
        evals += 1;
        let mut ok = true;
        for (sp, target) in quick {
            if target.search_cost(&run(&p, sp), 0, 1.0) > 0.0 {
                ok = false;
                break;
            }
        }
        if ok {
            for (sp, target) in full {
                if target.search_cost(&run(&p, sp), 0, 1.0) > 0.0 {
                    ok = false;
                    break;
                }
            }
        }
        if ok
            && spec_certify_corpus(&p, &dme_corpus()) == 0
            && spec_certify_corpus(&p, &dme_validation_corpus()) == 0
        {
            survivors.push(Survivor {
                brief: p.brief(),
                words: p.assemble().iter().map(|w| format!("{w:#06x}")).collect(),
                wrap: (p.wrap_bottom, p.wrap_top),
                delays: delays.clone(),
                size: p.size(),
            });
        }
        // Odometer over delay tuples with sum <= DELAY_BUDGET.
        let mut i = 0;
        loop {
            if i == len {
                return evals;
            }
            delays[i] += 1;
            if delays.iter().map(|&d| d as u32).sum::<u32>() <= DELAY_BUDGET as u32 {
                break;
            }
            delays[i] = 0;
            i += 1;
        }
    }
}

/// Run one shard: all structures whose FIRST slot is `alphabet[shard]`.
/// `stop` is a cooperative abort (checked once per structure, ~20ms max
/// latency): on trip the partial work is DISCARDED and `None` returned —
/// shard results are all-or-nothing, the ledger has no partial entries.
pub fn run_shard(shard: usize, len: usize, ops: &[Op], stop: Option<&std::sync::atomic::AtomicBool>) -> Option<ShardResult> {
    let probes = pattern_probes();
    let quick_lengths: Vec<usize> = vec![2, 3, 4];
    let (quick, _) = dme_spec_multilength_dataset(&quick_lengths, 32);
    let full_lengths: Vec<usize> = (5..=14).collect();
    let (full, _) = dme_spec_multilength_dataset(&full_lengths, 32);

    let mut res = ShardResult {
        shard,
        len,
        alphabet: ops.len(),
        structures: 0,
        screened: 0,
        pattern_pass: 0,
        timing_evals: 0,
        survivors: Vec::new(),
    };
    // Odometer over the remaining len-1 slots.
    let mut idx = vec![0usize; len - 1];
    let mut body: Vec<Op> = Vec::with_capacity(len);
    loop {
        if let Some(f) = stop {
            if f.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
        }
        body.clear();
        body.push(ops[shard].clone());
        for &j in &idx {
            body.push(ops[j].clone());
        }
        res.structures += 1;
        if body.iter().any(writes_pin) && body.iter().any(consumes_data) {
            res.screened += 1;
            let p = build(&body, &vec![0u8; len]);
            if p.validate().is_ok() && pattern_ok(&p, &probes) {
                res.pattern_pass += 1;
                res.timing_evals += timing_stage(&body, &quick, &full, &mut res.survivors);
            }
        }
        // Advance the odometer.
        let mut i = 0;
        loop {
            if i == idx.len() {
                return Some(res);
            }
            idx[i] += 1;
            if idx[i] < ops.len() {
                break;
            }
            idx[i] = 0;
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pattern screen must PASS the known-good structure: the seed with
    /// all delays zeroed (delay-independence is the screen's whole premise).
    #[test]
    fn seed_passes_pattern_screen() {
        let mut p = dme_spec_ref();
        for s in p.slots.iter_mut().flatten() {
            s.delay = 0;
        }
        assert!(pattern_ok(&p, &pattern_probes()), "zero-delay seed must pass the screen");
        // And the real seed (with its delays) too — same sequence, shifted.
        assert!(pattern_ok(&dme_spec_ref(), &pattern_probes()), "the seed itself must pass");
    }

    /// Programs that cannot encode must FAIL: empty (no edges) and a free-
    /// running toggler (edge count far off + no quiet tail).
    #[test]
    fn junk_fails_pattern_screen() {
        let probes = pattern_probes();
        let empty = Program::empty(dme_spec_ref().config);
        assert!(!pattern_ok(&empty, &probes), "empty program has no edges");
        let mut toggler = Program::empty(dme_spec_ref().config);
        toggler.slots[0] =
            Some(Insn { op: Op::Mov { dst: MovDst::Pins, op: MovOp::Invert, src: MovSrc::Pins }, delay: 0, sideset: None });
        toggler.wrap_bottom = 0;
        toggler.wrap_top = 0;
        assert!(!pattern_ok(&toggler, &probes), "free-running toggler must fail");
    }

    /// The alphabet is the shard-numbering contract — pin its size per len
    /// so an accidental reorder/regeneration is caught before it corrupts a
    /// resumable enumeration.
    #[test]
    fn alphabet_size_is_pinned() {
        assert_eq!(alphabet(4).len(), 161);
        assert_eq!(alphabet(5).len(), 169);
        assert_eq!(alphabet(6).len(), 177);
    }
}
