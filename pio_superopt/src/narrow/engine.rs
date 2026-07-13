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
    /// Fork values killed by the word behavioral quotient (ticket 009):
    /// the candidate's partial word is lemma-provably interchangeable
    /// with an already-generated sibling of the same fork.
    pub quotient_pruned: u64,
    /// Items refuted for fetching a slot at/past the searched length
    /// (out-of-footprint execution is undefined behavior on hardware;
    /// counted inside `refuted` too).
    pub oob_refuted: u64,
    /// Delay forks collapsed by the junk-window walk (008 stage 2):
    /// each is a whole delay-spelling family refuted by one
    /// representative walk (counted inside `refuted` too).
    pub look_refuted: u64,
    /// junk_walk invocations and total cycles stepped inside them —
    /// against `cycles_run` this attributes walk cost vs main-loop
    /// cost (walks ending Unclean are calls − look_refuted).
    pub walk_calls: u64,
    pub walk_cycles: u64,
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
    /// Probes that reached a record list (core + mask + state values
    /// all matched) and scanned it.
    pub memo_rec_scans: u64,
    /// Total records examined across those scans (mean list length =
    /// this / memo_rec_scans) — the probe-side linear-scan cost.
    pub memo_recs_scanned: u64,
    /// Longest record list seen at probe.
    pub memo_max_recs: u64,
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
    fn values_into(&self, cfg: &NCfg, out: &mut Vec<u16>) {
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
        // JMP's target is consulted only by a TAKEN execution — like
        // delay, it is demanded at consult time (deferred target fork
        // in the cycle loop), never at fetch.
        0 => &[(0x00E0, FieldKind::JmpCond)],
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

/// The canonical no-op spelling: `mov x, x` (op none). The only true
/// no-ops are the X/Y self-moves — MOV to ISR/OSR resets the target's
/// shift counter (datasheet 11.4.10), so the old canon `mov osr, osr`
/// is a real op. Forking the nop names X and sets `named`, so P1
/// disarms in nop-carrying unbound subtrees (the Y-spelled twin is
/// P2-pruned everywhere and covered via the mirror). Bits 8..12
/// (delay/side) excluded from the pattern.
pub const NOP_CANON: u16 = 0xA021;
const NOP_CANON_MASK: u16 = 0xE0FF;

/// Ticket 009: the lemma-verified behavioral word quotient. Maps a
/// slot word to its class-canonical spelling under `cfg` — two words
/// map together ONLY when a hand-audited, config-parameterized lemma
/// (checked line-by-line against `exec_op`, battery-gated by
/// `word_canon_battery_sound`) proves them interchangeable as FETCHED
/// instructions in EVERY state. Digest batteries may SUGGEST more
/// classes; unproven candidates stay singletons. Delay/side bits
/// (8..12) are always preserved — side-set asserts pins even on nops.
///
/// Lemmas: (1) WAIT-IRQ / IRQ behavior factors through
/// `resolve_irq_index` (rel folding is sm_id-dependent; IRQ operand
/// bit 7 is never read); (2) OUT to PINS/PINDIRS with out_count == 0
/// writes nothing ≡ OUT NULL; (3) MOV to PINS/PINDIRS with
/// out_count == 0 is a pure read ≡ no-op, and the X/Y self-moves
/// (op none) are no-ops; (4) MOV from STATUS with status_n == 0 reads
/// constant 0 ≡ NULL source; (5) SET to PINS/PINDIRS masks data to
/// set_count bits (count 0 ⇒ no-op); (6) PUSH/PULL never read operand
/// bits 0..4. The no-op representative is the class minimum:
/// `mov pins, pins` (0xA000) when out_count == 0, else NOP_CANON.
pub fn word_canon(w: u16, cfg: &NCfg) -> u16 {
    let ds = w & 0x1F00;
    let nop_rep = if cfg.out_count == 0 { 0xA000 | ds } else { NOP_CANON | ds };
    match (w >> 13) & 0x7 {
        1 => {
            if (w >> 5) & 0x3 == 2 {
                let r = super::resolve_irq_index((w & 0x1F) as u8, cfg.sm_id) as u16;
                (w & !0x001F) | r
            } else {
                w
            }
        }
        3 => {
            let dst = (w >> 5) & 0x7;
            if (dst == 0 || dst == 4) && cfg.out_count == 0 {
                (w & !0x00E0) | (3 << 5)
            } else {
                w
            }
        }
        4 => w & !0x001F,
        5 => {
            let (dst, op, src) = ((w >> 5) & 0x7, (w >> 3) & 0x3, w & 0x7);
            if (dst == 0 || dst == 3) && cfg.out_count == 0 {
                return nop_rep;
            }
            if op == 0 && dst == src && (dst == 1 || dst == 2) {
                return nop_rep;
            }
            if src == 5 && cfg.status_n == 0 {
                (w & !0x0007) | 3
            } else {
                w
            }
        }
        6 => {
            let r = super::resolve_irq_index((w & 0x1F) as u8, cfg.sm_id) as u16;
            (w & !0x009F) | r
        }
        7 => {
            let dst = (w >> 5) & 0x7;
            if dst == 0 || dst == 4 {
                if cfg.set_count == 0 {
                    return nop_rep;
                }
                let m = (1u16 << cfg.set_count.min(5)) - 1;
                (w & !0x001F) | (w & m)
            } else {
                w
            }
        }
        _ => w,
    }
}

/// Per-config quotient state: the full-word canon table plus lazily
/// built per-mask class-id tables for PARTIAL words. Under `mask`, two
/// masked values share an id iff every completion of the undecided
/// bits lands in the same full-word class — i.e. the partial words are
/// behaviorally interchangeable wherever the search can still take
/// them. Ids are exact (profile-interned), not hashes.
struct WordQuotient {
    canon: Vec<u16>,
    tables: FxMap<u16, FxMap<u16, u32>>,
}

impl WordQuotient {
    fn build(cfg: &NCfg) -> WordQuotient {
        WordQuotient {
            canon: (0..=0xFFFFu16).map(|w| word_canon(w, cfg)).collect(),
            tables: FxMap::default(),
        }
    }

    fn table(&mut self, mask: u16) -> &FxMap<u16, u32> {
        let canon = &self.canon;
        self.tables.entry(mask).or_insert_with(|| {
            let inv = !mask;
            let mut intern: FxMap<Vec<u16>, u32> = FxMap::default();
            let mut out: FxMap<u16, u32> = FxMap::default();
            let mut v = 0u16;
            loop {
                let mut profile = Vec::with_capacity(1usize << inv.count_ones().min(13));
                let mut c = 0u16;
                loop {
                    profile.push(canon[(v | c) as usize]);
                    if c == inv {
                        break;
                    }
                    c = c.wrapping_sub(inv) & inv;
                }
                let next = intern.len() as u32;
                let id = *intern.entry(profile).or_insert(next);
                out.insert(v, id);
                if v == mask {
                    break;
                }
                v = v.wrapping_sub(mask) & mask;
            }
            out
        })
    }
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

/// Is this word a PULL that READS X this cycle (a nonblocking pull
/// executing its empty-TX-FIFO path)? The one binding-asymmetric
/// instruction reachable from CODE (register fields there are virtual);
/// it distinguishes the twins iff x != y at execution. An IFEMPTY pull
/// whose guard fails (osr_count below threshold) is a complete no-op
/// and reads nothing; a blocking pull stalls instead of reading X.
fn is_pull_empty_read(w: u16, st: &NState, cfg: &NCfg) -> bool {
    if (w >> 13) & 0x7 != 4 || (w >> 7) & 1 == 0 {
        return false;
    }
    let if_empty = (w >> 6) & 1 != 0;
    let block = (w >> 5) & 1 != 0;
    if if_empty && st.osr_count < cfg.pull_threshold {
        return false;
    }
    st.tx.is_empty() && !block
}

/// Pre-step peek: would this JMP (opcode + cond decided) take its
/// branch in this state? Mirrors exec_op's condition table exactly,
/// minus the X--/Y-- decrements (the peek must not mutate).
fn jmp_taken(w: u16, st: &NState, cfg: &NCfg, gpio_in: u32) -> bool {
    debug_assert_eq!(w >> 13, 0);
    match (w >> 5) & 0x7 {
        0 => true,
        1 => st.x == 0,
        2 => st.x != 0,
        3 => st.y == 0,
        4 => st.y != 0,
        5 => st.x != st.y,
        6 => (gpio_in >> (cfg.jmp_pin & 0x1F)) & 1 != 0,
        _ => st.osr_count < cfg.pull_threshold,
    }
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
    if mask & SC_TX != 0 {
        out.push(st.tx.level() as u32);
        for i in 0..st.tx.level() {
            out.push(st.tx.peek(i));
        }
    }
    // RX projects its LEVEL only: no ISA path reads RX contents (there
    // is no rx-fifo source; PUSH/autopush/STATUS consult fullness or
    // level) and the observable trace is pins — two states differing
    // only in queued RX words evolve identically. (Probe census
    // 2026-07-12: full-contents RX was the single largest state-miss
    // component, 44% of near-miss diffs.)
    if mask & SC_RX != 0 {
        out.push(st.rx.level() as u32);
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
            let if_flag = if (w >> 6) & 1 != 0 { u16::MAX } else { 0 };
            if (w >> 7) & 1 != 0 {
                // PULL reads TX; on empty it reads X, and the binding
                // demand compares x != y. IFEMPTY guards on osr_count.
                SC_TX | SC_X | SC_Y | (if_flag & SC_OSR_CNT)
            } else {
                // PUSH; IFFULL guards on isr_count.
                SC_RX | SC_ISR | (if_flag & SC_ISR_CNT)
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
/// over everything else. `close_child` emits at most one entry per
/// slot, in slot order — `RecList` packing relies on both.
type Conds = Vec<(u8, u16, u16)>;

/// The memo maps are hashed with FxHash: SipHash was ~25% of L=3
/// search CPU (perf, 2026-07-12) and these keys carry no DoS surface.
/// Nothing decision-bearing consults map iteration order (purge
/// retention is a per-record predicate), so verdicts, champions and
/// stats are unchanged; only dump/snapshot line order shifts.
type FxMap<K, V> =
    std::collections::HashMap<K, V, std::hash::BuildHasherDefault<rustc_hash::FxHasher>>;

/// Pack one cond as `slot << 32 | mask << 16 | value`.
#[inline]
fn pack_cond(s: u8, m: u16, v: u16) -> u64 {
    (s as u64) << 32 | (m as u64) << 16 | v as u64
}

/// Does record `a` subsume record `b` — does `a` fire on every item
/// `b` fires on? True iff every cond of `a` is implied by a cond of
/// `b` on the same slot: `b` demands at least `a`'s bits with the same
/// values. Both packed lists are in ascending slot order.
fn subsumes(a: &[u64], b: &[u64]) -> bool {
    let mut bi = 0usize;
    'conds: for &pa in a {
        let (sa, ma, va) = ((pa >> 32) as u8, (pa >> 16) as u16, pa as u16);
        while bi < b.len() {
            let (sb, mb, vb) = ((b[bi] >> 32) as u8, (b[bi] >> 16) as u16, b[bi] as u16);
            if sb < sa {
                bi += 1;
            } else if sb == sa {
                if mb & ma == ma && vb & ma == va {
                    continue 'conds;
                }
                return false;
            } else {
                return false;
            }
        }
        return false;
    }
    true
}

/// A record list at one (core, read-mask, projected-values) key: every
/// record's packed conds live in ONE contiguous buffer, so the probe's
/// linear scan streams a single allocation instead of chasing a `Vec`
/// per record (that chase was ~55% of L=3 search CPU). Lists are kept
/// short by insert-side subsumption and `REC_LIST_CAP`.
#[derive(Default)]
struct RecList {
    packed: Vec<u64>,
    /// (start of the record's conds in `packed`, benefit); record i
    /// ends where record i+1 starts (the last at `packed.len()`).
    recs: Vec<(u32, u32)>,
}

/// Max records per value-key: eviction keeps the highest benefits.
/// Sound at any value (fewer records = fewer hits, never wrong ones);
/// bounds the probe scan as lists age. Swept on tx_a (2026-07-12):
/// 32 vs 64 vs 256 gave L=3-slice throughput 1.72M/s vs 1.28M/s vs
/// 0.69M/s at equal hit DENSITY (~6.5%), and 32 also won wall-clock on
/// both L=2 brackets — longer lists were pure scan cost. (The L=2 1..1
/// item-count inflation vs the pre-subsumption memo, 23.9M -> 56.9M,
/// is first-match selection, not the cap: 64 didn't recover it, and
/// wall-clock still improved 3-4x.)
const REC_LIST_CAP: usize = 32;

impl RecList {
    #[inline]
    fn conds_of(&self, i: usize) -> &[u64] {
        let start = self.recs[i].0 as usize;
        let end = self.recs.get(i + 1).map_or(self.packed.len(), |r| r.0 as usize);
        &self.packed[start..end]
    }

    /// Rebuild without the records whose indices are in `dead`
    /// (ascending). O(list) — lists are capped and short.
    fn remove(&mut self, dead: &[usize]) {
        if dead.is_empty() {
            return;
        }
        let mut packed = Vec::with_capacity(self.packed.len());
        let mut recs = Vec::with_capacity(self.recs.len() - dead.len());
        let mut d = 0usize;
        for i in 0..self.recs.len() {
            if d < dead.len() && dead[d] == i {
                d += 1;
                continue;
            }
            recs.push((packed.len() as u32, self.recs[i].1));
            packed.extend_from_slice(self.conds_of(i));
        }
        self.packed = packed;
        self.recs = recs;
    }

    /// Insert a record with subsumption both ways and the list cap;
    /// returns the change in stored-record count (-cap evictions can
    /// make it negative even on success).
    fn insert(&mut self, conds: &Conds, benefit: u32) -> i64 {
        // An existing record that subsumes the new one fires strictly
        // more often: the new record adds nothing. Keep the higher
        // benefit as the survivor's purge priority.
        let new_packed: Vec<u64> = conds.iter().map(|&(s, m, v)| pack_cond(s, m, v)).collect();
        let mut benefit = benefit;
        for i in 0..self.recs.len() {
            if subsumes(self.conds_of(i), &new_packed) {
                self.recs[i].1 = self.recs[i].1.max(benefit);
                return 0;
            }
        }
        // Records the new one subsumes are dead weight; fold their
        // benefits into the new record. Removing at least one always
        // frees room, so the cap only bites when `dead` is empty.
        let mut dead: Vec<usize> = Vec::new();
        for i in 0..self.recs.len() {
            if subsumes(&new_packed, self.conds_of(i)) {
                benefit = benefit.max(self.recs[i].1);
                dead.push(i);
            }
        }
        if dead.is_empty() && self.recs.len() >= REC_LIST_CAP {
            let (mi, &(_, mb)) = self
                .recs
                .iter()
                .enumerate()
                .min_by_key(|&(_, &(_, b))| b)
                .expect("cap > 0");
            if mb >= benefit {
                return 0; // new record is the weakest: drop it
            }
            self.remove(&[mi]);
            self.recs.push((self.packed.len() as u32, benefit));
            self.packed.extend_from_slice(&new_packed);
            return 0;
        }
        let removed = dead.len() as i64;
        self.remove(&dead);
        self.recs.push((self.packed.len() as u32, benefit));
        self.packed.extend_from_slice(&new_packed);
        1 - removed
    }

    /// Drop records below the purge bar; returns how many remain.
    fn retain_benefit(&mut self, bar: u32) -> usize {
        let dead: Vec<usize> = self
            .recs
            .iter()
            .enumerate()
            .filter(|&(_, &(_, b))| b < bar)
            .map(|(i, _)| i)
            .collect();
        self.remove(&dead);
        self.recs.len()
    }
}

/// Failure records at one key core, two-level like the playground's
/// MemoEntry: per distinct read-mask, a table from projected state
/// values to the record list — so a probe costs one projection + one
/// hash lookup per DISTINCT MASK at the core (typically one or two),
/// not a scan of every record ever stored.
#[derive(Default)]
struct MemoEntry {
    sets: Vec<(u16, FxMap<Vec<u32>, RecList>)>,
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
    memo: &mut FxMap<KeyCore, MemoEntry>,
    memo_cap: usize,
    min_benefit: &mut u32,
    snap: &mut Option<Snapshotter>,
    stats: &mut Stats,
    champion: bool,
    search_slots: u8,
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
        // Slots at/past `search_slots` are spec constants — never
        // forked, never twin-mirrored — so a cond on them matches every
        // prober by construction and is dropped. (Snapshot 2026-07-12:
        // filler-walk conds were 88% of all cond storage and pure
        // always-match scan overhead. Seeded bits INSIDE searched slots
        // stay: the binding fork mirrors those slots in the twin.)
        let mut conds: Conds = Vec::new();
        for s in 0..search_slots as usize {
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
                        table.retain(|_, list| {
                            let n = list.retain_benefit(*min_benefit);
                            kept += n as u64;
                            n > 0
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
                        entry.sets.push((f.state_reads, FxMap::default()));
                        &mut entry.sets.last_mut().unwrap().1
                    }
                };
                let delta = table.entry(vals).or_default().insert(&conds, benefit);
                stats.memo_entries = (stats.memo_entries as i64 + delta) as u64;
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
    counts: FxMap<MemoKey, u32>,
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
        counts: FxMap::default(),
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
/// values differ between two same-mask projections. TX is
/// variable-length (level + words), so the cursors advance per-side;
/// RX projects its level only.
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
    if mask & SC_TX != 0 {
        let (la, lb) = (a[i] as usize, b[j] as usize);
        if a[i..=i + la] != b[j..=j + lb] {
            diff |= SC_TX;
        }
        i += la + 1;
        j += lb + 1;
    }
    if mask & SC_RX != 0 && a[i] != b[j] {
        diff |= SC_RX;
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
    // Initial sampling stride. At stride 1 a miss-heavy search burns
    // the whole budget in the first minute of shallow region; start
    // coarser to spread detail across the run's depth.
    let stride0 = std::env::var("PIO_NARROW_PROBE_STRIDE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&s: &u64| s > 0)
        .unwrap_or(1);
    let f = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
    eprintln!("narrow-search: probe log to {path} (detail budget {budget} bytes)");
    Some(ProbeLog {
        w: std::io::BufWriter::new(f),
        bytes: 0,
        budget,
        next_thresh: budget / 2,
        stride: stride0,
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
            // The halving step integer-divides to 0 as the threshold
            // converges on the budget — clamp to 1 or this loop never
            // terminates (it froze the first instrumented L=3 run).
            self.next_thresh += ((self.budget - self.next_thresh) / 2).max(1);
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
                Some(list) => {
                    // State pattern matched; report the first failing
                    // condition of each record (cap 4).
                    let fails: Vec<String> = (0..list.recs.len().min(4))
                        .map(|i| {
                            let benefit = list.recs[i].1;
                            let f = list.conds_of(i).iter().copied().find(|&p| {
                                let (s, m, v) =
                                    ((p >> 32) as usize, (p >> 16) as u16, p as u16);
                                it_decided[s] & m != m || it_value[s] & m != v
                            });
                            match f {
                                Some(p) => {
                                    let (s, m, v) =
                                        ((p >> 32) as usize, (p >> 16) as u16, p as u16);
                                    format!(
                                        "{{\"slot\":{s},\"m\":{m},\"want\":{v},\"dec\":{},\"got\":{},\"benefit\":{benefit}}}",
                                        it_decided[s] & m,
                                        it_value[s] & m
                                    )
                                }
                                None => "{}".to_string(),
                            }
                        })
                        .collect();
                    sets.push(format!(
                        "{{\"mask\":{mask},\"recs\":{},\"fails\":[{}]}}",
                        list.recs.len(),
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
            "{{\"probe_log_end\":{{\"items\":{},\"core_matches\":{},\"state_misses\":{},\"cond_misses\":{},\"hits\":{},\"rec_scans\":{},\"recs_scanned\":{},\"max_recs\":{},\"detail_bytes\":{},\"final_stride\":{}}}}}",
            stats.items,
            stats.memo_core_matches,
            stats.memo_state_misses,
            stats.memo_cond_misses,
            stats.memo_hits,
            stats.memo_rec_scans,
            stats.memo_recs_scanned,
            stats.memo_max_recs,
            self.bytes,
            self.stride
        );
        let _ = self.w.flush();
    }
}

/// Pair-race divergence probe (instrumented sequential search only).
/// `PIO_NARROW_DELAY_PAIR` races cond-miss records differing from the
/// prober in EXACTLY one delay-bit cond; `PIO_NARROW_WORD_PAIR` widens
/// to ANY single decided-cond conflict in one searched slot (mining
/// for 008 stage 3 — sizes the cross-opcode outcome-equivalence class
/// by races, not fail-bit histograms). Verdicts tally per conflict
/// kind: delay (bits within delay_mask only), arg (same opcode,
/// non-delay bits), opcode (bits 15..13 differ). Lock-step simulate
/// both spellings forward from the probe point to classify whether
/// the record was over-conditioned on the differing bits:
///   co_refuted  — identical captures, both refute the same cycle
///                 (sharing would have been valid for this item)
///   absorbed    — states differed (the delay delta was expressed)
///                 then re-equalized before any capture diff (the
///                 WAIT/IRQ schedule swallowed it)
///   diverged    — captures differ (genuine timing effect)
///   one_refuted — exactly one spelling walks out of footprint
///   demand_edge — an undecided field would fork first (inconclusive)
///   unconsulted — race ended (horizon) with the differing delay
///                 never consulted — over-conditioned in-window
///   horizon     — trace end reached after the delta was expressed
/// Tallies stream to stderr every 4096 checks — a time-boxed kill
/// (50-min policy) loses nothing.
#[derive(Default)]
struct PairRace {
    /// Accept any single-cond conflict (word pair), not just delay bits.
    wide: bool,
    /// Recurse through equal-state joint demand edges (fork both
    /// machines with the same value, aggregate leaf verdicts) instead
    /// of terminating there. Verdict lattice: BAD < INC < CO.
    recurse: bool,
    /// All counters indexed by conflict kind: 0=delay, 1=arg, 2=opcode.
    checked: [u64; 3],
    /// Aggregate CO: every leaf of the joint race tree co-refuted —
    /// sharing the record with the prober would have been valid.
    share_co: [u64; 3],
    /// share_co sub-classification: race trees that wrote NO pin latch
    /// (capture equality is structural) / consulted NO external input
    /// (WAIT gpio/pin/irq, JMP PIN — schedule-resync instructions).
    co_latch_quiet: [u64; 3],
    co_ext_free: [u64; 3],
    /// Aggregate BAD by first cause: a leaf's captures differed /
    /// exactly one machine walked out of footprint.
    bad_diverged: [u64; 3],
    bad_one_refuted: [u64; 3],
    /// Inconclusive by first cause: flat-mode equal-state demand edge /
    /// demand edge with divergent states (no joint fork) / equal-state
    /// edge whose two words demand different fields (conflict slot
    /// executing) / trace end / step-or-depth budget.
    inc_demand_eq: [u64; 3],
    inc_demand_diff: [u64; 3],
    inc_mismatch: [u64; 3],
    inc_horizon: [u64; 3],
    inc_budget: [u64; 3],
    /// Leaf/tree telemetry: co-refuting leaves, their summed
    /// cycles-from-probe, joint forks taken, absorption events (states
    /// re-equalized after diverging).
    leaves_co: [u64; 3],
    cyc_leaf: [u64; 3],
    joint_forks: [u64; 3],
    absorb: [u64; 3],
    /// Summed race-tree step counts (cost model for a lever built on
    /// this walk shape).
    steps_sum: [u64; 3],
    unbound_skip: u64,
    /// Wide-mode candidate whose patched spelling violated a sibling
    /// cond on the same slot (overlapping masks) — not raceable.
    patch_skip: u64,
}

const PAIR_KIND: [&str; 3] = ["delay", "arg", "opcode"];

impl PairRace {
    fn print(&self, tag: &str) {
        let avg = |s: u64, n: u64| s as f64 / n.max(1) as f64;
        for k in 0..3 {
            if self.checked[k] == 0 {
                continue;
            }
            eprintln!(
                "pair-race {tag} [{}]: checked={} share_co={} (latch_quiet={}, ext_free={}) bad_diverged={} bad_one_refuted={} inc_demand_eq={} inc_demand_diff={} inc_mismatch={} inc_horizon={} inc_budget={} | leaves_co={} (avg {:.1}cy) forks={} absorb={} steps_avg={:.0}",
                PAIR_KIND[k],
                self.checked[k],
                self.share_co[k],
                self.co_latch_quiet[k],
                self.co_ext_free[k],
                self.bad_diverged[k],
                self.bad_one_refuted[k],
                self.inc_demand_eq[k],
                self.inc_demand_diff[k],
                self.inc_mismatch[k],
                self.inc_horizon[k],
                self.inc_budget[k],
                self.leaves_co[k],
                avg(self.cyc_leaf[k], self.leaves_co[k]),
                self.joint_forks[k],
                self.absorb[k],
                avg(self.steps_sum[k], self.checked[k]),
            );
        }
        eprintln!(
            "pair-race {tag}: unbound_skip={} patch_skip={}",
            self.unbound_skip, self.patch_skip
        );
    }

    /// Find a single-delay-conflict record for this prober and race the
    /// two spellings. `proj` is borrowed scratch.
    #[allow(clippy::too_many_arguments)]
    fn check(
        &mut self,
        spec: &EngineSpec,
        irq_at: &FxMap<u32, u8>,
        entry: &MemoEntry,
        it: &Item,
        delay_mask: u16,
        mirror_blocked: bool,
        proj: &mut Vec<u32>,
    ) {
        if delay_mask == 0 {
            return;
        }
        // A record ALL of whose conds the prober satisfies except
        // exactly one pure value conflict in a searched slot — confined
        // to the slot's delay bits unless `wide`.
        let mut found: Option<(usize, u16, u16)> = None;
        'sets: for &(mask, ref table) in &entry.sets {
            if mirror_blocked && mask & (SC_X | SC_Y) != 0 {
                continue;
            }
            proj.clear();
            project_state(&it.st, mask, proj);
            if let Some(list) = table.get(proj.as_slice()) {
                'recs: for i in 0..list.recs.len() {
                    let mut cand: Option<(usize, u16, u16)> = None;
                    for &p in list.conds_of(i) {
                        let (s, m, v) = ((p >> 32) as usize, (p >> 16) as u16, p as u16);
                        if it.decided[s] & m != m {
                            continue 'recs; // undecided demand, not a pure pair
                        }
                        let conflict = (it.value[s] & m) ^ v;
                        if conflict == 0 {
                            continue;
                        }
                        if cand.is_some()
                            || (!self.wide && conflict & !delay_mask != 0)
                            || s >= spec.slots as usize
                        {
                            continue 'recs;
                        }
                        cand = Some((s, m, v));
                    }
                    if let Some((s, m, v)) = cand {
                        // The patched spelling must satisfy every cond
                        // of this record — a sibling cond on the same
                        // slot with an overlapping mask can be violated
                        // by the patch.
                        let patched = (it.value[s] & !m) | v;
                        let ok = list.conds_of(i).iter().all(|&p| {
                            let (s2, m2, v2) = ((p >> 32) as usize, (p >> 16) as u16, p as u16);
                            s2 != s || patched & m2 == v2
                        });
                        if !ok {
                            self.patch_skip += 1;
                            continue 'recs;
                        }
                        found = Some((s, m, v));
                        break 'sets;
                    }
                }
            }
        }
        let Some((slot, cmask, cval)) = found else { return };
        // An unbound prober stands for a program and its mirror from
        // the same state — the mirror's race is the mirrored question
        // with the same verdict, so racing the spelling as-is is a
        // valid sample. Counted for the record (first 2..2 run: nearly
        // ALL delay-conflict probers are unbound).
        if it.unbound {
            self.unbound_skip += 1;
        }

        let mut cfg_a = spec.cfg.clone();
        cfg_a.code = it.value; // prober spelling
        let mut cfg_b = spec.cfg.clone();
        cfg_b.code = it.value;
        cfg_b.code[slot] = (cfg_b.code[slot] & !cmask) | cval; // record spelling
        let delta = cfg_a.code[slot] ^ cfg_b.code[slot];
        let kind = if delta & !delay_mask == 0 {
            0 // delay
        } else if delta >> 13 != 0 {
            2 // opcode
        } else {
            1 // arg
        };
        self.checked[kind] += 1;
        let mut decided = it.decided;
        let mut ctx = RaceCtx {
            spec,
            irq_at,
            delay_mask,
            preload: spec.inputs.len() <= 4,
            recurse: self.recurse,
            root_cyc: it.cycle,
            steps: 0,
            leaves_co: 0,
            cyc_leaf: 0,
            joint_forks: 0,
            absorb: 0,
            wrote_latch: false,
            ext_consult: false,
            bad_cause: "",
            inc_cause: "",
        };
        let rv = pair_race_walk(
            &mut ctx,
            &mut cfg_a,
            &mut cfg_b,
            &mut decided,
            it.st,
            it.st,
            it.next_input,
            it.next_input,
            it.cycle,
            0,
        );
        match rv {
            RV_CO => {
                self.share_co[kind] += 1;
                if !ctx.wrote_latch {
                    self.co_latch_quiet[kind] += 1;
                }
                if !ctx.ext_consult {
                    self.co_ext_free[kind] += 1;
                }
            }
            RV_BAD => match ctx.bad_cause {
                "one_refuted" => self.bad_one_refuted[kind] += 1,
                _ => self.bad_diverged[kind] += 1,
            },
            _ => match ctx.inc_cause {
                "demand_eq" => self.inc_demand_eq[kind] += 1,
                "demand_diff" => self.inc_demand_diff[kind] += 1,
                "mismatch" => self.inc_mismatch[kind] += 1,
                "horizon" => self.inc_horizon[kind] += 1,
                _ => self.inc_budget[kind] += 1,
            },
        }
        self.leaves_co[kind] += ctx.leaves_co;
        self.cyc_leaf[kind] += ctx.cyc_leaf;
        self.joint_forks[kind] += ctx.joint_forks;
        self.absorb[kind] += ctx.absorb;
        self.steps_sum[kind] += ctx.steps as u64;
        if self.checked.iter().sum::<u64>() % 4096 == 0 {
            self.print("tally");
        }
    }
}

/// Verdict lattice for the recursive pair race — aggregation over
/// joint-fork children is `min()`.
const RV_BAD: u8 = 0; // some leaf's outcomes genuinely differ
const RV_INC: u8 = 1; // truth unknown (budget / divergent-state edge)
const RV_CO: u8 = 2; // every leaf co-refuted: sharing was valid

/// Total simulated cycles across the whole race tree, and joint-fork
/// nesting depth. A capped race is INC, never a wrong verdict.
const RACE_STEP_CAP: u32 = 16384;
const RACE_DEPTH_CAP: u32 = 10;

/// Borrowed context + per-race facts for `pair_race_walk`; the caller
/// folds the facts into the per-kind tallies.
struct RaceCtx<'a> {
    spec: &'a EngineSpec,
    irq_at: &'a FxMap<u32, u8>,
    delay_mask: u16,
    preload: bool,
    recurse: bool,
    root_cyc: u32,
    steps: u32,
    leaves_co: u64,
    cyc_leaf: u64,
    joint_forks: u64,
    absorb: u64,
    wrote_latch: bool,
    ext_consult: bool,
    bad_cause: &'static str,
    inc_cause: &'static str,
}

impl RaceCtx<'_> {
    fn bad(&mut self, cause: &'static str) -> u8 {
        if self.bad_cause.is_empty() {
            self.bad_cause = cause;
        }
        RV_BAD
    }
    fn inc(&mut self, cause: &'static str) -> u8 {
        if self.inc_cause.is_empty() {
            self.inc_cause = cause;
        }
        RV_INC
    }
}

/// The fork the engine would take at a completing fetch, in engine
/// order: fetch-time field demand, deferred taken-JMP target, delay
/// post-fork. (The engine forks delay at completion, not fetch — the
/// pre-fetch patch is equivalent because the word carries its delay
/// and `step` consults it at completion; it only duplicates leaves
/// when the instruction refutes before completing, costing budget.)
enum RaceEdge {
    None,
    Demand(Field),
    Target,
    Delay,
}

fn race_edge(cfg: &NCfg, st: &NState, fetching: bool, g: u32, decided: &[u16; 32], delay_mask: u16) -> RaceEdge {
    if !fetching {
        return RaceEdge::None;
    }
    let pc = st.pc as usize;
    let w = cfg.code[pc];
    if let Some(f) = demand(decided[pc], w, cfg) {
        return RaceEdge::Demand(f);
    }
    if w >> 13 == 0 && decided[pc] & 0x001F != 0x001F && jmp_taken(w, st, cfg, g) {
        return RaceEdge::Target;
    }
    if decided[pc] & delay_mask != delay_mask {
        return RaceEdge::Delay;
    }
    RaceEdge::None
}

/// Lock-step race of two one-slot-conflicting spellings from a shared
/// core state, recursing through equal-state joint demand edges (both
/// machines fork the SAME value — sound because the conflict bits are
/// all decided, so the words agree on every undecided bit). Value
/// enumeration is `values_into` for demanded fields and raw target /
/// delay ranges for the deferred forks; engine canon prunes (P1/P2/P3/
/// P4) are NOT applied, so the race explores a superset of the
/// engine's subtree — spurious BAD on a pruned respelling undercounts
/// shareability, never overcounts.
#[allow(clippy::too_many_arguments)]
fn pair_race_walk(
    ctx: &mut RaceCtx,
    cfg_a: &mut NCfg,
    cfg_b: &mut NCfg,
    decided: &mut [u16; 32],
    mut sa: NState,
    mut sb: NState,
    mut na: u32,
    mut nb: u32,
    mut cyc: u32,
    depth: u32,
) -> u8 {
    let mut differed = sa != sb || na != nb;
    loop {
        if ctx.steps >= RACE_STEP_CAP {
            return ctx.inc("budget");
        }
        ctx.steps += 1;
        if cyc >= ctx.spec.cycles {
            return ctx.inc("horizon");
        }
        if let Some(&m) = ctx.irq_at.get(&cyc) {
            sa.irq_flags |= m;
            sb.irq_flags |= m;
        }
        if !ctx.preload {
            while (na as usize) < ctx.spec.inputs.len() && !sa.tx.is_full() {
                sa.tx.push(ctx.spec.inputs[na as usize]);
                na += 1;
            }
            while (nb as usize) < ctx.spec.inputs.len() && !sb.tx.is_full() {
                sb.tx.push(ctx.spec.inputs[nb as usize]);
                nb += 1;
            }
        }
        let ext = stim_at(&ctx.spec.stim, cyc);
        let ga = compose(&sa, ctx.spec.stim.mask, ext);
        let gb = compose(&sb, ctx.spec.stim.mask, ext);
        let fa = peek_tick(&sa, cfg_a) && will_fetch(&sa, cfg_a, ga);
        let fb = peek_tick(&sb, cfg_b) && will_fetch(&sb, cfg_b, gb);
        // Out-of-footprint fetch refutes (OOB rule).
        let oa = fa && sa.pc as usize >= ctx.spec.slots as usize;
        let ob = fb && sb.pc as usize >= ctx.spec.slots as usize;
        if oa || ob {
            if oa && ob {
                ctx.leaves_co += 1;
                ctx.cyc_leaf += (cyc - ctx.root_cyc) as u64;
                return RV_CO;
            }
            return ctx.bad("one_refuted");
        }
        let ea = race_edge(cfg_a, &sa, fa, ga, decided, ctx.delay_mask);
        let eb = race_edge(cfg_b, &sb, fb, gb, decided, ctx.delay_mask);
        if !matches!((&ea, &eb), (RaceEdge::None, RaceEdge::None)) {
            if !ctx.recurse {
                return ctx.inc(if differed { "demand_diff" } else { "demand_eq" });
            }
            if differed {
                return ctx.inc("demand_diff");
            }
            if depth >= RACE_DEPTH_CAP {
                return ctx.inc("budget");
            }
            // Equal states ⇒ equal pc; the two edges can still differ
            // when the conflict slot itself is fetching (different
            // words demand different fields) — no joint fork exists.
            let pc = sa.pc as usize;
            let (wa0, wb0, d0) = (cfg_a.code[pc], cfg_b.code[pc], decided[pc]);
            let mut vals: Vec<u16> = Vec::new();
            let fork_mask = match (ea, eb) {
                (RaceEdge::Demand(a), RaceEdge::Demand(b)) if a.mask == b.mask && a.kind == b.kind => {
                    a.values_into(cfg_a, &mut vals);
                    a.mask
                }
                (RaceEdge::Target, RaceEdge::Target) => {
                    vals.extend(0..ctx.spec.slots as u16);
                    0x001F
                }
                (RaceEdge::Delay, RaceEdge::Delay) => {
                    let max = ctx.delay_mask >> 8;
                    vals.extend((0..=max).map(|v| v << 8));
                    ctx.delay_mask
                }
                _ => return ctx.inc("mismatch"),
            };
            ctx.joint_forks += 1;
            decided[pc] = d0 | fork_mask;
            let mut agg = RV_CO;
            for v in vals {
                cfg_a.code[pc] = (wa0 & !fork_mask) | v;
                cfg_b.code[pc] = (wb0 & !fork_mask) | v;
                let rv = pair_race_walk(ctx, cfg_a, cfg_b, decided, sa, sb, na, nb, cyc, depth + 1);
                agg = agg.min(rv);
                if agg == RV_BAD {
                    break;
                }
            }
            cfg_a.code[pc] = wa0;
            cfg_b.code[pc] = wb0;
            decided[pc] = d0;
            return agg;
        }
        // External-schedule consults (WAIT any src, JMP PIN) by
        // whichever word can execute this cycle.
        for (st, cfg) in [(&sa, &*cfg_a), (&sb, &*cfg_b)] {
            if st.delay_count == 0 && peek_tick(st, cfg) {
                let w = st.pending_exec.unwrap_or(cfg.code[st.pc as usize & 31]);
                if w >> 13 == 1 || (w >> 13 == 0 && (w >> 5) & 7 == 6) {
                    ctx.ext_consult = true;
                }
            }
        }
        let la = (sa.out_latch, sa.dir_latch, sb.out_latch, sb.dir_latch);
        if clock_tick(&mut sa, cfg_a) {
            super::step(&mut sa, cfg_a, ga);
        }
        if clock_tick(&mut sb, cfg_b) {
            super::step(&mut sb, cfg_b, gb);
        }
        if (sa.out_latch, sa.dir_latch, sb.out_latch, sb.dir_latch) != la {
            ctx.wrote_latch = true;
        }
        let ca = capture_word(&sa, &ctx.spec.capture_pins, ctx.spec.stim.mask, ext);
        let cb = capture_word(&sb, &ctx.spec.capture_pins, ctx.spec.stim.mask, ext);
        if ca != cb {
            return ctx.bad("diverged");
        }
        let refuted = ca != ctx.spec.expected[cyc as usize];
        cyc += 1;
        if refuted {
            ctx.leaves_co += 1;
            ctx.cyc_leaf += (cyc - ctx.root_cyc) as u64;
            return RV_CO;
        }
        if sa != sb || na != nb {
            differed = true;
        } else if differed {
            ctx.absorb += 1;
            differed = false;
        }
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
        memo: &FxMap<KeyCore, MemoEntry>,
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
            "{{\"snapshot\":{{\"reason\":\"{reason}\",\"min_benefit\":{min_benefit},\"items\":{},\"memo_entries\":{},\"memo_hits\":{},\"core_matches\":{},\"state_misses\":{},\"cond_misses\":{},\"rec_scans\":{},\"recs_scanned\":{},\"max_recs\":{},\"purges\":{},\"frames\":[{}]}}}}",
            stats.items,
            stats.memo_entries,
            stats.memo_hits,
            stats.memo_core_matches,
            stats.memo_state_misses,
            stats.memo_cond_misses,
            stats.memo_rec_scans,
            stats.memo_recs_scanned,
            stats.memo_max_recs,
            stats.memo_purges,
            frames_s.join(",")
        );
        for (core, entry) in memo {
            for &(mask, ref table) in &entry.sets {
                for (vals, list) in table {
                    for i in 0..list.recs.len() {
                        let benefit = list.recs[i].1;
                        let conds_s: Vec<String> = list
                            .conds_of(i)
                            .iter()
                            .map(|&p| {
                                format!("[{},{},{}]", p >> 32, (p >> 16) as u16, p as u16)
                            })
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
            // JMP consults its target only when TAKEN — the deferred
            // target fork in the cycle loop owns those bits' seg_mask.
            0 => 0x00E0,
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

/// Does this word's execution read anything external-schedule-driven —
/// stim pins or the cycle-scheduled IRQ environment? Such reads break
/// the time-shift-invariance argument (their outcome depends on WHEN
/// the instruction runs, not just machine state).
fn reads_external(w: u16) -> bool {
    match w >> 13 {
        0 => (w >> 5) & 7 == 6, // JMP PIN
        1 => true,              // WAIT gpio/pin/irq/jmppin
        2 => (w >> 5) & 7 == 0, // IN PINS
        5 => w & 7 == 0,        // MOV src PINS
        6 => true,              // IRQ (flags are env-scheduled)
        _ => false,
    }
}

/// Cycle cap for the junk-window collapse walk (measured co-refutation
/// distance on tx_a L=3: avg ~60 cycles).
const JUNK_WALK_CAP: u32 = 192;

/// 008 stage 2 — time-shift-invariant refutation lookahead
/// ("junk-window collapse"), run at a delay post-fork BEFORE paying
/// the fork. Walks the representative spelling (every undecided delay
/// reads as 0 straight from the code word) up to `cap` cycles.
/// Returns true — refuting the ENTIRE delay-spelling family outright —
/// iff the walk hits a capture/expected mismatch or an out-of-
/// footprint fetch through a CLEAN window: no pin-latch value changes,
/// no external-schedule reads (`reads_external`), no fetch demand, no
/// deferred-target or binding fork edge, no horizon/cap exit.
///
/// Soundness (mined on 2..2: 96% of delay-conflict pairs co-refute,
/// 100% of those latch-quiet): in a clean window every spelling of
/// every undecided delay executes the SAME instruction sequence, only
/// time-shifted; with latches never changing value, the capture is one
/// static word for all spellings, so the first expected-trace mismatch
/// falls on the same absolute cycle for all of them — and a shifted
/// spelling meets its own fork edges only at/after the representative
/// refutation. An OOB fetch refutes shift-invariantly (same pc
/// trajectory). On success the walk's consulted fields and state reads
/// (accumulated exactly like the main loop's segment accounting, with
/// undecided delay bits excluded BY the theorem) merge into the
/// caller's segment; on an unclean exit nothing is merged and the
/// caller forks normally.
#[allow(clippy::too_many_arguments)]
fn junk_walk(
    spec: &EngineSpec,
    cfg: &NCfg,
    irq_at: &FxMap<u32, u8>,
    start: &Item,
    delay_mask: u16,
    memo_on: bool,
    seg_mask: &mut [u16; 32],
    seg_reads: &mut u16,
    mut x_prov: Prov,
    mut y_prov: Prov,
    wcyc: &mut u64,
) -> bool {
    let mut it = *start;
    debug_assert_eq!(it.st.delay_count, 0, "walk starts at a completion");
    let mut w_mask = [0u16; 32];
    let mut w_reads = 0u16;
    let preload = spec.inputs.len() <= 4;
    let end = it.cycle.saturating_add(JUNK_WALK_CAP).min(spec.cycles);
    // Undecided-delay completions crossed so far (the fork's own slot
    // counts: its children shift by up to max_delay each). A capture
    // mismatch refutes at the same ABSOLUTE cycle for every shifted
    // spelling (static latches, absolute stim), but an OOB fetch is
    // execution-POSITION-based: a spelling shifted past the horizon
    // never leaves the footprint and is a valid program — so the OOB
    // break is only family-valid when even the maximally shifted
    // spelling still fetches OOB in-horizon.
    let max_delay = (delay_mask >> 8) as u32;
    let mut shift_points = 1u32;
    let refuted = loop {
        if it.cycle >= end {
            return false; // horizon or cap: never conclude from a walk
        }
        *wcyc += 1;
        if let Some(&m) = irq_at.get(&it.cycle) {
            it.st.irq_flags |= m;
        }
        if !preload {
            while (it.next_input as usize) < spec.inputs.len() && !it.st.tx.is_full() {
                it.st.tx.push(spec.inputs[it.next_input as usize]);
                it.next_input += 1;
            }
        }
        let ext = stim_at(&spec.stim, it.cycle);
        let gpio_in = compose(&it.st, spec.stim.mask, ext);
        if memo_on && it.st.stall != Stall::None {
            w_reads |= stall_state_reads(it.st.stall);
        }
        let fetching = peek_tick(&it.st, cfg) && will_fetch(&it.st, cfg, gpio_in);
        let fetch_pc = it.st.pc as usize;
        if fetching && fetch_pc >= spec.slots as usize {
            if it.cycle + shift_points * max_delay < spec.cycles {
                break true; // every shifted spelling also fetches OOB
            }
            return false; // a shifted spelling may outlive the horizon
        }
        if fetching {
            if demand(it.decided[fetch_pc], it.value[fetch_pc], cfg).is_some() {
                return false; // fetch-demand fork edge
            }
            if memo_on {
                w_mask[fetch_pc] |=
                    fetch_footprint(it.decided[fetch_pc], it.value[fetch_pc], cfg)
                        & it.decided[fetch_pc];
            }
            let w = it.value[fetch_pc];
            if w >> 13 == 0 && jmp_taken(w, &it.st, cfg, gpio_in) {
                if it.decided[fetch_pc] & 0x001F != 0x001F {
                    return false; // deferred-target fork edge
                }
                if memo_on {
                    w_mask[fetch_pc] |= 0x001F;
                }
            }
        }
        let exec_word = it.st.pending_exec.unwrap_or(it.value[fetch_pc & 31]);
        let exec_pending = it.st.pending_exec.is_some();
        if it.st.delay_count == 0 && peek_tick(&it.st, cfg) {
            if reads_external(exec_word) {
                return false; // time-dependent read
            }
            if memo_on {
                let r = word_state_reads(exec_word, cfg);
                consume_reads(r, x_prov, y_prov, &mut w_reads, &mut w_mask);
            }
        }
        if it.unbound && peek_tick(&it.st, cfg) {
            let demands_binding = if !will_execute(&it.st, cfg, gpio_in) {
                false
            } else if let Some(w) = it.st.pending_exec {
                word_touches_regs(w)
            } else {
                it.st.x != it.st.y && is_pull_empty_read(it.value[fetch_pc & 31], &it.st, cfg)
            };
            if demands_binding {
                return false; // binding fork edge
            }
        }
        let pre_delay = it.st.delay_count;
        let ticked = clock_tick(&mut it.st, cfg);
        let latches = (it.st.out_latch, it.st.dir_latch);
        if ticked {
            super::step(&mut it.st, cfg, gpio_in);
        }
        if (it.st.out_latch, it.st.dir_latch) != latches {
            return false; // latch value change: window unclean
        }
        let w = capture_word(&it.st, &spec.capture_pins, spec.stim.mask, ext);
        if w != spec.expected[it.cycle as usize] {
            break true; // co-refutation of the whole family
        }
        it.cycle += 1;
        let completed = ticked && pre_delay == 0 && it.st.stall == Stall::None;
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
        // A completed delay — DECIDED or not — contributes NO cond:
        // the time-shift theorem makes the verdict independent of its
        // value either way, so walk records are delay-AGNOSTIC (a
        // reconvergent spelling with a different decided delay
        // memo-hits instead of re-walking; measured pre-change: walks
        // were 51% of engine cycles and decided-delay values were 50%
        // of residual cond conflicts). Every crossing IS a shift point
        // for the OOB horizon bound, since a prober's value may exceed
        // the walked one by up to max_delay.
        if fetching && it.st.stall == Stall::None && delay_mask != 0 {
            shift_points += 1;
        }
    };
    if refuted && memo_on {
        *seg_reads |= w_reads;
        for s in 0..32 {
            seg_mask[s] |= w_mask[s];
        }
    }
    refuted
}

/// Exhaustive needed-narrowing search. Deterministic: DFS order is fixed
/// by field order and value enumeration order (memo hits only skip
/// subtrees that would have been fully refuted, so champions and
/// verdicts are unchanged).
pub fn search(spec: &EngineSpec, champion_cap: usize) -> SearchResult {
    search_impl(spec, champion_cap, true, None, None)
}

/// `instrument: false` silences the heartbeat and disables the
/// env-driven dump/probe-log/snapshot channels — the split driver's
/// phase-1 and worker searches must not each open the dump files or
/// spam stderr; the coordinator reports aggregate progress instead.
fn search_impl(
    spec: &EngineSpec,
    champion_cap: usize,
    instrument: bool,
    progress: Option<&std::sync::atomic::AtomicU64>,
    quotient: Option<&mut WordQuotient>,
) -> SearchResult {
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
    let mut irq_at: FxMap<u32, u8> = FxMap::default();
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
    let mut sib_ids: Vec<u32> = Vec::with_capacity(32);
    // The quotient depends only on the config, so a split worker builds
    // it once and lends it to every unit it runs (1.7M-unit frontiers
    // made per-search builds a real fixed cost).
    let mut wq_local: WordQuotient;
    let wq: &mut WordQuotient = match quotient {
        Some(w) => w,
        None => {
            wq_local = WordQuotient::build(&cfg);
            &mut wq_local
        }
    };
    let mut last_beat = std::time::Instant::now();

    // Consulted-set memo (failure-only): frames mirror the DFS stack —
    // LIFO makes every subtree contiguous — with a synthetic,
    // never-recorded root frame absorbing top-level merges.
    let memo_on = spec.memo_cap > 0;
    let mut dump = if memo_on && instrument { dump_open() } else { None };
    let mut probe_log = if memo_on && instrument { probe_log_open(spec.cycles) } else { None };
    let mut snap = if memo_on && instrument { snapshot_open() } else { None };
    let mut pair_race = if memo_on && instrument {
        let wide = std::env::var("PIO_NARROW_WORD_PAIR").is_ok();
        let recurse = std::env::var("PIO_NARROW_PAIR_RECURSE").is_ok();
        if wide || recurse || std::env::var("PIO_NARROW_DELAY_PAIR").is_ok() {
            Some(PairRace { wide: wide || recurse, recurse, ..Default::default() })
        } else {
            None
        }
    } else {
        None
    };
    let mut memo: FxMap<KeyCore, MemoEntry> = FxMap::default();
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
        // Cheap live-progress export for the split coordinator (a
        // masked test per item; the counter is monotonic and shared by
        // every unit this worker runs).
        if stats.items & 0xF_FFFF == 0 {
            if let Some(p) = progress {
                p.fetch_add(1 << 20, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if instrument && last_beat.elapsed().as_secs() >= 10 {
            eprintln!(
                "narrow-search: items={} forks={} refuted={} prefilt={} canon={} quo={} look={} walks={} wcyc={} cyc={} memo_hit={} memo_ent={} minben={} purges={} recs_avg={:.1} recs_max={} champions={} stack={}",
                stats.items,
                stats.forks,
                stats.refuted,
                stats.prefiltered,
                stats.canon_pruned,
                stats.quotient_pruned,
                stats.look_refuted,
                stats.walk_calls,
                stats.walk_cycles,
                stats.cycles_run,
                stats.memo_hits,
                stats.memo_entries,
                min_benefit,
                stats.memo_purges,
                stats.memo_recs_scanned as f64 / stats.memo_rec_scans.max(1) as f64,
                stats.memo_max_recs,
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
                    if let Some(list) = table.get(&proj) {
                        outcome = PROBE_COND_MISS;
                        stats.memo_rec_scans += 1;
                        stats.memo_recs_scanned += list.recs.len() as u64;
                        stats.memo_max_recs = stats.memo_max_recs.max(list.recs.len() as u64);
                        for i in 0..list.recs.len() {
                            let conds = list.conds_of(i);
                            if conds.iter().all(|&p| {
                                let (s, m, v) =
                                    ((p >> 32) as usize, (p >> 16) as u16, p as u16);
                                it.decided[s] & m == m && it.value[s] & m == v
                            }) {
                                hit = Some((
                                    conds
                                        .iter()
                                        .map(|&p| ((p >> 32) as u8, (p >> 16) as u16, p as u16))
                                        .collect(),
                                    mask,
                                ));
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
                        // Rides the probe log's miss sampling (needs
                        // both env vars set).
                        if outcome == PROBE_COND_MISS {
                            if let Some(dp) = pair_race.as_mut() {
                                dp.check(spec, &irq_at, entry, &it, delay_mask, mirror_blocked, &mut proj);
                            }
                        }
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
                close_child(&mut frames, &mut memo, spec.memo_cap, &mut min_benefit, &mut snap, &mut stats, false, spec.slots);
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
            // OOB refutation: fetching a slot at/past the searched
            // length is undefined behavior on hardware — the rest of
            // the 32-word ROM belongs to other programs, so a program
            // that guarantees behavior out there is really a 32-word
            // program. Such candidates are OUTSIDE the space (space =
            // programs whose execution stays within their L words for
            // the whole horizon). Reachable via fall-through past the
            // last slot or a computed MOV/OUT PC; JMP targets are
            // already generation-restricted. Sound for the memo: the
            // verdict depends only on pc (core key) and spec.slots (a
            // search constant).
            if fetching && fetch_pc >= spec.slots as usize {
                stats.refuted += 1;
                stats.oob_refuted += 1;
                if memo_on {
                    merge_segment(frames.last_mut().expect("root frame"), &seg_mask, seg_reads, &it.value);
                    close_child(&mut frames, &mut memo, spec.memo_cap, &mut min_benefit, &mut snap, &mut stats, false, spec.slots);
                }
                continue 'items;
            }
            if fetching {
                if memo_on {
                    seg_mask[fetch_pc] |=
                        fetch_footprint(it.decided[fetch_pc], it.value[fetch_pc], &cfg)
                            & it.decided[fetch_pc];
                }
                if let Some(field) = demand(it.decided[fetch_pc], it.value[fetch_pc], &cfg) {
                    values.clear();
                    field.values_into(&cfg, &mut values);
                    sib_ids.clear();
                    let qtab = wq.table(it.decided[fetch_pc] | field.mask);
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
                        // P2 canonical nop: the X/Y self-moves with op
                        // none are pure no-ops; the kept spelling is
                        // `mov x,x` (NOP_CANON), so only y,y is pruned.
                        // ISR/OSR self-moves reset the target's shift
                        // counter (datasheet 11.4.10) — real ops, kept.
                        if field.kind == FieldKind::MovSrc {
                            let dst = (it.value[fetch_pc] >> 5) & 0x7;
                            let mov_op = (it.value[fetch_pc] >> 3) & 0x3;
                            if mov_op == 0 && raw == dst && dst == 2 {
                                stats.canon_pruned += 1;
                                continue;
                            }
                        }
                        // 009 word-quotient sibling dedup: skip a
                        // candidate whose partial word (this slot, child
                        // mask) is lemma-provably interchangeable with an
                        // already-generated sibling of this same fork —
                        // that sibling's subtree IS this subtree. A
                        // sibling later killed by the pin-write
                        // pre-filter kills this value with it (class
                        // members share every capture).
                        let qid = qtab[&(it.value[fetch_pc] | v)];
                        if sib_ids.contains(&qid) {
                            stats.quotient_pruned += 1;
                            continue;
                        }
                        sib_ids.push(qid);
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
                            close_child(&mut frames, &mut memo, spec.memo_cap, &mut min_benefit, &mut snap, &mut stats, false, spec.slots);
                        }
                    }
                    continue 'items;
                }
                // 008 deferred target demand: a JMP consults its target
                // only on a TAKEN execution (exec_op writes pc from it
                // iff `take`). Fork the target here, at consult time —
                // an untaken execution leaves it undecided, and a JMP
                // that never takes keeps a plain don't-care target and
                // memo records free of target conds. Targets outside
                // the searched length would execute foreign ROM — not
                // part of this space (see the OOB refutation above).
                let w = it.value[fetch_pc];
                if w >> 13 == 0 && jmp_taken(w, &it.st, &cfg, gpio_in) {
                    if it.decided[fetch_pc] & 0x001F == 0x001F {
                        if memo_on {
                            // Taken with a decided target: consulted.
                            seg_mask[fetch_pc] |= 0x001F;
                        }
                    } else {
                        let cond = (w >> 5) & 0x7;
                        let ft = if fetch_pc as u8 == cfg.wrap_top {
                            cfg.wrap_bottom
                        } else {
                            (fetch_pc as u8 + 1) & 0x1F
                        };
                        let mut pushed = 0u32;
                        for t in 0..spec.slots as u16 {
                            // P4 vacuous control (relocated from the
                            // fetch fork): a JMP to its own fallthrough
                            // with a non-writing condition is a no-op
                            // respelling; the canonical spelling lives
                            // in the MOV sibling of the opcode fork.
                            if cond != 2 && cond != 4 && t == ft as u16 {
                                stats.canon_pruned += 1;
                                continue;
                            }
                            let mut child = it;
                            child.decided[fetch_pc] |= 0x001F;
                            child.value[fetch_pc] |= t;
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
                                // Every target filtered (single-slot
                                // jmp-to-self): leaf, covered by the
                                // canonical nop sibling.
                                close_child(&mut frames, &mut memo, spec.memo_cap, &mut min_benefit, &mut snap, &mut stats, false, spec.slots);
                            }
                        }
                        continue 'items;
                    }
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
                        && is_pull_empty_read(cfg.code[it.st.pc as usize], &it.st, &cfg)
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
                    close_child(&mut frames, &mut memo, spec.memo_cap, &mut min_benefit, &mut snap, &mut stats, false, spec.slots);
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
                    // 008 stage 2 — junk-window collapse: one
                    // representative walk refutes the whole
                    // delay-spelling family when its window is clean
                    // (see junk_walk). Fires BEFORE the fork so the
                    // 8^k spelling subtree never exists.
                    stats.walk_calls += 1;
                    if junk_walk(
                        spec, &cfg, &irq_at, &it, delay_mask, memo_on,
                        &mut seg_mask, &mut seg_reads, x_prov, y_prov,
                        &mut stats.walk_cycles,
                    ) {
                        stats.refuted += 1;
                        stats.look_refuted += 1;
                        if memo_on {
                            merge_segment(frames.last_mut().expect("root frame"), &seg_mask, seg_reads, &it.value);
                            close_child(&mut frames, &mut memo, spec.memo_cap, &mut min_benefit, &mut snap, &mut stats, false, spec.slots);
                        }
                        continue 'items;
                    }
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
            close_child(&mut frames, &mut memo, spec.memo_cap, &mut min_benefit, &mut snap, &mut stats, true, spec.slots);
        }
    }
    debug_assert!(frames.len() == 1, "unbalanced frame accounting");
    if let Some(dp) = pair_race.as_ref() {
        dp.print("final");
    }
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

/// Fold one worker's stats into the total: counters sum, the
/// histogram sums elementwise, the max-tracker takes the max.
fn merge_stats(into: &mut Stats, s: &Stats) {
    into.items += s.items;
    into.forks += s.forks;
    into.refuted += s.refuted;
    into.champions_found += s.champions_found;
    into.cycles_run += s.cycles_run;
    into.prefiltered += s.prefiltered;
    into.canon_pruned += s.canon_pruned;
    into.quotient_pruned += s.quotient_pruned;
    into.oob_refuted += s.oob_refuted;
    into.look_refuted += s.look_refuted;
    into.walk_calls += s.walk_calls;
    into.walk_cycles += s.walk_cycles;
    into.memo_hits += s.memo_hits;
    into.memo_core_matches += s.memo_core_matches;
    into.memo_state_misses += s.memo_state_misses;
    into.memo_cond_misses += s.memo_cond_misses;
    into.memo_rec_scans += s.memo_rec_scans;
    into.memo_recs_scanned += s.memo_recs_scanned;
    into.memo_max_recs = into.memo_max_recs.max(s.memo_max_recs);
    into.memo_entries += s.memo_entries;
    into.memo_purges += s.memo_purges;
    for i in 0..32 {
        into.benefit_hist[i] += s.benefit_hist[i];
    }
}

/// Parallel exhaustive search: same refutation verdicts as [`search`],
/// wall-clock divided across `threads` workers.
///
/// Phase 1 runs the sequential engine on a TRUNCATED spec (a prefix of
/// the expected trace); its champions-with-don't-cares are exactly the
/// surviving frontier subspaces — the work units. The truncation cycle
/// doubles until the unit count comfortably exceeds the thread count
/// (the playground's measured straggler lesson: a too-shallow split
/// collapses parallelism onto a few heavy groups for hours). Phase 2
/// workers pull units off a shared counter and run the ordinary SEEDED
/// search over the full spec — re-deriving the cheap shared prefix
/// inside the unit, with a per-unit memo — so the result is
/// deterministic regardless of scheduling.
///
/// Champion-list semantics vs [`search`]: the covered set of
/// trace-reproducing programs is identical, but the REPRESENTATION may
/// differ — binding-free units that name registers are expanded into
/// explicit identity + register-mirror units (a seeded worker cannot
/// carry the unbound twin), and P3's fresh-fork delay canonicalization
/// does not see across the seed boundary, so workers may emit delay
/// spellings the sequential walk canonicalized away. Zero-champion
/// (impossibility) verdicts are exactly equivalent — that is this
/// driver's job.
pub fn search_split(spec: &EngineSpec, champion_cap: usize, threads: usize) -> SearchResult {
    let threads = threads.max(1);
    // 128x threads, not 16x: a shallow frontier makes units that are
    // individually near-unconstrained, and on hard brackets EVERY unit
    // is then hours deep (L=3 0..2 sat at 0/1520 settled for 16min+ at
    // 25 busy cores). More, deeper-truncated units cost phase-1 a few
    // extra cheap passes and bound both the head and the straggler
    // tail; the 4M-cap fallback still guards the frontier explosion.
    let target = (threads * 128).max(512);

    // Phase 1: widen the frontier until it feeds every thread. The
    // frontier can leap orders of magnitude between adjacent cycles
    // (one fetch fans every operand field), so grow the truncation
    // cycle gently and keep the last under-target frontier as a
    // fallback if a pass overflows the unit cap.
    let mut c = 1u32;
    let mut prev: Option<(Vec<Champion>, Stats)> = None;
    let (units, phase1) = loop {
        if c >= spec.cycles {
            // Never got wide enough — the sequential search IS the
            // cheapest correct answer.
            return search_impl(spec, champion_cap, true, None, None);
        }
        let mut spec_t = spec.clone();
        spec_t.cycles = c;
        spec_t.expected.truncate(c as usize);
        let r = search_impl(&spec_t, 1 << 22, false, None, None);
        if r.champion_cap_hit {
            // A truncated frontier is an incomplete cover — unusable.
            // Parallelize on the widest sound frontier we saw instead.
            let p = prev.expect("phase-1 frontier exceeded 4M units at its first cycle");
            break p;
        }
        if r.champions.is_empty() {
            // Refuted on the prefix: the full space is empty.
            return SearchResult { champions: Vec::new(), stats: r.stats, champion_cap_hit: false };
        }
        if r.champions.len() >= target {
            break (r.champions, r.stats);
        }
        prev = Some((r.champions, r.stats));
        c += (c / 2).max(1);
    };
    // Phase-1 stats carry the frontier walk's work counters (last
    // widening pass only); its "champions" are work units, not
    // solutions.
    let mut phase1 = phase1;
    phase1.champions_found = 0;

    // Units -> seeds; binding-free register-naming units get an
    // explicit mirror twin.
    let seed_of = |u: &Champion, val: &[u16; 32]| -> Vec<(u8, u16, u16)> {
        (0..spec.slots as usize)
            .filter(|&s| u.decided[s] != 0)
            .map(|s| (s as u8, u.decided[s], val[s] & u.decided[s]))
            .collect()
    };
    let mut seeds: Vec<Vec<(u8, u16, u16)>> = Vec::with_capacity(units.len() * 2);
    for u in &units {
        let id = seed_of(u, &u.value);
        if u.binding_free {
            let mut mv = u.value;
            for s in 0..spec.slots as usize {
                mv[s] = mirror_word(mv[s]);
            }
            let mirror = seed_of(u, &mv);
            if mirror != id {
                seeds.push(id);
                seeds.push(mirror);
                continue;
            }
        }
        seeds.push(id);
    }
    eprintln!(
        "narrow-split: {} units (frontier cycle {c}, {} pre-mirror) on {threads} threads",
        seeds.len(),
        units.len()
    );

    // Phase 2: dynamic pull off a shared counter; per-unit results are
    // scheduling-independent, merged in unit order.
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let live = AtomicU64::new(0);
    let results: std::sync::Mutex<Vec<(usize, SearchResult)>> =
        std::sync::Mutex::new(Vec::with_capacity(seeds.len()));
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                // Config-only state: build once per worker, lend to
                // every unit (per-unit builds were a real fixed cost on
                // million-unit frontiers).
                let mut wq = WordQuotient::build(&spec.cfg);
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(seed) = seeds.get(i) else { break };
                    let mut spec_w = spec.clone();
                    spec_w.seed = seed.clone();
                    let r = search_impl(&spec_w, champion_cap, false, Some(&live), Some(&mut wq));
                    results.lock().unwrap().push((i, r));
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        // Coordinator heartbeat, replacing the workers' silenced ones.
        // `live` counts in-flight work too (in 2^20 chunks), so a wall
        // of long-running units still shows visible progress.
        let mut last = std::time::Instant::now();
        while done.load(Ordering::Relaxed) < seeds.len() {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if last.elapsed().as_secs() >= 10 {
                let d = done.load(Ordering::Relaxed);
                eprintln!(
                    "narrow-split: {}/{} units settled, ~{} worker items (live)",
                    d,
                    seeds.len(),
                    live.load(Ordering::Relaxed)
                );
                last = std::time::Instant::now();
            }
        }
    });

    let mut rs = results.into_inner().unwrap();
    rs.sort_unstable_by_key(|&(i, _)| i);
    let mut champions = Vec::new();
    let mut stats = phase1;
    let mut cap_hit = false;
    for (_, r) in &rs {
        champions.extend(r.champions.iter().copied());
        merge_stats(&mut stats, &r.stats);
        cap_hit |= r.champion_cap_hit;
    }
    if champions.len() > champion_cap {
        champions.truncate(champion_cap);
        cap_hit = true;
    }
    SearchResult { champions, stats, champion_cap_hit: cap_hit }
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
    run_spec_oob(spec, code).0
}

/// [`run_spec`] plus an out-of-footprint flag: true iff the run ever
/// fetched a slot at/past `spec.slots` — the same dynamic condition
/// the search refutes on (undefined behavior on hardware). The trace
/// is still completed over the nop filler so callers can compare it;
/// an oob word is OUTSIDE the search space regardless of its trace.
pub fn run_spec_oob(spec: &EngineSpec, code: [u16; 32]) -> (Vec<u32>, bool) {
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
    let mut irq_at: FxMap<u32, u8> = FxMap::default();
    for &(c, m) in &spec.irq_sets {
        *irq_at.entry(c).or_insert(0u8) |= m;
    }

    let mut out = Vec::with_capacity(spec.cycles as usize);
    let mut oob = false;
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
        oob |= peek_tick(&st, &cfg)
            && will_fetch(&st, &cfg, gpio_in)
            && st.pc >= spec.slots;
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
    (out, oob)
}
