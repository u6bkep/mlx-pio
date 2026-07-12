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
//! - Consulted-set memoization (failure-only): an exhausted,
//!   champion-free subtree records — keyed on the always-read state
//!   CORE (cycle, input position, pc, delay, stall, pending, clk_acc,
//!   pin latches) — the program fields it consulted AND the state
//!   components it read (per-opcode read table, `word_state_reads`),
//!   quantifying its failure claim over everything else. A later item
//!   matching core + read pattern + program conditions is refuted
//!   outright. Records are benefit-gated with purge-and-raise at the
//!   cap; dropping records only loses pruning, never soundness.

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
    /// Consulted-set memo capacity in entries (0 disables). Records are
    /// FAILURE-ONLY (an exhausted, champion-free subtree keyed by its
    /// fork state + the decided fields it consulted) and benefit-gated:
    /// a record must summarize a big-enough subtree to earn a slot, and
    /// reaching the cap purges low-benefit records and raises the bar
    /// instead of freezing. Dropping records only loses pruning, never
    /// soundness.
    pub memo_cap: usize,
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
    /// Fork values killed by the canonicity filters P2/P3/P4 (duplicate
    /// spellings whose representative is generated elsewhere).
    pub canon_pruned: u64,
    /// Items refuted by a consulted-set memo hit (subtree skipped).
    pub memo_hits: u64,
    /// Probes whose key CORE matched at least one record (hit or not) —
    /// the gap to `memo_hits` is pattern/condition mismatches.
    pub memo_core_matches: u64,
    /// Core-matched probes where no record's state pattern matched.
    pub memo_state_misses: u64,
    /// Core-matched probes where a state pattern matched but every
    /// record's program conditions failed.
    pub memo_cond_misses: u64,
    /// Failure records currently in the memo.
    pub memo_entries: u64,
    /// Times the memo hit its cap and purged low-benefit records.
    pub memo_purges: u64,
    /// log2 histogram of candidate-record benefits (subtree item counts
    /// of champion-free, recordable subtrees at finalize, BEFORE the
    /// benefit gate) — where the memo's value actually lives.
    pub benefit_hist: [u64; 32],
}

impl Stats {
    /// Nonzero benefit-histogram buckets, e.g. "2^0:512 2^4:31".
    pub fn benefit_hist_compact(&self) -> String {
        self.benefit_hist
            .iter()
            .enumerate()
            .filter(|(_, &c)| c > 0)
            .map(|(i, c)| format!("2^{i}:{c}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
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
    /// P3 run state: the last completed instruction was a canonical nop
    /// whose delay was FRESHLY forked this pass and is below max. Set
    /// only at fork time (never for seeded/revisited delays, whose
    /// front-loaded representative may not exist in the space); any
    /// other completion resets it.
    prev_nop_submax: bool,
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

/// The canonical no-op spelling: `mov osr, osr` (op none). Chosen
/// register-free so it never sets `named` and leaves the P1 prune armed.
/// Bits 8..12 (delay/side) excluded from the pattern.
pub const NOP_CANON: u16 = 0xA0E7;
const NOP_CANON_MASK: u16 = 0xE0FF;

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

/// Full fork-point identity, kept per frame for record projection and
/// the hit dump. `next_input` covers the streamed-input driver position
/// (preloaded inputs live in the FIFO already).
type MemoKey = (u32, u32, NState);

// --- Consulted-state components (ticket 007) -------------------------
// The memo key splits into an always-read CORE (hashed) and PATTERN
// components a record mentions only if its subtree actually read them.
// A record then covers every state agreeing on the core and its read
// components — one record where the monolithic key needed unboundedly
// many.

const SC_X: u16 = 1 << 0;
const SC_Y: u16 = 1 << 1;
const SC_ISR: u16 = 1 << 2;
const SC_OSR: u16 = 1 << 3;
const SC_ISR_CNT: u16 = 1 << 4;
const SC_OSR_CNT: u16 = 1 << 5;
const SC_IRQ: u16 = 1 << 6;
const SC_TX: u16 = 1 << 7;
const SC_RX: u16 = 1 << 8;

/// The always-read key core: fetch reads pc, the divider reads clk_acc,
/// the per-cycle capture/compose read the latches, and step's entry
/// path consults delay/stall/pending every executing cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct KeyCore {
    cycle: u32,
    next_input: u32,
    pc: u8,
    delay_count: u8,
    stall: Stall,
    pending_exec: Option<u16>,
    clk_acc: u32,
    out_latch: u32,
    dir_latch: u32,
}

fn key_core(cycle: u32, next_input: u32, st: &NState) -> KeyCore {
    KeyCore {
        cycle,
        next_input,
        pc: st.pc,
        delay_count: st.delay_count,
        stall: st.stall,
        pending_exec: st.pending_exec,
        clk_acc: st.clk_acc,
        out_latch: st.out_latch,
        dir_latch: st.dir_latch,
    }
}

/// Append the values of `mask`'s components in canonical order. FIFOs
/// are read-normalized (level + queued words in pop order), so
/// observably-equal FIFOs with different buffer layouts compare equal.
fn project_state(st: &NState, mask: u16, out: &mut Vec<u32>) {
    if mask & SC_X != 0 {
        out.push(st.x);
    }
    if mask & SC_Y != 0 {
        out.push(st.y);
    }
    if mask & SC_ISR != 0 {
        out.push(st.isr);
    }
    if mask & SC_OSR != 0 {
        out.push(st.osr);
    }
    if mask & SC_ISR_CNT != 0 {
        out.push(st.isr_count as u32);
    }
    if mask & SC_OSR_CNT != 0 {
        out.push(st.osr_count as u32);
    }
    if mask & SC_IRQ != 0 {
        out.push(st.irq_flags as u32);
    }
    for f in [(mask & SC_TX != 0).then_some(&st.tx), (mask & SC_RX != 0).then_some(&st.rx)]
        .into_iter()
        .flatten()
    {
        out.push(f.level() as u32);
        for i in 0..f.level() {
            out.push(f.peek(i));
        }
    }
}

/// Pattern components executing this word can READ this cycle (beyond
/// the core). OVER-approximating reads is sound (a too-big mask only
/// matches fewer probers); missing a read is a false impossibility —
/// derived line-by-line from `exec_op`. Writes as such add nothing: any
/// dataflow into a later-read component passes through a read counted
/// here or through the core.
fn word_state_reads(w: u16, cfg: &NCfg) -> u16 {
    match (w >> 13) & 0x7 {
        0 => match (w >> 5) & 0x7 {
            1 | 2 => SC_X,
            3 | 4 => SC_Y,
            5 => SC_X | SC_Y,
            7 => SC_OSR_CNT,
            _ => 0, // always / pin (gpio is core)
        },
        1 => {
            if (w >> 5) & 0x3 == 2 {
                SC_IRQ
            } else {
                0
            }
        }
        2 => {
            let src = match (w >> 5) & 0x7 {
                1 => SC_X,
                2 => SC_Y,
                6 => SC_ISR,
                7 => SC_OSR,
                _ => 0,
            };
            src | SC_ISR | SC_ISR_CNT | if cfg.autopush { SC_RX } else { 0 }
        }
        3 => SC_OSR | SC_OSR_CNT | if cfg.autopull { SC_TX } else { 0 },
        4 => {
            if (w >> 7) & 1 != 0 {
                // PULL reads TX; on empty it reads X, and the binding
                // demand compares x != y.
                SC_TX | SC_X | SC_Y
            } else {
                SC_RX | SC_ISR
            }
        }
        5 => match w & 0x7 {
            1 => SC_X,
            2 => SC_Y,
            6 => SC_ISR,
            7 => SC_OSR,
            5 => {
                if cfg.status_sel {
                    SC_RX
                } else {
                    SC_TX
                }
            }
            _ => 0,
        },
        6 => SC_IRQ,
        _ => 0, // SET reads nothing
    }
}

/// Segment-local provenance of a scratch register's CURRENT value —
/// what a read of it actually depends on. Reset to `Fork` at every item
/// pop; sound for every enclosing frame because a write and any read of
/// it within one fork-free segment are enclosed by exactly the same
/// frames (forks end segments).
#[derive(Clone, Copy)]
enum Prov {
    /// Still the pop-time value: a read consults the state pattern.
    Fork,
    /// An immediate CODE field (`set x/y, imm`): a read consults that
    /// program field instead of the state — this is what lets records
    /// generalize over "register loaded then compared" chains.
    Field(u8, u16),
    /// Computed from sources that were consulted when written (register
    /// moves, OUT data, a decrement that first read the register, exec'd
    /// immediates — pending words are core): a read adds nothing new.
    Accounted,
}

/// Consume a word's state reads into the segment, routing X/Y through
/// their provenance.
fn consume_reads(
    r: u16,
    x_prov: Prov,
    y_prov: Prov,
    seg_reads: &mut u16,
    seg_mask: &mut [u16; 32],
) {
    *seg_reads |= r & !(SC_X | SC_Y);
    for (bit, prov) in [(SC_X, x_prov), (SC_Y, y_prov)] {
        if r & bit != 0 {
            match prov {
                Prov::Fork => *seg_reads |= bit,
                Prov::Field(s, m) => seg_mask[s as usize] |= m,
                Prov::Accounted => {}
            }
        }
    }
}

/// Which scratch registers a COMPLETED execution of this word definitely
/// writes, and whether the written value is a code-immediate (SET's
/// 5-bit data). Conditional writes that first READ the target (JMP
/// X--/Y--) are Accounted-safe: the read pins the branch, so matching
/// probers evolve identically.
fn word_reg_writes(w: u16) -> (bool, bool, bool) {
    let f = (w >> 5) & 0x7;
    match (w >> 13) & 0x7 {
        0 => (f == 2, f == 4, false),
        3 | 5 => (f == 1, f == 2, false),
        7 => (f == 1, f == 2, true),
        _ => (false, false, false),
    }
}

/// Pattern components a pending stall re-check reads each cycle.
fn stall_state_reads(stall: Stall) -> u16 {
    match stall {
        Stall::Pull => SC_TX,
        Stall::Push => SC_RX,
        Stall::WaitIrq { .. } | Stall::IrqWait { .. } => SC_IRQ,
        _ => 0, // gpio/pin waits read the core latches + stimulus
    }
}

/// Per-slot consulted-condition set: `(mask, value)` of the DECIDED
/// fields a subtree's evaluation read. The failure claim quantifies
/// over everything else.
type Conds = Vec<(u8, u16, u16)>;

/// Failure records at one key core, two-level like the playground's
/// MemoEntry: per distinct read-mask, a table from projected state
/// values to the (program conds, benefit) records — so a probe costs
/// one projection + one hash lookup per DISTINCT MASK at the core
/// (typically one or two), not a scan of every record ever stored.
#[derive(Default)]
struct MemoEntry {
    sets: Vec<(u16, std::collections::HashMap<Vec<u32>, Vec<(Conds, u32)>>)>,
}

/// One frame of the fork tree, mirroring the DFS stack (LIFO makes each
/// subtree contiguous). Accumulates the subtree's consulted set for the
/// failure record at finalize.
struct Frame {
    key: MemoKey,
    /// Decided masks at fork time — the filter that keeps only
    /// at-or-above-decided fields as record conditions (fields forked
    /// below drop out here: all their values were explored).
    decided: [u16; 32],
    /// Children pushed and not yet accounted.
    children_left: u32,
    /// Union of consulted (mask, value) per slot across the subtree.
    consulted_mask: [u16; 32],
    consulted_value: [u16; 32],
    /// Union of pattern state components read anywhere in the subtree.
    state_reads: u16,
    any_champion: bool,
    /// False when a record would be unsound: a binding fork below mixes
    /// runs from two root states (identity S, twin sigma(S)) under one
    /// key, or a value conflict surfaced in the consulted union.
    recordable: bool,
    /// `stats.items` when this frame opened; subtrees are contiguous in
    /// the DFS, so `stats.items - items_at_open` at finalize is exactly
    /// the subtree's item count — the record's benefit.
    items_at_open: u64,
}

impl Frame {
    /// Union a consulted (mask, value) into this frame's subtree set.
    /// A value conflict on a field DECIDED AT THIS FRAME (possible when
    /// a binding fork's identity and twin sides consult the same slot
    /// under different spellings) makes the frame unrecordable — the
    /// union no longer describes one consistent condition set. Bits the
    /// frame forked below are EXPECTED to differ across children (the
    /// union over explored values) and are dropped by the decided
    /// filter at finalize, so they never count as conflicts.
    fn merge(&mut self, slot: usize, mask: u16, value: u16) {
        if mask == 0 {
            return;
        }
        let overlap = self.consulted_mask[slot] & mask & self.decided[slot];
        if self.consulted_value[slot] & overlap != value & overlap {
            self.recordable = false;
        }
        self.consulted_mask[slot] |= mask;
        self.consulted_value[slot] = (self.consulted_value[slot] & !mask) | (value & mask);
    }
}

/// Merge a finished pop-segment's consulted fields and state reads into
/// the open frame.
fn merge_segment(fr: &mut Frame, seg_mask: &[u16; 32], seg_reads: u16, value: &[u16; 32]) {
    fr.state_reads |= seg_reads;
    for s in 0..32 {
        if seg_mask[s] != 0 {
            fr.merge(s, seg_mask[s], value[s] & seg_mask[s]);
        }
    }
}

/// Account one completed child of the open frame; finalize frames whose
/// subtrees are done — recording champion-free ones — and merge their
/// (decided-filtered) conditions upward.
///
/// Records are benefit-gated: a subtree smaller than `min_benefit`
/// items is not worth a table slot. When the table hits `memo_cap`,
/// low-benefit records are purged and the bar quadruples — insertion
/// never freezes, and the table converges on the records that each
/// save the most work.
fn close_child(
    frames: &mut Vec<Frame>,
    memo: &mut std::collections::HashMap<KeyCore, MemoEntry>,
    memo_cap: usize,
    min_benefit: &mut u32,
    snap: &mut Option<Snapshotter>,
    stats: &mut Stats,
    champion: bool,
) {
    let mut champ = champion;
    loop {
        let top = frames.last_mut().expect("frame stack underflow");
        top.any_champion |= champ;
        top.children_left -= 1;
        if top.children_left > 0 || frames.len() == 1 {
            return;
        }
        let f = frames.pop().expect("root frame is never popped");
        // Conditions = consulted fields decided at-or-above the fork;
        // fields forked below drop out (all their values explored).
        let mut conds: Conds = Vec::new();
        for s in 0..32 {
            let m = f.consulted_mask[s] & f.decided[s];
            if m != 0 {
                conds.push((s as u8, m, f.consulted_value[s] & m));
            }
        }
        let benefit = (stats.items - f.items_at_open).min(u32::MAX as u64) as u32;
        if !f.any_champion && f.recordable {
            stats.benefit_hist[(benefit.max(1).ilog2() as usize).min(31)] += 1;
        }
        if !f.any_champion && f.recordable && benefit >= *min_benefit {
            if (stats.memo_entries as usize) >= memo_cap {
                if let Some(sn) = snap.as_mut() {
                    sn.write("purge", memo, *min_benefit, stats, frames);
                }
                // Purge low-benefit records; raise the bar.
                *min_benefit = min_benefit.saturating_mul(2);
                let mut kept = 0u64;
                memo.retain(|_, entry| {
                    let mut any = false;
                    for (_, table) in entry.sets.iter_mut() {
                        table.retain(|_, recs| {
                            recs.retain(|&(_, b)| b >= *min_benefit);
                            kept += recs.len() as u64;
                            !recs.is_empty()
                        });
                        any |= !table.is_empty();
                    }
                    entry.sets.retain(|(_, t)| !t.is_empty());
                    any
                });
                stats.memo_entries = kept;
                stats.memo_purges += 1;
            }
            if benefit >= *min_benefit {
                // The record's state pattern: fork-time values of the
                // components the subtree read.
                let mut vals = Vec::new();
                project_state(&f.key.2, f.state_reads, &mut vals);
                let entry = memo.entry(key_core(f.key.0, f.key.1, &f.key.2)).or_default();
                let table = match entry.sets.iter_mut().position(|(m, _)| *m == f.state_reads) {
                    Some(i) => &mut entry.sets[i].1,
                    None => {
                        entry.sets.push((f.state_reads, std::collections::HashMap::new()));
                        &mut entry.sets.last_mut().unwrap().1
                    }
                };
                let recs = table.entry(vals).or_default();
                if !recs.iter().any(|(c, _)| *c == conds) {
                    recs.push((conds.clone(), benefit));
                    stats.memo_entries += 1;
                }
            }
        }
        let parent = frames.last_mut().expect("synthetic root below every frame");
        parent.state_reads |= f.state_reads;
        for &(s, m, v) in &conds {
            parent.merge(s as usize, m, v);
        }
        champ = f.any_champion;
    }
}

/// Memo-hit dump for offline convergence analysis, enabled by the
/// `PIO_NARROW_DUMP=<path>` env var (line cap via
/// `PIO_NARROW_DUMP_CAP`, default 200k). Every memo hit is a
/// machine-found pair of DIFFERENT partial programs with provably
/// interchangeable futures from the same state — exactly the
/// "functionally identical but not generation-pruned" signal that
/// seeds new canonicity levers. Hits are log-sampled per key (first 4,
/// then powers of two); a search-end summary lists the hottest
/// convergence clusters.
struct HitDump {
    w: std::io::BufWriter<std::fs::File>,
    written: u64,
    cap: u64,
    counts: std::collections::HashMap<MemoKey, u32>,
}

fn dump_open() -> Option<HitDump> {
    let path = std::env::var("PIO_NARROW_DUMP").ok()?;
    let cap = std::env::var("PIO_NARROW_DUMP_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200_000);
    let f = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
    eprintln!("narrow-search: dumping memo-hit pairs to {path} (cap {cap})");
    Some(HitDump {
        w: std::io::BufWriter::new(f),
        written: 0,
        cap,
        counts: std::collections::HashMap::new(),
    })
}

impl HitDump {
    fn hit(&mut self, key: &MemoKey, conds: &Conds, it_decided: &[u16; 32], it_value: &[u16; 32], slots: u8) {
        use std::io::Write;
        let n = self.counts.entry(*key).or_insert(0);
        *n += 1;
        let n = *n;
        if !(n <= 4 || n & (n - 1) == 0) || self.written >= self.cap {
            return;
        }
        self.written += 1;
        let st = &key.2;
        let conds_s: Vec<String> =
            conds.iter().map(|&(s, m, v)| format!("[{s},{m:#06x},{v:#06x}]")).collect();
        let prober: Vec<String> = (0..slots as usize)
            .map(|s| format!("[{:#06x},{:#06x}]", it_decided[s], it_value[s]))
            .collect();
        let _ = writeln!(
            self.w,
            "{{\"cycle\":{},\"ni\":{},\"pc\":{},\"x\":{:#x},\"y\":{:#x},\"osr\":{:#x},\"isr\":{:#x},\"irq\":{:#x},\"out\":{:#x},\"dir\":{:#x},\"stall\":\"{:?}\",\"hits\":{},\"conds\":[{}],\"prober\":[{}]}}",
            key.0, key.1, st.pc, st.x, st.y, st.osr, st.isr, st.irq_flags,
            st.out_latch, st.dir_latch, st.stall, n,
            conds_s.join(","),
            prober.join(",")
        );
    }

    fn finish(&mut self, stats: &Stats) {
        use std::io::Write;
        let mut hot: Vec<(&MemoKey, &u32)> = self.counts.iter().collect();
        hot.sort_by(|a, b| b.1.cmp(a.1));
        for (key, hits) in hot.iter().take(50) {
            let _ = writeln!(
                self.w,
                "{{\"cluster\":{{\"cycle\":{},\"ni\":{},\"pc\":{},\"stall\":\"{:?}\",\"hits\":{}}}}}",
                key.0, key.1, key.2.pc, key.2.stall, hits
            );
        }
        let _ = writeln!(
            self.w,
            "{{\"search_end\":{{\"items\":{},\"memo_hits\":{},\"distinct_hit_keys\":{},\"benefit_hist\":\"{}\"}}}}",
            stats.items,
            stats.memo_hits,
            self.counts.len(),
            stats.benefit_hist_compact()
        );
        let _ = self.w.flush();
    }
}

// --- Probe/table instrumentation (offline mining) --------------------
// Two independent flags, both inert unless set and both side-effect
// free w.r.t. the search (verdicts, champions, stats and DFS order are
// bit-identical with them on — gated by
// `instrumentation_flags_do_not_change_search`). Big dumps belong on
// the data drive: point them under /data/pio_optimization/runs/.
//
// `PIO_NARROW_PROBE_LOG=<path>`: JSONL stream classifying EVERY memo
// probe (nocore / state_miss / cond_miss / hit) into a per-cycle
// census, plus sampled per-probe diagnostic lines for the two miss
// classes: on a state miss, the nearest stored record's differing
// state components (which field blocks sharing); on a cond miss, the
// first failing program condition. Detail volume is bounded by
// `PIO_NARROW_PROBE_BYTES` (default 8 GiB): past half the budget the
// sampling stride doubles each time half the REMAINING budget is
// spent, so late/deep probes are thinned, never truncated. Census and
// totals are exact regardless. Diagnostic lines scan hash tables in
// iteration order, so they are not byte-stable across runs; the
// census/summary lines are.
//
// `PIO_NARROW_SNAPSHOT=<dir>`: dumps the ENTIRE memo table as JSONL
// (one line per record: core, read-mask, projected values, conds,
// benefit) plus the open frame stack, immediately BEFORE each purge
// (the table at its fullest) and at search end. At the 8M-entry cap a
// snapshot is ~2 GB; `PIO_NARROW_SNAPSHOT_MAX` (default 8) caps the
// purge snapshots, the end snapshot always writes.

const PROBE_NOCORE: usize = 0;
const PROBE_STATE_MISS: usize = 1;
const PROBE_COND_MISS: usize = 2;
const PROBE_HIT: usize = 3;

/// State components (canonical `project_state` order) whose projected
/// values differ between two same-mask projections. FIFO components
/// are variable-length (level + words), so the cursors advance
/// per-side.
fn component_diff(mask: u16, a: &[u32], b: &[u32]) -> u16 {
    let mut diff = 0u16;
    let (mut i, mut j) = (0usize, 0usize);
    for bit in [SC_X, SC_Y, SC_ISR, SC_OSR, SC_ISR_CNT, SC_OSR_CNT, SC_IRQ] {
        if mask & bit != 0 {
            if a[i] != b[j] {
                diff |= bit;
            }
            i += 1;
            j += 1;
        }
    }
    for bit in [SC_TX, SC_RX] {
        if mask & bit != 0 {
            let (la, lb) = (a[i] as usize, b[j] as usize);
            if a[i..=i + la] != b[j..=j + lb] {
                diff |= bit;
            }
            i += la + 1;
            j += lb + 1;
        }
    }
    diff
}

/// Per-probe outcome stream, `PIO_NARROW_PROBE_LOG`.
struct ProbeLog {
    w: std::io::BufWriter<std::fs::File>,
    /// Detail bytes written / budget / next stride-doubling threshold.
    bytes: u64,
    budget: u64,
    next_thresh: u64,
    /// Every `stride`-th miss gets a detail line; doubles as the budget
    /// drains.
    stride: u64,
    misses_seen: u64,
    /// Probe outcomes per cycle (nocore/state_miss/cond_miss/hit).
    census: Vec<[u64; 4]>,
}

/// Bounded nearest-record scan per set on a state miss.
const PROBE_NEAR_SCAN: usize = 256;

fn probe_log_open(cycles: u32) -> Option<ProbeLog> {
    let path = std::env::var("PIO_NARROW_PROBE_LOG").ok()?;
    let budget = std::env::var("PIO_NARROW_PROBE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8u64 << 30);
    let f = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
    eprintln!("narrow-search: probe log to {path} (detail budget {budget} bytes)");
    Some(ProbeLog {
        w: std::io::BufWriter::new(f),
        bytes: 0,
        budget,
        next_thresh: budget / 2,
        stride: 1,
        misses_seen: 0,
        census: vec![[0u64; 4]; cycles as usize + 1],
    })
}

impl ProbeLog {
    /// Record one probe outcome in the census; returns whether this
    /// probe's diagnostic detail is sampled in.
    fn count(&mut self, cycle: u32, outcome: usize) -> bool {
        let c = (cycle as usize).min(self.census.len() - 1);
        self.census[c][outcome] += 1;
        if outcome != PROBE_STATE_MISS && outcome != PROBE_COND_MISS {
            return false;
        }
        self.misses_seen += 1;
        self.stride > 0 && self.misses_seen % self.stride == 0
    }

    fn wrote(&mut self, n: usize) {
        self.bytes += n as u64;
        while self.next_thresh < self.budget && self.bytes >= self.next_thresh {
            self.stride *= 2;
            self.next_thresh += (self.budget - self.next_thresh) / 2;
        }
        if self.bytes >= self.budget {
            self.stride = 0; // budget exhausted: census only from here
        }
    }

    /// One sampled miss, with per-set diagnosis against the entry's
    /// current tables.
    #[allow(clippy::too_many_arguments)]
    fn miss(
        &mut self,
        outcome: usize,
        key: &MemoKey,
        entry: &MemoEntry,
        mirror_blocked: bool,
        it_decided: &[u16; 32],
        it_value: &[u16; 32],
        slots: u8,
        proj: &mut Vec<u32>,
    ) {
        use std::io::Write;
        let st = &key.2;
        let mut sets = Vec::new();
        for &(mask, ref table) in &entry.sets {
            if mirror_blocked && mask & (SC_X | SC_Y) != 0 {
                sets.push(format!("{{\"mask\":{mask},\"blocked\":true}}"));
                continue;
            }
            proj.clear();
            project_state(st, mask, proj);
            match table.get(proj.as_slice()) {
                None => {
                    // Nearest record: min differing-component count over
                    // a bounded scan of this set's table.
                    let mut near: Option<(u32, u16)> = None;
                    for k in table.keys().take(PROBE_NEAR_SCAN) {
                        let d = component_diff(mask, proj, k);
                        let n = d.count_ones();
                        if near.map_or(true, |(bn, _)| n < bn) {
                            near = Some((n, d));
                        }
                    }
                    let near_s = near.map_or("null".to_string(), |(n, d)| {
                        format!("{{\"ncomp\":{n},\"diff\":{d}}}")
                    });
                    sets.push(format!(
                        "{{\"mask\":{mask},\"proj\":{:?},\"keys\":{},\"near\":{near_s}}}",
                        proj,
                        table.len()
                    ));
                }
                Some(recs) => {
                    // State pattern matched; report the first failing
                    // condition of each record (cap 4).
                    let fails: Vec<String> = recs
                        .iter()
                        .take(4)
                        .map(|(conds, benefit)| {
                            let f = conds.iter().find(|&&(s, m, v)| {
                                it_decided[s as usize] & m != m || it_value[s as usize] & m != v
                            });
                            match f {
                                Some(&(s, m, v)) => format!(
                                    "{{\"slot\":{s},\"m\":{m},\"want\":{v},\"dec\":{},\"got\":{},\"benefit\":{benefit}}}",
                                    it_decided[s as usize] & m,
                                    it_value[s as usize] & m
                                ),
                                None => "{}".to_string(),
                            }
                        })
                        .collect();
                    sets.push(format!(
                        "{{\"mask\":{mask},\"recs\":{},\"fails\":[{}]}}",
                        recs.len(),
                        fails.join(",")
                    ));
                }
            }
        }
        let kind = if outcome == PROBE_STATE_MISS { "state_miss" } else { "cond_miss" };
        let prober: Vec<String> = (0..slots as usize)
            .map(|s| format!("\"{:04x}/{:04x}\"", it_decided[s], it_value[s]))
            .collect();
        let line = format!(
            "{{\"probe\":\"{kind}\",\"cycle\":{},\"ni\":{},\"pc\":{},\"stall\":\"{:?}\",\"clk\":{},\"x\":{},\"y\":{},\"osr\":{},\"isr\":{},\"irq\":{},\"out\":{},\"dir\":{},\"prober\":[{}],\"sets\":[{}]}}",
            key.0, key.1, st.pc, st.stall, st.clk_acc, st.x, st.y, st.osr, st.isr,
            st.irq_flags, st.out_latch, st.dir_latch,
            prober.join(","),
            sets.join(",")
        );
        if self.w.write_all(line.as_bytes()).is_ok() && self.w.write_all(b"\n").is_ok() {
            self.wrote(line.len() + 1);
        }
    }

    /// Exact census + totals, written at search end.
    fn finish(&mut self, stats: &Stats) {
        use std::io::Write;
        for (c, row) in self.census.iter().enumerate() {
            if row.iter().any(|&n| n > 0) {
                let _ = writeln!(
                    self.w,
                    "{{\"census_cycle\":{c},\"nocore\":{},\"state_miss\":{},\"cond_miss\":{},\"hit\":{}}}",
                    row[0], row[1], row[2], row[3]
                );
            }
        }
        let _ = writeln!(
            self.w,
            "{{\"probe_log_end\":{{\"items\":{},\"core_matches\":{},\"state_misses\":{},\"cond_misses\":{},\"hits\":{},\"detail_bytes\":{},\"final_stride\":{}}}}}",
            stats.items,
            stats.memo_core_matches,
            stats.memo_state_misses,
            stats.memo_cond_misses,
            stats.memo_hits,
            self.bytes,
            self.stride
        );
        let _ = self.w.flush();
    }
}

/// Full-table snapshots, `PIO_NARROW_SNAPSHOT`.
struct Snapshotter {
    dir: std::path::PathBuf,
    seq: u32,
    max_purge: u32,
}

fn snapshot_open() -> Option<Snapshotter> {
    let dir = std::path::PathBuf::from(std::env::var("PIO_NARROW_SNAPSHOT").ok()?);
    let max_purge = std::env::var("PIO_NARROW_SNAPSHOT_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    if std::fs::create_dir_all(&dir).is_err() {
        eprintln!("narrow-search: cannot create snapshot dir {}", dir.display());
        return None;
    }
    eprintln!(
        "narrow-search: memo snapshots to {} (max {max_purge} purge snaps + end)",
        dir.display()
    );
    Some(Snapshotter { dir, seq: 0, max_purge })
}

impl Snapshotter {
    /// Dump the whole table + open frame stack. `reason` is "purge"
    /// (called BEFORE the retain, `min_benefit` is the pre-raise bar)
    /// or "end".
    fn write(
        &mut self,
        reason: &str,
        memo: &std::collections::HashMap<KeyCore, MemoEntry>,
        min_benefit: u32,
        stats: &Stats,
        frames: &[Frame],
    ) {
        use std::io::Write;
        if reason == "purge" {
            if self.seq >= self.max_purge {
                return;
            }
        }
        let path = self.dir.join(format!(
            "memo-{:03}-{reason}-items{}.jsonl",
            self.seq, stats.items
        ));
        self.seq += 1;
        let Ok(f) = std::fs::File::create(&path) else {
            eprintln!("narrow-search: snapshot open failed: {}", path.display());
            return;
        };
        let mut w = std::io::BufWriter::new(f);
        let frames_s: Vec<String> = frames
            .iter()
            .map(|f| {
                format!(
                    "{{\"cycle\":{},\"ni\":{},\"pc\":{},\"children_left\":{},\"items_at_open\":{},\"recordable\":{}}}",
                    f.key.0, f.key.1, f.key.2.pc, f.children_left, f.items_at_open, f.recordable
                )
            })
            .collect();
        let _ = writeln!(
            w,
            "{{\"snapshot\":{{\"reason\":\"{reason}\",\"min_benefit\":{min_benefit},\"items\":{},\"memo_entries\":{},\"memo_hits\":{},\"core_matches\":{},\"state_misses\":{},\"cond_misses\":{},\"purges\":{},\"frames\":[{}]}}}}",
            stats.items,
            stats.memo_entries,
            stats.memo_hits,
            stats.memo_core_matches,
            stats.memo_state_misses,
            stats.memo_cond_misses,
            stats.memo_purges,
            frames_s.join(",")
        );
        for (core, entry) in memo {
            for &(mask, ref table) in &entry.sets {
                for (vals, recs) in table {
                    for (conds, benefit) in recs {
                        let conds_s: Vec<String> = conds
                            .iter()
                            .map(|&(s, m, v)| format!("[{s},{m},{v}]"))
                            .collect();
                        let pending = core
                            .pending_exec
                            .map_or("null".to_string(), |p| p.to_string());
                        let _ = writeln!(
                            w,
                            "{{\"cycle\":{},\"ni\":{},\"pc\":{},\"delay\":{},\"stall\":\"{:?}\",\"pending\":{pending},\"clk\":{},\"out\":{},\"dir\":{},\"mask\":{mask},\"vals\":{:?},\"conds\":[{}],\"benefit\":{benefit}}}",
                            core.cycle, core.next_input, core.pc, core.delay_count,
                            core.stall, core.clk_acc, core.out_latch, core.dir_latch,
                            vals,
                            conds_s.join(",")
                        );
                    }
                }
            }
        }
        let _ = w.flush();
        eprintln!("narrow-search: snapshot written: {}", path.display());
    }
}

/// The program bits an executing instruction consults at fetch: side +
/// opcode + the (decided) opcode's operand fields. Delay is consulted
/// separately at completion. Over-approximating consultation only adds
/// record conditions — weaker matching, never unsoundness.
fn fetch_footprint(decided: u16, value: u16, cfg: &NCfg) -> u16 {
    let mut m = 0xE000u16;
    if cfg.side_count > 0 {
        m |= (((1u16 << cfg.side_count) - 1) << (5 - cfg.side_count)) << 8;
    }
    if decided & 0xE000 == 0xE000 {
        m |= match (value >> 13) & 0x7 {
            4 => 0x00E0,
            6 => 0x007F,
            // SET to X/Y: the 5-bit immediate is NOT consulted at
            // execution — it flows into the register's provenance and
            // is consulted only if the register is actually read.
            7 if decided & 0x00E0 == 0x00E0 && matches!((value >> 5) & 0x7, 1 | 2) => 0x00E0,
            _ => 0x00FF,
        };
    }
    m
}

/// Exhaustive needed-narrowing search. Deterministic: DFS order is fixed
/// by field order and value enumeration order (memo hits only skip
/// subtrees that would have been fully refuted, so champions and
/// verdicts are unchanged).
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
        prev_nop_submax: false,
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

    // Consulted-set memo (failure-only): frames mirror the DFS stack —
    // LIFO makes every subtree contiguous — with a synthetic,
    // never-recorded root frame absorbing top-level merges.
    let memo_on = spec.memo_cap > 0;
    let mut dump = if memo_on { dump_open() } else { None };
    let mut probe_log = if memo_on { probe_log_open(spec.cycles) } else { None };
    let mut snap = if memo_on { snapshot_open() } else { None };
    let mut memo: std::collections::HashMap<KeyCore, MemoEntry> =
        std::collections::HashMap::new();
    // Records must beat this subtree size to earn a slot; quadruples at
    // every cap purge.
    let mut min_benefit: u32 = 4;
    // Scratch for probe-side state projection.
    let mut proj: Vec<u32> = Vec::new();
    let mut frames: Vec<Frame> = vec![Frame {
        key: (0, 0, root.st),
        decided: [0u16; 32],
        children_left: 1,
        consulted_mask: [0u16; 32],
        consulted_value: [0u16; 32],
        state_reads: 0,
        any_champion: false,
        recordable: false,
        items_at_open: 0,
    }];

    'items: while let Some(mut it) = stack.pop() {
        stats.items += 1;
        if last_beat.elapsed().as_secs() >= 10 {
            eprintln!(
                "narrow-search: items={} forks={} refuted={} prefilt={} canon={} memo_hit={} memo_ent={} minben={} purges={} champions={} stack={}",
                stats.items,
                stats.forks,
                stats.refuted,
                stats.prefiltered,
                stats.canon_pruned,
                stats.memo_hits,
                stats.memo_entries,
                min_benefit,
                stats.memo_purges,
                stats.champions_found,
                stack.len()
            );
            last_beat = std::time::Instant::now();
        }
        cfg.code = it.value;

        // Apply the per-cycle environment BEFORE probing: frame keys
        // are captured at fork time, i.e. post-env, and application is
        // idempotent (the cycle loop re-applies it harmlessly).
        if memo_on && it.cycle < spec.cycles {
            if let Some(&m) = irq_at.get(&it.cycle) {
                it.st.irq_flags |= m;
            }
            if !preload {
                while (it.next_input as usize) < spec.inputs.len() && !it.st.tx.is_full() {
                    it.st.tx.push(spec.inputs[it.next_input as usize]);
                    it.next_input += 1;
                }
            }
        }

        // Memo probe: a recorded failure whose key core matches this
        // item, whose state pattern matches on every component the
        // recorded subtree READ, and whose program conditions this
        // item's decided fields satisfy refutes the item outright. An
        // UNBOUND item with x != y also stands for its mirror twin
        // rooted at the SWAPPED state — such a hit is only valid if the
        // record read neither X nor Y (then it covers the twin too).
        if memo_on {
            let core = key_core(it.cycle, it.next_input, &it.st);
            let mirror_blocked = it.unbound && it.st.x != it.st.y;
            let mut outcome = PROBE_NOCORE;
            let mut hit: Option<(Conds, u16)> = None;
            if let Some(entry) = memo.get(&core) {
                stats.memo_core_matches += 1;
                outcome = PROBE_STATE_MISS;
                'sets: for &(mask, ref table) in &entry.sets {
                    if mirror_blocked && mask & (SC_X | SC_Y) != 0 {
                        continue;
                    }
                    proj.clear();
                    project_state(&it.st, mask, &mut proj);
                    if let Some(recs) = table.get(&proj) {
                        outcome = PROBE_COND_MISS;
                        for (conds, _) in recs {
                            if conds.iter().all(|&(s, m, v)| {
                                it.decided[s as usize] & m == m
                                    && it.value[s as usize] & m == v
                            }) {
                                hit = Some((conds.clone(), mask));
                                outcome = PROBE_HIT;
                                break 'sets;
                            }
                        }
                    }
                }
                match outcome {
                    PROBE_STATE_MISS => stats.memo_state_misses += 1,
                    PROBE_COND_MISS => stats.memo_cond_misses += 1,
                    _ => {}
                }
            }
            if let Some(pl) = probe_log.as_mut() {
                if pl.count(it.cycle, outcome) {
                    if let Some(entry) = memo.get(&core) {
                        pl.miss(
                            outcome,
                            &(it.cycle, it.next_input, it.st),
                            entry,
                            mirror_blocked,
                            &it.decided,
                            &it.value,
                            spec.slots,
                            &mut proj,
                        );
                    }
                }
            }
            if let Some((conds, rmask)) = hit {
                stats.memo_hits += 1;
                if let Some(d) = dump.as_mut() {
                    d.hit(&(it.cycle, it.next_input, it.st), &conds, &it.decided, &it.value, spec.slots);
                }
                let top = frames.last_mut().expect("root frame");
                top.state_reads |= rmask;
                for &(s, m, v) in &conds {
                    top.merge(s as usize, m, v);
                }
                close_child(&mut frames, &mut memo, spec.memo_cap, &mut min_benefit, &mut snap, &mut stats, false);
                continue 'items;
            }
        }
        // This pop's run segment: which decided program fields and
        // which state components it reads, with segment-local X/Y
        // provenance.
        let mut seg_mask = [0u16; 32];
        let mut seg_reads: u16 = 0;
        let mut x_prov = Prov::Fork;
        let mut y_prov = Prov::Fork;

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

            // A pending stall re-check reads its subject every cycle.
            if memo_on && it.st.stall != Stall::None {
                seg_reads |= stall_state_reads(it.st.stall);
            }

            let fetching = peek_tick(&it.st, &cfg) && will_fetch(&it.st, &cfg, gpio_in);
            let fetch_pc = it.st.pc as usize;
            if fetching {
                if memo_on {
                    seg_mask[fetch_pc] |=
                        fetch_footprint(it.decided[fetch_pc], it.value[fetch_pc], &cfg)
                            & it.decided[fetch_pc];
                }
                if let Some(field) = demand(it.decided[fetch_pc], it.value[fetch_pc], &cfg) {
                    values.clear();
                    field.values_into(&cfg, spec.slots, &mut values);
                    let mut pushed = 0u32;
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
                        // P2 canonical nop: MOV self-moves with op none
                        // (x,x / y,y / isr,isr) are pure no-ops; the one
                        // kept spelling is `mov osr,osr` (NOP_CANON).
                        if field.kind == FieldKind::MovSrc {
                            let dst = (it.value[fetch_pc] >> 5) & 0x7;
                            let mov_op = (it.value[fetch_pc] >> 3) & 0x3;
                            if mov_op == 0 && raw == dst && matches!(dst, 1 | 2 | 6) {
                                stats.canon_pruned += 1;
                                continue;
                            }
                        }
                        // P4 vacuous control: a JMP to its own
                        // fallthrough with a non-writing condition
                        // (everything but X--/Y--) is another no-op
                        // spelling; the canonical nop lives in the
                        // sibling MOV branch of this same fork tree.
                        if field.kind == FieldKind::JmpTarget {
                            let cond = (it.value[fetch_pc] >> 5) & 0x7;
                            if cond != 2 && cond != 4 {
                                let ft = if fetch_pc as u8 == cfg.wrap_top {
                                    cfg.wrap_bottom
                                } else {
                                    (fetch_pc as u8 + 1) & 0x1F
                                };
                                if raw == ft as u16 {
                                    stats.canon_pruned += 1;
                                    continue;
                                }
                            }
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
                            // The lookahead's verdict reads whatever the
                            // candidate word reads — those are subtree
                            // conditions even for values never pushed.
                            if memo_on {
                                let r = word_state_reads(child_value, &cfg);
                                consume_reads(r, x_prov, y_prov, &mut seg_reads, &mut seg_mask);
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
                        pushed += 1;
                    }
                    if memo_on {
                        merge_segment(frames.last_mut().expect("root frame"), &seg_mask, seg_reads, &it.value);
                        if pushed > 0 {
                            frames.push(Frame {
                                key: (it.cycle, it.next_input, it.st),
                                decided: it.decided,
                                children_left: pushed,
                                consulted_mask: [0u16; 32],
                                consulted_value: [0u16; 32],
                                any_champion: false,
                                recordable: true,
                                state_reads: 0,
                                items_at_open: stats.items,
                            });
                        } else {
                            // Every value filtered: this fork is a leaf.
                            close_child(&mut frames, &mut memo, spec.memo_cap, &mut min_benefit, &mut snap, &mut stats, false);
                        }
                    }
                    continue 'items;
                }
            }

            // An instruction (fetched or pending) that can execute this
            // cycle reads its per-opcode components; over-approximating
            // (e.g. while a stall persists) is sound. Reads route
            // through X/Y provenance; the word is remembered for the
            // post-step write processing.
            let exec_word = it.st.pending_exec.unwrap_or(cfg.code[fetch_pc]);
            let exec_pending = it.st.pending_exec.is_some();
            if memo_on && it.st.delay_count == 0 && peek_tick(&it.st, &cfg) {
                let r = word_state_reads(exec_word, &cfg);
                consume_reads(r, x_prov, y_prov, &mut seg_reads, &mut seg_mask);
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
                    let mut pushed = 1;
                    if twin_differs {
                        std::mem::swap(&mut twin.st.x, &mut twin.st.y);
                        stack.push(twin);
                        stats.forks += 1;
                        pushed = 2;
                    }
                    if memo_on {
                        merge_segment(frames.last_mut().expect("root frame"), &seg_mask, seg_reads, &it.value);
                        frames.push(Frame {
                            key: (it.cycle, it.next_input, it.st),
                            decided: it.decided,
                            children_left: pushed,
                            consulted_mask: [0u16; 32],
                            consulted_value: [0u16; 32],
                            any_champion: false,
                            recordable: true,
                            state_reads: 0,
                            items_at_open: stats.items,
                        });
                    }
                    continue 'items;
                }
            }

            let pre_delay = it.st.delay_count;
            let ticked = clock_tick(&mut it.st, &cfg);
            if ticked {
                super::step(&mut it.st, &cfg, gpio_in);
            }
            stats.cycles_run += 1;

            let w = capture_word(&it.st, &spec.capture_pins, spec.stim.mask, ext);
            if w != spec.expected[it.cycle as usize] {
                stats.refuted += 1;
                if memo_on {
                    merge_segment(frames.last_mut().expect("root frame"), &seg_mask, seg_reads, &it.value);
                    close_child(&mut frames, &mut memo, spec.memo_cap, &mut min_benefit, &mut snap, &mut stats, false);
                }
                continue 'items;
            }
            it.cycle += 1;

            // Did an instruction COMPLETE this cycle (not a delay tick,
            // not left stalled)? Only then is delay consulted, and only
            // then does the P3 nop-run state advance.
            let completed = ticked && pre_delay == 0 && it.st.stall == Stall::None;
            // A completed execution's register writes update the
            // segment provenance (stalled executions skip their writes,
            // so this must be post-step and completion-gated). Exec'd
            // words come from data — their immediates are core-covered,
            // hence Accounted.
            if memo_on && completed {
                let (wx, wy, imm) = word_reg_writes(exec_word);
                let p = if imm && !exec_pending {
                    Prov::Field(fetch_pc as u8, 0x001F)
                } else {
                    Prov::Accounted
                };
                if wx {
                    x_prov = p;
                }
                if wy {
                    y_prov = p;
                }
            }
            // A FETCHED canonical nop (P3 only applies without side-set,
            // whose assertion would make delay splits observable).
            let nop_run = completed
                && fetching
                && cfg.side_count == 0
                && cfg.code[fetch_pc] & NOP_CANON_MASK == NOP_CANON;

            // Delay post-fork: delay is consulted only at COMPLETION, so
            // it is forked only for instructions that actually completed
            // (no stall) on an already-survived cycle. Refuted items
            // never pay the delay multiplier; an instruction completing
            // on the final cycle keeps delay as a don't-care.
            if fetching && it.st.stall == Stall::None && delay_mask != 0 {
                let undecided = it.decided[fetch_pc] & delay_mask != delay_mask;
                if undecided && it.cycle < spec.cycles {
                    let max = (1u16 << delay_bits) - 1;
                    let pushed;
                    // P3 delay-normal form: consecutive freshly-forked
                    // canonical nops carry front-loaded delays — once a
                    // run nop chose below max, every later nop in the
                    // run contributes 0 (one spelling per total; the
                    // representative differs only in how the same idle
                    // cycles split across pure no-ops).
                    if nop_run && it.prev_nop_submax {
                        let mut child = it;
                        child.decided[fetch_pc] |= delay_mask;
                        child.st.delay_count = 0;
                        child.prev_nop_submax = true;
                        stack.push(child);
                        stats.forks += 1;
                        stats.canon_pruned += max as u64;
                        pushed = 1;
                    } else {
                        for v in 0..=max {
                            let mut child = it;
                            child.decided[fetch_pc] |= delay_mask;
                            child.value[fetch_pc] |= v << 8;
                            child.st.delay_count = v as u8;
                            child.prev_nop_submax = nop_run && v < max;
                            stack.push(child);
                            stats.forks += 1;
                        }
                        pushed = max as u32 + 1;
                    }
                    if memo_on {
                        merge_segment(frames.last_mut().expect("root frame"), &seg_mask, seg_reads, &it.value);
                        frames.push(Frame {
                            key: (it.cycle, it.next_input, it.st),
                            decided: it.decided,
                            children_left: pushed,
                            consulted_mask: [0u16; 32],
                            consulted_value: [0u16; 32],
                            any_champion: false,
                            recordable: true,
                            state_reads: 0,
                            items_at_open: stats.items,
                        });
                    }
                    continue 'items;
                }
                if memo_on && !undecided {
                    // Completion consults the (already decided) delay.
                    seg_mask[fetch_pc] |= delay_mask;
                }
            }
            if completed {
                // Seeded/revisited delays break the run: their
                // front-loaded representative may not exist in-space.
                it.prev_nop_submax = false;
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
        if memo_on {
            merge_segment(frames.last_mut().expect("root frame"), &seg_mask, seg_reads, &it.value);
            close_child(&mut frames, &mut memo, spec.memo_cap, &mut min_benefit, &mut snap, &mut stats, true);
        }
    }
    debug_assert!(frames.len() == 1, "unbalanced frame accounting");
    if let Some(d) = dump.as_mut() {
        d.finish(&stats);
    }
    if let Some(pl) = probe_log.as_mut() {
        pl.finish(&stats);
    }
    if let Some(sn) = snap.as_mut() {
        sn.write("end", &memo, min_benefit, &stats, &frames);
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
