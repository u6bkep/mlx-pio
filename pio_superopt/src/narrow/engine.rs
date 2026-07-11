//! The needed-narrowing fork loop over hole programs.
//!
//! A candidate is a PARTIAL assignment of the 32x16-bit instruction
//! space: per slot, a `decided` bit-mask and the decided bits' `value`.
//! Evaluation runs the certified interpreter ([`super::step`]) until it
//! is about to FETCH a slot with undecided consulted fields — then it
//! forks one child per legal value of the FIRST such field (semantic
//! order: side-set, opcode, operands, delay), and abandons the parent.
//! A cycle whose captured output disagrees with the spec kills the item
//! and with it the entire subspace of every still-undecided field.
//! An item that survives all cycles is a champion; its undecided fields
//! are don't-cares (statically dead bits, never-fetched slots, and the
//! opt-side-set value bits under a clear enable are never forked at all).
//!
//! v1 scope, documented trade-offs:
//! - Demand is EAGER WITHIN A FETCH: every field the opcode can consult
//!   is decided before the instruction runs (JMP targets are not
//!   cond-lazy; delay is demanded at fetch, not completion). Sound —
//!   over-demanding only costs don't-care coverage, never correctness.
//! - Canonicality filter P1-lite (OFF by default): register-symmetry
//!   breaking — while no scratch register has been named, field values
//!   whose only register is Y are pruned (the X-renamed twin lives in a
//!   kept branch); `JMP X!=Y` names both at once (its own mirror). It is
//!   a FLAG because X/Y renaming is NOT a true ISA symmetry: a
//!   non-blocking or `if_empty` PULL on an empty TX FIFO loads X
//!   specifically, so a pruned Y-first program containing such a PULL
//!   has no behavioral twin in the kept branch. Enable only for spaces
//!   where pull-on-empty is unreachable; the full P1 (virtual registers
//!   with a link binding that models PULL's implicit X) replaces this.
//! - No memoization, plain DFS (checkpoint prefix sharing comes free:
//!   children copy the parent's `NState` at the fork cycle).

use super::{clock_tick, compose, still_stalled, NCfg, NState, Stall, Stim};

/// One search problem: a fixed config, a driver schedule, and the exact
/// per-cycle trace a candidate must reproduce (trace_pads format: bit j
/// = level of `capture_pins[j]`, bit 16+j = its output-enable).
#[derive(Debug, Clone)]
pub struct EngineSpec {
    /// SM config. `cfg.code` is ignored — the hole program owns the code.
    pub cfg: NCfg,
    /// Slots 0..slots are searchable; slots >= slots are fixed NOPs.
    pub slots: u8,
    pub cycles: u32,
    /// TX FIFO words: pre-loaded if <= 4, else streamed (refill-to-full
    /// before each cycle) — the `run::run` driver contract.
    pub inputs: Vec<u32>,
    pub output_pins: Vec<u8>,
    pub capture_pins: Vec<u8>,
    pub stim: Stim,
    /// IRQ stimulus: `irq_flags |= mask` at the start of each listed
    /// cycle (models a sibling SM's `irq set`).
    pub irq_sets: Vec<(u32, u8)>,
    /// Expected trace, one word per cycle.
    pub expected: Vec<u32>,
    /// P1-lite register-symmetry pruning. UNSOUND when a non-blocking /
    /// if_empty PULL can execute on an empty TX FIFO (see module docs);
    /// leave off unless the space provably excludes that.
    pub p1_register_symmetry: bool,
}

/// A surviving candidate subspace: decided fields plus don't-cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Champion {
    pub decided: [u16; 32],
    pub value: [u16; 32],
}

impl Champion {
    /// Materialize the canonical concrete program (don't-cares = 0).
    pub fn words(&self) -> [u16; 32] {
        self.value
    }
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub items: u64,
    pub forks: u64,
    pub refuted: u64,
    pub champions_found: u64,
    pub cycles_run: u64,
}

pub struct SearchResult {
    pub champions: Vec<Champion>,
    pub stats: Stats,
    /// True if the champion list was truncated at the cap.
    pub champion_cap_hit: bool,
}

/// A work item: partial assignment + evaluator checkpoint. Copy — a
/// fork is a struct copy plus one field write.
#[derive(Clone, Copy)]
struct Item {
    decided: [u16; 32],
    value: [u16; 32],
    st: NState,
    cycle: u32,
    next_input: u32,
    seen_x: bool,
    seen_y: bool,
}

/// A forkable bit-field of one slot: contiguous `mask`, candidate raw
/// values produced by `values_into`.
struct Field {
    mask: u16,
    kind: FieldKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    Side,
    Delay,
    Opcode,
    JmpCond,
    JmpTarget,
    WaitPol,
    WaitSrc,
    WaitIdx,
    InSrc,
    BitCount, // IN/OUT count field (raw 0 = 32)
    OutDst,
    PushPullBits,
    MovDst,
    MovOp,
    MovSrc,
    IrqBits,
    IrqIdx,
    SetDst,
    SetData,
}

/// Which scratch registers a candidate field value names (for P1-lite).
fn names_regs(kind: FieldKind, v: u16) -> (bool, bool) {
    match kind {
        FieldKind::JmpCond => match v {
            1 | 2 => (true, false), // !X, X--
            3 | 4 => (false, true), // !Y, Y--
            5 => (true, true),      // X != Y (its own mirror)
            _ => (false, false),
        },
        FieldKind::InSrc | FieldKind::OutDst | FieldKind::MovDst | FieldKind::MovSrc
        | FieldKind::SetDst => match v {
            1 => (true, false),
            2 => (false, true),
            _ => (false, false),
        },
        _ => (false, false),
    }
}

impl Field {
    fn shift(&self) -> u16 {
        self.mask.trailing_zeros() as u16
    }

    /// Push every legal raw value (already positioned under `mask`).
    fn values_into(&self, cfg: &NCfg, slots: u8, out: &mut Vec<u16>) {
        let put = |out: &mut Vec<u16>, v: u16| out.push(v << self.shift());
        match self.kind {
            FieldKind::Side => {
                let count = cfg.side_count as u16;
                if cfg.side_en {
                    // Enable clear => value bits are dead: canonical 0 only.
                    put(out, 0);
                    let en_bit = 1u16 << (count - 1);
                    for v in 0..(1u16 << (count - 1)) {
                        put(out, en_bit | v);
                    }
                } else {
                    for v in 0..(1u16 << count) {
                        put(out, v);
                    }
                }
            }
            FieldKind::Delay => {
                for v in 0..(1u16 << (5 - cfg.side_count)) {
                    put(out, v);
                }
            }
            FieldKind::Opcode => {
                for v in 0..8 {
                    put(out, v);
                }
            }
            FieldKind::JmpCond | FieldKind::MovDst => {
                for v in 0..8 {
                    put(out, v);
                }
            }
            // Targets outside the searched length would execute NOP-land
            // and count as larger footprint — not part of this space.
            FieldKind::JmpTarget => {
                for v in 0..slots as u16 {
                    put(out, v);
                }
            }
            FieldKind::WaitPol => {
                put(out, 0);
                put(out, 1);
            }
            // WAIT src 3 (JMPPIN) is a stub in the emulator — synthesizing
            // against a stubbed behavior would be unsound on hardware.
            FieldKind::WaitSrc => {
                for v in 0..3 {
                    put(out, v);
                }
            }
            FieldKind::WaitIdx | FieldKind::SetData => {
                for v in 0..32 {
                    put(out, v);
                }
            }
            FieldKind::InSrc => {
                for v in [0u16, 1, 2, 3, 6, 7] {
                    put(out, v);
                }
            }
            FieldKind::BitCount => {
                for v in 0..32 {
                    put(out, v); // raw 0 = 32
                }
            }
            FieldKind::OutDst => {
                for v in 0..8 {
                    put(out, v);
                }
            }
            FieldKind::PushPullBits => {
                for v in 0..8 {
                    put(out, v);
                }
            }
            FieldKind::MovOp => {
                for v in 0..3 {
                    put(out, v);
                }
            }
            FieldKind::MovSrc => {
                for v in [0u16, 1, 2, 3, 5, 6, 7] {
                    put(out, v);
                }
            }
            FieldKind::IrqBits => {
                for v in 0..4 {
                    put(out, v); // clear/wait combos; bit7 is dead (0)
                }
            }
            // IRQ index: bits 0..2 select, bit 4 = rel; bit 3 is dead.
            FieldKind::IrqIdx => {
                for v in 0..32u16 {
                    if v & 0x8 == 0 {
                        put(out, v);
                    }
                }
            }
            FieldKind::SetDst => {
                for v in [0u16, 1, 2, 4] {
                    put(out, v);
                }
            }
        }
    }
}

/// The first undecided field the fetched slot will consult, in semantic
/// order. `None` = everything the instruction can consult is decided.
fn demand(decided: u16, value: u16, cfg: &NCfg) -> Option<Field> {
    let need = |mask: u16, kind: FieldKind| -> Option<Field> {
        if decided & mask != mask {
            Some(Field { mask, kind })
        } else {
            None
        }
    };

    // Side-set bits assert on the first cycle even under stall.
    if cfg.side_count > 0 {
        let side_mask = (((1u16 << cfg.side_count) - 1) << (5 - cfg.side_count)) << 8;
        if let Some(f) = need(side_mask, FieldKind::Side) {
            return Some(f);
        }
    }
    if let Some(f) = need(0xE000, FieldKind::Opcode) {
        return Some(f);
    }

    let opcode = (value >> 13) & 0x7;
    let operand_fields: &[(u16, FieldKind)] = match opcode {
        0 => &[(0x00E0, FieldKind::JmpCond), (0x001F, FieldKind::JmpTarget)],
        1 => &[(0x0080, FieldKind::WaitPol), (0x0060, FieldKind::WaitSrc), (0x001F, FieldKind::WaitIdx)],
        2 => &[(0x00E0, FieldKind::InSrc), (0x001F, FieldKind::BitCount)],
        3 => &[(0x00E0, FieldKind::OutDst), (0x001F, FieldKind::BitCount)],
        4 => &[(0x00E0, FieldKind::PushPullBits)], // low 5 bits dead
        5 => &[(0x00E0, FieldKind::MovDst), (0x0018, FieldKind::MovOp), (0x0007, FieldKind::MovSrc)],
        6 => &[(0x0060, FieldKind::IrqBits), (0x001F, FieldKind::IrqIdx)], // bit7 dead
        _ => &[(0x00E0, FieldKind::SetDst), (0x001F, FieldKind::SetData)],
    };
    for &(mask, kind) in operand_fields {
        if let Some(f) = need(mask, kind) {
            return Some(f);
        }
    }

    // Delay last (v1: eager at fetch; completion-lazy is a v2 refinement).
    if cfg.side_count < 5 {
        let delay_mask = ((1u16 << (5 - cfg.side_count)) - 1) << 8;
        if let Some(f) = need(delay_mask, FieldKind::Delay) {
            return Some(f);
        }
    }
    None
}

/// Would the next `step` fetch `code[pc]`? (No pending delay, no forced
/// word, and any stall resolves this cycle.)
fn will_fetch(st: &NState, cfg: &NCfg, gpio_in: u32) -> bool {
    if st.delay_count > 0 || st.pending_exec.is_some() {
        return false;
    }
    if st.stall != Stall::None && still_stalled(st, cfg, gpio_in) {
        return false;
    }
    true
}

/// Peek whether the divider fires this system clock, without mutating.
fn peek_tick(st: &NState, cfg: &NCfg) -> bool {
    let threshold = if cfg.clkdiv_int == 0 {
        65536u32 * 256
    } else {
        (cfg.clkdiv_int as u32) * 256 + cfg.clkdiv_frac as u32
    };
    st.clk_acc + 256 >= threshold
}

/// Exhaustive needed-narrowing search. Deterministic: DFS order is fixed
/// by field order and value enumeration order.
pub fn search(spec: &EngineSpec, champion_cap: usize) -> SearchResult {
    let nop = crate::ir::Insn::nop_for(&crate::ir::SideCfg {
        count: spec.cfg.side_count,
        en: spec.cfg.side_en,
    });
    let nop_word = crate::encode::encode_insn(
        &nop,
        &crate::ir::SideCfg { count: spec.cfg.side_count, en: spec.cfg.side_en },
    );

    let mut root = Item {
        decided: [0u16; 32],
        value: [0u16; 32],
        st: NState::new(&spec.cfg),
        cycle: 0,
        next_input: 0,
        seen_x: false,
        seen_y: false,
    };
    for s in spec.slots as usize..32 {
        root.decided[s] = 0xFFFF;
        root.value[s] = nop_word;
    }
    for &p in &spec.output_pins {
        root.st.dir_latch |= 1u32 << p;
    }
    let preload = spec.inputs.len() <= 4;
    if preload {
        for &w in &spec.inputs {
            root.st.tx.push(w);
        }
        root.next_input = spec.inputs.len() as u32;
    }

    // IRQ sets indexed by cycle for O(1) lookup.
    let mut irq_at = std::collections::HashMap::new();
    for &(c, m) in &spec.irq_sets {
        *irq_at.entry(c).or_insert(0u8) |= m;
    }

    let mut stack = vec![root];
    let mut champions = Vec::new();
    let mut champion_cap_hit = false;
    let mut stats = Stats::default();
    let mut cfg = spec.cfg.clone();
    let mut values = Vec::with_capacity(32);
    let mut last_beat = std::time::Instant::now();

    'items: while let Some(mut it) = stack.pop() {
        stats.items += 1;
        if last_beat.elapsed().as_secs() >= 10 {
            eprintln!(
                "narrow-search: items={} forks={} refuted={} champions={} stack={}",
                stats.items, stats.forks, stats.refuted, stats.champions_found, stack.len()
            );
            last_beat = std::time::Instant::now();
        }
        cfg.code = it.value;

        while it.cycle < spec.cycles {
            // Deterministic per-cycle environment (idempotent, so a fork
            // re-entering this cycle re-applies it safely).
            if let Some(&m) = irq_at.get(&it.cycle) {
                it.st.irq_flags |= m;
            }
            if !preload {
                while (it.next_input as usize) < spec.inputs.len() && !it.st.tx.is_full() {
                    it.st.tx.push(spec.inputs[it.next_input as usize]);
                    it.next_input += 1;
                }
            }
            let ext = self::stim_at(&spec.stim, it.cycle);
            let gpio_in = compose(&it.st, spec.stim.mask, ext);

            if peek_tick(&it.st, &cfg) && will_fetch(&it.st, &cfg, gpio_in) {
                let pc = it.st.pc as usize;
                if let Some(field) = demand(it.decided[pc], it.value[pc], &cfg) {
                    values.clear();
                    field.values_into(&cfg, spec.slots, &mut values);
                    for &v in &values {
                        let raw = v >> field.shift();
                        let (nx, ny) = names_regs(field.kind, raw);
                        // P1-lite: first named scratch register must be X
                        // (or both at once, the self-mirror X!=Y).
                        if spec.p1_register_symmetry && !it.seen_x && !it.seen_y && ny && !nx {
                            continue;
                        }
                        let mut child = it;
                        child.decided[pc] |= field.mask;
                        child.value[pc] |= v;
                        child.seen_x |= nx;
                        child.seen_y |= ny;
                        stack.push(child);
                        stats.forks += 1;
                    }
                    continue 'items;
                }
            }

            if clock_tick(&mut it.st, &cfg) {
                super::step(&mut it.st, &cfg, gpio_in);
            }
            stats.cycles_run += 1;

            let levels = compose(&it.st, spec.stim.mask, ext);
            let mut w = 0u32;
            for (j, &p) in spec.capture_pins.iter().enumerate() {
                if (levels >> p) & 1 != 0 {
                    w |= 1 << j;
                }
                if (it.st.dir_latch >> p) & 1 != 0 {
                    w |= 1 << (16 + j);
                }
            }
            if w != spec.expected[it.cycle as usize] {
                stats.refuted += 1;
                continue 'items;
            }
            it.cycle += 1;
        }

        stats.champions_found += 1;
        if champions.len() < champion_cap {
            champions.push(Champion { decided: it.decided, value: it.value });
        } else {
            champion_cap_hit = true;
        }
    }

    SearchResult { champions, stats, champion_cap_hit }
}

fn stim_at(stim: &Stim, cycle: u32) -> u32 {
    if stim.values.is_empty() {
        0
    } else {
        stim.values[(cycle as usize).min(stim.values.len() - 1)]
    }
}

/// Run one CONCRETE program under an [`EngineSpec`]'s driver schedule
/// (same cycle loop as the search, no forking). Used to generate the
/// expected trace from a reference program and to re-check champions.
pub fn run_spec(spec: &EngineSpec, code: [u16; 32]) -> Vec<u32> {
    let mut cfg = spec.cfg.clone();
    cfg.code = code;
    let mut st = NState::new(&cfg);
    for &p in &spec.output_pins {
        st.dir_latch |= 1u32 << p;
    }
    let preload = spec.inputs.len() <= 4;
    let mut next_input = 0usize;
    if preload {
        for &w in &spec.inputs {
            st.tx.push(w);
        }
        next_input = spec.inputs.len();
    }
    let mut irq_at = std::collections::HashMap::new();
    for &(c, m) in &spec.irq_sets {
        *irq_at.entry(c).or_insert(0u8) |= m;
    }

    let mut out = Vec::with_capacity(spec.cycles as usize);
    for cycle in 0..spec.cycles {
        if let Some(&m) = irq_at.get(&cycle) {
            st.irq_flags |= m;
        }
        if !preload {
            while next_input < spec.inputs.len() && !st.tx.is_full() {
                st.tx.push(spec.inputs[next_input]);
                next_input += 1;
            }
        }
        let ext = stim_at(&spec.stim, cycle);
        let gpio_in = compose(&st, spec.stim.mask, ext);
        if clock_tick(&mut st, &cfg) {
            super::step(&mut st, &cfg, gpio_in);
        }
        let levels = compose(&st, spec.stim.mask, ext);
        let mut w = 0u32;
        for (j, &p) in spec.capture_pins.iter().enumerate() {
            if (levels >> p) & 1 != 0 {
                w |= 1 << j;
            }
            if (st.dir_latch >> p) & 1 != 0 {
                w |= 1 << (16 + j);
            }
        }
        out.push(w);
    }
    out
}
