//! Simulated annealing (Metropolis-Hastings) over the program genome.
//!
//! Cost (agreed): `W * correctness + size`, `W` larger than the max size so
//! correctness strictly dominates and size only breaks ties among already-
//! correct programs — STOKE's two-phase behavior without an explicit
//! switch. Illegal genomes are infinite cost (decision ②); but the
//! generator and moves only ever produce legal IR, so that's a safety net.
//!
//! The first validation fixes the SM config and searches only the
//! instruction slots + wrap, within a small slot window.

use crate::cost::score;
use crate::ir::*;
use crate::program::Program;
use crate::rng::Rng;
use crate::run::RunSpec;

/// What the search may vary: the first `slots` instruction slots and the
/// wrap bounds. The config (incl. side-set) is taken from the template and
/// held fixed; `side` is needed to generate legal delay/side-set values.
#[derive(Debug, Clone, Copy)]
pub struct Space {
    pub slots: u8,
    pub side: SideCfg,
    /// If false, wrap bounds are fixed to the template's and not mutated.
    pub search_wrap: bool,
}

/// Annealing schedule and weights.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub iters: u32,
    pub restarts: u32,
    pub t0: f64,
    pub t_end: f64,
    pub w: f64,
}

impl Default for Params {
    fn default() -> Self {
        // Temperature is scaled to the cost gap of a single wrong pin-cycle
        // (= w). t0 ~ 2w lets correctness barriers be crossed early;
        // t_end < w makes the tail greedy on size.
        Params { iters: 4000, restarts: 24, t0: 128.0, t_end: 1.0, w: 64.0 }
    }
}

// Legal operand alphabets. Data-dependent control flow (MOV/OUT to PC,
// EXEC) is deliberately excluded from proposals — still representable in
// the IR, just not explored early (review gating decision).
const JMP_CONDS: [JmpCond; 8] = [
    JmpCond::Always, JmpCond::NotX, JmpCond::XPostDec, JmpCond::NotY,
    JmpCond::YPostDec, JmpCond::XneY, JmpCond::Pin, JmpCond::NotOsrEmpty,
];
const WAIT_SRCS: [WaitSrc; 4] = [WaitSrc::GpioAbs, WaitSrc::PinRel, WaitSrc::Irq, WaitSrc::JmpPin];
const IN_SRCS: [InSrc; 6] = [InSrc::Pins, InSrc::X, InSrc::Y, InSrc::Null, InSrc::Isr, InSrc::Osr];
const OUT_DSTS: [OutDst; 6] =
    [OutDst::Pins, OutDst::X, OutDst::Y, OutDst::Null, OutDst::PinDirs, OutDst::Isr];
const MOV_DSTS: [MovDst; 6] =
    [MovDst::Pins, MovDst::X, MovDst::Y, MovDst::PinDirs, MovDst::Isr, MovDst::Osr];
const MOV_OPS: [MovOp; 3] = [MovOp::None, MovOp::Invert, MovOp::BitReverse];
const MOV_SRCS: [MovSrc; 7] =
    [MovSrc::Pins, MovSrc::X, MovSrc::Y, MovSrc::Null, MovSrc::Status, MovSrc::Isr, MovSrc::Osr];
const SET_DSTS: [SetDst; 4] = [SetDst::Pins, SetDst::X, SetDst::Y, SetDst::PinDirs];

fn random_op(rng: &mut Rng, slots: u8) -> Op {
    match rng.below(8) {
        0 => Op::Jmp { cond: *rng.pick(&JMP_CONDS), target: rng.below(slots as u32) as u8 },
        1 => Op::Wait { polarity: rng.boolean(), src: *rng.pick(&WAIT_SRCS), index: rng.below(32) as u8 },
        2 => Op::In { src: *rng.pick(&IN_SRCS), count: 1 + rng.below(32) as u8 },
        3 => Op::Out { dst: *rng.pick(&OUT_DSTS), count: 1 + rng.below(32) as u8 },
        4 => {
            if rng.boolean() {
                Op::Push { if_full: rng.boolean(), block: rng.boolean() }
            } else {
                Op::Pull { if_empty: rng.boolean(), block: rng.boolean() }
            }
        }
        5 => Op::Mov { dst: *rng.pick(&MOV_DSTS), op: *rng.pick(&MOV_OPS), src: *rng.pick(&MOV_SRCS) },
        6 => Op::Irq { clear: rng.boolean(), wait: rng.boolean(), index: rng.below(32) as u8 },
        _ => Op::Set { dst: *rng.pick(&SET_DSTS), data: rng.below(32) as u8 },
    }
}

fn random_delay(rng: &mut Rng, side: &SideCfg) -> u8 {
    rng.below(side.max_delay() as u32 + 1) as u8
}

fn random_sideset(rng: &mut Rng, side: &SideCfg) -> Option<u8> {
    match side.sideset_value_bits() {
        None => None,
        Some(bits) => {
            let val = rng.below(1u32 << bits) as u8;
            // In `opt` mode an instruction may decline to side-set.
            if side.en && rng.below(4) == 0 { None } else { Some(val) }
        }
    }
}

fn random_insn(rng: &mut Rng, space: &Space) -> Insn {
    Insn {
        op: random_op(rng, space.slots),
        delay: random_delay(rng, &space.side),
        sideset: random_sideset(rng, &space.side),
    }
}

/// Indices in the window currently holding an instruction.
fn occupied(p: &Program, slots: u8) -> Vec<usize> {
    (0..slots as usize).filter(|&i| p.slots[i].is_some()).collect()
}

/// A fresh random program: each window slot ~half-filled, valid wrap.
fn random_program(template: &Program, space: &Space, rng: &mut Rng) -> Program {
    let mut p = template.clone();
    for i in 0..space.slots as usize {
        p.slots[i] = if rng.boolean() { Some(random_insn(rng, space)) } else { None };
    }
    for i in space.slots as usize..32 {
        p.slots[i] = None;
    }
    if space.search_wrap {
        set_random_wrap(&mut p, space.slots, rng);
    }
    p
}

fn set_random_wrap(p: &mut Program, slots: u8, rng: &mut Rng) {
    let bottom = rng.below(slots as u32) as u8;
    let top = bottom + rng.below(slots as u32 - bottom as u32) as u8;
    p.wrap_bottom = bottom;
    p.wrap_top = top;
}

/// One mutation move. Always yields legal IR (range-aware by construction).
fn mutate(p: &Program, space: &Space, rng: &mut Rng) -> Program {
    let mut m = p.clone();
    let slots = space.slots;
    match rng.below(7) {
        0 => {
            // ReplaceOp
            let i = rng.below(slots as u32) as usize;
            m.slots[i] = Some(random_insn(rng, space));
        }
        1 => {
            // Clear
            let i = rng.below(slots as u32) as usize;
            m.slots[i] = None;
        }
        2 => {
            // Fill an empty slot (or replace if full)
            let i = rng.below(slots as u32) as usize;
            if m.slots[i].is_none() {
                m.slots[i] = Some(random_insn(rng, space));
            }
        }
        3 => {
            // MutateDelay on an occupied slot
            if let Some(i) = pick_occupied(&m, slots, rng) {
                if let Some(insn) = &mut m.slots[i] {
                    insn.delay = random_delay(rng, &space.side);
                }
            }
        }
        4 => {
            // MutateSideset on an occupied slot
            if let Some(i) = pick_occupied(&m, slots, rng) {
                if let Some(insn) = &mut m.slots[i] {
                    insn.sideset = random_sideset(rng, &space.side);
                }
            }
        }
        5 if space.search_wrap => set_random_wrap(&mut m, slots, rng), // Retarget wrap
        _ => {
            // MutateOperand: re-roll the op, keep delay/sideset
            if let Some(i) = pick_occupied(&m, slots, rng) {
                if let Some(insn) = &mut m.slots[i] {
                    insn.op = random_op(rng, slots);
                }
            }
        }
    }
    m
}

fn pick_occupied(p: &Program, slots: u8, rng: &mut Rng) -> Option<usize> {
    let occ = occupied(p, slots);
    if occ.is_empty() {
        None
    } else {
        Some(occ[rng.below(occ.len() as u32) as usize])
    }
}

/// Anneal and return the best program found and its score.
pub fn anneal(
    template: &Program,
    space: &Space,
    golden: &[u32],
    spec: &RunSpec,
    params: &Params,
    seed: u64,
) -> (Program, crate::cost::Score) {
    let mut rng = Rng::new(seed);
    let cost_of = |p: &Program| -> f64 {
        let s = score(p, golden, spec);
        if !s.valid {
            f64::INFINITY
        } else {
            params.w * s.correctness as f64 + s.size as f64
        }
    };

    let mut best: Option<(Program, f64)> = None;
    for _ in 0..params.restarts {
        let mut cur = random_program(template, space, &mut rng);
        let mut cur_cost = cost_of(&cur);
        for i in 0..params.iters {
            let frac = i as f64 / params.iters as f64;
            let t = params.t0 * (params.t_end / params.t0).powf(frac);
            let cand = mutate(&cur, space, &mut rng);
            let cand_cost = cost_of(&cand);
            let d = cand_cost - cur_cost;
            if d <= 0.0 || rng.unit() < (-d / t).exp() {
                cur = cand;
                cur_cost = cand_cost;
            }
            if best.as_ref().map_or(true, |(_, bc)| cur_cost < *bc) {
                best = Some((cur.clone(), cur_cost));
            }
        }
    }

    let (p, _) = best.expect("at least one restart");
    let s = score(&p, golden, spec);
    (p, s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::*;
    use crate::run::run;

    const DATA: u8 = 0;
    const CLK: u8 = 1;

    fn spi_template() -> Program {
        // Same fixed config as the reference oracle; slots are searched.
        Program::empty(Config {
            side: SideCfg { count: 1, en: false },
            clkdiv_int: 1,
            clkdiv_frac: 0,
            shift: ShiftCfg {
                autopull: true,
                pull_threshold: 8,
                out_dir: ShiftDir::Left,
                ..ShiftCfg::default()
            },
            pins: PinMap { out_base: DATA, out_count: 1, sideset_base: CLK, ..PinMap::default() },
            ..Config::default()
        })
    }

    fn spi_reference() -> Program {
        let mut p = spi_template();
        p.slots[0] = Some(Insn { op: Op::Out { dst: OutDst::Pins, count: 1 }, delay: 0, sideset: Some(0) });
        p.slots[1] = Some(Insn {
            op: Op::Mov { dst: MovDst::Y, op: MovOp::None, src: MovSrc::Y },
            delay: 0,
            sideset: Some(1),
        });
        p.wrap_bottom = 0;
        p.wrap_top = 1;
        p
    }

    fn spi_spec() -> RunSpec {
        RunSpec {
            block: 0,
            sm: 0,
            inputs: vec![0xA5 << 24],
            output_pins: vec![DATA, CLK],
            capture_pins: vec![DATA, CLK],
            cycles: 16,
        }
    }

    /// Convergence validation: a 2-slot window, loop fixed to (0,1). The
    /// annealer reliably drives cost to zero — a correctness-0, size-2
    /// program — proving the move set + temperature + cost mechanics work.
    ///
    /// With the level+OE oracle (see `cost`/`trace_pads`) the found program
    /// is a *real* SPI transmitter: `OUT PINS side 0` drives the data line,
    /// side-set toggles the clock (the other slot is a pin-inert no-op).
    /// The earlier `OUT PINDIRS` exploit — which faked the level by toggling
    /// direction — is rejected because it diverges on the captured OE.
    /// Slow (~40s); opt-in via `--ignored`.
    #[test]
    #[ignore = "slow convergence validation; run with: cargo test -- --ignored"]
    fn rediscovers_spi_optimum_fixed_wrap() {
        let spec = spi_spec();
        let golden = run(&spi_reference(), &spec);
        let mut template = spi_template();
        template.wrap_bottom = 0;
        template.wrap_top = 1;
        let space = Space { slots: 2, side: SideCfg { count: 1, en: false }, search_wrap: false };
        let params = Params { iters: 5000, restarts: 40, ..Params::default() };

        let (best, s) = anneal(&template, &space, &golden, &spec, &params, 0xC0FFEE);
        eprintln!("best: {s:?} span={:?}\n  s0={:?}\n  s1={:?}", best.span(), best.slots[0], best.slots[1]);
        assert_eq!(s.correctness, 0, "search must find a waveform-correct program");
        assert!(s.size <= 2, "should be the 2-slot optimum, got size {}", s.size);
    }
}
