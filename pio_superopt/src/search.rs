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

use crate::cost::{edge_cost, edge_cost_w, score_masked, Metric};
use crate::ir::*;
use crate::program::{Config, Program, ShiftDir};
use crate::rng::Rng;
use crate::run::{run, RunSpec};
use crate::spec_cost::spec_cost;
use std::sync::Mutex;

/// The oracle target one curriculum dataset row scores a candidate capture
/// against (ticket 005's row-type generalization). One curriculum engine, two
/// oracle row types — the ladder, retries, mining, and trace never fork; only
/// the per-row cost dispatches here.
///
/// The two variants are the two tiers' *search* metrics: [`Target::Wave`] is
/// the cycle-exact reference-waveform oracle master's testbed depends on;
/// [`Target::SpecBits`] is the loose spec oracle. Both share the banded-edit-
/// distance shape (missing = 1, spurious = `spurious_w`, matched = `Δ/(win+1)`),
/// so the same `window`/`spurious_w` knobs and the same `window = 0` strict
/// form drive either without any change to the search.
#[derive(Clone, Debug)]
pub enum Target {
    /// Cycle-exact oracle: the capture must match `golden` edge-for-edge under
    /// `mask` ([`edge_cost_w`]).
    Wave { golden: Vec<u32>, mask: Vec<u32> },
    /// Spec oracle: the capture's bit-0 transitions must land on the nearest
    /// spec-compliant DME grid for `bits` (cell `2*h`, data at `+h`, phase
    /// `<= phi_max`, polarity free — [`spec_cost`]).
    SpecBits { bits: Vec<bool>, h: usize, phi_max: usize },
}

impl Target {
    /// Smooth search cost of a candidate capture `wave` against this target,
    /// with gradient-shaping `window` (timing band) and `spurious_w`. The
    /// strict per-row error (the gate currency) is this at `window = 0`.
    pub fn search_cost(&self, wave: &[u32], window: usize, spurious_w: f64) -> f64 {
        match self {
            Target::Wave { golden, mask } => edge_cost_w(golden, wave, mask, window, spurious_w),
            Target::SpecBits { bits, h, phi_max } => spec_cost(wave, bits, *h, *phi_max, window, spurious_w),
        }
    }
}

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
    /// Enable building-block macro moves (e.g. insert a counted loop as one
    /// atomic mutation). Targets the conjunctive bottleneck — assembling the
    /// counted-loop spine — that point moves can't reach. Off by default so
    /// the baseline experiments stay reproducible.
    pub macro_moves: bool,
    /// Async island migration between the concurrent gene-annealing chains of a
    /// stage (ticket 001). `None` ⇒ chains run fully independently (the
    /// reproducible baseline). `Some` ⇒ chains share a blackboard and adopt
    /// better peers mid-run; adoption intensifies as the chain cools, so late
    /// chains converge onto the best basin (late-stage consensus). Makes a run
    /// non-deterministic (adoption depends on inter-thread timing).
    pub migrate: Option<MigrateCfg>,
    /// The smooth search-gradient metric. Default [`Metric::LevelTolerant`]
    /// preserves prior behavior; [`Metric::Edge`] scores transition events
    /// instead of per-cycle levels (for transition codes like DME, where level
    /// matching is deceptive).
    pub metric: Metric,
    /// Spurious-edge weight for the breeding engine's densified edge cost
    /// (`< 1` biases toward attempting edges). Only affects `synthesize_flat_breed`.
    pub densify_w: f64,
}

/// Async migration knobs for the gene-search chains (ticket 001). Modeled on
/// mlx86's `SolverParallelTempering` island migration: each chain periodically
/// *posts* its current gene to a shared slot and periodically *polls* a random
/// peer, adopting the peer's gene with probability rising as the cost gap grows
/// and as temperature falls.
#[derive(Debug, Clone, Copy)]
pub struct MigrateCfg {
    /// Post the current gene to the shared blackboard every `post_rate` iters.
    pub post_rate: u32,
    /// Attempt to adopt a peer every `poll_rate` iters.
    pub poll_rate: u32,
    /// Multiplier on the cost gap in the adoption rule (mlx86's
    /// `score_diff_neighbor_multiplier`). >1 adopts more eagerly.
    pub intensity: f64,
}

impl Default for MigrateCfg {
    fn default() -> Self {
        MigrateCfg { post_rate: 20, poll_rate: 50, intensity: 1.0 }
    }
}

impl Default for Params {
    fn default() -> Self {
        // Temperature is scaled to the cost gap of a single wrong pin-cycle
        // (= w). t0 ~ 2w lets correctness barriers be crossed early;
        // t_end < w makes the tail greedy on size.
        Params {
            iters: 4000,
            restarts: 24,
            t0: 128.0,
            t_end: 1.0,
            w: 64.0,
            seed_from_template: false,
            macro_moves: false,
            migrate: None,
            metric: Metric::LevelTolerant,
            densify_w: 0.5,
        }
    }
}

// Legal operand alphabets. Data-dependent control flow (MOV/OUT to PC,
// EXEC) is deliberately excluded from proposals — still representable in
// the IR, just not explored early (review gating decision).
pub(crate) const JMP_CONDS: [JmpCond; 8] = [
    JmpCond::Always, JmpCond::NotX, JmpCond::XPostDec, JmpCond::NotY,
    JmpCond::YPostDec, JmpCond::XneY, JmpCond::Pin, JmpCond::NotOsrEmpty,
];
pub(crate) const WAIT_SRCS: [WaitSrc; 4] = [WaitSrc::GpioAbs, WaitSrc::PinRel, WaitSrc::Irq, WaitSrc::JmpPin];
pub(crate) const IN_SRCS: [InSrc; 6] = [InSrc::Pins, InSrc::X, InSrc::Y, InSrc::Null, InSrc::Isr, InSrc::Osr];
pub(crate) const OUT_DSTS: [OutDst; 6] =
    [OutDst::Pins, OutDst::X, OutDst::Y, OutDst::Null, OutDst::PinDirs, OutDst::Isr];
pub(crate) const MOV_DSTS: [MovDst; 6] =
    [MovDst::Pins, MovDst::X, MovDst::Y, MovDst::PinDirs, MovDst::Isr, MovDst::Osr];
pub(crate) const MOV_OPS: [MovOp; 3] = [MovOp::None, MovOp::Invert, MovOp::BitReverse];
pub(crate) const MOV_SRCS: [MovSrc; 7] =
    [MovSrc::Pins, MovSrc::X, MovSrc::Y, MovSrc::Null, MovSrc::Status, MovSrc::Isr, MovSrc::Osr];
pub(crate) const SET_DSTS: [SetDst; 4] = [SetDst::Pins, SetDst::X, SetDst::Y, SetDst::PinDirs];

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

pub(crate) fn random_delay(rng: &mut Rng, side: &SideCfg) -> u8 {
    rng.below(side.max_delay() as u32 + 1) as u8
}

pub(crate) fn random_sideset(rng: &mut Rng, side: &SideCfg) -> Option<u8> {
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
///
/// `pub` so the eval-hot-path benches (ticket 004) can build a representative
/// starting candidate; not otherwise part of the public API.
pub fn random_program(template: &Program, space: &Space, rng: &mut Rng) -> Program {
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

/// Insert a **counted serialization** building block as one atomic move: fetch,
/// counter init, a one-instruction shift body, and a post-decrement jump that
/// closes the loop, with `wrap` set to enclose it —
///
///   pull            ; slot t    (fetch a word)
///   set x, N        ; slot t+1  (N searchable — bit count)
///   out pins, 1 [d] ; slot t+2  (the body; search may extend/replace it)
///   jmp x-- ->t+2   ; slot t+3  (loop back to the body top)
///
/// This is the spine the diagnostics pinned as the synthesis bottleneck (see
/// the decomposition experiments): point moves can't assemble it because the
/// pieces are spatially separated with no gradient between them. Critically the
/// block is **self-sufficient** — it includes `pull`, so when it lands it
/// immediately serializes *real* data (not zeros from an empty OSR), giving it
/// fitness to survive selection and accrete framing around. The search still
/// chooses the count, delays, body, and how the loop integrates with framing,
/// so novelty is preserved (it injects the idiom, not the solution).
fn insert_counted_loop(m: &mut Program, space: &Space, rng: &mut Rng) {
    let slots = space.slots as usize;
    if slots < 4 {
        return;
    }
    // Top slot leaves room for pull + set x + body + the closing jmp.
    let t = rng.below(slots as u32 - 3) as usize;
    let ss = |rng: &mut Rng| random_sideset(rng, &space.side);
    m.slots[t] = Some(Insn { op: Op::Pull { if_empty: false, block: true }, delay: 0, sideset: ss(rng) });
    m.slots[t + 1] = Some(Insn { op: Op::Set { dst: SetDst::X, data: rng.below(32) as u8 }, delay: 0, sideset: ss(rng) });
    m.slots[t + 2] = Some(Insn { op: Op::Out { dst: OutDst::Pins, count: 1 }, delay: random_delay(rng, &space.side), sideset: ss(rng) });
    m.slots[t + 3] = Some(Insn { op: Op::Jmp { cond: JmpCond::XPostDec, target: (t + 2) as u8 }, delay: 0, sideset: ss(rng) });
    if space.search_wrap {
        m.wrap_bottom = t as u8;
        m.wrap_top = (t + 3) as u8;
    }
}

/// Re-roll the immediate operand of an occupied slot's op **without changing
/// the op kind** — the loop count in `set x,N`, an `out`/`in` bit count, a
/// `jmp` target, a `wait`/`irq` index. Point moves otherwise can only change an
/// immediate by re-rolling the whole op (losing the op), so a well-placed
/// counted loop could never have its count dialed in. Ops with no immediate
/// (mov, push, pull) are left unchanged.
fn mutate_immediate(insn: &mut Insn, slots: u8, rng: &mut Rng) {
    let new = match &insn.op {
        Op::Set { dst, .. } => Op::Set { dst: *dst, data: rng.below(32) as u8 },
        Op::Out { dst, .. } => Op::Out { dst: *dst, count: 1 + rng.below(32) as u8 },
        Op::In { src, .. } => Op::In { src: *src, count: 1 + rng.below(32) as u8 },
        Op::Jmp { cond, .. } => Op::Jmp { cond: *cond, target: rng.below(slots as u32) as u8 },
        Op::Wait { polarity, src, .. } => Op::Wait { polarity: *polarity, src: *src, index: rng.below(32) as u8 },
        Op::Irq { clear, wait, .. } => Op::Irq { clear: *clear, wait: *wait, index: rng.below(32) as u8 },
        _ => return,
    };
    insn.op = new;
}

/// One mutation move. Always yields legal IR (range-aware by construction).
/// With `macros`, a fraction of moves insert a building-block idiom (see
/// [`insert_counted_loop`]) instead of a single-field point edit.
///
/// `pub` so the eval-hot-path benches (ticket 004) can measure the real
/// per-iteration move; not otherwise part of the public API.
pub fn mutate(p: &Program, space: &Space, macros: bool, rng: &mut Rng) -> Program {
    mutate_lib(p, space, macros, &[], rng)
}

/// [`mutate`] with a library of **self-mined macros** (see [`mine_macros`]):
/// when `lib` is non-empty, ~1 move in 8 splices a library idiom over a random
/// contiguous window instead of a point edit. With an empty library the RNG
/// stream is byte-identical to [`mutate`] (the splice branch draws nothing),
/// so enabling mining only on retry attempts leaves first attempts unchanged.
pub fn mutate_lib(p: &Program, space: &Space, macros: bool, lib: &[MinedMacro], rng: &mut Rng) -> Program {
    let mut m = p.clone();
    let slots = space.slots;
    // Mined-macro move: ~1 in 8 when a library is present.
    if !lib.is_empty() && rng.below(8) == 0 {
        let mac = &lib[rng.below(lib.len() as u32) as usize];
        splice_macro(&mut m, mac, slots, rng);
        return m;
    }
    // Building-block move: ~1 in 8 when enabled and the window has room.
    if macros && slots >= 4 && rng.below(8) == 0 {
        insert_counted_loop(&mut m, space, rng);
        return m;
    }
    // Roughly one move in five touches config, when any gene is live.
    if space.genes.any() && rng.below(5) == 0 {
        mutate_config_gene(&mut m.config, &space.genes, rng);
        return m;
    }
    match rng.below(8) {
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
        5 => {
            // MutateImmediate: re-roll an op's immediate, keep the op kind
            if let Some(i) = pick_occupied(&m, slots, rng) {
                if let Some(insn) = &mut m.slots[i] {
                    mutate_immediate(insn, slots, rng);
                }
            }
        }
        6 if space.search_wrap => set_random_wrap(&mut m, slots, rng), // Retarget wrap
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

/// A **self-mined macro**: a short contiguous instruction run harvested from a
/// population of search minima (rather than hand-authored — hand macros encode
/// human priors; mined macros encode what the search itself keeps discovering,
/// e.g. the OSR-refill idiom `jmp NotOsrEmpty / pull`). Jmp targets that
/// pointed inside the run (or one slot past its end — the "skip over" shape)
/// are stored RELATIVE to the run start with the matching `rel` flag set, so a
/// splice rebases them to wherever the macro lands; targets pointing elsewhere
/// stay absolute (they were context-specific — point edits can retune them).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MinedMacro {
    pub insns: Vec<Insn>,
    /// Per-insn: is this a jmp whose target is stored run-relative?
    pub rel: Vec<bool>,
}

impl MinedMacro {
    /// One-line rendering for logs, e.g. `jmp NotOsrEmpty->+2 ; pull`.
    pub fn brief(&self) -> String {
        self.insns
            .iter()
            .zip(&self.rel)
            .map(|(i, r)| {
                let s = i.brief();
                if *r { s.replace("->", "->+") } else { s }
            })
            .collect::<Vec<_>>()
            .join(" ; ")
    }
}

/// A program's op-level structure key: opcode+operands per slot plus the wrap
/// region, ignoring delays and side-sets. Two programs with the same key are
/// timing variants of the same structure — used to dedup restart minima into a
/// structurally diverse pool (64 warm restarts parked in one basin collapse to
/// a single entry).
fn structure_key(p: &Program, slots: u8) -> String {
    let ops: Vec<String> = (0..slots as usize)
        .map(|i| match &p.slots[i] {
            Some(insn) => format!("{:?}", insn.op),
            None => "-".into(),
        })
        .collect();
    format!("{}|w{}..{}", ops.join(";"), p.wrap_bottom, p.wrap_top)
}

/// Harvest recurring instruction runs (lengths 2..=3, all slots occupied) from
/// a population of programs, count them by op structure (delays/side-sets
/// ignored so timing variants of one idiom pool together; the first-seen
/// representative's fields are kept), and return the most frequent ones. Only
/// runs seen in at least two DISTINCT programs qualify — callers should dedup
/// the population by [`structure_key`] first so a monoculture basin doesn't
/// vote many times. Fully deterministic: ties order by key string.
pub fn mine_macros(pool: &[&Program], slots: u8, max: usize) -> Vec<MinedMacro> {
    use std::collections::HashMap;
    let mut counts: HashMap<String, (u32, MinedMacro)> = HashMap::new();
    for p in pool {
        let mut seen_here: std::collections::HashSet<String> = std::collections::HashSet::new();
        for len in 2..=3usize {
            if (slots as usize) < len {
                continue;
            }
            for start in 0..=(slots as usize - len) {
                let window: Option<Vec<Insn>> = (0..len).map(|k| p.slots[start + k].clone()).collect();
                let Some(mut insns) = window else { continue };
                let mut rel = vec![false; len];
                for (k, insn) in insns.iter_mut().enumerate() {
                    if let Op::Jmp { target, .. } = &mut insn.op {
                        let t = *target as usize;
                        // Internal targets (incl. one-past-end) become relative.
                        if t >= start && t <= start + len {
                            *target = (t - start) as u8;
                            rel[k] = true;
                        }
                    }
                }
                let key: String = insns
                    .iter()
                    .zip(&rel)
                    .map(|(i, r)| format!("{:?}|{r}", i.op))
                    .collect::<Vec<_>>()
                    .join(";");
                // Count each distinct run once per program.
                if !seen_here.insert(key.clone()) {
                    continue;
                }
                counts
                    .entry(key)
                    .and_modify(|e| e.0 += 1)
                    .or_insert((1, MinedMacro { insns, rel }));
            }
        }
    }
    let mut v: Vec<(String, (u32, MinedMacro))> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(&b.0)));
    v.into_iter().filter(|(_, (c, _))| *c >= 2).take(max).map(|(_, (_, m))| m).collect()
}

/// Overwrite a random contiguous window with a mined macro, rebasing its
/// relative jmp targets to the landing position (clamped into the window
/// space — a one-past-end target landing at the top edge clamps to the last
/// slot; a point edit can retune it).
fn splice_macro(m: &mut Program, mac: &MinedMacro, slots: u8, rng: &mut Rng) {
    let len = mac.insns.len();
    let slots = slots as usize;
    if len == 0 || len > slots {
        return;
    }
    let t = rng.below((slots - len + 1) as u32) as usize;
    for (k, (insn, rel)) in mac.insns.iter().zip(&mac.rel).enumerate() {
        let mut insn = insn.clone();
        if *rel {
            if let Op::Jmp { target, .. } = &mut insn.op {
                *target = (t + *target as usize).min(slots - 1) as u8;
            }
        }
        m.slots[t + k] = Some(insn);
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

/// Shared blackboard for async island migration between flat chains (ticket
/// 001, ported from the gene engine). One slot per chain holds its posted
/// current program + edge cost.
struct FlatMigration {
    slots: Vec<Mutex<Option<(Program, f64)>>>,
    cfg: MigrateCfg,
}

impl FlatMigration {
    fn new(n: u32, cfg: MigrateCfg) -> Self {
        FlatMigration { slots: (0..n).map(|_| Mutex::new(None)).collect(), cfg }
    }
    fn post(&self, idx: usize, p: &Program, cost: f64) {
        *self.slots[idx].lock().unwrap() = Some((p.clone(), cost));
    }
    fn sample_peer(&self, idx: usize, rng: &mut Rng) -> Option<(Program, f64)> {
        let n = self.slots.len();
        if n < 2 {
            return None;
        }
        let mut j = rng.below(n as u32) as usize;
        if j == idx {
            j = (j + 1) % n;
        }
        self.slots[j].lock().unwrap().clone()
    }
}

/// Edge cost with the densify bias (`spurious_w < 1`), for the breeding engine.
/// The per-candidate objective: `validate` + `run` + `edge_cost` + `size`.
///
/// `pub` so the eval-hot-path benches (ticket 004) can measure the real
/// per-candidate cost (incl. the `validate` short-circuit); not otherwise part
/// of the public API.
pub fn edge_breed_cost(p: &Program, golden: &[u32], mask: &[u32], spec: &RunSpec, w: f64, window: usize, spurious_w: f64) -> f64 {
    if p.validate().is_err() {
        return f64::INFINITY;
    }
    let wave = run(p, spec);
    w * edge_cost_w(golden, &wave, mask, window, spurious_w) + p.size() as f64
}

/// Single-range slot crossover: child = `a` with a random contiguous slot range
/// overwritten by `b`'s. Splicing can break `JMP` targets — that's the point:
/// selection discards broken offspring, and recombining slot regions is how a
/// "has clock" parent and a "has a mid fragment" parent can yield a child with
/// both — crossing the conjunctive gap no gradient reaches. Occasionally also
/// inherits `b`'s wrap.
fn crossover(a: &Program, b: &Program, slots: u8, rng: &mut Rng) -> Program {
    let mut child = a.clone();
    let s = slots as u32;
    if s == 0 {
        return child;
    }
    let lo = rng.below(s) as usize;
    let len = 1 + rng.below(s - lo as u32) as usize;
    for i in lo..(lo + len).min(slots as usize) {
        child.slots[i] = b.slots[i].clone();
    }
    if rng.boolean() {
        child.wrap_bottom = b.wrap_bottom;
        child.wrap_top = b.wrap_top;
    }
    child
}

/// One continuous breeding island: a long anneal on the densified edge objective
/// at a *fixed* `window` (no staging), interleaving local moves with
/// **recombination** — periodically pull a peer's posted program and splice it
/// with the current via [`crossover`], accepting the child by Metropolis. Posts
/// its current to the shared board for peers to breed with.
#[allow(clippy::too_many_arguments)]
fn flat_breed_chain(
    template: &Program,
    space: &Space,
    golden: &[u32],
    mask: &[u32],
    spec: &RunSpec,
    params: &Params,
    window: usize,
    seed: u64,
    board: &FlatMigration,
    idx: usize,
) -> (Program, f64) {
    let mut rng = Rng::new(seed);
    let mut cur = random_program(template, space, &mut rng);
    let mut cur_cost = edge_breed_cost(&cur, golden, mask, spec, params.w, window, params.densify_w);
    let mut local_best = (cur.clone(), cur_cost);
    let cfg = board.cfg;
    for i in 0..params.iters {
        let t = params.t0 * (params.t_end / params.t0).powf(i as f64 / params.iters as f64);
        // local move
        let cand = mutate(&cur, space, false, &mut rng);
        let cc = edge_breed_cost(&cand, golden, mask, spec, params.w, window, params.densify_w);
        if cc - cur_cost <= 0.0 || rng.unit() < (-(cc - cur_cost) / t).exp() {
            cur = cand;
            cur_cost = cc;
        }
        // cross-breeding: publish, then recombine with a peer. `poll_rate == 0`
        // disables recombination entirely (islands run independent) — the
        // no-cooperation arm of the cooperative-vs-independent A/B (dme_breed_ab).
        // Posting still runs so board overhead matches; only the crossover step
        // is toggled.
        if i % cfg.post_rate == 0 {
            board.post(idx, &cur, cur_cost);
        }
        if cfg.poll_rate != 0 && i % cfg.poll_rate == 0 {
            if let Some((peer, _)) = board.sample_peer(idx, &mut rng) {
                let child = crossover(&cur, &peer, space.slots, &mut rng);
                let ch = edge_breed_cost(&child, golden, mask, spec, params.w, window, params.densify_w);
                if ch - cur_cost <= 0.0 || rng.unit() < (-(ch - cur_cost) / t).exp() {
                    cur = child;
                    cur_cost = ch;
                }
            }
        }
        if cur_cost < local_best.1 {
            local_best = (cur.clone(), cur_cost);
        }
    }
    local_best
}

/// **Continuous cross-breeding island engine** — the post-staging path. One
/// persistent island per `windows` entry (a *fixed window ladder*: hot islands
/// explore with loose edge timing, cold islands certify — replacing the staged
/// graduated schedule that overstayed its welcome), each a long continuous
/// anneal on the densified edge objective. Islands share a board and
/// **recombine** (slot-range crossover) rather than copy, so conjunctive
/// structure assembled across different islands can be merged. Certified by
/// strict (window 0) edge cost. Runs the islands as persistent parallel threads
/// (one long-lived emulator each), so it scales to large per-island iteration
/// budgets.
pub fn synthesize_flat_breed(
    template: &Program,
    space: &Space,
    golden: &[u32],
    mask: &[u32],
    spec: &RunSpec,
    params: &Params,
    windows: &[usize],
    seed: u64,
) -> (Program, crate::cost::Score) {
    let n = windows.len() as u32;
    let cfg = params.migrate.unwrap_or_default(); // breeding always on; reuse the rate knobs
    let board = FlatMigration::new(n, cfg);
    let board_ref = &board;
    let bests: Vec<(Program, f64)> = std::thread::scope(|s| {
        let handles: Vec<_> = windows
            .iter()
            .enumerate()
            .map(|(i, &window)| {
                let cs = seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                s.spawn(move || flat_breed_chain(template, space, golden, mask, spec, params, window, cs, board_ref, i))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    // Global best by strict (window 0) edge cost — the correctness certifier.
    let mut best: Option<(Program, f64)> = None;
    for (p, _) in &bests {
        if p.validate().is_err() {
            continue;
        }
        let wave = run(p, spec);
        let ec = edge_cost(golden, &wave, mask, 0) + p.size() as f64 * 1e-6;
        if best.as_ref().map_or(true, |(_, bc)| ec < *bc) {
            best = Some((p.clone(), ec));
        }
    }
    let (p, _) = best.expect("at least one island");
    let sc = score_masked(&p, golden, mask, spec);
    (p, sc)
}

/// Summed densified edge cost of `p` over one curriculum stage: a *set* of data
/// sequences `(spec, golden, mask)` the candidate must match ALL of. Scoring
/// across multiple data values is what makes the data-CONDITIONAL toggle
/// output-visible — it is precisely the difference between the sequences' golden
/// traces — and forbids hardcoding a single waveform. Size is counted once.
fn multidata_cost(p: &Program, dataset: &[(RunSpec, Target)], w: f64, window: usize, spurious_w: f64) -> f64 {
    if p.validate().is_err() {
        return f64::INFINITY;
    }
    let mut total = 0.0;
    for (sp, target) in dataset {
        total += target.search_cost(&run(p, sp), window, spurious_w);
    }
    w * total + p.size() as f64
}

/// One eval-cache entry: an EXACT program key and the per-group raw
/// (unweighted) edge-error sums it evaluated to. `mask` records which groups'
/// sums are valid — an entry cached while a group's weight was 0 never ran
/// that group's rows.
struct EvalEntry {
    words: [u16; 32],
    wrap: (u8, u8),
    config: Config,
    mask: u32,
    raw: Box<[f64]>,
}

/// Transposition cache for candidate evaluations (ticket 004). Duplicate
/// candidates are ~32-39% of all evals across stall regimes (measured by
/// key-stream replay, 2026-07-05), and repeats are temporally local — a
/// direct-mapped table with replace-on-collision captures ~96% of the
/// unbounded-cache ceiling; no associativity or admission policy needed.
///
/// Correctness contract (this is what keeps resume byte-identical):
/// - Keys are EXACT — assembled words + wrap + full config. Two programs that
///   key equal produce identical captures, so identical raw errors.
/// - Values are per-group RAW sums; weights are applied at lookup. A hit
///   therefore recomputes `Σ wt_g * raw_g` with the same f64 operands in the
///   same order as a miss would, so hit-vs-miss is bit-identical and the
///   cache never needs snapshotting — a resumed run starts cold and still
///   reproduces the original byte-for-byte.
///
/// One cache per restart thread, dropped at attempt end (a stricter
/// invalidation than the per-rung minimum the design requires).
/// `SUPEROPT_EVAL_CACHE=0` disables it (A/B transparency checks).
struct EvalCache {
    slots: Vec<Option<Box<EvalEntry>>>,
    /// Per-group contiguous `[start, end)` row ranges of the dataset.
    ranges: Vec<(usize, usize)>,
    enabled: bool,
    lookups: u64,
    hits: u64,
}

const EVAL_CACHE_BITS: u32 = 16;

impl EvalCache {
    fn new(groups: &[usize], n_groups: usize) -> Self {
        assert!(n_groups <= 32, "group mask is a u32");
        let mut ranges = vec![(0usize, 0usize); n_groups];
        let mut prev = 0usize;
        for (i, &g) in groups.iter().enumerate() {
            assert!(g >= prev, "dataset groups must be contiguous ascending");
            if g != prev || i == 0 {
                ranges[g].0 = i;
            }
            ranges[g].1 = i + 1;
            prev = g;
        }
        let enabled = std::env::var_os("SUPEROPT_EVAL_CACHE").map(|v| v != "0").unwrap_or(true);
        EvalCache {
            slots: if enabled { (0..1usize << EVAL_CACHE_BITS).map(|_| None).collect() } else { Vec::new() },
            ranges,
            enabled,
            lookups: 0,
            hits: 0,
        }
    }

    fn slot_of(words: &[u16; 32], wrap: (u8, u8)) -> usize {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (words, wrap).hash(&mut h);
        (h.finish() & ((1u64 << EVAL_CACHE_BITS) - 1)) as usize
    }

    fn hit_rate(&self) -> f64 {
        if self.lookups == 0 { 0.0 } else { self.hits as f64 / self.lookups as f64 }
    }
}

/// Weighted multi-data cost through the eval cache, with an optional REJECT
/// BOUND (pass `f64::INFINITY` for the plain checkpoint/init variant).
///
/// Each active group's rows (weight > 0) sum to a raw subtotal first; the
/// weighted total accumulates `wt_g * raw_g` in group order. Groups whose
/// raw sums are cached skip the emulator entirely; freshly evaluated groups
/// merge into the entry, so a checkpoint re-score under new weights (or the
/// selection metric's extra lookahead group) only pays for the groups it has
/// never seen. Skipped-when-weight-0 semantics are preserved.
///
/// Bound behavior: totals accumulate non-negatively, so once the partial
/// weighted sum exceeds `bound` the caller is guaranteed to reject — return
/// `(partial, false)` without the remaining rows and without inserting. The
/// miss path also checks per-row inside a group (cheaper bail); a hit checks
/// at group boundaries only. The two agree on the `exact` flag: a per-row
/// bail inside group g implies the group-g boundary partial exceeds the
/// bound too (monotone accumulation), and both non-exact returns are
/// rejected identically, so hit-vs-miss stays bit-identical. Invalid
/// programs return `(INFINITY, false)` — the caller's non-exact path must
/// reject AND consume the Metropolis draw.
fn weighted_multidata_cost_cached(
    p: &Program,
    dataset: &[(RunSpec, Target)],
    weights: &[f64],
    w: f64,
    spurious_w: f64,
    bound: f64,
    cache: &mut EvalCache,
) -> (f64, bool) {
    if p.validate().is_err() {
        return (f64::INFINITY, false);
    }
    let n_groups = cache.ranges.len();
    let key = if cache.enabled { Some((p.assemble(), (p.wrap_bottom, p.wrap_top))) } else { None };
    let slot = key.as_ref().map(|(words, wrap)| EvalCache::slot_of(words, *wrap));
    // Pull any valid raw sums out of a key-matching entry.
    let (mut raw, mut have): (Vec<f64>, u32) = match (&key, slot) {
        (Some((words, wrap)), Some(s)) => match &cache.slots[s] {
            Some(e) if e.words == *words && e.wrap == *wrap && e.config == p.config => {
                (e.raw.to_vec(), e.mask)
            }
            _ => (vec![0.0; n_groups], 0),
        },
        _ => (vec![0.0; n_groups], 0),
    };
    let needed: u32 = (0..n_groups).filter(|&g| weights[g] > 0.0).fold(0, |m, g| m | 1 << g);
    if cache.enabled {
        cache.lookups += 1;
        if needed & !have == 0 {
            cache.hits += 1;
        }
    }
    let mut total = 0.0;
    for g in 0..n_groups {
        let wt = weights[g];
        if wt <= 0.0 {
            continue;
        }
        if have & (1 << g) == 0 {
            let (start, end) = cache.ranges[g];
            let mut sub = 0.0;
            for (sp, target) in &dataset[start..end] {
                sub += target.search_cost(&run(p, sp), 0, spurious_w);
                if w * (total + wt * sub) > bound {
                    return (w * (total + wt * sub), false);
                }
            }
            raw[g] = sub;
            have |= 1 << g;
        }
        total += wt * raw[g];
        if w * total > bound {
            return (w * total, false);
        }
    }
    if let (Some((words, wrap)), Some(s)) = (key, slot) {
        cache.slots[s] = Some(Box::new(EvalEntry {
            words,
            wrap,
            config: p.config,
            mask: have,
            raw: raw.into_boxed_slice(),
        }));
    }
    (w * total + p.size() as f64, true)
}

/// Unweighted raw edge-error total for a single length-`group` of a curriculum
/// dataset (no size term, no cost weight) — the promotion gate's currency. A
/// frontier rung is "solved" when this falls to (or below) `solve_eps`.
fn group_edge_errors(p: &Program, dataset: &[(RunSpec, Target)], groups: &[usize], group: usize, spurious_w: f64) -> f64 {
    if p.validate().is_err() {
        return f64::INFINITY;
    }
    let mut total = 0.0;
    for (i, (sp, target)) in dataset.iter().enumerate() {
        if groups[i] == group {
            total += target.search_cost(&run(p, sp), 0, spurious_w);
        }
    }
    total
}

/// The schedule **shape** meta-genome for the performance-gated curriculum ladder
/// ([`synthesize_curriculum_gated`]). The *trigger* — advance to a longer length
/// only once the current frontier length is solved — is structural and fixed;
/// these are the continuous knobs the meta-tuner anneals *around* that fixed
/// trigger. Compute scale (restarts, per-rung iters) is passed separately: it's
/// budget, not shape.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CurriculumHp {
    /// Cost weight on edge-errors vs the (once-counted) size term.
    pub w: f64,
    /// Spurious-edge weight in [`edge_cost_w`] (<1 densifies the gradient).
    pub densify_w: f64,
    /// The SCRATCH (first/L=2) rung's start temperature — the from-scratch crack
    /// of the conjunction. Hot.
    pub t0: f64,
    /// Every rung's end temperature.
    pub t_end: f64,
    /// Warm-rung **reheat**, as a fraction of the `t0..t_end` span: a promoted
    /// (warm-started) rung restarts at `t_end + reheat·(t0 − t_end)`. reheat→1 is
    /// a full reheat (hot enough to jump basins back to the level-driving
    /// attractor — destroys the loop); reheat→0 is pure low-temperature
    /// refinement (can't graft the new length's structure). The sweet spot — hot
    /// enough to extend the loop, cold enough to keep it — is the central knob.
    pub reheat: f64,
    /// **Handoff** width: the newest (frontier) length's weight ramps 0→1 over
    /// this fraction of the rung (0 = step straight to full weight). A soft
    /// handoff introduces the new length while the rung is still hot, so the
    /// search adapts to it instead of hitting a cost cliff at full weight.
    pub handoff: f64,
    /// Weight held on already-solved lengths (1 = fully anchored, never forget
    /// the structure that solved them; <1 = let old lengths fade so the frontier
    /// dominates the gradient).
    pub anchor: f64,
}

impl Default for CurriculumHp {
    fn default() -> Self {
        CurriculumHp { w: 64.0, densify_w: 0.5, t0: 128.0, t_end: 1.0, reheat: 0.15, handoff: 0.3, anchor: 1.0 }
    }
}

impl CurriculumHp {
    /// Perturb one schedule knob multiplicatively, then clamp. `w` is held fixed
    /// (it trades edge-errors against size, not schedule shape); the six tuned
    /// fields are the temperatures, reheat, handoff, anchor, and densify.
    pub fn perturb(&self, rng: &mut Rng) -> CurriculumHp {
        let mut h = *self;
        match rng.below(6) {
            0 => h.t0 = (h.t0 * meta_mult(rng)).clamp(2.0, 8192.0),
            1 => h.t_end = (h.t_end * meta_mult(rng)).clamp(0.05, 64.0),
            2 => h.reheat = (h.reheat * meta_mult(rng)).clamp(0.01, 1.0),
            3 => h.handoff = (h.handoff * meta_mult(rng)).clamp(0.02, 0.95),
            4 => h.anchor = (h.anchor * meta_mult(rng)).clamp(0.1, 1.0),
            _ => h.densify_w = (h.densify_w * meta_mult(rng)).clamp(0.05, 1.0),
        }
        if h.t_end >= h.t0 {
            h.t_end = h.t0 * 0.5;
        }
        h
    }
}

/// One restart's mid-anneal state as captured at a checkpoint barrier: the
/// state at the TOP of the checkpoint iteration (before the checkpoint block's
/// best-update runs), so a resumed run re-enters the loop at that iteration,
/// re-runs the (RNG-free, idempotent) checkpoint block, and continues the exact
/// draw sequence — byte-identical to the uninterrupted run.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RestartSnap {
    /// Full RNG state ([`Rng::state`]).
    pub rng: u64,
    pub cur: Program,
    pub best: Program,
    pub best_sel: f64,
}

/// A resumable snapshot of [`synthesize_curriculum_gated`]: everything the
/// engine needs to re-enter the ladder at (`frontier`, `attempt`, `iter`).
/// Emitted inline into the trace as [`TraceEvent::Snapshot`] — periodically at
/// heartbeat checkpoints, at attempt/rung boundaries, and on a stop request —
/// so resuming is "parse the last snapshot line of the trace".
///
/// `restarts` empty means the attempt has not started (a boundary snapshot):
/// resume initializes the restart pool exactly as a fresh attempt would.
/// Non-empty, it holds one [`RestartSnap`] per restart index and `iter` is the
/// checkpoint iteration to re-enter at.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GatedSnapshot {
    pub frontier: usize,
    pub attempt: usize,
    pub iter: u32,
    /// The rung's incoming warm base (previous rung's champion).
    pub champ: Option<Program>,
    pub solved_through: usize,
    /// Reheat in effect for this attempt (pre-escalation).
    pub reheat: f64,
    pub pool: Vec<Program>,
    pub lib: Vec<MinedMacro>,
    /// Restart minima accumulated across this rung's earlier attempts:
    /// `(best program, selection cost, frontier error)`.
    pub minima: Vec<(Program, f64, f64)>,
    pub restarts: Vec<RestartSnap>,
}

/// Structured trace event emitted by [`synthesize_curriculum_gated`] when a trace
/// sink is attached (`on_trace = Some(..)`). Every event carries the rung
/// (`frontier`) and `attempt`, and the per-restart events also the restart index
/// `r` and `iter` — enough keys that the log, which interleaves across the scoped
/// restart threads, can be grouped and ordered in post-processing. Programs are
/// passed by reference so the sink formats them (via [`Program::brief`]) only if
/// it wants to; a `None` sink costs nothing. The sink is only ever handed values
/// the search already computed (it never triggers an extra eval on the hot path),
/// so logging cannot perturb the RNG stream or the accept/reject decisions.
#[derive(Clone, Debug)]
pub enum TraceEvent<'a> {
    /// Periodic time-series sample — one per checkpoint per restart.
    Checkpoint {
        frontier: usize,
        attempt: usize,
        r: usize,
        iter: u32,
        temp: f64,
        cur_cost: f64,
        best_sel_cost: f64,
        best_frontier_err: f64,
        best_size: u8,
    },
    /// A restart's running best improved (at a checkpoint or via the miss-fix path).
    NewBest {
        frontier: usize,
        attempt: usize,
        r: usize,
        iter: u32,
        program: &'a Program,
        sel_cost: f64,
        frontier_err: f64,
        size: u8,
    },
    /// A restart finished its attempt — its final best. Emitted for ALL restarts:
    /// this is the minima-distribution data (which basins the restarts settled in).
    RestartEnd {
        frontier: usize,
        attempt: usize,
        r: usize,
        program: &'a Program,
        sel_cost: f64,
        frontier_err: f64,
        size: u8,
    },
    /// An attempt finished — the chosen rung champion plus early-stop metadata.
    AttemptEnd {
        frontier: usize,
        attempt: usize,
        program: &'a Program,
        solved: bool,
        reheat: f64,
        /// Checkpoint iteration at which early-stop fired, or `None` if the
        /// attempt ran to full budget without any restart clearing the frontier.
        early_stop_checkpoint: Option<u32>,
    },
    /// A resumable state snapshot (see [`GatedSnapshot`]). The sink should
    /// persist it machine-readably; the LAST snapshot in a trace is the
    /// resume point.
    Snapshot { snap: &'a GatedSnapshot },
}

/// **Performance-gated curriculum ladder** — the generality engine. The trigger
/// is structural: start with only the shortest length active and advance to the
/// next length **only when the current frontier is solved** (its raw edge-errors
/// fall to `solve_eps`), warm-starting each new rung from the previous champion.
/// This is the decisive fix over the open-loop weight-ramp (`synthesize_curriculum_ramp`,
/// retired — see docs/journal.md),
/// which advanced longer lengths on an iteration clock regardless of whether the
/// conjunction was assembled yet — so the search chased level-driving on the
/// longer sequence before it had a loop. Here a length cannot flood the gradient
/// until the prior structure exists.
///
/// Within a rung the temperature cools `t_start → hp.t_end` while the frontier
/// length's weight ramps in over `hp.handoff` (already-solved lengths held at
/// `hp.anchor`). The scratch rung starts hot (`hp.t0`); promoted rungs reheat
/// only partway (`hp.reheat`) so they extend the loop without jumping basins.
/// `restarts` parallel anneals run per rung. Rung selection is **lexicographic**:
/// restarts that SOLVED the frontier (edge-errors <= `solve_eps`) are preferred,
/// and among them the **generality filter** — best over all active lengths *plus
/// one length beyond* the frontier — picks the champion, so that when a length
/// admits both a general loop and a memoryless length-specific fit (both zero-cost
/// on the frontier), the loop that also handles the next length is promoted. Only
/// if no restart solved does selection fall back to argmin of the generality
/// metric. (Preferring solvers first stops a frontier-solver from losing selection
/// to a lower-metric non-solver and stalling the ladder falsely.)
///
/// A stalled frontier is RETRIED (up to a small fixed number of attempts) with
/// escalating exploration — a hotter reheat, a half-random restart pool, and
/// (from the failed attempt's restart minima) a structurally-deduped **warm
/// pool** the retry's warm restarts fan out across plus a **self-mined macro
/// library** ([`mine_macros`]) whose idioms become splice moves — and the
/// ladder only breaks once every attempt stalls. Within a rung, restarts
/// **early-stop** once any of them clears the frontier: a shared flag, read at
/// per-checkpoint barriers that keep the restarts in lockstep, makes that
/// decision a deterministic function of the seeds (independent of thread timing).
/// From restart 0 a heartbeat line streams to stderr each ~1/8 rung.
///
/// Returns `(champion, full-objective cost over every length, highest solved
/// group +1)`. A full cost below `hp.w` with `solved == n_groups` is a general
/// data-driven loop. All schedule shape lives in [`CurriculumHp`]; only compute
/// budget is passed here.
pub fn synthesize_curriculum_gated(
    template: &Program,
    space: &Space,
    dataset: &[(RunSpec, Target)],
    groups: &[usize],
    n_groups: usize,
    hp: &CurriculumHp,
    restarts: usize,
    rung_iters: u32,
    solve_eps: f64,
    seed: u64,
    on_trace: Option<&(dyn Fn(&TraceEvent) + Sync)>,
    // RESUMABILITY: `resume` re-enters the ladder at a prior run's
    // [`GatedSnapshot`]; `stop` is a cooperative interrupt — when it goes
    // true (e.g. from a Ctrl-C handler) every restart snapshots at its next
    // checkpoint barrier and the engine returns early (the caller detects
    // the interruption by re-reading the flag). Both `None` = the historical
    // behavior, RNG-stream-identical.
    resume: Option<GatedSnapshot>,
    stop: Option<&std::sync::atomic::AtomicBool>,
    mut on_rung: impl FnMut(usize, &Program, f64, bool),
) -> (Program, f64, usize) {
    let mut champ: Option<Program> = resume.as_ref().and_then(|s| s.champ.clone());
    let mut solved_through = resume.as_ref().map(|s| s.solved_through).unwrap_or(0);
    let start_frontier = resume.as_ref().map(|s| s.frontier).unwrap_or(0);
    let stopped = || stop.map(|s| s.load(std::sync::atomic::Ordering::SeqCst)).unwrap_or(false);
    for frontier in start_frontier..n_groups {
        // Selection metric (the GENERALITY FILTER): unweighted cost over every
        // active length PLUS one length beyond the frontier. A length L admits
        // two zero-cost basins — the general conjunction (a loop) and a memoryless
        // length-specific fit (level-driving) — and the gated trigger can't tell
        // them apart on the frontier alone. The lookahead group breaks the tie:
        // the conjunction partially handles the next length, the level-driving fit
        // fails it badly, so promotion always carries the loopy champion forward.
        // The anneal itself never optimizes this group (its anneal weight stays 0);
        // it only ranks which restart to promote.
        let look = (frontier + 1).min(n_groups - 1);
        let active_ones: Vec<f64> = (0..n_groups).map(|g| if g <= look { 1.0 } else { 0.0 }).collect();

        // Early-promote: once a champion is a genuine loop it already solves every
        // longer length, so re-annealing the full per-rung budget at each higher
        // rung is pure waste — if the warm-start already clears the frontier, carry
        // it up instantly. Gate on the frontier ALONE (not frontier+1): a fragile
        // near-loop that passes a length without generalizing is self-correcting —
        // it gets early-promoted one rung, then fails the next frontier and falls
        // through to a fresh anneal. Gating on the lookahead instead would block the
        // cascade entirely, since these champions verify general one length at a
        // time, so every rung would re-anneal. Quality is already protected by the
        // lookahead in rung SELECTION; this is purely the carry-forward speedup.
        if let Some(w) = &champ {
            let fe = group_edge_errors(w, dataset, groups, frontier, hp.densify_w);
            if fe <= solve_eps {
                on_rung(frontier, w, fe, true);
                solved_through = frontier + 1;
                continue;
            }
        }
        // STALL RECOVERY: the incoming champion (previous rung) is the warm base
        // for EVERY attempt of this rung. On a stalled frontier, retry the rung up
        // to `MAX_RETRIES` times with escalating exploration — a hotter reheat and a
        // half-random restart pool — before giving up and breaking the ladder.
        let warm_base = champ.clone();
        let scratch = warm_base.is_none();
        const MAX_RETRIES: usize = 2;
        // CROSS-POLLINATION + SELF-MINED MACROS (added after the 2026-07-04
        // trace analysis: on a stalled rung ALL warm restarts ended at exactly
        // the warm champ — a monoculture parked in one deep basin — while random
        // restarts independently assembled useful idioms, e.g. the OSR-refill
        // `jmp NotOsrEmpty / pull`, but plateaued 3-4x above the champ).
        // After a failed attempt the restart minima are (a) deduped by op
        // structure into a diverse WARM POOL — retry restarts fan out across it
        // instead of 32 copies of one champ — and (b) mined for recurring
        // instruction runs, which become macro splice moves so a warm restart
        // can graft a random restart's idiom without walking there point-edit
        // by point-edit. Attempt 0 keeps the pure warm start and an empty
        // library (RNG stream unchanged vs the pre-mining engine).
        const POOL_MAX: usize = 8;
        const LIB_MAX: usize = 8;
        // On the resumed rung, the rung-level state (pool/lib/minima/reheat and
        // the attempt to enter at) comes from the snapshot; every later rung
        // initializes fresh, exactly as before.
        let rs = resume.as_ref().filter(|s| s.frontier == frontier);
        let mut pool: Vec<Program> = match rs {
            Some(s) => s.pool.clone(),
            None => warm_base.iter().cloned().collect(),
        };
        let mut lib: Vec<MinedMacro> = rs.map(|s| s.lib.clone()).unwrap_or_default();
        // Restart minima accumulated across ALL attempts of this rung. Attempt 0
        // is all-warm and typically ends as a monoculture (one distinct basin),
        // so mining only the latest attempt would leave the first retry with
        // nothing; the union keeps every basin any attempt ever reached.
        let mut minima: Vec<(Program, f64, f64)> = rs.map(|s| s.minima.clone()).unwrap_or_default();
        let mut reheat = rs.map(|s| s.reheat).unwrap_or(hp.reheat);
        let start_attempt = rs.map(|s| s.attempt).unwrap_or(0);
        let mut chosen: Option<(Program, f64)> = None; // (rung champion, its frontier error)
        for attempt in start_attempt..=MAX_RETRIES {
            // Mid-anneal restore applies only to the exact resumed attempt.
            let mid: Option<&GatedSnapshot> =
                rs.filter(|s| s.attempt == attempt && !s.restarts.is_empty());
            if let Some(s) = mid {
                assert_eq!(
                    s.restarts.len(),
                    restarts.max(1),
                    "snapshot restart count must match the run's `restarts`"
                );
            }
            let start_iter = mid.map(|s| s.iter).unwrap_or(0);
            // Promoted rungs reheat only partway; the scratch rung always starts
            // hot (reheat does not apply). Each retry bumps reheat toward 1.0.
            let t_start = if scratch { hp.t0 } else { hp.t_end + reheat * (hp.t0 - hp.t_end) };
            // On retries, half the restarts start from a fresh random program
            // instead of the warm champion (mirrors `synthesize_curriculum_stage`),
            // to break out of the warm basin the earlier attempt got stuck in.
            let half_random = attempt > 0;

            // EARLY-STOP + HEARTBEAT shared state, per attempt. `solved_flag` is set
            // by any restart whose best hits frontier edge-errors <= solve_eps. The
            // per-checkpoint `barrier` forces all restarts into lockstep at
            // checkpoint granularity so the flag read is DETERMINISTIC (see the
            // determinism note below), not dependent on wall-clock thread order.
            let solved_flag = std::sync::atomic::AtomicBool::new(false);
            // The (deterministic) checkpoint iteration at which the frontier was
            // first cleared, for the AttemptEnd trace. `fetch_min` is order-free,
            // and — since the barrier makes every restart break at that same
            // checkpoint — it is only ever written at that one checkpoint.
            let early_stop_iter = std::sync::atomic::AtomicU32::new(u32::MAX);
            let barrier = std::sync::Barrier::new(restarts.max(1));
            // SNAPSHOT/STOP shared state. `stop_latch` is restart 0's sample of
            // the external `stop` flag, published before the checkpoint barrier
            // so every restart sees the SAME value after it (reading the
            // external flag directly would race: restarts could disagree
            // mid-checkpoint and desynchronize the barrier protocol).
            // `snap_slots` is the per-restart deposit box for snapshot gathers.
            let stop_latch = std::sync::atomic::AtomicBool::new(false);
            let snap_slots: std::sync::Mutex<Vec<Option<RestartSnap>>> =
                std::sync::Mutex::new(vec![None; restarts.max(1)]);

            // Each restart returns (best program, its SELECTION cost, its frontier
            // edge-error) — the frontier error is returned so rung selection needn't
            // re-evaluate it.
            let results: Vec<(Program, f64, f64)> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..restarts.max(1))
                    .map(|r| {
                        let cs = seed
                            ^ ((frontier as u64) << 40)
                            ^ ((attempt as u64) << 52)
                            ^ (r as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                        // Half-random pool on retries: odd restarts go random.
                        // Warm restarts fan out over the (diverse) warm pool.
                        let warm = if pool.is_empty() || (half_random && r % 2 == 1) {
                            None
                        } else {
                            Some(pool[(r / 2) % pool.len()].clone())
                        };
                        let active_ones = &active_ones;
                        let lib = &lib;
                        let solved_flag = &solved_flag;
                        let early_stop_iter = &early_stop_iter;
                        let barrier = &barrier;
                        let stop_latch = &stop_latch;
                        let snap_slots = &snap_slots;
                        // Read-only refs for restart 0's snapshot assembly.
                        let pool = &pool;
                        let warm_base = &warm_base;
                        let minima = &minima;
                        let mid_snap = mid;
                        s.spawn(move || {
                            let mut rng = match mid_snap {
                                Some(s) => Rng::from_state(s.restarts[r].rng),
                                None => Rng::new(cs),
                            };
                            let step = (rung_iters / 256).max(1);
                            // Heartbeat/snapshot cadence: every 32nd CHECKPOINT (~1/8
                            // rung). Counted in checkpoints, not raw iters — `i % hb`
                            // only fires when hb happens to be a multiple of `step`,
                            // which held for the 4M default budget and silently never
                            // fired for most other budgets.
                            // Weights for the frontier rung at within-rung fraction `f`.
                            // The handoff ramp only applies to PROMOTED rungs (easing a
                            // new length in alongside existing structure); the scratch
                            // rung has nothing to hand off from, so its frontier carries
                            // full weight throughout — a sub-1 weight there would briefly
                            // make the cost size-only and drift toward the empty program.
                            let weights_at = |f: f64| -> Vec<f64> {
                                let front = if scratch || hp.handoff <= 0.0 { 1.0 } else { (f / hp.handoff).min(1.0) };
                                (0..n_groups)
                                    .map(|g| if g < frontier { hp.anchor } else if g == frontier { front } else { 0.0 })
                                    .collect::<Vec<f64>>()
                            };
                            // Mid-anneal resume restores cur/best/RNG verbatim; the
                            // restored RNG must not be advanced by init (so no
                            // random_program call on this path).
                            let mut cur = match mid_snap {
                                Some(s) => s.restarts[r].cur.clone(),
                                None => match &warm {
                                    Some(w) => w.clone(),
                                    None => random_program(template, space, &mut rng),
                                },
                            };
                            // Per-restart eval cache, dropped at attempt end.
                            // Transparent (hit == miss bit-for-bit), so it is
                            // NOT part of the snapshot: resume starts cold and
                            // still reproduces the run byte-identically.
                            let mut cache = EvalCache::new(groups, n_groups);
                            let mut weights = weights_at(0.0);
                            let mut cur_cost = weighted_multidata_cost_cached(&cur, dataset, &weights, hp.w, hp.densify_w, f64::INFINITY, &mut cache).0;
                            // Track the best by the SELECTION metric (all active lengths).
                            let mut best = match mid_snap {
                                Some(s) => (s.restarts[r].best.clone(), s.restarts[r].best_sel),
                                None => (cur.clone(), weighted_multidata_cost_cached(&cur, dataset, active_ones, hp.w, hp.densify_w, f64::INFINITY, &mut cache).0),
                            };
                            for i in start_iter..rung_iters {
                                // Temp is computed here (unconditionally, unchanged) so
                                // the checkpoint trace can report it without recomputing.
                                let temp = t_start * (hp.t_end / t_start).powf(i as f64 / rung_iters.max(1) as f64);
                                if i % step == 0 {
                                    // Pre-block state stash (rng, cur, best, best_sel):
                                    // if this checkpoint takes a snapshot, it captures
                                    // the state ENTERING iteration `i` — the checkpoint
                                    // block below is RNG-free and idempotent, so a
                                    // resumed run re-enters at `i`, re-runs the block,
                                    // and continues byte-identically.
                                    let stash = (rng.state(), cur.clone(), best.0.clone(), best.1);
                                    weights = weights_at(i as f64 / rung_iters as f64);
                                    cur_cost = weighted_multidata_cost_cached(&cur, dataset, &weights, hp.w, hp.densify_w, f64::INFINITY, &mut cache).0;
                                    let active = weighted_multidata_cost_cached(&cur, dataset, active_ones, hp.w, hp.densify_w, f64::INFINITY, &mut cache).0;
                                    let improved = active < best.1;
                                    if improved {
                                        best = (cur.clone(), active);
                                    }
                                    // Solve detection on the current best; set the shared
                                    // flag if this restart has cleared the frontier.
                                    let bfe = group_edge_errors(&best.0, dataset, groups, frontier, hp.densify_w);
                                    if bfe <= solve_eps {
                                        solved_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                                        early_stop_iter.fetch_min(i, std::sync::atomic::Ordering::SeqCst);
                                    }
                                    // Trace: reuse the values just computed (no extra
                                    // evals), so on/off logging is search-identical.
                                    if let Some(tr) = on_trace {
                                        if improved {
                                            tr(&TraceEvent::NewBest {
                                                frontier, attempt, r, iter: i,
                                                program: &best.0, sel_cost: best.1, frontier_err: bfe, size: best.0.size(),
                                            });
                                        }
                                        tr(&TraceEvent::Checkpoint {
                                            frontier, attempt, r, iter: i, temp,
                                            cur_cost, best_sel_cost: best.1, best_frontier_err: bfe, best_size: best.0.size(),
                                        });
                                    }
                                    // STOP LATCH: only restart 0 samples the external
                                    // stop flag, pre-barrier; everyone reads the latch
                                    // post-barrier. All restarts therefore agree on
                                    // whether this checkpoint is stopping, keeping the
                                    // barrier protocol below in lockstep.
                                    if r == 0 {
                                        let ext = stop.map(|s| s.load(std::sync::atomic::Ordering::SeqCst)).unwrap_or(false);
                                        stop_latch.store(ext, std::sync::atomic::Ordering::SeqCst);
                                    }
                                    // Lockstep barrier: every restart is at the same
                                    // checkpoint here, so the store above (from any
                                    // restart) is visible to the load below on all of
                                    // them. This makes the early-stop decision a pure
                                    // function of the seeds, independent of thread timing.
                                    barrier.wait();
                                    let done = solved_flag.load(std::sync::atomic::Ordering::SeqCst);
                                    let stopping = stop_latch.load(std::sync::atomic::Ordering::SeqCst);
                                    // SNAPSHOT GATHER: at heartbeat cadence, and always
                                    // when stopping. Deposit, barrier (all deposited),
                                    // restart 0 assembles + emits, barrier (emit done
                                    // before anyone can re-deposit). The predicate is
                                    // uniform across restarts, so barrier counts match.
                                    let hb_checkpoint = (i / step) % 32 == 0;
                                    if on_trace.is_some() && (stopping || hb_checkpoint) {
                                        snap_slots.lock().unwrap()[r] = Some(RestartSnap {
                                            rng: stash.0,
                                            cur: stash.1,
                                            best: stash.2,
                                            best_sel: stash.3,
                                        });
                                        barrier.wait();
                                        if r == 0 {
                                            let snaps: Vec<RestartSnap> = snap_slots
                                                .lock()
                                                .unwrap()
                                                .iter_mut()
                                                .map(|s| s.take().expect("all restarts deposited at the barrier"))
                                                .collect();
                                            let gs = GatedSnapshot {
                                                frontier,
                                                attempt,
                                                iter: i,
                                                champ: warm_base.clone(),
                                                solved_through,
                                                reheat,
                                                pool: pool.clone(),
                                                lib: lib.clone(),
                                                minima: minima.clone(),
                                                restarts: snaps,
                                            };
                                            if let Some(tr) = on_trace {
                                                tr(&TraceEvent::Snapshot { snap: &gs });
                                            }
                                        }
                                        barrier.wait();
                                    }
                                    // Heartbeat: restart 0 only, ~1/8 rung, at a checkpoint
                                    // (never in the hot path).
                                    if r == 0 && hb_checkpoint {
                                        eprintln!(
                                            "  [hb] rung#{frontier} attempt {attempt} iter {i}/{rung_iters} best_sel={:.1} best_fe={bfe:.1} cache={:.0}%{}",
                                            best.1, 100.0 * cache.hit_rate(), if done { "  (frontier solved)" } else { "" }
                                        );
                                    }
                                    // Early stop once ANY restart has solved the frontier.
                                    // All restarts are at this same checkpoint (barrier),
                                    // so they all break together — no thread is left
                                    // waiting on a barrier that a broken peer will never
                                    // reach, and the break checkpoint is identical for all.
                                    if done {
                                        break;
                                    }
                                    // Cooperative interrupt: the snapshot for this
                                    // checkpoint is already in the trace (above), so
                                    // every restart can just quit here in lockstep.
                                    if stopping {
                                        break;
                                    }
                                }
                                let cand = mutate_lib(&cur, space, false, lib, &mut rng);
                                // REJECT-BOUND EARLY EXIT: a candidate whose partial
                                // cost already exceeds cur + 40*temp has acceptance
                                // probability < e^-40 (~4e-18, below the RNG's 2^-53
                                // resolution) — stop evaluating rows and reject. The
                                // discarded draw keeps the RNG stream identical to
                                // the exact path (which draws whenever cc > cur).
                                let (cc, exact) = weighted_multidata_cost_cached(
                                    &cand, dataset, &weights, hp.w, hp.densify_w,
                                    cur_cost + 40.0 * temp, &mut cache,
                                );
                                let accepted = if exact {
                                    cc - cur_cost <= 0.0 || rng.unit() < (-(cc - cur_cost) / temp).exp()
                                } else {
                                    let _ = rng.unit();
                                    false
                                };
                                if accepted {
                                    cur = cand;
                                    cur_cost = cc;
                                    // MISS FIX: the periodic sample above only checks the
                                    // selection metric every `step` iters. When the anneal
                                    // cost drops below `w` the weighted edge-error is < 1
                                    // (possibly zero) — a moment the sample can miss — so
                                    // check the selection metric immediately.
                                    if cur_cost < hp.w {
                                        let active = weighted_multidata_cost_cached(&cur, dataset, active_ones, hp.w, hp.densify_w, f64::INFINITY, &mut cache).0;
                                        if active < best.1 {
                                            best = (cur.clone(), active);
                                            if let Some(tr) = on_trace {
                                                // Frontier error is not tracked between
                                                // checkpoints; compute it only when a sink
                                                // is attached. This eval is RNG-free and
                                                // never runs with logging off, so on/off
                                                // determinism is preserved.
                                                let fe = group_edge_errors(&best.0, dataset, groups, frontier, hp.densify_w);
                                                tr(&TraceEvent::NewBest {
                                                    frontier, attempt, r, iter: i,
                                                    program: &best.0, sel_cost: active, frontier_err: fe, size: best.0.size(),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            let active = weighted_multidata_cost_cached(&cur, dataset, active_ones, hp.w, hp.densify_w, f64::INFINITY, &mut cache).0;
                            if active < best.1 {
                                best = (cur, active);
                            }
                            let fe = group_edge_errors(&best.0, dataset, groups, frontier, hp.densify_w);
                            // RestartEnd — final best of every restart (minima data).
                            if let Some(tr) = on_trace {
                                tr(&TraceEvent::RestartEnd {
                                    frontier, attempt, r,
                                    program: &best.0, sel_cost: best.1, frontier_err: fe, size: best.0.size(),
                                });
                            }
                            (best.0, best.1, fe)
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });

            // INTERRUPTED: the restarts broke at a stop checkpoint and its
            // snapshot is already in the trace — return without attempt-end
            // processing (selection/mining would advance state past the
            // snapshot). The caller distinguishes this from completion by
            // re-reading its stop flag. (RestartEnd rows from the cut-short
            // attempt do land in the trace; resume ignores them — it reads
            // only the last Snapshot.)
            if stopped() {
                let c = champ.clone().unwrap_or_else(|| template.clone());
                let full = multidata_cost(&c, dataset, hp.w, 0, hp.densify_w);
                return (c, full, solved_through);
            }

            // LEXICOGRAPHIC RUNG SELECTION: prefer restarts that SOLVED the frontier
            // (edge-errors <= solve_eps), ranking those by the selection metric (the
            // lookahead group still breaks the general-loop vs level-driving tie).
            // Only if no restart solved do we fall back to argmin selection metric.
            // This stops a frontier-solver from losing selection to a lower-metric
            // non-solver and stalling the ladder falsely.
            //
            // DETERMINISM (early-stop is result-safe): with the per-checkpoint
            // barrier all restarts break at the SAME checkpoint — the first at which
            // any restart's best cleared the frontier — which is a deterministic
            // function of the seeds. So each restart's returned triple is
            // deterministic, and the set of solvers and their selection costs do not
            // depend on thread timing. (Early-stop DOES change the result versus not
            // having it: a would-be solver that only clears the frontier at a later
            // checkpoint is cut off — but that cut-off is itself deterministic, so
            // two runs of the same seed return the identical champion. `min_by`
            // returns the first minimum, and restarts are collected in index order,
            // so metric ties resolve deterministically too.)
            let has_solver = results.iter().any(|(_, _, fe)| *fe <= solve_eps);
            let (rc, _rc_sel, rc_fe) = results
                .iter()
                .filter(|(_, _, fe)| !has_solver || *fe <= solve_eps)
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .cloned()
                .unwrap();
            let solved_this = rc_fe <= solve_eps;
            // Per-rung/attempt progress hook — fires each attempt so a long ladder
            // streams its climb (and its retries) instead of going dark.
            on_rung(frontier, &rc, rc_fe, solved_this);
            if let Some(tr) = on_trace {
                let esc = match early_stop_iter.load(std::sync::atomic::Ordering::SeqCst) {
                    u32::MAX => None,
                    v => Some(v),
                };
                tr(&TraceEvent::AttemptEnd {
                    frontier, attempt, program: &rc, solved: solved_this, reheat, early_stop_checkpoint: esc,
                });
            }
            chosen = Some((rc, rc_fe));
            if solved_this {
                break; // frontier solved — no more retries
            }
            // FAILED ATTEMPT — rebuild the warm pool and macro library from the
            // minima of EVERY attempt so far (accumulated union). Rank by
            // selection cost (stable sort: ties keep accumulation order —
            // deterministic), dedup by op structure so one basin contributes one
            // pool entry, then mine macros from ALL structurally distinct minima
            // (not just the top POOL_MAX — a mid-ranked random restart may hold
            // the needed idiom).
            minima.extend(results.iter().cloned());
            let mut ranked: Vec<&(Program, f64, f64)> = minima.iter().collect();
            ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            let mut seen = std::collections::HashSet::new();
            let distinct: Vec<&Program> =
                ranked.iter().filter(|(p, _, _)| seen.insert(structure_key(p, space.slots))).map(|(p, _, _)| p).collect();
            lib = mine_macros(&distinct, space.slots, LIB_MAX);
            pool = distinct.iter().take(POOL_MAX).map(|p| (*p).clone()).collect();
            eprintln!(
                "  [mine] rung#{frontier} attempt {attempt}: {} distinct minima -> pool={} lib={}",
                distinct.len(), pool.len(), lib.len()
            );
            for m in &lib {
                eprintln!("  [mine]   {}", m.brief());
            }
            // Escalate exploration for the next attempt.
            reheat = reheat + (1.0 - reheat) * 0.5;
        }

        let (rc, rc_fe) = chosen.expect("at least one attempt runs");
        champ = Some(rc);
        if rc_fe <= solve_eps {
            solved_through = frontier + 1;
        } else {
            break; // frontier unsolved after all retries — the ladder stops here
        }
    }
    let c = champ.expect("at least one group");
    let full = multidata_cost(&c, dataset, hp.w, 0, hp.densify_w);
    (c, full, solved_through)
}

/// **Meta-anneal** over [`CurriculumHp`] — tune the gated ladder's schedule shape.
/// SA in hyperparameter space over the curriculum genome, starting from a
/// caller-supplied `seed_hp` (so the base shape / compute budget is set outside).
/// The `eval` closure runs the ladder at a reduced budget and returns a
/// meta-cost to MINIMIZE — for generality that should be held-out validation
/// failures (+ a small size term), never training edge-cost, which overfits the
/// tuner. `meta_t0`/`meta_t_end` set the meta-temperature to the eval's scale.
pub fn meta_anneal_curriculum<F: Fn(&CurriculumHp) -> f64>(
    seed_hp: CurriculumHp,
    eval: F,
    meta_iters: u32,
    meta_t0: f64,
    meta_t_end: f64,
    seed: u64,
) -> (CurriculumHp, f64) {
    let mut rng = Rng::new(seed);
    let mut cur = seed_hp;
    let mut cur_cost = eval(&cur);
    let mut best = (cur, cur_cost);
    for i in 0..meta_iters {
        let t = meta_t0 * (meta_t_end / meta_t0).powf(i as f64 / meta_iters.max(1) as f64);
        let cand = cur.perturb(&mut rng);
        let cc = eval(&cand);
        if cc <= cur_cost || rng.unit() < (-(cc - cur_cost) / t).exp() {
            cur = cand;
            cur_cost = cc;
        }
        if cc < best.1 {
            best = (cand, cc);
        }
    }
    best
}

// ---- meta-optimization: tune the breeding engine's hyperparameters (ticket
// 002), modeled on mlx86's *_hyperparameters.cpp — the optimizer optimizes its
// own knobs. Multiplicative (scale-invariant) perturbation + hard clamps; the
// objective is a fixed-seed mini-trial of the inner search.

/// The tunable hyperparameters of the breeding engine — the meta-genome.
#[derive(Clone, Copy, Debug)]
pub struct BreedHp {
    pub w: f64,
    pub t0: f64,
    pub t_end: f64,
    pub densify_w: f64,
    pub post_rate: u32,
    pub poll_rate: u32,
    /// Hottest island's window; the ladder ramps this down to 0 across islands.
    pub max_window: usize,
}

impl Default for BreedHp {
    fn default() -> Self {
        BreedHp { w: 64.0, t0: 128.0, t_end: 1.0, densify_w: 0.5, post_rate: 20, poll_rate: 50, max_window: 8 }
    }
}

/// A uniform multiplier in [0.5, 2) — scale-invariant perturbation (mlx86).
fn meta_mult(rng: &mut Rng) -> f64 {
    let x = rng.unit() + 1.0; // [1, 2)
    if rng.boolean() {
        1.0 / x
    } else {
        x
    }
}

impl BreedHp {

    /// The breeding `Params` for this HP set at a given iteration budget.
    pub fn to_params(&self, iters: u32) -> Params {
        Params {
            iters,
            w: self.w,
            t0: self.t0,
            t_end: self.t_end,
            densify_w: self.densify_w,
            migrate: Some(MigrateCfg { post_rate: self.post_rate, poll_rate: self.poll_rate, intensity: 1.0 }),
            ..Params::default()
        }
    }

    /// A window ladder of `n` islands ramping linearly from `max_window` to 0.
    pub fn ladder(&self, n: usize) -> Vec<usize> {
        let denom = (n.max(2) - 1) as f64;
        (0..n).map(|i| ((self.max_window as f64) * (1.0 - i as f64 / denom)).round() as usize).collect()
    }
}

/// Anneal against a partial-credit `mask` (see [`crate::cost::hamming_masked`]). A curriculum
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
            let cand = mutate(&cur, space, params.macro_moves, &mut rng);
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
            autopull_pad: 0,
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
            autopull_pad: 0,
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

    #[test]
    fn mined_macros_harvest_and_splice() {
        // Two structurally distinct programs sharing the OSR-refill idiom
        // `jmp NotOsrEmpty (skip 2) ; pull` at DIFFERENT positions: mining must
        // relativize the internal skip target so the two occurrences pool into
        // one macro, and a splice must rebase that target to the landing slot.
        let mut a = spi_template();
        a.slots[4] = Some(Insn::plain(Op::Jmp { cond: JmpCond::NotOsrEmpty, target: 6 }));
        a.slots[5] = Some(Insn::plain(Op::Pull { if_empty: false, block: true }));
        let mut b = spi_template();
        b.slots[0] = Some(Insn::plain(Op::Jmp { cond: JmpCond::NotOsrEmpty, target: 2 }));
        b.slots[1] = Some(Insn::plain(Op::Pull { if_empty: false, block: true }));
        b.slots[2] = Some(Insn::nop());
        let lib = mine_macros(&[&a, &b], 10, 8);
        // Only the shared bigram is seen in >= 2 distinct programs.
        assert_eq!(lib.len(), 1, "lib: {:?}", lib.iter().map(|m| m.brief()).collect::<Vec<_>>());
        let refill = &lib[0];
        assert_eq!(refill.brief(), "jmp NotOsrEmpty->+2 ; pull");
        assert_eq!(refill.rel, vec![true, false]);
        // Splice rebases the relative target to landing-slot + 2 (clamped to
        // the top slot at the edge), for every landing position the RNG picks.
        let mut rng = Rng::new(7);
        for _ in 0..64 {
            let mut m = spi_template();
            splice_macro(&mut m, refill, 10, &mut rng);
            let t = (0..10).find(|&i| m.slots[i].is_some()).unwrap();
            match &m.slots[t].as_ref().unwrap().op {
                Op::Jmp { target, .. } => assert_eq!(*target as usize, (t + 2).min(9)),
                op => panic!("expected jmp at slot {t}, got {op:?}"),
            }
            assert!(matches!(m.slots[t + 1].as_ref().unwrap().op, Op::Pull { .. }));
        }
    }
}
