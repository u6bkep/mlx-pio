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
use crate::program::{Config, Program, ShiftDir};
use crate::rng::Rng;
use crate::run::RunSpec;

/// What the search may vary: the first `slots` instruction slots, the wrap
/// bounds, and the config `genes`. Everything not searched is taken from
/// the template and held fixed; `side` is needed to generate legal
/// delay/side-set values.
#[derive(Debug, Clone, Copy)]
pub struct Space {
    pub slots: u8,
    pub side: SideCfg,
    /// If false, wrap bounds are fixed to the template's and not mutated.
    pub search_wrap: bool,
    /// Which config fields the search may mutate.
    pub genes: Genes,
}

/// Which SM-config fields are search genes. Everything else stays at the
/// template's value (the fixed per-target contract: pin bases, side-set
/// layout, …). Pin bases are never genes — you can't rewire the board.
#[derive(Debug, Clone, Copy, Default)]
pub struct Genes {
    pub clkdiv: bool,
    pub pull_threshold: bool,
    pub out_dir: bool,
    pub autopull: bool,
}

impl Genes {
    fn any(self) -> bool {
        self.clkdiv || self.pull_threshold || self.out_dir || self.autopull
    }
    /// Tags of the live genes, for uniform random selection.
    fn live(self) -> Vec<u8> {
        let mut v = Vec::new();
        if self.clkdiv {
            v.push(0);
        }
        if self.pull_threshold {
            v.push(1);
        }
        if self.out_dir {
            v.push(2);
        }
        if self.autopull {
            v.push(3);
        }
        v
    }
}

/// Upper bound on clkdiv integer part during search — comms dividers are
/// small, and int must be >= 1 (0 means 65536; the search avoids it).
const CLKDIV_INT_MAX: u16 = 8;

/// Set every live gene to a fresh random in-range value.
fn randomize_config(c: &mut Config, genes: &Genes, rng: &mut Rng) {
    if genes.clkdiv {
        c.clkdiv_int = 1 + rng.below(CLKDIV_INT_MAX as u32) as u16;
        c.clkdiv_frac = rng.below(256) as u8;
    }
    if genes.pull_threshold {
        c.shift.pull_threshold = 1 + rng.below(32) as u8;
    }
    if genes.out_dir {
        c.shift.out_dir = if rng.boolean() { ShiftDir::Left } else { ShiftDir::Right };
    }
    if genes.autopull {
        c.shift.autopull = rng.boolean();
    }
}

/// Perturb one randomly-chosen live gene.
fn mutate_config_gene(c: &mut Config, genes: &Genes, rng: &mut Rng) {
    let live = genes.live();
    if live.is_empty() {
        return;
    }
    match *rng.pick(&live) {
        0 => {
            c.clkdiv_int = 1 + rng.below(CLKDIV_INT_MAX as u32) as u16;
            c.clkdiv_frac = rng.below(256) as u8;
        }
        1 => c.shift.pull_threshold = 1 + rng.below(32) as u8,
        2 => c.shift.out_dir = if rng.boolean() { ShiftDir::Left } else { ShiftDir::Right },
        _ => c.shift.autopull = rng.boolean(),
    }
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
    if space.genes.any() {
        randomize_config(&mut p.config, &space.genes, rng);
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
    // Roughly one move in five touches config, when any gene is live.
    if space.genes.any() && rng.below(5) == 0 {
        mutate_config_gene(&mut m.config, &space.genes, rng);
        return m;
    }
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

/// The scalar cost: `w * correctness + size`, infinite for an illegal
/// genome (decision ②). Lower is better.
fn cost(p: &Program, golden: &[u32], spec: &RunSpec, w: f64) -> f64 {
    let s = score(p, golden, spec);
    if !s.valid {
        f64::INFINITY
    } else {
        w * s.correctness as f64 + s.size as f64
    }
}

/// A slot's `(delay, sideset)` for re-op'ing it, or the defaults a freshly
/// filled empty slot must carry.
fn slot_fields(p: &Program, i: usize, fill_ss: Option<u8>) -> (u8, Option<u8>) {
    match &p.slots[i] {
        Some(ins) => (ins.delay, ins.sideset),
        None => (0, fill_ss),
    }
}

/// Legal side-set values for a config (mirrors the encoder's budget).
fn legal_sidesets(side: &SideCfg) -> Vec<Option<u8>> {
    match side.sideset_value_bits() {
        None => vec![None],
        Some(bits) => {
            let max = if bits == 0 { 0 } else { (1u16 << bits) - 1 } as u8;
            let mut v: Vec<Option<u8>> = (0..=max).map(Some).collect();
            if side.en {
                v.push(None);
            }
            v
        }
    }
}

/// A comprehensive (but bounded) set of candidate ops for local search:
/// every legal opcode/operand combination, with bit-count/data/index
/// fields sampled at representative values. Includes `OUT PINS,1` and the
/// other data-driving ops the polish needs to grind out value residuals.
fn op_neighbors(slots: u8) -> Vec<Op> {
    let counts = [1u8, 2, 8, 32];
    let datas = [0u8, 1, 31];
    let indices = [0u8, 1];
    let mut v = Vec::new();
    for &cond in &JMP_CONDS {
        for t in 0..slots {
            v.push(Op::Jmp { cond, target: t });
        }
    }
    for &polarity in &[false, true] {
        for &src in &WAIT_SRCS {
            for &index in &indices {
                v.push(Op::Wait { polarity, src, index });
            }
        }
    }
    for &src in &IN_SRCS {
        for &count in &counts {
            v.push(Op::In { src, count });
        }
    }
    for &dst in &OUT_DSTS {
        for &count in &counts {
            v.push(Op::Out { dst, count });
        }
    }
    for &a in &[false, true] {
        for &block in &[false, true] {
            v.push(Op::Push { if_full: a, block });
            v.push(Op::Pull { if_empty: a, block });
        }
    }
    for &dst in &MOV_DSTS {
        for &op in &MOV_OPS {
            for &src in &MOV_SRCS {
                v.push(Op::Mov { dst, op, src });
            }
        }
    }
    for &clear in &[false, true] {
        for &wait in &[false, true] {
            for &index in &indices {
                v.push(Op::Irq { clear, wait, index });
            }
        }
    }
    for &dst in &SET_DSTS {
        for &data in &datas {
            v.push(Op::Set { dst, data });
        }
    }
    v
}

fn consider(cand: Program, cur_cost: f64, best: &mut Option<(Program, f64)>, golden: &[u32], spec: &RunSpec, w: f64) {
    let c = cost(&cand, golden, spec, w);
    if c < cur_cost && best.as_ref().map_or(true, |(_, bc)| c < *bc) {
        *best = Some((cand, c));
    }
}

/// Deterministic best-improvement local search to a single-move local
/// optimum. Each sweep tries every single-field neighbor — per slot: every
/// op (`op_neighbors`), every side-set value, every delay, and clear; plus
/// every wrap bound and every value of each live config gene — and applies
/// the single best strict improvement, repeating until none remains.
///
/// This grinds out the exact-value residuals annealing leaves on the table
/// (the data-line needle — see `diagnose_near_misses`): residuals that a
/// stochastic walk rarely proposes but an exhaustive neighbor sweep finds.
///
/// `two_opt`: when a single-move sweep stalls, also try changing **two**
/// slots' ops jointly. Some optima sit two coordinated op-swaps away with
/// no improving single-move path (e.g. moving the data `OUT` from one slot
/// to another while the other becomes a NOP). Costs O(slots² · ops²) per
/// kick, so reserve it for a final polish, not every restart.
pub fn polish(
    start: &Program,
    space: &Space,
    golden: &[u32],
    spec: &RunSpec,
    w: f64,
    two_opt: bool,
) -> Program {
    let ops = op_neighbors(space.slots);
    let sidesets = legal_sidesets(&space.side);
    let max_delay = space.side.max_delay();
    // Side-set a freshly-filled (previously empty) slot must carry.
    let fill_ss = if space.side.count.min(5) > 0 && !space.side.en { Some(0) } else { None };

    let mut cur = start.clone();
    let mut cur_cost = cost(&cur, golden, spec, w);

    loop {
        let mut best: Option<(Program, f64)> = None;

        for i in 0..space.slots as usize {
            let (delay, base_ss) = match &cur.slots[i] {
                Some(ins) => (ins.delay, ins.sideset),
                None => (0, fill_ss),
            };
            for op in &ops {
                let mut cand = cur.clone();
                cand.slots[i] = Some(Insn { op: op.clone(), delay, sideset: base_ss });
                consider(cand, cur_cost, &mut best, golden, spec, w);
            }
            if cur.slots[i].is_some() {
                for &ss in &sidesets {
                    let mut cand = cur.clone();
                    if let Some(ins) = &mut cand.slots[i] {
                        ins.sideset = ss;
                    }
                    consider(cand, cur_cost, &mut best, golden, spec, w);
                }
                for d in 0..=max_delay {
                    let mut cand = cur.clone();
                    if let Some(ins) = &mut cand.slots[i] {
                        ins.delay = d;
                    }
                    consider(cand, cur_cost, &mut best, golden, spec, w);
                }
            }
            let mut cleared = cur.clone();
            cleared.slots[i] = None;
            consider(cleared, cur_cost, &mut best, golden, spec, w);
        }

        if space.search_wrap {
            for b in 0..space.slots {
                for t in b..space.slots {
                    let mut cand = cur.clone();
                    cand.wrap_bottom = b;
                    cand.wrap_top = t;
                    consider(cand, cur_cost, &mut best, golden, spec, w);
                }
            }
        }

        let g = &space.genes;
        if g.out_dir {
            for dir in [ShiftDir::Left, ShiftDir::Right] {
                let mut cand = cur.clone();
                cand.config.shift.out_dir = dir;
                consider(cand, cur_cost, &mut best, golden, spec, w);
            }
        }
        if g.autopull {
            for ap in [false, true] {
                let mut cand = cur.clone();
                cand.config.shift.autopull = ap;
                consider(cand, cur_cost, &mut best, golden, spec, w);
            }
        }
        if g.pull_threshold {
            for thr in 1..=32u8 {
                let mut cand = cur.clone();
                cand.config.shift.pull_threshold = thr;
                consider(cand, cur_cost, &mut best, golden, spec, w);
            }
        }
        if g.clkdiv {
            for int in 1..=CLKDIV_INT_MAX {
                let mut cand = cur.clone();
                cand.config.clkdiv_int = int;
                cand.config.clkdiv_frac = 0;
                consider(cand, cur_cost, &mut best, golden, spec, w);
            }
            for frac in 0..=255u8 {
                let mut cand = cur.clone();
                cand.config.clkdiv_frac = frac;
                consider(cand, cur_cost, &mut best, golden, spec, w);
            }
        }

        // 2-opt kick: if single-move is stuck, try joint op changes on
        // every pair of slots — escapes the two-coordinated-swaps traps.
        if best.is_none() && two_opt {
            for i in 0..space.slots as usize {
                let (di, si) = slot_fields(&cur, i, fill_ss);
                for j in (i + 1)..space.slots as usize {
                    let (dj, sj) = slot_fields(&cur, j, fill_ss);
                    for oa in &ops {
                        for ob in &ops {
                            let mut cand = cur.clone();
                            cand.slots[i] = Some(Insn { op: oa.clone(), delay: di, sideset: si });
                            cand.slots[j] = Some(Insn { op: ob.clone(), delay: dj, sideset: sj });
                            consider(cand, cur_cost, &mut best, golden, spec, w);
                        }
                    }
                }
            }
        }

        match best {
            Some((c, cc)) => {
                cur = c;
                cur_cost = cc;
            }
            None => return cur,
        }
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
    let mut best: Option<(Program, f64)> = None;
    for _ in 0..params.restarts {
        let mut cur = random_program(template, space, &mut rng);
        let mut cur_cost = cost(&cur, golden, spec, params.w);
        let mut local_best = (cur.clone(), cur_cost);
        for i in 0..params.iters {
            let frac = i as f64 / params.iters as f64;
            let t = params.t0 * (params.t_end / params.t0).powf(frac);
            let cand = mutate(&cur, space, &mut rng);
            let cand_cost = cost(&cand, golden, spec, params.w);
            let d = cand_cost - cur_cost;
            if d <= 0.0 || rng.unit() < (-d / t).exp() {
                cur = cand;
                cur_cost = cand_cost;
            }
            if cur_cost < local_best.1 {
                local_best = (cur.clone(), cur_cost);
            }
        }
        // Cheap single-move polish of this restart's best, folded into the
        // global best. (The expensive 2-opt is saved for one final pass.)
        let polished = polish(&local_best.0, space, golden, spec, params.w, false);
        let pc = cost(&polished, golden, spec, params.w);
        if best.as_ref().map_or(true, |(_, bc)| pc < *bc) {
            best = Some((polished, pc));
        }
    }

    // Final polish of the global best with the 2-opt kick — grinds out
    // residuals that sit two coordinated op-swaps from the optimum.
    let (p, _) = best.expect("at least one restart");
    let p = polish(&p, space, golden, spec, params.w, true);
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
    ///
    /// With the greedy polish appended to annealing, this needs only a tiny
    /// budget (1000×4) — polish deterministically finishes the data needle
    /// that previously demanded ~5000×40 of stochastic search. Opt-in
    /// (a few seconds in debug from the final 2-opt; instant in release).
    #[test]
    #[ignore = "convergence validation; run with: cargo test --release -- --ignored"]
    fn rediscovers_spi_optimum_fixed_wrap() {
        let spec = spi_spec();
        let golden = run(&spi_reference(), &spec);
        let mut template = spi_template();
        template.wrap_bottom = 0;
        template.wrap_top = 1;
        let space = Space {
            slots: 2,
            side: SideCfg { count: 1, en: false },
            search_wrap: false,
            genes: Genes::default(),
        };
        let params = Params { iters: 1000, restarts: 4, ..Params::default() };

        let (best, s) = anneal(&template, &space, &golden, &spec, &params, 0xC0FFEE);
        eprintln!("best: {s:?} span={:?}\n  s0={:?}\n  s1={:?}", best.span(), best.slots[0], best.slots[1]);
        assert_eq!(s.correctness, 0, "search must find a waveform-correct program");
        assert!(s.size <= 2, "should be the 2-slot optimum, got size {}", s.size);
    }

    /// Cheap 1-opt polish: from a pair of NOPs (clock right via side-set,
    /// data never driven) a single op-swap fills `OUT PINS,1` into the low
    /// slot, reaching the optimum. Fast — no 2-opt.
    #[test]
    fn polish_1opt_fills_missing_data() {
        let spec = spi_spec();
        let golden = run(&spi_reference(), &spec);
        let space = Space {
            slots: 2,
            side: SideCfg { count: 1, en: false },
            search_wrap: false,
            genes: Genes::default(),
        };
        let nop = |ss| {
            Some(Insn {
                op: Op::Mov { dst: MovDst::Y, op: MovOp::None, src: MovSrc::Y },
                delay: 0,
                sideset: Some(ss),
            })
        };
        let mut near = spi_template();
        near.wrap_bottom = 0;
        near.wrap_top = 1;
        near.slots[0] = nop(0);
        near.slots[1] = nop(1);
        assert!(score(&near, &golden, &spec).correctness > 0);
        let polished = polish(&near, &space, &golden, &spec, 64.0, false);
        assert_eq!(score(&polished, &golden, &spec).correctness, 0, "1-opt must fill the data OUT");
    }

    /// 2-opt polish grinds the documented seed-1 near-miss — data via
    /// `MOV PINS, BitReverse OSR`, correctness 1 — to the exact optimum. It
    /// sits two coordinated op-swaps away, so single-move can't reach it.
    /// Slow (O(ops²) sweep); opt-in.
    #[test]
    #[ignore = "2-opt polish (~4s); run with --ignored"]
    fn polish_grinds_out_data_residual() {
        let spec = spi_spec();
        let golden = run(&spi_reference(), &spec);
        let space = Space {
            slots: 2,
            side: SideCfg { count: 1, en: false },
            search_wrap: false,
            genes: Genes::default(),
        };
        let mut near = spi_template();
        near.wrap_bottom = 0;
        near.wrap_top = 1;
        near.slots[0] = Some(Insn {
            op: Op::Mov { dst: MovDst::Pins, op: MovOp::BitReverse, src: MovSrc::Osr },
            delay: 0,
            sideset: Some(0),
        });
        near.slots[1] = Some(Insn {
            op: Op::Out { dst: OutDst::Pins, count: 1 },
            delay: 0,
            sideset: Some(1),
        });
        assert!(score(&near, &golden, &spec).correctness > 0, "starts as a near-miss");
        let polished = polish(&near, &space, &golden, &spec, 64.0, true);
        let s = score(&polished, &golden, &spec);
        assert_eq!(s.correctness, 0, "polish must reach the exact optimum");
        assert!(s.size <= 2);
    }

    /// Widened search: free wrap + config genes (out_dir, autopull,
    /// pull_threshold), all randomized at init. The search must recover the
    /// pinned config values (out_dir Left and autopull on are *required* by
    /// the golden — Right shifts the zero half of the word, autopull-off
    /// stalls with no data) along with the loop bounds and instructions, and
    /// still reach a correct program. (clkdiv is a gene too but excluded
    /// here: an exact-match oracle makes clkdiv_frac a needle at 0 — that
    /// matters more on protocols with timing slack.)
    ///
    /// History: before the greedy polish, exact-0 scaled badly with the
    /// space — the widened case needed ~12000×400 of stochastic search vs
    /// 5000×40 fixed-config, because the rough strict-Hamming data needle
    /// got exponentially rarer. With polish appended, the deterministic
    /// finisher does that work and the budget collapses to 2000×12.
    #[test]
    #[ignore = "widened-search validation; run with: cargo test --release -- --ignored"]
    fn rediscovers_spi_free_wrap_and_genes() {
        let spec = spi_spec();
        let golden = run(&spi_reference(), &spec);
        let space = Space {
            slots: 2,
            side: SideCfg { count: 1, en: false },
            search_wrap: true,
            genes: Genes { clkdiv: false, pull_threshold: true, out_dir: true, autopull: true },
        };
        let params = Params { iters: 2000, restarts: 12, ..Params::default() };

        let (best, s) = anneal(&spi_template(), &space, &golden, &spec, &params, 0xC0FFEE);
        eprintln!(
            "best: {s:?} span={:?} wrap=({},{}) out_dir={:?} autopull={} pull_thr={}",
            best.span(), best.wrap_bottom, best.wrap_top,
            best.config.shift.out_dir, best.config.shift.autopull, best.config.shift.pull_threshold,
        );
        assert_eq!(s.correctness, 0, "widened search must still reach a correct program");
        assert!(s.size <= 2, "should still be the 2-slot optimum, got size {}", s.size);
        // The pinned genes must be recovered.
        assert_eq!(best.config.shift.out_dir, ShiftDir::Left, "MSB-first requires Left shift");
        assert!(best.config.shift.autopull, "no data without autopull");
    }

    /// Diagnostic (not an assertion): collect near-miss champions and
    /// characterize each residual. For every captured signal it reports how
    /// many cycles differ, and whether a small linear time-shift of the
    /// candidate collapses the Hamming distance. If a shift helps a lot, the
    /// residual is a PHASE error (a metric problem → edge-distance/DTW). If
    /// not, it's a VALUE error on a correctly-timed waveform (a search/
    /// operator problem → greedy polish / better neighborhoods).
    ///
    /// Fixed config + fixed wrap, so the residual isn't confounded by
    /// config/wrap drift — it isolates the instruction-search plateau.
    /// Run: `cargo test --release -- --ignored diagnose_near_misses --nocapture`
    #[test]
    #[ignore = "diagnostic; run with --release ... --nocapture"]
    fn diagnose_near_misses() {
        let spec = spi_spec();
        let golden = run(&spi_reference(), &spec);
        let space = Space {
            slots: 2,
            side: SideCfg { count: 1, en: false },
            search_wrap: false,
            genes: Genes::default(),
        };
        let mut template = spi_template();
        template.wrap_bottom = 0;
        template.wrap_top = 1;

        // Bit positions in a trace_pads sample: level in bit j, OE in 16+j;
        // capture_pins = [DATA, CLK] -> DATA=j0, CLK=j1.
        let bits = |wave: &[u32], bit: u32| -> String {
            wave.iter().map(|s| if (s >> bit) & 1 != 0 { '#' } else { '_' }).collect()
        };
        // Hamming with the candidate shifted by `k` cycles (out-of-range = 0).
        let shifted = |g: &[u32], c: &[u32], k: i32| -> u32 {
            (0..g.len())
                .map(|i| {
                    let j = i as i32 + k;
                    let cv = if j >= 0 && (j as usize) < c.len() { c[j as usize] } else { 0 };
                    (g[i] ^ cv).count_ones()
                })
                .sum()
        };

        eprintln!("GOLDEN DATA:{} CLK:{}", bits(&golden, 0), bits(&golden, 1));
        eprintln!("       DATAoe:{} CLKoe:{}", bits(&golden, 16), bits(&golden, 17));

        // Modest budget so most seeds land as near-misses, not exact 0.
        let params = Params { iters: 2500, restarts: 8, ..Params::default() };
        for seed in 1u64..=8 {
            let (best, s) = anneal(&template, &space, &golden, &spec, &params, seed);
            if s.correctness == 0 {
                eprintln!("\nseed {seed}: reached correctness 0 (exact) — skipped");
                continue;
            }
            let cand = run(&best, &spec);
            let strict = shifted(&golden, &cand, 0);
            let (mut bk, mut bh) = (0i32, strict);
            for k in -3..=3 {
                let h = shifted(&golden, &cand, k);
                if h < bh {
                    bh = h;
                    bk = k;
                }
            }
            let dim = |bit: u32| {
                golden.iter().zip(&cand).filter(|(g, c)| ((*g >> bit) & 1) != ((*c >> bit) & 1)).count()
            };
            eprintln!("\nseed {seed}: correctness={} size={}", s.correctness, s.size);
            eprintln!("  s0={:?}", best.slots[0]);
            eprintln!("  s1={:?}", best.slots[1]);
            eprintln!("  CAND   DATA:{} CLK:{}", bits(&cand, 0), bits(&cand, 1));
            eprintln!("         DATAoe:{} CLKoe:{}", bits(&cand, 16), bits(&cand, 17));
            eprintln!(
                "  residual by signal: DATA_lvl={} CLK_lvl={} DATA_oe={} CLK_oe={}",
                dim(0), dim(1), dim(16), dim(17)
            );
            eprintln!(
                "  strict={strict}  best-over-shift=k{bk}->{bh}  =>  {}",
                if bh + 2 < strict { "PHASE (shift helps)" } else { "VALUE (shift doesn't help)" }
            );
        }
    }
}
