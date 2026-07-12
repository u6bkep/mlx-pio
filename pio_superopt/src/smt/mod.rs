//! Symbolic PIO semantics over Z3 bitvectors — the SAT/SMT synthesis track
//! (2026-07-06 pivot experiment: CEGIS instead of / alongside annealing).
//!
//! [`step`] is a bit-for-bit mirror of the vendored emulator's
//! `StateMachine::execute_cycle` (`vendor/picoem-common/src/pio/sm.rs`) for a
//! restricted **single-SM, single-pin, TX-only** subset, with the program's
//! 16-bit instruction words as (optionally) free bitvector variables. Unrolled
//! over T cycles it turns "does a program with this waveform exist?" into one
//! SMT query; [`legal_word`] confines the free words to the subset the mirror
//! actually models.
//!
//! ## Fidelity is the whole game
//!
//! A SAT model is protected by the certifier (any candidate is re-checked in
//! the real emulator), but an UNSAT verdict is only as good as this mirror.
//! Every semantic detail below is copied from `execute_cycle` and guarded by
//! the differential tests at the bottom (`differential_*`): random supported
//! programs × configs, emulator waveform vs symbolic waveform, cycle-exact on
//! both level and output-enable. Treat any diff-test failure as "the mirror is
//! wrong" until proven otherwise. Do not extend the supported subset without
//! extending the differential coverage.
//!
//! ## Supported subset (v1)
//!
//! Instructions (the enumeration alphabet, plus PULL, plus side-set):
//!   * `JMP` — all 8 conditions (PIN reads the one-cycle loopback of the
//!     output pad, exactly like the emulator's merged-GPIO view)
//!   * `OUT {Pins, X, Y, Null, PinDirs}, 1..=32` (with autopull)
//!   * `PULL {block, noblock, ifempty}`
//!   * `MOV {Pins, X, Y, PinDirs, Isr, Osr} <- {None, Invert} {Pins, X, Y,
//!     Null, Isr, Osr}`
//!   * `SET {Pins, X, Y, PinDirs}, imm`
//!   * per-instruction delay and side-set (value or pindir drive)
//!
//! Excluded (same rationale as `enumerate.rs` v1 scope): `WAIT`/`IRQ` (stall
//! on external events that never fire single-SM), `IN`/`PUSH` (no RX path in
//! a TX encoder), `OUT/MOV -> Exec/Pc` (self-modifying / computed jumps),
//! `MOV` bit-reverse, `MOV` from Status.
//!
//! Config: fixed per query (concrete Rust values — the solver searches over
//! programs, not configs; sweep configs with separate queries). See
//! [`supported_config`] for the exact contract: clkdiv 1 (every sysclk is a
//! PIO cycle), all pin groups base 0 / count 1 (one observed pin), autopush
//! off, autopull free, both shift directions.
//!
//! The TX FIFO is modeled as an index into the concrete input word list —
//! valid for both the harness fast path (inputs pre-loaded) and streaming
//! (refill before every step): pops happen in order and "empty" ⇔ the list is
//! exhausted (`run::tests::stream_matches_fast` pins their equivalence).

// `_eq` is deprecated in favor of an inherent `eq`, but `eq` collides with
// `PartialEq::eq` in reading position; the explicit name is clearer here.
#![allow(deprecated)]

use z3::ast::{Bool, BV};

use crate::ir::SideCfg;
use crate::program::{Config, Program, ShiftDir};

pub mod cegis;

// ---- small constructors --------------------------------------------------

fn bvu(v: u64, sz: u32) -> BV {
    BV::from_u64(v, sz)
}
fn bt(v: bool) -> Bool {
    Bool::from_bool(v)
}
/// Bit `i` of `bv` as a `Bool`.
fn bit(bv: &BV, i: u32) -> Bool {
    bv.extract(i, i)._eq(&bvu(1, 1))
}
/// A `Bool` as a 32-bit 0/1 value.
fn bool_to_bv32(b: &Bool) -> BV {
    b.ite(&bvu(1, 32), &bvu(0, 32))
}

// ---- the symbolic machine -------------------------------------------------

/// Symbolic SM state, one instant. Mirrors the `StateMachine` fields the
/// supported subset can reach. `pin`/`oe` are the observed pin's slice of the
/// block-shared `shared_pin_values` / `shared_pin_dirs` latches (bit 0; no
/// other bit is reachable when every pin group is base 0 / count 1).
#[derive(Clone)]
pub struct SymState {
    pub pc: BV,       // 5 bits
    pub x: BV,        // 32
    pub y: BV,        // 32
    pub osr: BV,      // 32
    pub isr: BV,      // 32
    pub osr_cnt: BV,  // 8 bits, 0..=32
    pub delay: BV,    // 5 bits, remaining delay cycles
    pub stalled: Bool, // only StallKind::Pull is reachable in the subset
    pub fifo_next: BV, // 8 bits: index of the next TX word to pop
    pub pin: Bool,    // shared_pin_values bit 0 (latch, resets HIGH)
    pub oe: Bool,     // shared_pin_dirs bit 0 (set_output(0) ran: starts true)
}

impl SymState {
    /// Post-`Pio::reset` + `configure()` state: PC 0, X/Y/ISR 0, OSR empty
    /// (`osr_count = 32` — autopull fires on the first OUT), value latch HIGH
    /// (`shared_pin_values` resets to all-ones), direction driven (the
    /// harness's `set_output(0)` force-executes `SET PINDIRS, 1`).
    pub fn initial() -> SymState {
        SymState {
            pc: bvu(0, 5),
            x: bvu(0, 32),
            y: bvu(0, 32),
            osr: bvu(0, 32),
            isr: bvu(0, 32),
            osr_cnt: bvu(32, 8),
            delay: bvu(0, 5),
            stalled: bt(false),
            fifo_next: bvu(0, 8),
            pin: bt(true),
            oe: bt(true),
        }
    }

    /// The externally observed level this instant: the harness captures the
    /// merged GPIO word, where a pin reads its latch iff driven and the
    /// external stimulus (constant 0 — none is applied) otherwise.
    pub fn level(&self) -> Bool {
        (&self.pin) & (&self.oe)
    }
}

/// A 32-slot program as bitvector words. Words may be Z3 constants (built
/// [`SymProgram::from_program`], for differential testing) or free variables
/// (built [`SymProgram::free`], for synthesis). Wrap bounds are concrete —
/// the synthesis convention, like `enumerate.rs`, is wrap = (0, len-1).
pub struct SymProgram {
    pub words: Vec<BV>, // 32 × 16 bits, slot index == address
    pub wrap_bottom: u8,
    pub wrap_top: u8,
}

impl SymProgram {
    /// Concrete program → constant words (via the pinned encoder).
    pub fn from_program(p: &Program) -> SymProgram {
        SymProgram {
            words: p.assemble().iter().map(|&w| bvu(w as u64, 16)).collect(),
            wrap_bottom: p.wrap_bottom,
            wrap_top: p.wrap_top,
        }
    }

    /// Free 16-bit variables in slots `0..len` (named `w0..`), NOP words in
    /// the rest (what `Program::assemble` puts in empty slots), wrap =
    /// `(0, len-1)`. Callers must assert [`legal_word`] on each free word.
    pub fn free(len: usize, side: &SideCfg) -> SymProgram {
        Self::with_holes(&vec![None; len], side)
    }

    /// Like [`SymProgram::free`], but with a template: `Some(word)` slots are
    /// concrete, `None` slots are free variables (named `w<i>`). Wrap =
    /// `(0, len-1)`; slots past the template are NOP. Callers must assert
    /// [`legal_word`] on each free (`None`) slot.
    pub fn with_holes(template: &[Option<u16>], side: &SideCfg) -> SymProgram {
        let len = template.len();
        assert!((1..=32).contains(&len));
        let nop = crate::encode::encode_insn(&crate::ir::Insn::nop_for(side), side);
        let words = (0..32)
            .map(|i| match template.get(i) {
                Some(Some(w)) => bvu(*w as u64, 16),
                Some(None) => BV::new_const(format!("w{i}"), 16),
                None => bvu(nop as u64, 16),
            })
            .collect();
        SymProgram { words, wrap_bottom: 0, wrap_top: (len - 1) as u8 }
    }
}

/// Check that `cfg` is inside the modeled contract. The symbolic step is
/// only a faithful mirror under these; reject everything else loudly.
pub fn supported_config(cfg: &Config) -> Result<(), String> {
    let c = cfg;
    if c.clkdiv_int != 1 || c.clkdiv_frac != 0 {
        return Err("smt model requires clkdiv 1.0 (every sysclk is a PIO cycle)".into());
    }
    let p = &c.pins;
    if p.out_base != 0 || p.set_base != 0 || p.in_base != 0 || p.sideset_base != 0 {
        return Err("smt model requires all pin bases == 0".into());
    }
    if p.out_count != 1 || p.set_count != 1 {
        return Err("smt model requires out_count == set_count == 1".into());
    }
    if c.jmp_pin != 0 {
        return Err("smt model requires jmp_pin == 0".into());
    }
    if c.side.count > 5 {
        return Err("sideset count > 5".into());
    }
    let actual_pins = c.side.count.saturating_sub(c.side.en as u8);
    if actual_pins > 1 {
        return Err("smt model supports at most 1 side-set value pin".into());
    }
    let s = &c.shift;
    if s.autopush {
        return Err("smt model does not support autopush (TX only)".into());
    }
    if s.fjoin_rx || s.fjoin_tx {
        return Err("smt model does not support FIFO join".into());
    }
    if !(1..=32).contains(&s.pull_threshold) {
        return Err("pull_threshold out of 1..=32".into());
    }
    Ok(())
}

/// Legality of a (possibly free) instruction word under the modeled subset:
/// exactly the opcodes/operands [`step`] mirrors faithfully. Synthesis MUST
/// assert this on every free word — an unconstrained word could decode to
/// WAIT/IRQ/IN/PUSH or `MOV/OUT -> Exec/Pc`, which the mirror does not model.
/// `len` additionally confines JMP targets to real slots.
pub fn legal_word(word: &BV, len: u8) -> Bool {
    let opcode = word.extract(15, 13);
    let operand = word.extract(7, 0);

    // JMP: any condition, target inside the program.
    let jmp_ok = opcode._eq(&bvu(0, 3)) & operand.extract(4, 0).bvult(&bvu(len as u64, 5));

    // OUT: dst in {Pins, X, Y, Null, PinDirs} = codes 0..=4 (5 = Pc, 6 = Isr,
    // 7 = Exec are out of scope). Any bit count (0 encodes 32).
    let out_ok = opcode._eq(&bvu(3, 3)) & operand.extract(7, 5).bvule(&bvu(4, 3));

    // PULL: opcode 100 with bit 7 set (bit 7 clear = PUSH, excluded); the
    // canonical encoding keeps the unused low 5 bits zero.
    let pull_ok = opcode._eq(&bvu(4, 3))
        & bit(&operand, 7)
        & operand.extract(4, 0)._eq(&bvu(0, 5));

    // MOV: dst in {Pins, X, Y, PinDirs, Isr, Osr} (not Exec=4 / Pc=5), op in
    // {None, Invert} (no bit-reverse / reserved), src in {Pins, X, Y, Null,
    // Isr, Osr} (not reserved=4 / Status=5).
    let mdst = operand.extract(7, 5);
    let msrc = operand.extract(2, 0);
    let mov_ok = opcode._eq(&bvu(5, 3))
        & !(mdst._eq(&bvu(4, 3)) | mdst._eq(&bvu(5, 3)))
        & operand.extract(4, 3).bvule(&bvu(1, 2))
        & !(msrc._eq(&bvu(4, 3)) | msrc._eq(&bvu(5, 3)));

    // SET: dst in {Pins=0, X=1, Y=2, PinDirs=4}.
    let sdst = operand.extract(7, 5);
    let set_ok = opcode._eq(&bvu(7, 3))
        & (sdst.bvule(&bvu(2, 3)) | sdst._eq(&bvu(4, 3)));

    jmp_ok | out_ok | pull_ok | mov_ok | set_ok
}

/// One PIO cycle: `execute_cycle` in bitvectors. `inputs` is the concrete TX
/// word stream ([`SymState::fifo_next`] indexes it). Returns the next state;
/// the captured sample for this cycle is `next.level()` / `next.oe` (the
/// harness samples the merged pads *after* stepping).
pub fn step(st: &SymState, prog: &SymProgram, cfg: &Config, inputs: &[u32]) -> SymState {
    debug_assert!(supported_config(cfg).is_ok());
    let thresh = cfg.shift.pull_threshold as u64; // 1..=32 (encode-time 0=32 already resolved)
    let autopull = cfg.shift.autopull;
    let out_right = cfg.shift.out_dir == ShiftDir::Right;
    let side = cfg.side;

    // The SM's GPIO input view this cycle: previous cycle's merged pads
    // (one-cycle loopback; no external stimulus, so undriven reads 0).
    let gpio = st.level();

    // -- phase select (execute_cycle's early returns) -----------------------
    let in_delay = st.delay.bvugt(&bvu(0, 5));
    let fifo_empty = st.fifo_next.bvuge(&bvu(inputs.len() as u64, 8));
    // Only StallKind::Pull is reachable: still stalled iff the FIFO is empty.
    let blocked = !(&in_delay) & &st.stalled & &fifo_empty;
    let attempt = !(&in_delay) & !(&blocked);

    // -- fetch + field decode ------------------------------------------------
    let word = (0..32).rev().fold(prog.words[31].clone(), |acc, i| {
        st.pc._eq(&bvu(i as u64, 5)).ite(&prog.words[i], &acc)
    });
    let opcode = word.extract(15, 13);
    let field = word.extract(12, 8); // shared delay/side-set field
    let operand = word.extract(7, 0);

    // Delay / side-set split (decode.rs): side-set is the TOP `side.count`
    // bits of the field, delay the bottom `5 - count`; with `en`, the MSB of
    // the side-set part is a per-instruction enable.
    let delay_bits = 5 - side.count.min(5) as u32;
    let decoded_delay = if delay_bits == 0 {
        bvu(0, 5)
    } else if delay_bits == 5 {
        field.clone()
    } else {
        field.extract(delay_bits - 1, 0).zero_ext(5 - delay_bits)
    };
    let actual_pins = side.count.saturating_sub(side.en as u8);
    // (assert, value-bit) of this instruction's side-set, if the config has a
    // drivable side-set pin at all.
    let sideset: Option<(Bool, Bool)> = if actual_pins == 0 {
        None
    } else {
        let assert = if side.en { bit(&field, 4) } else { bt(true) };
        // Value bit: MSB of the field minus the enable bit.
        let vbit = if side.en { bit(&field, 3) } else { bit(&field, 4) };
        Some((assert, vbit))
    };

    let is_jmp = opcode._eq(&bvu(0, 3));
    let is_out = opcode._eq(&bvu(3, 3));
    let is_pull = opcode._eq(&bvu(4, 3)) & bit(&operand, 7);
    let is_mov = opcode._eq(&bvu(5, 3));
    let is_set = opcode._eq(&bvu(7, 3));

    // -- FIFO pop value (concrete stream, symbolic index) --------------------
    let popped = inputs.iter().enumerate().rev().fold(bvu(0, 32), |acc, (i, &w)| {
        st.fifo_next._eq(&bvu(i as u64, 8)).ite(&bvu(w as u64, 32), &acc)
    });

    // -- JMP ------------------------------------------------------------------
    let cond = operand.extract(7, 5);
    let target = operand.extract(4, 0);
    let x_zero = st.x._eq(&bvu(0, 32));
    let y_zero = st.y._eq(&bvu(0, 32));
    let jmp_taken_if = |c: u64, b: Bool| cond._eq(&bvu(c, 3)) & b;
    let jmp_cond = jmp_taken_if(0, bt(true))
        | jmp_taken_if(1, x_zero.clone())
        | jmp_taken_if(2, !(&x_zero))
        | jmp_taken_if(3, y_zero.clone())
        | jmp_taken_if(4, !(&y_zero))
        | jmp_taken_if(5, !(&st.x._eq(&st.y)))
        | jmp_taken_if(6, gpio.clone())
        | jmp_taken_if(7, st.osr_cnt.bvult(&bvu(thresh, 8)));
    let jmp_taken = (&is_jmp) & (&jmp_cond);
    // X-- / Y-- decrement only when the branch is taken (datasheet §3.4.4).
    let x_dec = (&is_jmp) & cond._eq(&bvu(2, 3)) & !(&x_zero);
    let y_dec = (&is_jmp) & cond._eq(&bvu(4, 3)) & !(&y_zero);

    // -- OUT (with autopull) ---------------------------------------------------
    // Pre-shift refill: with autopull on and osr_count >= threshold, pop the
    // FIFO into the OSR (count 0) or stall if it is empty — before any shift.
    let ap_due = if autopull { st.osr_cnt.bvuge(&bvu(thresh, 8)) } else { bt(false) };
    let out_stall = (&is_out) & (&ap_due) & (&fifo_empty);
    let out_refill = (&is_out) & (&ap_due) & !(&fifo_empty);
    let out_runs = (&is_out) & !(&out_stall);
    let osr_pre = out_refill.ite(&popped, &st.osr);
    let cnt_pre = out_refill.ite(&bvu(0, 8), &st.osr_cnt);

    // Bit count: 5-bit field, 0 encodes 32. Widen to 32 for shift amounts.
    let bc_raw = operand.extract(4, 0);
    let bc = bc_raw._eq(&bvu(0, 5)).ite(&bvu(32, 8), &bc_raw.zero_ext(3));
    let bc32 = bc.zero_ext(24);
    let inv32 = bvu(32, 32).bvsub(&bc32); // 0..=31 (bc >= 1)
    let (out_data, osr_shifted) = if out_right {
        // Right shift: data is the LOW bc bits, OSR loses them off the bottom.
        (osr_pre.bvshl(&inv32).bvlshr(&inv32), osr_pre.bvlshr(&bc32))
    } else {
        // Left shift: data is the HIGH bc bits, OSR loses them off the top.
        (osr_pre.bvlshr(&inv32), osr_pre.bvshl(&bc32))
    };
    let cnt_sum = cnt_pre.bvadd(&bc);
    let cnt_out = cnt_sum.bvule(&bvu(32, 8)).ite(&cnt_sum, &bvu(32, 8));
    let odst = operand.extract(7, 5);

    // -- PULL --------------------------------------------------------------------
    // IFEMPTY: do nothing unless the output shift count has reached its
    // threshold (datasheet 11.4.7) — the guard is evaluated before any
    // FIFO access. Past the guard, Block decides: stall on empty, or
    // copy X into the OSR.
    let if_empty = bit(&operand, 6);
    let block = bit(&operand, 5);
    let osre_miss = (&if_empty) & st.osr_cnt.bvult(&bvu(thresh, 8));
    let pull_go = (&is_pull) & !(&osre_miss);
    let pull_stall = (&pull_go) & (&fifo_empty) & (&block);
    let pull_pops = (&pull_go) & !(&fifo_empty);
    let pull_x = (&pull_go) & (&fifo_empty) & !(&block);

    // -- MOV ------------------------------------------------------------------------
    let msrc = operand.extract(2, 0);
    let mop = operand.extract(4, 3);
    let mdst = operand.extract(7, 5);
    let gpio32 = bool_to_bv32(&gpio); // read_pins: only bit 0 can be driven
    let mval_raw = [
        (0u64, gpio32),
        (1, st.x.clone()),
        (2, st.y.clone()),
        (3, bvu(0, 32)),
        (6, st.isr.clone()),
        (7, st.osr.clone()),
    ]
    .iter()
    .rev()
    .fold(bvu(0, 32), |acc, (c, v)| msrc._eq(&bvu(*c, 3)).ite(v, &acc));
    let mval = mop._eq(&bvu(1, 2)).ite(&mval_raw.bvnot(), &mval_raw);

    // -- SET ---------------------------------------------------------------------------
    let sdst = operand.extract(7, 5);
    let sdata = operand.extract(4, 0).zero_ext(27);

    // -- compose next state ------------------------------------------------------------
    let new_stall = (&out_stall) | (&pull_stall);
    let exec_ok = (&attempt) & !(&new_stall); // instruction completed this cycle

    let sel32 = |guard: &Bool, val: &BV, els: &BV| ((&attempt) & guard).ite(val, els);

    let x_next = sel32(
        &((&x_dec) | ((&out_runs) & odst._eq(&bvu(1, 3)))
            | ((&is_mov) & mdst._eq(&bvu(1, 3)))
            | ((&is_set) & sdst._eq(&bvu(1, 3)))),
        &(&x_dec).ite(
            &st.x.bvsub(&bvu(1, 32)),
            &(&is_out).ite(&out_data, &(&is_mov).ite(&mval, &sdata)),
        ),
        &st.x,
    );
    let y_next = sel32(
        &((&y_dec) | ((&out_runs) & odst._eq(&bvu(2, 3)))
            | ((&is_mov) & mdst._eq(&bvu(2, 3)))
            | ((&is_set) & sdst._eq(&bvu(2, 3)))),
        &(&y_dec).ite(
            &st.y.bvsub(&bvu(1, 32)),
            &(&is_out).ite(&out_data, &(&is_mov).ite(&mval, &sdata)),
        ),
        &st.y,
    );

    // OSR: OUT shifts (after any refill); PULL loads the popped word or X;
    // MOV OSR <- val overwrites AND resets osr_count to 0 (RP2350 datasheet
    // ch.11: MOV dst OSR "Output shift counter is reset to 0 by this
    // operation, i.e. full").
    let mov_osr = (&is_mov) & mdst._eq(&bvu(7, 3));
    let osr_next = sel32(
        &out_runs.clone(),
        &osr_shifted,
        &sel32(
            &pull_pops.clone(),
            &popped,
            &sel32(
                &pull_x.clone(),
                &st.x,
                &sel32(&mov_osr, &mval, &st.osr),
            ),
        ),
    );
    // MOV OSR joins PULL in zeroing the count. (The model tracks no ISR shift
    // counter, so MOV ISR / OUT ISR have no counter to reset here.)
    let cnt_next = ((&attempt) & (&out_runs)).ite(
        &cnt_out,
        &((&attempt) & ((&pull_pops) | (&pull_x) | (&mov_osr))).ite(&bvu(0, 8), &st.osr_cnt),
    );
    let isr_next = sel32(&((&is_mov) & mdst._eq(&bvu(6, 3))), &mval, &st.isr);

    let fifo_pop = (&attempt) & ((&out_refill) | (&pull_pops));
    let fifo_next = fifo_pop.ite(&st.fifo_next.bvadd(&bvu(1, 8)), &st.fifo_next);

    // Pin value / direction latches (bit 0 of the shared block latches).
    // Instruction writes first, then an asserted side-set overrides — the
    // emulator applies side-set AFTER execute_insn, on every attempted cycle
    // (including ones that freshly stall).
    let selb = |guard: &Bool, val: &Bool, els: &Bool| -> Bool {
        ((&attempt) & guard).ite(val, els)
    };
    let pin_wr = ((&out_runs) & odst._eq(&bvu(0, 3)) & bit(&out_data, 0))
        | ((&is_mov) & mdst._eq(&bvu(0, 3)) & bit(&mval, 0))
        | ((&is_set) & sdst._eq(&bvu(0, 3)) & bit(&operand, 0));
    let pin_writes = ((&out_runs) & odst._eq(&bvu(0, 3)))
        | ((&is_mov) & mdst._eq(&bvu(0, 3)))
        | ((&is_set) & sdst._eq(&bvu(0, 3)));
    let mut pin_next = selb(&pin_writes, &pin_wr, &st.pin);
    let oe_wr = ((&out_runs) & odst._eq(&bvu(4, 3)) & bit(&out_data, 0))
        | ((&is_mov) & mdst._eq(&bvu(3, 3)) & bit(&mval, 0))
        | ((&is_set) & sdst._eq(&bvu(4, 3)) & bit(&operand, 0));
    let oe_writes = ((&out_runs) & odst._eq(&bvu(4, 3)))
        | ((&is_mov) & mdst._eq(&bvu(3, 3)))
        | ((&is_set) & sdst._eq(&bvu(4, 3)));
    let mut oe_next = selb(&oe_writes, &oe_wr, &st.oe);
    if let Some((ss_assert, ss_val)) = sideset {
        let applies = (&attempt) & (&ss_assert);
        if cfg.side_pindir {
            oe_next = applies.ite(&ss_val, &oe_next);
        } else {
            pin_next = applies.ite(&ss_val, &pin_next);
        }
    }

    // PC: taken JMP sets it; otherwise wrap-aware increment — only when the
    // instruction completed (a stalled instruction re-executes at the same PC).
    let advance = st.pc._eq(&bvu(prog.wrap_top as u64, 5)).ite(
        &bvu(prog.wrap_bottom as u64, 5),
        &st.pc.bvadd(&bvu(1, 5)),
    );
    let pc_next = (&exec_ok).ite(&jmp_taken.ite(&target, &advance), &st.pc);

    // Delay: loaded from the completed instruction, else counts down.
    let delay_next = (&exec_ok).ite(
        &decoded_delay,
        &in_delay.ite(&st.delay.bvsub(&bvu(1, 5)), &st.delay),
    );
    let stalled_next = (&attempt).ite(&new_stall, &st.stalled);

    SymState {
        pc: pc_next,
        x: x_next,
        y: y_next,
        osr: osr_next,
        isr: isr_next,
        osr_cnt: cnt_next,
        delay: delay_next,
        stalled: stalled_next,
        fifo_next,
        pin: pin_next,
        oe: oe_next,
    }
}

/// A symbolic run: per-cycle captured level and output-enable (what
/// `RunSpec { capture_pins: [0] }` packs into bits 0 and 16), plus the state
/// trajectory for anyone who wants invariants over it.
pub struct SymTrace {
    pub states: Vec<SymState>, // states[0] = initial, states[t+1] = after cycle t
    pub levels: Vec<Bool>,     // levels[t] = captured level of cycle t
    pub oes: Vec<Bool>,        // oes[t] = captured output-enable of cycle t
}

/// Unroll `cycles` PIO cycles from the post-configure initial state.
///
/// Pure expression form: each cycle's state is the full nested term over the
/// previous one. Right for GROUND evaluation (differential tests fold it with
/// `simplify`), pathological for solving — with free program words the term
/// tree duplicates exponentially. Solver queries should use
/// [`unroll_interned`] instead.
pub fn unroll(prog: &SymProgram, cfg: &Config, inputs: &[u32], cycles: usize) -> SymTrace {
    supported_config(cfg).expect("unsupported config for the smt model");
    assert!(inputs.len() < 256, "fifo_next is 8 bits");
    let mut states = Vec::with_capacity(cycles + 1);
    let mut levels = Vec::with_capacity(cycles);
    let mut oes = Vec::with_capacity(cycles);
    states.push(SymState::initial());
    for t in 0..cycles {
        let next = step(&states[t], prog, cfg, inputs);
        levels.push(next.level());
        oes.push(next.oe.clone());
        states.push(next);
    }
    SymTrace { states, levels, oes }
}

/// [`unroll`] in SSA/BMC form: after every cycle each state field is replaced
/// by a fresh constant asserted equal on `solver` (`u<tag>_<cycle>_<field>`),
/// so the formula stays linear in `cycles` instead of nesting — the standard
/// bounded-model-checking shape. Semantically identical to [`unroll`]
/// (`interned_unroll_matches_pure` pins it); use for every solver query.
pub fn unroll_interned(
    solver: &z3::Solver,
    prog: &SymProgram,
    cfg: &Config,
    inputs: &[u32],
    cycles: usize,
    tag: usize,
) -> SymTrace {
    supported_config(cfg).expect("unsupported config for the smt model");
    assert!(inputs.len() < 256, "fifo_next is 8 bits");
    let mut states = Vec::with_capacity(cycles + 1);
    let mut levels = Vec::with_capacity(cycles);
    let mut oes = Vec::with_capacity(cycles);
    states.push(SymState::initial());
    for t in 0..cycles {
        let next = step(&states[t], prog, cfg, inputs);
        let ibv = |name: &str, e: &BV| -> BV {
            let v = BV::new_const(format!("u{tag}_{t}_{name}"), e.get_size());
            solver.assert(&v._eq(e));
            v
        };
        let ib = |name: &str, e: &Bool| -> Bool {
            let v = Bool::new_const(format!("u{tag}_{t}_{name}"));
            solver.assert(&v.iff(e));
            v
        };
        let next = SymState {
            pc: ibv("pc", &next.pc),
            x: ibv("x", &next.x),
            y: ibv("y", &next.y),
            osr: ibv("osr", &next.osr),
            isr: ibv("isr", &next.isr),
            osr_cnt: ibv("cnt", &next.osr_cnt),
            delay: ibv("dly", &next.delay),
            stalled: ib("stl", &next.stalled),
            fifo_next: ibv("fifo", &next.fifo_next),
            pin: ib("pin", &next.pin),
            oe: ib("oe", &next.oe),
        };
        levels.push(next.level());
        oes.push(next.oe.clone());
        states.push(next);
    }
    SymTrace { states, levels, oes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{dme_cfg, dme_corpus, dme_spec_ref, DME_CYCLES};
    use z3::ast::Ast;
    use crate::ir::{Insn, Op};
    use crate::program::Program;
    use crate::rng::Rng;
    use crate::run::{run, RunSpec};

    /// Fold a ground (variable-free) Bool to a concrete value.
    fn ground(b: &Bool) -> bool {
        b.simplify().as_bool().expect("expression is not ground")
    }

    fn spec_for(inputs: Vec<u32>, cycles: u64) -> RunSpec {
        RunSpec {
            block: 0,
            sm: 0,
            inputs,
            output_pins: vec![0],
            capture_pins: vec![0],
            cycles,
            autopull_pad: 0,
        }
    }

    /// Emulator vs mirror, cycle-exact on (level, oe). Any mismatch prints
    /// the first diverging cycle with both waveform tails.
    fn assert_matches(p: &Program, inputs: &[u32], cycles: usize, label: &str) {
        let wave = run(p, &spec_for(inputs.to_vec(), cycles as u64));
        let sym = SymProgram::from_program(p);
        let tr = unroll(&sym, &p.config, inputs, cycles);
        for t in 0..cycles {
            let e_lvl = wave[t] & 1 != 0;
            let e_oe = wave[t] >> 16 & 1 != 0;
            let s_lvl = ground(&tr.levels[t]);
            let s_oe = ground(&tr.oes[t]);
            assert!(
                e_lvl == s_lvl && e_oe == s_oe,
                "{label}: first divergence at cycle {t}: emulator (lvl={e_lvl}, oe={e_oe}) \
                 vs smt (lvl={s_lvl}, oe={s_oe})\nprogram: {}",
                p.brief()
            );
        }
    }

    /// The 8-instruction spec-shaped seed (autopull, JMP X--, MOV, OUT,
    /// interior delays) over the full locked capture window — the flagship
    /// concrete case for the mirror.
    #[test]
    fn differential_dme_spec_ref() {
        let p = dme_spec_ref();
        assert_matches(&p, &dme_corpus(), DME_CYCLES as usize, "dme_spec_ref");
    }

    /// Random programs drawn from the supported subset across the config
    /// grid: side-set none / mandatory-1 / optional-1(+en) / pindir-drive,
    /// autopull on/off, both OUT shift directions, random pull thresholds,
    /// random delays, streaming (>4 words) and pre-loaded input lengths.
    #[test]
    fn differential_random_programs() {
        let mut rng = Rng::new(0x5EED_CAFE);
        let grid: Vec<(SideCfg, bool)> = vec![
            (SideCfg::NONE, false),
            (SideCfg { count: 1, en: false }, false),
            (SideCfg { count: 2, en: true }, false),
            (SideCfg { count: 1, en: false }, true), // side-set drives pindirs
        ];
        for case in 0..60 {
            let (side, side_pindir) = grid[case % grid.len()];
            let autopull = rng.boolean();
            let len = 2 + rng.below(5) as usize; // 2..=6
            let p = random_supported_program(&mut rng, len, side, side_pindir, autopull);
            let n_inputs = 1 + rng.below(6) as usize; // 1..=6: exercises streaming
            let inputs: Vec<u32> = (0..n_inputs).map(|_| rng.next_u64() as u32).collect();
            assert_matches(&p, &inputs, 64, &format!("random case {case}"));
        }
    }

    /// Deep tier of `differential_random_programs`: same generator, 2000
    /// cases with longer captures. Run before trusting any UNSAT verdict:
    /// `cargo test --release --features smt -- --ignored differential_fuzz`.
    #[test]
    #[ignore = "deep differential fuzz (~minutes); run before trusting UNSAT results"]
    fn differential_fuzz() {
        let mut rng = Rng::new(0xF0_22_D1FF);
        let grid: Vec<(SideCfg, bool)> = vec![
            (SideCfg::NONE, false),
            (SideCfg { count: 1, en: false }, false),
            (SideCfg { count: 2, en: true }, false),
            (SideCfg { count: 1, en: false }, true),
            (SideCfg { count: 2, en: true }, true),
        ];
        for case in 0..2000 {
            let (side, side_pindir) = grid[case % grid.len()];
            let autopull = rng.boolean();
            let len = 2 + rng.below(7) as usize; // 2..=8
            let p = random_supported_program(&mut rng, len, side, side_pindir, autopull);
            let n_inputs = rng.below(8) as usize; // 0..=7, incl. no-input starvation
            let inputs: Vec<u32> = (0..n_inputs).map(|_| rng.next_u64() as u32).collect();
            assert_matches(&p, &inputs, 128, &format!("fuzz case {case}"));
        }
    }

    /// The SSA/BMC unroll must be semantically identical to the pure one:
    /// same concrete program, solver-completed interned levels == ground
    /// pure levels, every cycle.
    #[test]
    fn interned_unroll_matches_pure() {
        let mut rng = Rng::new(0x155A_B3C5);
        for case in 0..5 {
            let autopull = rng.boolean();
            let p = random_supported_program(&mut rng, 4, SideCfg::NONE, false, autopull);
            let inputs = [0x0Bu32, 0x1D];
            let sym = SymProgram::from_program(&p);
            let pure = unroll(&sym, &p.config, &inputs, 48);
            let solver = z3::Solver::new();
            let interned = unroll_interned(&solver, &sym, &p.config, &inputs, 48, 0);
            assert_eq!(solver.check(), z3::SatResult::Sat, "concrete chain must be sat");
            let model = solver.get_model().unwrap();
            for t in 0..48 {
                let iv = model.eval(&interned.levels[t], true).unwrap().as_bool().unwrap();
                assert_eq!(iv, ground(&pure.levels[t]), "case {case} cycle {t}");
            }
        }
    }

    /// Build a random program from exactly the modeled subset: the
    /// enumeration alphabet plus PULL variants, random delays within the
    /// side-set budget, random side-set values per config.
    fn random_supported_program(
        rng: &mut Rng,
        len: usize,
        side: SideCfg,
        side_pindir: bool,
        autopull: bool,
    ) -> Program {
        let mut cfg = dme_cfg();
        cfg.side = side;
        cfg.side_pindir = side_pindir;
        cfg.shift.autopull = autopull;
        cfg.shift.pull_threshold = 1 + rng.below(32) as u8;
        cfg.shift.out_dir = if rng.boolean() { ShiftDir::Right } else { ShiftDir::Left };
        let ops = crate::enumerate::alphabet(len);
        let mut p = Program::empty(cfg);
        for i in 0..len {
            let op = if rng.below(100) < 15 {
                Op::Pull { if_empty: rng.boolean(), block: rng.boolean() }
            } else {
                rng.pick(&ops).clone()
            };
            let delay = rng.below(side.max_delay() as u32 + 1) as u8;
            let sideset = match side.max_sideset() {
                None => None,
                Some(max) => {
                    if side.en && rng.boolean() {
                        None // opt out
                    } else {
                        Some(rng.below(max as u32 + 1) as u8)
                    }
                }
            };
            p.slots[i] = Some(Insn { op, delay, sideset });
        }
        p.wrap_bottom = 0;
        p.wrap_top = (len - 1) as u8;
        p.validate().expect("generator produced an invalid program");
        p
    }

    /// End-to-end ∃ direction: ask the solver for a 1-instruction program
    /// whose pin level toggles every cycle, then run the synthesized word in
    /// the REAL emulator and check it actually toggles. Closes the
    /// solver → certifier loop the CEGIS engine will rely on.
    #[test]
    fn synthesize_len1_toggler() {
        let cfg = dme_cfg(); // side-set NONE, autopull off
        let sym = SymProgram::free(1, &cfg.side);
        let cycles = 8;
        let tr = unroll(&sym, &cfg, &[], cycles);

        let solver = z3::Solver::new();
        solver.assert(&legal_word(&sym.words[0], 1));
        for t in 1..cycles {
            solver.assert(&(&tr.levels[t]).xor(&tr.levels[t - 1]));
        }
        assert_eq!(solver.check(), z3::SatResult::Sat, "no 1-insn toggler found");
        let model = solver.get_model().unwrap();
        let word = model.eval(&sym.words[0], true).unwrap().as_u64().unwrap() as u16;

        // Decode the word back to IR and run it through the real emulator.
        let insn = crate::decode::decode_insn(word, &cfg.side)
            .expect("synthesized word must decode to legal IR");
        let mut p = Program::empty(cfg);
        p.slots[0] = Some(insn.clone());
        p.wrap_bottom = 0;
        p.wrap_top = 0;
        let wave = run(&p, &spec_for(vec![], cycles as u64));
        for t in 1..cycles {
            assert_ne!(
                wave[t] & 1,
                wave[t - 1] & 1,
                "synthesized program does not toggle in the emulator: {} (word {word:#06x})",
                insn.brief()
            );
        }
    }
}
