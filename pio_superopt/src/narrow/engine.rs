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
//! - Demand is EAGER WITHIN A FETCH for the op fields (JMP targets are
//!   not cond-lazy). Sound — over-demanding only costs don't-care
//!   coverage, never correctness. Two laziness levers ARE implemented:
//!   DELAY forks post-step, only for instructions that completed on an
//!   already-survived cycle (refuted items never pay the 8x delay
//!   multiplier); and SIDE-SET values are pre-filtered against the
//!   expected trace at the fetch cycle (an asserted side-set overwrites
//!   the op's writes on its pins, so it is refutable before the opcode
//!   is forked — see `side_value_consistent` for the OE soundness rule).
//! - A third lever, the PIN-WRITE PRE-FILTER: when a fork value completes
//!   the fetch's consulted fields and the instruction writes a pin latch
//!   (SET/OUT PINS/PINDIRS, MOV PINS/PINDIRS), the cycle is evaluated on
//!   a scratch state and refuted against the trace before the child is
//!   ever pushed — an exact one-cycle lookahead reusing `step` itself, so
//!   champion sets are provably identical (the killed child would have
//!   been refuted on this cycle before forking anything).
//! - Canonicality filter P1 (register symmetry, ALWAYS ON): while no
//!   scratch register has been named and the binding is unbound, field
//!   values whose only register is Y are pruned — the item then stands
//!   for BOTH its words and their register-mirror (`JMP X!=Y` names both
//!   at once, its own mirror). X/Y renaming is NOT a free ISA symmetry;
//!   the two asymmetric channels (audited against `exec_op`, the only
//!   two) are handled by a BINDING FORK at execution time:
//!   1. PULL nonblocking / `if_empty` on an empty TX FIFO reads
//!      physical X into OSR — distinguishes the twins iff x != y.
//!   2. A `pending_exec` word (OUT/MOV EXEC) comes from DATA, which the
//!      mirror does not rename, so an exec'd word touching any register
//!      executes un-mirrored in both twins.
//!   When such an event is about to execute on an Unbound item, the item
//!   forks into an Identity child and a Mirrored child (x/y swapped;
//!   champions materialize with register fields mirrored via
//!   `mirror_word`). A champion still Unbound at the end covers its
//!   mirror twin too (`Champion::binding_free`).
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
    /// Pre-decided fields of the root item: `(slot, decided_mask,
    /// value)` — constrained resynthesis (pin known scaffolding, search
    /// the rest). A seed that names a register disables the P1
    /// first-naming prune (the caller chose the spelling); NOTE that
    /// champions may still be register-MIRRORS of the seed when a
    /// binding fork executes — post-filter if the seed spelling is
    /// load-bearing.
    pub seed: Vec<(u8, u16, u16)>,
}

/// A surviving candidate subspace: decided fields plus don't-cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Champion {
    pub decided: [u16; 32],
    pub value: [u16; 32],
    /// True when no binding-asymmetric event executed on this item's
    /// path: the register-mirror of this subspace (swap X/Y in every
    /// decided field, `mirror_word`) reproduces the trace too.
    pub binding_free: bool,
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
    /// Fork values killed by the pin-write pre-filter (never pushed).
    pub prefiltered: u64,
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
    /// Any register-naming field decided yet? (Gates the P1 prune.)
    named: bool,
    /// True while no binding-asymmetric event has executed: the item
    /// stands for both its words AND their register-mirror (whose true
    /// machine state is the x/y-swap of this item's — equal until an
    /// asymmetric event, which is exactly when this flag drops). The
    /// binding fork turns the twin into its own ordinary item by
    /// mirroring the decided words and swapping x/y.
    unbound: bool,
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

    // Delay is NOT demanded at fetch: it is only consulted when the
    // instruction COMPLETES, so the search forks it post-step (and only
    // for items that survived this cycle's trace check) — see the delay
    // post-fork in `search`.
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

/// Can side-set raw value `raw` produce this cycle's expected capture?
/// Sound one-sided check: an asserted value-drive side-set overwrites
/// the op's writes on its pins, so wherever the expected trace pins a
/// pin's OE high (surviving items must match OE too) and the pin isn't
/// stimulus-forced, the expected LEVEL must equal the side-set bit.
/// PINDIR-drive and non-asserted values are never refuted here.
fn side_value_consistent(
    raw: u16,
    cfg: &NCfg,
    capture_pins: &[u8],
    stim_mask: u32,
    expected: u32,
) -> bool {
    if cfg.side_pindir {
        return true;
    }
    let (asserted, val, value_bits) = if cfg.side_en {
        let en = (raw >> (cfg.side_count - 1)) & 1 != 0;
        (en, raw & ((1u16 << (cfg.side_count - 1)) - 1), cfg.side_count - 1)
    } else {
        (true, raw, cfg.side_count)
    };
    if !asserted {
        return true;
    }
    for b in 0..value_bits {
        let pin = (cfg.sideset_base + b) & 31;
        if (stim_mask >> pin) & 1 != 0 {
            continue;
        }
        if let Some(j) = capture_pins.iter().position(|&p| p == pin) {
            if (expected >> (16 + j)) & 1 != 0 {
                let want = (expected >> j) & 1;
                let have = ((val >> b) & 1) as u32;
                if want != have {
                    return false;
                }
            }
        }
    }
    true
}

/// Swap every X/Y register naming in a (possibly partially decided)
/// word: JMP conds !X/X-- <-> !Y/Y--, IN/OUT/MOV/SET register codes
/// 1 <-> 2. Undecided fields are 0 and map to 0. PUSH/PULL/WAIT/IRQ
/// carry no register fields — PULL's implicit X read is physical and is
/// handled by the binding fork at execution time, not by renaming.
pub fn mirror_word(w: u16) -> u16 {
    let swap12 = |v: u16| match v {
        1 => 2,
        2 => 1,
        v => v,
    };
    match (w >> 13) & 0x7 {
        0 => {
            let cond = match (w >> 5) & 0x7 {
                1 => 3,
                3 => 1,
                2 => 4,
                4 => 2,
                c => c,
            };
            (w & !(0x7 << 5)) | (cond << 5)
        }
        2 | 3 | 7 => (w & !(0x7 << 5)) | (swap12((w >> 5) & 0x7) << 5),
        5 => {
            let dst = swap12((w >> 5) & 0x7);
            let src = swap12(w & 0x7);
            (w & !((0x7 << 5) | 0x7)) | (dst << 5) | src
        }
        _ => w,
    }
}

/// Does executing this concrete word consult or write a physical
/// scratch register (including PULL's implicit X read on an empty TX
/// FIFO)? Used for `pending_exec` words: they come from DATA, are not
/// renamed by the mirror, and so distinguish the two bindings whenever
/// they touch a register (writes diverge even at x == y — an exec'd
/// `SET Y, n` writes physical Y in BOTH twins).
fn word_touches_regs(w: u16) -> bool {
    let f = (w >> 5) & 0x7;
    match (w >> 13) & 0x7 {
        0 => matches!(f, 1..=5),
        2 | 3 | 7 => f == 1 || f == 2,
        5 => f == 1 || f == 2 || (w & 0x7) == 1 || (w & 0x7) == 2,
        4 => (w >> 7) & 1 != 0, // any PULL may read X when TX is empty
        _ => false,
    }
}

/// Is this word a PULL that READS X this cycle (nonblocking or if_empty
/// variant executing on an empty TX FIFO)? The one binding-asymmetric
/// instruction reachable from CODE (register fields there are virtual);
/// it distinguishes the twins iff x != y at execution.
fn is_pull_empty_read(w: u16, st: &NState) -> bool {
    if (w >> 13) & 0x7 != 4 || (w >> 7) & 1 == 0 {
        return false;
    }
    let if_empty = (w >> 6) & 1 != 0;
    let block = (w >> 5) & 1 != 0;
    st.tx.is_empty() && (if_empty || !block)
}

/// Would the next `step` EXECUTE an instruction (fetched or pending)?
/// Like `will_fetch` but pending_exec counts as execution.
fn will_execute(st: &NState, cfg: &NCfg, gpio_in: u32) -> bool {
    if st.delay_count > 0 {
        return false;
    }
    if st.stall != Stall::None && still_stalled(st, cfg, gpio_in) {
        return false;
    }
    true
}

/// Does this (fully fetch-decided) word write a pin latch when it
/// executes? SET/OUT to PINS or PINDIRS, MOV to PINS or PINDIRS. These
/// are the only instructions whose OPERAND value can change the current
/// cycle's capture — the pre-filter's scratch step is spent nowhere else.
fn writes_pin_latch(word: u16) -> bool {
    let dst = (word >> 5) & 0x7;
    match (word >> 13) & 0x7 {
        3 | 7 => dst == 0 || dst == 4, // OUT/SET: PINS, PINDIRS
        5 => dst == 0 || dst == 3,     // MOV: PINS, PINDIRS
        _ => false,
    }
}

/// The trace word one cycle produces: levels of `capture_pins` in the
/// low half, their output-enables in the high half (trace_pads format).
#[inline]
fn capture_word(st: &NState, capture_pins: &[u8], stim_mask: u32, ext: u32) -> u32 {
    let levels = compose(st, stim_mask, ext);
    let mut w = 0u32;
    for (j, &p) in capture_pins.iter().enumerate() {
        if (levels >> p) & 1 != 0 {
            w |= 1 << j;
        }
        if (st.dir_latch >> p) & 1 != 0 {
            w |= 1 << (16 + j);
        }
    }
    w
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
        named: false,
        unbound: true,
    };
    for s in spec.slots as usize..32 {
        root.decided[s] = 0xFFFF;
        root.value[s] = nop_word;
    }
    for &(s, d, v) in &spec.seed {
        root.decided[s as usize] |= d;
        root.value[s as usize] |= v & d;
        // A register-naming seed fixes the spelling by caller fiat; an
        // opcode-incomplete seed is treated conservatively the same way
        // (disabling the P1 prune is always sound, only less pruned).
        if d & 0xE000 != 0xE000 || word_touches_regs(v) {
            root.named = true;
        }
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

    let delay_bits = 5 - spec.cfg.side_count.min(5);
    let delay_mask: u16 = if delay_bits == 0 { 0 } else { ((1u16 << delay_bits) - 1) << 8 };

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
                "narrow-search: items={} forks={} refuted={} prefilt={} champions={} stack={}",
                stats.items,
                stats.forks,
                stats.refuted,
                stats.prefiltered,
                stats.champions_found,
                stack.len()
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

            let fetching = peek_tick(&it.st, &cfg) && will_fetch(&it.st, &cfg, gpio_in);
            let fetch_pc = it.st.pc as usize;
            if fetching {
                if let Some(field) = demand(it.decided[fetch_pc], it.value[fetch_pc], &cfg) {
                    values.clear();
                    field.values_into(&cfg, spec.slots, &mut values);
                    for &v in &values {
                        let raw = v >> field.shift();
                        // Side-set pre-filter: an ASSERTED side-set is
                        // applied after the op's writes, so it determines
                        // this cycle's captured level of every pin it
                        // drives — refutable against expected[cycle]
                        // before the opcode is ever forked.
                        if field.kind == FieldKind::Side
                            && !side_value_consistent(
                                raw,
                                &cfg,
                                &spec.capture_pins,
                                spec.stim.mask,
                                spec.expected[it.cycle as usize],
                            )
                        {
                            continue;
                        }
                        let (nx, ny) = names_regs(field.kind, raw);
                        // P1: while unbound and unnamed, the first named
                        // scratch register must be X (or both at once,
                        // the self-mirror X!=Y) — the Y-first spelling is
                        // this item's mirror twin, which the item itself
                        // stands for until a binding fork. After a fork
                        // the item represents ONE concrete program, so
                        // both spellings must be enumerated.
                        if !it.named && it.unbound && ny && !nx {
                            continue;
                        }
                        // Pin-write pre-filter, an exact one-cycle
                        // lookahead: once this value completes the fetch's
                        // consulted fields AND the decoded instruction
                        // writes a pin latch, the cycle is fully concrete
                        // (undecided delay is consulted only at completion
                        // and cannot move this cycle's capture) — run it on
                        // a scratch state and refute against the expected
                        // trace before ever paying the child push/pop. A
                        // killed value's child would be refuted on this
                        // very cycle before forking anything (delay forks
                        // only after the trace check), so champion sets are
                        // untouched. `cfg.code` needs no restore: every
                        // path out of this fork loop abandons the item.
                        let child_value = it.value[fetch_pc] | v;
                        if writes_pin_latch(child_value)
                            && demand(it.decided[fetch_pc] | field.mask, child_value, &cfg)
                                .is_none()
                        {
                            cfg.code[fetch_pc] = child_value;
                            let mut scratch = it.st;
                            if clock_tick(&mut scratch, &cfg) {
                                super::step(&mut scratch, &cfg, gpio_in);
                            }
                            let w = capture_word(&scratch, &spec.capture_pins, spec.stim.mask, ext);
                            if w != spec.expected[it.cycle as usize] {
                                stats.prefiltered += 1;
                                continue;
                            }
                        }
                        let mut child = it;
                        child.decided[fetch_pc] |= field.mask;
                        child.value[fetch_pc] |= v;
                        child.named |= nx | ny;
                        stack.push(child);
                        stats.forks += 1;
                    }
                    continue 'items;
                }
            }

            // Binding demand: an unbound item and its mirror twin
            // diverge the moment an asymmetric event executes — PULL's
            // implicit physical-X read with x != y, or a pending_exec
            // word (data is not mirror-renamed) touching any register.
            // Fork: one child keeps the words as spelled; the other
            // BECOMES the twin as an ordinary concrete item — decided
            // searched slots mirrored, x/y swapped (the twin's true
            // machine state; every other state component is equal by
            // the equivariance of all prior, symmetric steps). Both
            // re-enter this cycle (the environment re-application is
            // idempotent) bound, and step straight through.
            if it.unbound && peek_tick(&it.st, &cfg) {
                let demands_binding = if !will_execute(&it.st, &cfg, gpio_in) {
                    false
                } else if let Some(w) = it.st.pending_exec {
                    word_touches_regs(w)
                } else {
                    it.st.x != it.st.y
                        && is_pull_empty_read(cfg.code[it.st.pc as usize], &it.st)
                };
                if demands_binding {
                    it.unbound = false;
                    let mut twin = it;
                    // The twin is distinct unless state AND words are
                    // both mirror-fixed (possible on the exec path with
                    // x == y and register-free decided words) — there
                    // the bound child's full future enumeration already
                    // contains every mirror spelling, so skip the dup.
                    let mut twin_differs = it.st.x != it.st.y;
                    for s in 0..spec.slots as usize {
                        let m = mirror_word(twin.value[s]);
                        twin_differs |= m != twin.value[s];
                        twin.value[s] = m;
                    }
                    stack.push(it);
                    stats.forks += 1;
                    if twin_differs {
                        std::mem::swap(&mut twin.st.x, &mut twin.st.y);
                        stack.push(twin);
                        stats.forks += 1;
                    }
                    continue 'items;
                }
            }

            if clock_tick(&mut it.st, &cfg) {
                super::step(&mut it.st, &cfg, gpio_in);
            }
            stats.cycles_run += 1;

            let w = capture_word(&it.st, &spec.capture_pins, spec.stim.mask, ext);
            if w != spec.expected[it.cycle as usize] {
                stats.refuted += 1;
                continue 'items;
            }
            it.cycle += 1;

            // Delay post-fork: delay is consulted only at COMPLETION, so
            // it is forked only for instructions that actually completed
            // (no stall) on an already-survived cycle. Refuted items
            // never pay the delay multiplier; an instruction completing
            // on the final cycle keeps delay as a don't-care.
            if fetching && it.st.stall == Stall::None && delay_mask != 0 {
                let undecided = it.decided[fetch_pc] & delay_mask != delay_mask;
                if undecided && it.cycle < spec.cycles {
                    for v in 0..(1u16 << delay_bits) {
                        let mut child = it;
                        child.decided[fetch_pc] |= delay_mask;
                        child.value[fetch_pc] |= v << 8;
                        child.st.delay_count = v as u8;
                        stack.push(child);
                        stats.forks += 1;
                    }
                    continue 'items;
                }
            }
        }

        stats.champions_found += 1;
        if champions.len() < champion_cap {
            champions.push(Champion {
                decided: it.decided,
                value: it.value,
                binding_free: it.unbound,
            });
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
