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

use crate::cost::score_masked;
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
    /// Optimization mode: every restart starts from the template program
    /// (a known-correct reference) instead of a random one. Shrinks/improves
    /// a working program rather than synthesizing from scratch.
    pub seed_from_template: bool,
}

impl Default for Params {
    fn default() -> Self {
        // Temperature is scaled to the cost gap of a single wrong pin-cycle
        // (= w). t0 ~ 2w lets correctness barriers be crossed early;
        // t_end < w makes the tail greedy on size.
        Params { iters: 4000, restarts: 24, t0: 128.0, t_end: 1.0, w: 64.0, seed_from_template: false }
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
/// genome (decision ②). Lower is better. `correctness` is masked Hamming —
/// a full (all-ones) mask is the strict metric; a partial mask scores one
/// sub-waveform (e.g. framing only) for the curriculum.
fn cost(p: &Program, golden: &[u32], mask: &[u32], spec: &RunSpec, w: f64) -> f64 {
    let s = score_masked(p, golden, mask, spec);
    if !s.valid {
        f64::INFINITY
    } else {
        w * s.correctness as f64 + s.size as f64
    }
}

/// An all-ones mask over a golden's length — the strict metric, where every
/// captured bit must match.
fn full_mask(golden: &[u32]) -> Vec<u32> {
    vec![u32::MAX; golden.len()]
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

fn consider(cand: Program, cur_cost: f64, best: &mut Option<(Program, f64)>, golden: &[u32], mask: &[u32], spec: &RunSpec, w: f64) {
    let c = cost(&cand, golden, mask, spec, w);
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
    polish_masked(start, space, golden, &full_mask(golden), spec, w, two_opt)
}

/// Masked variant of [`polish`]: local search against a partial-credit mask.
/// The strict [`polish`] is this with a full mask.
pub fn polish_masked(
    start: &Program,
    space: &Space,
    golden: &[u32],
    mask: &[u32],
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
    let mut cur_cost = cost(&cur, golden, mask, spec, w);

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
                consider(cand, cur_cost, &mut best, golden, mask, spec, w);
            }
            if cur.slots[i].is_some() {
                for &ss in &sidesets {
                    let mut cand = cur.clone();
                    if let Some(ins) = &mut cand.slots[i] {
                        ins.sideset = ss;
                    }
                    consider(cand, cur_cost, &mut best, golden, mask, spec, w);
                }
                for d in 0..=max_delay {
                    let mut cand = cur.clone();
                    if let Some(ins) = &mut cand.slots[i] {
                        ins.delay = d;
                    }
                    consider(cand, cur_cost, &mut best, golden, mask, spec, w);
                }
            }
            let mut cleared = cur.clone();
            cleared.slots[i] = None;
            consider(cleared, cur_cost, &mut best, golden, mask, spec, w);
        }

        if space.search_wrap {
            for b in 0..space.slots {
                for t in b..space.slots {
                    let mut cand = cur.clone();
                    cand.wrap_bottom = b;
                    cand.wrap_top = t;
                    consider(cand, cur_cost, &mut best, golden, mask, spec, w);
                }
            }
        }

        let g = &space.genes;
        if g.out_dir {
            for dir in [ShiftDir::Left, ShiftDir::Right] {
                let mut cand = cur.clone();
                cand.config.shift.out_dir = dir;
                consider(cand, cur_cost, &mut best, golden, mask, spec, w);
            }
        }
        if g.autopull {
            for ap in [false, true] {
                let mut cand = cur.clone();
                cand.config.shift.autopull = ap;
                consider(cand, cur_cost, &mut best, golden, mask, spec, w);
            }
        }
        if g.pull_threshold {
            for thr in 1..=32u8 {
                let mut cand = cur.clone();
                cand.config.shift.pull_threshold = thr;
                consider(cand, cur_cost, &mut best, golden, mask, spec, w);
            }
        }
        if g.clkdiv {
            for int in 1..=CLKDIV_INT_MAX {
                let mut cand = cur.clone();
                cand.config.clkdiv_int = int;
                cand.config.clkdiv_frac = 0;
                consider(cand, cur_cost, &mut best, golden, mask, spec, w);
            }
            for frac in 0..=255u8 {
                let mut cand = cur.clone();
                cand.config.clkdiv_frac = frac;
                consider(cand, cur_cost, &mut best, golden, mask, spec, w);
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
                            consider(cand, cur_cost, &mut best, golden, mask, spec, w);
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

/// Anneal against the strict (all-bits) metric and return the best program
/// and its score. Thin wrapper over [`anneal_masked`] with a full mask.
pub fn anneal(
    template: &Program,
    space: &Space,
    golden: &[u32],
    spec: &RunSpec,
    params: &Params,
    seed: u64,
) -> (Program, crate::cost::Score) {
    anneal_masked(template, space, golden, &full_mask(golden), spec, params, seed)
}

/// Anneal against a partial-credit `mask` (see [`hamming_masked`]). A curriculum
/// stage masks out the not-yet-targeted cycles (e.g. data) so the search scores
/// only the sub-waveform it should solve now; warm-starting the next stage
/// (`seed_from_template`) from this stage's champion carries the structure
/// forward. The returned `Score.correctness` is measured under the same mask.
pub fn anneal_masked(
    template: &Program,
    space: &Space,
    golden: &[u32],
    mask: &[u32],
    spec: &RunSpec,
    params: &Params,
    seed: u64,
) -> (Program, crate::cost::Score) {
    let mut rng = Rng::new(seed);
    let mut best: Option<(Program, f64)> = None;
    for _ in 0..params.restarts {
        let mut cur = if params.seed_from_template {
            template.clone()
        } else {
            random_program(template, space, &mut rng)
        };
        let mut cur_cost = cost(&cur, golden, mask, spec, params.w);
        let mut local_best = (cur.clone(), cur_cost);
        for i in 0..params.iters {
            let frac = i as f64 / params.iters as f64;
            let t = params.t0 * (params.t_end / params.t0).powf(frac);
            let cand = mutate(&cur, space, &mut rng);
            let cand_cost = cost(&cand, golden, mask, spec, params.w);
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
        let polished = polish_masked(&local_best.0, space, golden, mask, spec, params.w, false);
        let pc = cost(&polished, golden, mask, spec, params.w);
        if best.as_ref().map_or(true, |(_, bc)| pc < *bc) {
            best = Some((polished, pc));
        }
    }

    // Final polish of the global best with the 2-opt kick — grinds out
    // residuals that sit two coordinated op-swaps from the optimum.
    let (p, _) = best.expect("at least one restart");
    let p = polish_masked(&p, space, golden, mask, spec, params.w, true);
    let s = score_masked(&p, golden, mask, spec);
    (p, s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::score;
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

    /// A UART-TX program (8n1) built in IR with framing via `SET PINS`
    /// rather than side-set:
    ///   pull                 ; fetch byte
    ///   set pins, 0  [7]     ; start bit low (8 cycles)
    ///   set x, 7             ; bit counter
    /// bitloop:
    ///   out pins, 1  [6]     ; one data bit (LSB first), 8 cycles
    ///   jmp x-- bitloop
    ///   set pins, 1  [7]     ; stop bit high (8 cycles)
    /// 6 instructions: framing, a counter, a loop, delays — meaningfully
    /// harder than the 2-instruction SPI. Avoids side-set deliberately: the
    /// emulator's merge overlays a latched opt side-set value onto OUT pins
    /// every cycle (mod.rs merge_pin_outputs), so the pico same-pin
    /// side-set+OUT UART would be mis-emulated. SET and OUT both land in
    /// shared_pin_values with no overlay, so this is faithful.
    fn uart_reference() -> (Program, RunSpec) {
        const TX: u8 = 0;
        let cfg = Config {
            side: SideCfg::NONE,
            side_pindir: false,
            clkdiv_int: 1,
            clkdiv_frac: 0,
            shift: ShiftCfg {
                autopull: false,
                pull_threshold: 8,
                out_dir: ShiftDir::Right, // UART is LSB first
                ..ShiftCfg::default()
            },
            pins: PinMap { out_base: TX, out_count: 1, set_base: TX, set_count: 1, ..PinMap::default() },
            ..Config::default()
        };
        let plain = |op| Some(Insn { op, delay: 0, sideset: None });
        let delayed = |op, d| Some(Insn { op, delay: d, sideset: None });
        let mut r = Program::empty(cfg);
        r.slots[0] = plain(Op::Pull { if_empty: false, block: true });
        r.slots[1] = delayed(Op::Set { dst: SetDst::Pins, data: 0 }, 7); // start, low, 8 cycles
        r.slots[2] = plain(Op::Set { dst: SetDst::X, data: 7 });
        r.slots[3] = delayed(Op::Out { dst: OutDst::Pins, count: 1 }, 6); // data bit, 7 cycles
        r.slots[4] = plain(Op::Jmp { cond: JmpCond::XPostDec, target: 3 }); // loop -> 8 cycles/bit
        r.slots[5] = delayed(Op::Set { dst: SetDst::Pins, data: 1 }, 7); // stop, high, 8 cycles
        r.wrap_bottom = 0;
        r.wrap_top = 5;
        let spec = RunSpec {
            block: 0,
            sm: 0,
            // Several distinct bytes (~82 cycles/frame): a program that
            // ignores the input and replays one fixed pattern (e.g. driving
            // the pin from the loop counter) can't match all of them. This
            // is the anti-overfitting guard — a single byte is trivially
            // overfittable.
            inputs: vec![0x55, 0x3C, 0xF0, 0x41],
            output_pins: vec![TX],
            capture_pins: vec![TX],
            cycles: 336,
        };
        (r, spec)
    }

    /// EXPERIMENT (not a hard assertion): does optimization-mode shrinking of
    /// a known-correct UART reference stay tractable, while synthesis from
    /// scratch falls off a cliff? Validates the scaling strategy before DME.
    #[test]
    #[ignore = "experiment; run with: cargo test --release -- --ignored uart_tx --nocapture"]
    fn uart_tx_optimization_vs_synthesis() {
        let (reference, spec) = uart_reference();
        assert!(reference.validate().is_ok(), "{:?}", reference.validate());
        let golden = run(&reference, &spec);
        let tx: String = golden.iter().map(|s| if s & 1 != 0 { '#' } else { '_' }).collect();
        let oe: String = golden.iter().map(|s| if (s >> 16) & 1 != 0 { '#' } else { '_' }).collect();
        eprintln!("UART TX 0x55 level: {tx}");
        eprintln!("UART TX 0x55 oe:    {oe}");
        let rs = score(&reference, &golden, &spec);
        eprintln!("reference: size={} correctness={}", rs.size, rs.correctness);

        let side = SideCfg::NONE;

        // OPTIMIZATION MODE: every restart starts from the reference; the
        // search tries to shrink it while staying correct.
        let opt_space = Space { slots: 7, side, search_wrap: true, genes: Genes::default() };
        let opt_params = Params { iters: 3000, restarts: 16, seed_from_template: true, ..Params::default() };
        let (opt_best, opt_s) = anneal(&reference, &opt_space, &golden, &spec, &opt_params, 0x0AA0);
        eprintln!(
            "\nOPTIMIZE (seed=reference): correctness={} size={}  [ref size {}]",
            opt_s.correctness, opt_s.size, rs.size
        );
        for i in 0..opt_space.slots as usize {
            if let Some(ins) = &opt_best.slots[i] {
                eprintln!("  slot{i}: {:?}", ins);
            }
        }

        // SYNTHESIS MODE: same config, but from scratch over a 6-slot window.
        let syn_template = {
            let (r, _) = uart_reference();
            Program::empty(r.config)
        };
        let syn_space = Space { slots: 8, side, search_wrap: true, genes: Genes::default() };
        let syn_params = Params { iters: 8000, restarts: 48, ..Params::default() };
        let (_syn_best, syn_s) = anneal(&syn_template, &syn_space, &golden, &spec, &syn_params, 0x0AA1);
        eprintln!(
            "\nSYNTHESIZE (from scratch): correctness={} size={}  (0 = perfect)",
            syn_s.correctness, syn_s.size
        );

        eprintln!(
            "\n=> optimization residual {} vs synthesis residual {}",
            opt_s.correctness, syn_s.correctness
        );
    }

    /// A k-data-bit UART-TX frame (8n1 framing, k bits of payload). Same
    /// structure as `uart_reference` for every k — only the bit counter
    /// (`set x, k-1`) changes. The golden waveform is start-low (8 cyc), then
    /// k data bits (8 cyc each, LSB first), then stop-high (8 cyc), per byte.
    ///
    /// This is the rung generator for the bit-count curriculum: rung k targets
    /// a k-bit frame. The loop-free optimum unrolls to `pull / set0 / out[7]×k
    /// / set1` (k+3 slots, grows by one `out` per rung); the looped optimum is
    /// the 6-slot reference for *every* k (only `set x` differs). Either way,
    /// rung k's solution is a short edit from rung k+1's.
    /// Framing-free **data loop**: serialize a k-bit byte LSB-first to the pin,
    /// 8 cycles per bit, back-to-back across bytes — no start/stop framing. This
    /// is the conjunctive core of UART (FIFO read + counted shift loop) in
    /// isolation; structurally it's SPI-TX without the clock, which we know
    /// synthesizes from scratch. Fork A synthesizes this fragment, then composes
    /// framing around it.
    ///
    ///   pull          ; fetch byte
    ///   set x, k-1    ; bit counter
    /// loop:
    ///   out pins, 1 [6]   ; one bit, 7 cyc
    ///   jmp x-- loop      ; +1 = 8 cyc/bit
    fn data_loop_reference(k: u8) -> (Program, RunSpec) {
        const TX: u8 = 0;
        let cfg = Config {
            side: SideCfg::NONE,
            side_pindir: false,
            clkdiv_int: 1,
            clkdiv_frac: 0,
            shift: ShiftCfg {
                autopull: false,
                pull_threshold: 8,
                out_dir: ShiftDir::Right, // LSB first
                ..ShiftCfg::default()
            },
            pins: PinMap { out_base: TX, out_count: 1, set_base: TX, set_count: 1, ..PinMap::default() },
            ..Config::default()
        };
        let plain = |op| Some(Insn { op, delay: 0, sideset: None });
        let delayed = |op, d| Some(Insn { op, delay: d, sideset: None });
        let mut r = Program::empty(cfg);
        r.slots[0] = plain(Op::Pull { if_empty: false, block: true });
        r.slots[1] = plain(Op::Set { dst: SetDst::X, data: k - 1 });
        r.slots[2] = delayed(Op::Out { dst: OutDst::Pins, count: 1 }, 6); // 7 cyc
        r.slots[3] = plain(Op::Jmp { cond: JmpCond::XPostDec, target: 2 }); // +1 = 8 cyc/bit
        r.wrap_bottom = 0;
        r.wrap_top = 3;
        // Per byte: pull(1) + setx(1) + k*(out[6]=7 + jmp=1).
        let frame = 2 + 8 * k as u64;
        let spec = RunSpec {
            block: 0,
            sm: 0,
            inputs: vec![0x55, 0x3C, 0xF0, 0x41],
            output_pins: vec![TX],
            capture_pins: vec![TX],
            cycles: 4 * frame,
        };
        (r, spec)
    }

    fn k_bit_uart_reference(k: u8) -> (Program, RunSpec) {
        const TX: u8 = 0;
        let cfg = Config {
            side: SideCfg::NONE,
            side_pindir: false,
            clkdiv_int: 1,
            clkdiv_frac: 0,
            shift: ShiftCfg {
                autopull: false,
                pull_threshold: 8,
                out_dir: ShiftDir::Right, // UART is LSB first
                ..ShiftCfg::default()
            },
            pins: PinMap { out_base: TX, out_count: 1, set_base: TX, set_count: 1, ..PinMap::default() },
            ..Config::default()
        };
        let plain = |op| Some(Insn { op, delay: 0, sideset: None });
        let delayed = |op, d| Some(Insn { op, delay: d, sideset: None });
        let mut r = Program::empty(cfg);
        r.slots[0] = plain(Op::Pull { if_empty: false, block: true });
        r.slots[1] = delayed(Op::Set { dst: SetDst::Pins, data: 0 }, 7); // start, low, 8 cyc
        r.slots[2] = plain(Op::Set { dst: SetDst::X, data: k - 1 }); // bit counter
        r.slots[3] = delayed(Op::Out { dst: OutDst::Pins, count: 1 }, 6); // data bit, 7+jmp = 8 cyc
        r.slots[4] = plain(Op::Jmp { cond: JmpCond::XPostDec, target: 3 });
        r.slots[5] = delayed(Op::Set { dst: SetDst::Pins, data: 1 }, 7); // stop, high, 8 cyc
        r.wrap_bottom = 0;
        r.wrap_top = 5;
        // Per byte: pull(1) + set0[7](8) + setx(1) + k*(out[6]=7 + jmp=1) + set1[7](8).
        let frame = 18 + 8 * k as u64;
        let spec = RunSpec {
            block: 0,
            sm: 0,
            // Distinct low bits across the four bytes defeat the "ignore the
            // input, replay a constant" exploit even at k=1 (low bits 1,0,0,1).
            inputs: vec![0x55, 0x3C, 0xF0, 0x41],
            output_pins: vec![TX],
            capture_pins: vec![TX],
            cycles: 4 * frame,
        };
        (r, spec)
    }

    /// Compact one-line rendering of a program's occupied slots + wrap, for
    /// experiment logs.
    fn brief(ins: &Insn) -> String {
        let d = if ins.delay > 0 { format!("[{}]", ins.delay) } else { String::new() };
        let ss = match ins.sideset {
            Some(v) => format!(".s{v}"),
            None => String::new(),
        };
        let op = match &ins.op {
            Op::Jmp { cond, target } => format!("jmp {cond:?}->{target}"),
            Op::Wait { polarity, src, index } => format!("wait{} {src:?}{index}", *polarity as u8),
            Op::In { src, count } => format!("in {src:?},{count}"),
            Op::Out { dst, count } => format!("out {dst:?},{count}"),
            Op::Push { .. } => "push".into(),
            Op::Pull { .. } => "pull".into(),
            Op::Mov { dst, op, src } => format!("mov {dst:?},{op:?}{src:?}"),
            Op::Irq { index, .. } => format!("irq {index}"),
            Op::Set { dst, data } => format!("set {dst:?},{data}"),
        };
        format!("{op}{ss}{d}")
    }

    fn show(p: &Program, slots: u8) -> String {
        let parts: Vec<String> = (0..slots as usize)
            .filter_map(|i| p.slots[i].as_ref().map(|ins| format!("{i}:{}", brief(ins))))
            .collect();
        format!("wrap({},{}) [{}]", p.wrap_bottom, p.wrap_top, parts.join("  "))
    }

    /// DIAGNOSTIC: is the *base* of the bit-count curriculum — a single 1-bit
    /// UART frame — even synthesizable from scratch? The full ramp plateaued at
    /// rung 1 (correctness 16, a degenerate pin-toggle). This isolates whether
    /// that's a budget problem (5-slot window + free delays + required `pull` is
    /// a far bigger space than SPI's 2 slots) or a conjunction wall (a 1-bit
    /// frame is still load+drive+frame+time simultaneously, with no partial
    /// credit). Tight 4-slot window, large budget, several seeds.
    ///
    /// Run: `cargo test --release -- --ignored uart_k1_base --nocapture`
    #[test]
    #[ignore = "diagnostic; run with --release ... --nocapture"]
    fn uart_k1_base_solvable() {
        let side = SideCfg::NONE;
        let (reference, spec) = k_bit_uart_reference(1);
        let golden = run(&reference, &spec);

        // Sanity: the true reference scores 0, and optimization-mode holds it.
        let rs = score(&reference, &golden, &spec);
        eprintln!("reference: correctness={} size={}  [{}]", rs.correctness, rs.size, show(&reference, 6));
        assert_eq!(rs.correctness, 0, "reference must match itself");

        // From scratch, tight window, big budget, multiple seeds.
        let space = Space { slots: 4, side, search_wrap: true, genes: Genes::default() };
        let params = Params { iters: 15000, restarts: 48, seed_from_template: false, ..Params::default() };
        let template = Program::empty(reference.config);
        let mut best_overall = u32::MAX;
        for seed in 1u64..=6 {
            let (best, s) = anneal(&template, &space, &golden, &spec, &params, seed);
            best_overall = best_overall.min(s.correctness);
            eprintln!("seed {seed}: correctness={:>3} size={} | {}", s.correctness, s.size, show(&best, 4));
        }
        eprintln!("\n=> best correctness over 6 seeds: {best_overall} (0 = base is synthesizable)");
    }

    /// CURRICULUM EXPERIMENT (not a hard assertion): does a bit-count ramp,
    /// warm-starting each rung from the previous rung's champion, bridge the
    /// synthesis cliff that defeats cold UART synthesis (correctness ~67)?
    ///
    /// Rung k targets a k-bit UART frame in a (k+4)-slot window. Rung 1 is cold
    /// synthesis (no prior). Rungs 2..8 seed every restart from the previous
    /// champion. A cold-synthesis control at k=8 measures the cliff in this same
    /// harness for comparison. Reports correctness/size/structure per rung —
    /// where (if anywhere) the ladder cliffs tells us whether the barrier is
    /// reach (warm-start bridges it) or a structural seam (needs composition).
    ///
    /// Run: `cargo test --release -- --ignored uart_curriculum --nocapture`
    #[test]
    #[ignore = "curriculum experiment; run with --release ... --nocapture"]
    fn uart_curriculum_bit_ramp() {
        let side = SideCfg::NONE;
        let mut champion: Option<Program> = None;

        eprintln!("k | win | seed | correctness | size | structure");
        eprintln!("--+-----+------+-------------+------+----------");
        for k in 1u8..=8 {
            let (reference, spec) = k_bit_uart_reference(k);
            assert!(reference.validate().is_ok(), "{:?}", reference.validate());
            let golden = run(&reference, &spec);
            let slots = k + 4; // unrolled (k+3) plus a slack slot
            let space = Space { slots, side, search_wrap: true, genes: Genes::default() };

            let (template, seed_kind, params) = match &champion {
                Some(c) => (
                    c.clone(),
                    "warm",
                    Params { iters: 3000, restarts: 8, seed_from_template: true, ..Params::default() },
                ),
                None => (
                    Program::empty(reference.config),
                    "cold",
                    Params { iters: 6000, restarts: 24, seed_from_template: false, ..Params::default() },
                ),
            };

            let (best, s) = anneal(&template, &space, &golden, &spec, &params, 0xC0FFEE + k as u64);
            eprintln!(
                "{k} | {slots:>3} | {seed_kind} | {:>11} | {:>4} | {}",
                s.correctness,
                s.size,
                show(&best, slots),
            );
            champion = Some(best);
        }

        // CONTROL: cold synthesis straight at k=8, same harness, comparable
        // total budget to the curriculum (~8 rungs worth). Expected to cliff.
        let (reference, spec) = k_bit_uart_reference(8);
        let golden = run(&reference, &spec);
        let space = Space { slots: 12, side, search_wrap: true, genes: Genes::default() };
        let template = Program::empty(reference.config);
        let params = Params { iters: 8000, restarts: 48, seed_from_template: false, ..Params::default() };
        let (cold, cs) = anneal(&template, &space, &golden, &spec, &params, 0x0AA1);
        eprintln!("\nCONTROL cold k=8: correctness={} size={}", cs.correctness, cs.size);
        eprintln!("  {}", show(&cold, 12));
    }

    /// Per-cycle, per-bit "framing" mask: care about every captured bit that is
    /// **data-independent** — identical across two input sets that differ in
    /// every data bit. Running the reference on the real bytes and on their
    /// complement, the cycles/bits that agree are framing/idle/OE (not data);
    /// the data cycles diverge and become don't-care. No hardcoded cycle math —
    /// this derives the mask from the protocol's own behavior, so it generalizes.
    fn framing_mask(reference: &Program, spec: &RunSpec) -> (Vec<u32>, Vec<u32>) {
        let golden_a = run(reference, spec);
        let spec_b = RunSpec {
            inputs: spec.inputs.iter().map(|w| w ^ 0xFF).collect(), // flip all 8 data bits
            ..spec.clone()
        };
        let golden_b = run(reference, &spec_b);
        let mask: Vec<u32> = golden_a
            .iter()
            .zip(&golden_b)
            .map(|(a, b)| !(a ^ b)) // care where the two runs agree
            .collect();
        (golden_a, mask)
    }

    /// Autopull serializer: continuous LSB-first serialization with **autopull
    /// ON** (threshold 8) and a free `wrap` loop — no explicit `pull`, no
    /// counter. This is the SPI-style structure that avoids the counted-loop
    /// spine. Contrast with `data_loop_reference` (explicit pull+counter+jmp).
    ///
    ///   out pins, 1 [7]   ; wrap(0,0); OSR auto-refills every 8 bits
    fn serializer_autopull_reference() -> (Program, RunSpec) {
        const TX: u8 = 0;
        let cfg = Config {
            side: SideCfg::NONE,
            side_pindir: false,
            clkdiv_int: 1,
            clkdiv_frac: 0,
            shift: ShiftCfg {
                autopull: true,
                pull_threshold: 8,
                out_dir: ShiftDir::Right,
                ..ShiftCfg::default()
            },
            pins: PinMap { out_base: TX, out_count: 1, set_base: TX, set_count: 1, ..PinMap::default() },
            ..Config::default()
        };
        let mut r = Program::empty(cfg);
        r.slots[0] = Some(Insn { op: Op::Out { dst: OutDst::Pins, count: 1 }, delay: 7, sideset: None });
        r.wrap_bottom = 0;
        r.wrap_top = 0;
        let spec = RunSpec {
            block: 0,
            sm: 0,
            inputs: vec![0x55, 0x3C, 0xF0, 0x41],
            output_pins: vec![TX],
            capture_pins: vec![TX],
            cycles: 4 * 8 * 8, // 4 bytes * 8 bits * 8 cyc/bit
        };
        (r, spec)
    }

    /// CONTROL for the spine hypothesis: the autopull serializer (no spine)
    /// should synthesize to ~0, where the explicit-spine `data_loop` plateaus
    /// at 6/26. If so, the conjunctive obstacle is the FIFO-management +
    /// counted-loop spine, not framing and not length.
    ///
    /// Run: `cargo test --release -- --ignored serializer_autopull --nocapture`
    #[test]
    #[ignore = "control; run with --release ... --nocapture"]
    fn serializer_autopull_synthesizes() {
        let side = SideCfg::NONE;
        let (reference, spec) = serializer_autopull_reference();
        assert!(reference.validate().is_ok(), "{:?}", reference.validate());
        let golden = run(&reference, &spec);
        assert_eq!(score(&reference, &golden, &spec).correctness, 0);

        let space = Space { slots: 3, side, search_wrap: true, genes: Genes::default() };
        let template = Program::empty(reference.config);
        let params = Params { iters: 4000, restarts: 12, seed_from_template: false, ..Params::default() };
        let mut best: Option<(Program, crate::cost::Score)> = None;
        for seed in 1u64..=4 {
            let (b, s) = anneal(&template, &space, &golden, &spec, &params, 0x5E21A1 + seed);
            if best.as_ref().map_or(true, |(_, bs)| s.correctness < bs.correctness) {
                best = Some((b, s));
            }
        }
        let (b, s) = best.unwrap();
        eprintln!("autopull serializer: best correctness={} size={}", s.correctness, s.size);
        eprintln!("  {}", show(&b, 3));
    }

    /// FORK A, STEP 1 (premise + fragment): does the framing-free data loop
    /// synthesize from scratch? It's the conjunctive core of UART in isolation.
    /// If this reaches strict-0 where cold *full*-UART synthesis cliffs, the
    /// decomposition boundary is confirmed: the hard part is the data loop, and
    /// framing is a cheap wrap. Prints the synthesized fragment so the
    /// composition operator can be designed around its actual structure.
    ///
    /// Run: `cargo test --release -- --ignored uart_data_loop --nocapture`
    #[test]
    #[ignore = "fragment synthesis; run with --release ... --nocapture"]
    fn uart_data_loop_synthesizes() {
        let side = SideCfg::NONE;
        for k in [4u8, 8] {
            let (reference, spec) = data_loop_reference(k);
            assert!(reference.validate().is_ok(), "{:?}", reference.validate());
            let golden = run(&reference, &spec);
            let rs = score(&reference, &golden, &spec);
            assert_eq!(rs.correctness, 0, "reference must match itself");

            let slots = k + 3; // room for the looped (4) or a short unroll
            let space = Space { slots, side, search_wrap: true, genes: Genes::default() };
            let template = Program::empty(reference.config);
            let params =
                Params { iters: 8000, restarts: 24, seed_from_template: false, ..Params::default() };
            let mut best: Option<(Program, crate::cost::Score)> = None;
            for seed in 1u64..=4 {
                let (b, s) = anneal(&template, &space, &golden, &spec, &params, 0xDA7A + seed);
                if best.as_ref().map_or(true, |(_, bs)| s.correctness < bs.correctness) {
                    best = Some((b, s));
                }
            }
            let (b, s) = best.unwrap();
            eprintln!(
                "k={k} window={slots} ref-size={}: best correctness={} size={}",
                rs.size, s.correctness, s.size
            );
            eprintln!("  {}", show(&b, slots));
        }
    }

    /// MASKED CURRICULUM EXPERIMENT (not a hard assertion): the central test of
    /// the decomposition thesis. The flat conjunctive landscape has no partial
    /// credit until load+drive+frame+time all align at once — cold synthesis of
    /// even a 1-bit frame plateaus (see `uart_k1_base_solvable`, residual ~10).
    ///
    /// Manufacture a gradient with a two-stage curriculum on one program:
    ///   Stage A (framing): data cycles masked don't-care, so the search solves
    ///     only start/stop framing + frame timing + keep-pin-driven (OE). The
    ///     hardest conjunct — exact data values — is removed.
    ///   Stage B (strict): warm-start every restart from Stage A's champion and
    ///     score all bits. Framing is already held, so the *only* unsatisfied
    ///     conjunct is the data path — and each correctly-driven bit now yields
    ///     immediate partial credit. The gradient we lacked.
    ///
    /// If Stage B reaches strict correctness 0 where cold synthesis cliffs, the
    /// barrier was reach (the curriculum bridges it) and no composition operator
    /// is needed. Reports both stages per k; the cold control is in
    /// `uart_curriculum_bit_ramp` / `uart_k1_base_solvable`.
    ///
    /// Run: `cargo test --release -- --ignored uart_masked_curriculum --nocapture`
    #[test]
    #[ignore = "curriculum experiment; run with --release ... --nocapture"]
    fn uart_masked_curriculum() {
        let side = SideCfg::NONE;

        for k in [1u8, 4, 8] {
            let (reference, spec) = k_bit_uart_reference(k);
            assert!(reference.validate().is_ok(), "{:?}", reference.validate());
            let (golden, mask) = framing_mask(&reference, &spec);
            let strict = full_mask(&golden);
            let cared: u32 = mask.iter().map(|m| m.count_ones()).sum();
            let total: u32 = strict.iter().map(|m| m.count_ones()).sum();
            let slots = k + 4;
            let space = Space { slots, side, search_wrap: true, genes: Genes::default() };

            eprintln!(
                "\n===== k={k}  window={slots}  framing-mask cares {cared}/{total} bits =====",
            );

            // Stage A: framing only, cold synthesis.
            let template = Program::empty(reference.config);
            let a_params =
                Params { iters: 6000, restarts: 16, seed_from_template: false, ..Params::default() };
            let (champ_a, sa) =
                anneal_masked(&template, &space, &golden, &mask, &spec, &a_params, 0xC0FFEE + k as u64);
            // How good/bad is the framing champion under the *strict* metric?
            let a_strict = score_masked(&champ_a, &golden, &strict, &spec).correctness;
            eprintln!(
                "  A framing : masked-correctness={} size={} | strict-residual={}",
                sa.correctness, sa.size, a_strict
            );
            eprintln!("    {}", show(&champ_a, slots));

            // Stage B: strict, warm-started from the framing champion.
            let b_params =
                Params { iters: 4000, restarts: 12, seed_from_template: true, ..Params::default() };
            let (champ_b, sb) =
                anneal_masked(&champ_a, &space, &golden, &strict, &spec, &b_params, 0xBEEF + k as u64);
            eprintln!(
                "  B strict  : correctness={} size={}  (0 = full frame solved)",
                sb.correctness, sb.size
            );
            eprintln!("    {}", show(&champ_b, slots));
        }
    }
}
