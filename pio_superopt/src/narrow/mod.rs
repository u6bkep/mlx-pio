//! The narrowing engine's own PIO evaluator.
//!
//! v1 is a CONCRETE interpreter: tiny memcpy-able state (`NState`), an
//! immutable per-candidate config (`NCfg`), and a per-cycle `step` that
//! mirrors the vendored emulator's SM semantics exactly. The contract is
//! written down in `docs/evaluator-spec.md` (the "twin spec" — it also
//! serves the planned shard formalization); the differential gate is
//! `tests/narrow_diff.rs`, which pins byte-identical output against the
//! vendored-emulator path (`crate::run::run`).
//!
//! Why not reuse the vendored emulator: the narrowing engine forks
//! evaluator state at undecided instruction bit-fields, so state must be
//! a small flat `Copy` value — not a `StateMachine` embedded in a
//! whole-chip `Bus`. Decode here is TOTAL and works on raw bit-fields
//! (no IR round-trip): `pending_exec` can carry arbitrary words, and the
//! future hole-forking layer forks on exactly these fields.

use crate::program::{Program, ShiftDir};
use crate::run::RunSpec;

pub mod engine;

/// FIFO with configurable depth (4 normal, 8 joined, 0 joined-away).
/// `Copy` twin of the vendored `PioFifo` (same observable behavior).
/// NOTE for hashing (the memo key): stale `buf` slots and the `head`
/// rotation are hashed as-is, so observably-equal FIFOs can hash
/// differently — that only costs memo sharing, never soundness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fifo {
    buf: [u32; 8],
    head: u8,
    count: u8,
    depth: u8,
}

impl Fifo {
    pub fn new(depth: u8) -> Self {
        Fifo { buf: [0; 8], head: 0, count: 0, depth }
    }
    /// Push a value; `false` (value dropped) when full or depth 0.
    pub fn push(&mut self, val: u32) -> bool {
        if self.count >= self.depth {
            return false;
        }
        self.buf[((self.head + self.count) % self.depth) as usize] = val;
        self.count += 1;
        true
    }
    pub fn pop(&mut self) -> Option<u32> {
        if self.count == 0 {
            return None;
        }
        let val = self.buf[self.head as usize];
        self.head = (self.head + 1) % self.depth;
        self.count -= 1;
        Some(val)
    }
    pub fn is_full(&self) -> bool {
        self.depth > 0 && self.count >= self.depth
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn level(&self) -> u8 {
        self.count
    }
}

/// Why the SM is stalled (re-evaluated each cycle until it clears).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stall {
    None,
    WaitGpio { polarity: bool, index: u8 },
    WaitPin { polarity: bool, index: u8 },
    WaitIrq { polarity: bool, index: u8 },
    Pull,
    Push,
    IrqWait { index: u8 },
}

/// All per-cycle-varying evaluator state — flat and `Copy`, so the
/// narrowing engine can checkpoint it with a memcpy at a fork point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NState {
    pub pc: u8,
    pub x: u32,
    pub y: u32,
    pub isr: u32,
    pub osr: u32,
    pub isr_count: u8,
    pub osr_count: u8,
    pub delay_count: u8,
    pub stall: Stall,
    pub pending_exec: Option<u16>,
    /// Block-level IRQ flags 0..7 (carried per-state for the future
    /// two-SM product machine).
    pub irq_flags: u8,
    /// Shared pin VALUE latch (block-level `pad_out` with one SM).
    pub out_latch: u32,
    /// Shared pin DIRECTION latch (`pad_oe`).
    pub dir_latch: u32,
    /// Clock-divider accumulator, ×256 fixed point.
    pub clk_acc: u32,
    pub tx: Fifo,
    pub rx: Fifo,
}

impl NState {
    pub fn new(cfg: &NCfg) -> Self {
        NState {
            pc: 0,
            x: 0,
            y: 0,
            isr: 0,
            osr: 0,
            isr_count: 0,
            // OSR "empty" at reset = all 32 bits already shifted out
            // (vendored/real RP2350: autopull fires on the first OUT, and
            // `jmp !OSRE` is FALSE at reset).
            osr_count: 32,
            delay_count: 0,
            stall: Stall::None,
            pending_exec: None,
            irq_flags: 0,
            // The pin VALUE latch idles ALL-ONES (vendored `PioBlock::new`
            // / `reset`: `shared_pin_values: u32::MAX`) — a pin whose
            // direction is set before anything writes its value drives
            // HIGH. The DME fixtures encode against this ("the emulator
            // pad idles HIGH"), so it is contract, not accident.
            out_latch: u32::MAX,
            dir_latch: 0,
            clk_acc: 0,
            tx: Fifo::new(cfg.tx_depth),
            rx: Fifo::new(cfg.rx_depth),
        }
    }
}

/// Immutable per-candidate configuration: the program words plus every
/// SM-config field the semantics consult, pre-decoded from `Program`.
#[derive(Debug, Clone)]
pub struct NCfg {
    pub code: [u16; 32],
    pub wrap_bottom: u8,
    pub wrap_top: u8,
    /// PINCTRL_SIDESET_COUNT — bits of the 5-bit field used by side-set,
    /// INCLUDING the enable bit when `side_en`.
    pub side_count: u8,
    pub side_en: bool,
    pub side_pindir: bool,
    pub jmp_pin: u8,
    pub in_base: u8,
    pub out_base: u8,
    pub out_count: u8,
    pub set_base: u8,
    pub set_count: u8,
    pub sideset_base: u8,
    /// SHIFTCTRL IN_COUNT (RP2350 MOV-PINS mask; 0 = unmasked). The
    /// harness never programs it, so `Program`-derived configs use 0.
    pub in_count: u8,
    pub autopush: bool,
    pub autopull: bool,
    pub push_threshold: u8, // 1..=32 (resolved; encoded 0 means 32)
    pub pull_threshold: u8, // 1..=32
    pub in_shift_right: bool,
    pub out_shift_right: bool,
    pub clkdiv_int: u16, // 0 means 65536
    pub clkdiv_frac: u8,
    /// EXECCTRL STATUS_SEL / STATUS_N (MOV STATUS). Harness leaves 0.
    pub status_sel: bool,
    pub status_n: u8,
    pub sm_id: u8,
    pub tx_depth: u8,
    pub rx_depth: u8,
}

impl NCfg {
    /// Derive from a search [`Program`], mirroring `run::configure_regs`.
    pub fn from_program(p: &Program, sm_id: u8) -> Self {
        let c = &p.config;
        // FJOIN depth mapping (vendored `apply_fifo_join`): TX join wins.
        let (tx_depth, rx_depth) = if c.shift.fjoin_tx {
            (8, 0)
        } else if c.shift.fjoin_rx {
            (0, 8)
        } else {
            (4, 4)
        };
        NCfg {
            code: p.assemble(),
            wrap_bottom: p.wrap_bottom,
            wrap_top: p.wrap_top,
            side_count: c.side.count.min(5),
            side_en: c.side.en,
            side_pindir: c.side_pindir,
            jmp_pin: c.jmp_pin,
            // Counts masked to their PINCTRL field widths — the register
            // path truncates them (`(pinctrl>>20)&0x3F`, `(pinctrl>>26)&7`),
            // so an out-of-range `PinMap` must behave identically here.
            in_base: c.pins.in_base & 0x1F,
            out_base: c.pins.out_base & 0x1F,
            out_count: c.pins.out_count & 0x3F,
            set_base: c.pins.set_base & 0x1F,
            set_count: c.pins.set_count & 0x7,
            sideset_base: c.pins.sideset_base & 0x1F,
            in_count: 0,
            autopush: c.shift.autopush,
            autopull: c.shift.autopull,
            push_threshold: if c.shift.push_threshold == 0 { 32 } else { c.shift.push_threshold },
            pull_threshold: if c.shift.pull_threshold == 0 { 32 } else { c.shift.pull_threshold },
            in_shift_right: c.shift.in_dir == ShiftDir::Right,
            out_shift_right: c.shift.out_dir == ShiftDir::Right,
            clkdiv_int: c.clkdiv_int,
            clkdiv_frac: c.clkdiv_frac,
            status_sel: false,
            status_n: 0,
            sm_id,
            tx_depth,
            rx_depth,
        }
    }
}

/// Split the 5-bit delay/side-set field per config. `None` side-set =
/// this instruction performs no side-set this cycle (pins HOLD).
#[inline]
fn split_side_delay(field: u8, cfg: &NCfg) -> (Option<u8>, u8) {
    let count = cfg.side_count;
    if count == 0 {
        return (None, field & 0x1F);
    }
    let delay_bits = 5 - count;
    let delay = field & ((1u8 << delay_bits) - 1);
    let ss_raw = field >> delay_bits;
    let sideset = if cfg.side_en {
        if (ss_raw >> (count - 1)) & 1 != 0 {
            Some(ss_raw & ((1u8 << (count - 1)) - 1))
        } else {
            None
        }
    } else {
        Some(ss_raw)
    };
    (sideset, delay)
}

/// Replace `count` bits of `field` starting at `base` (mod-32 rotate).
#[inline]
fn write_pin_field(field: &mut u32, value: u32, base: u8, count: u8) {
    if count == 0 {
        return;
    }
    let mask = if count >= 32 { u32::MAX } else { (1u32 << count) - 1 };
    let positioned_val = (value & mask).rotate_left(base as u32);
    let positioned_mask = mask.rotate_left(base as u32);
    *field = (*field & !positioned_mask) | positioned_val;
}

/// Advance the clock-divider accumulator; `true` = the SM runs a PIO
/// cycle this system clock.
#[inline]
pub fn clock_tick(st: &mut NState, cfg: &NCfg) -> bool {
    let threshold = if cfg.clkdiv_int == 0 {
        65536u32 * 256
    } else {
        (cfg.clkdiv_int as u32) * 256 + cfg.clkdiv_frac as u32
    };
    st.clk_acc += 256;
    if st.clk_acc >= threshold {
        st.clk_acc -= threshold;
        true
    } else {
        false
    }
}

#[inline]
fn still_stalled(st: &NState, cfg: &NCfg, gpio_in: u32) -> bool {
    match st.stall {
        Stall::None => false,
        Stall::Pull => st.tx.is_empty(),
        Stall::Push => st.rx.is_full(),
        Stall::WaitGpio { polarity, index } => ((gpio_in >> (index & 31)) & 1 != 0) != polarity,
        Stall::WaitPin { polarity, index } => {
            let pin = (cfg.in_base as u32 + index as u32) & 31;
            ((gpio_in >> pin) & 1 != 0) != polarity
        }
        // Report-only: the clear-and-complete belongs to the re-executed
        // WAIT itself (clearing here would re-stall it forever).
        Stall::WaitIrq { polarity, index } => ((st.irq_flags >> (index & 7)) & 1 != 0) != polarity,
        Stall::IrqWait { index } => (st.irq_flags >> (index & 7)) & 1 != 0,
    }
}

#[inline]
fn resolve_irq_index(index: u8, sm_id: u8) -> u8 {
    if index & 0x10 != 0 {
        (((index & 3) + sm_id) % 4) | (index & 4)
    } else {
        index & 7
    }
}

#[inline]
fn advance_pc(st: &mut NState, cfg: &NCfg) {
    if st.pc == cfg.wrap_top {
        st.pc = cfg.wrap_bottom;
    } else {
        st.pc = (st.pc + 1) & 0x1F;
    }
}

/// One PIO cycle (the divided clock has already fired). Mirrors the
/// vendored `StateMachine::execute_cycle` ordering exactly:
/// delay countdown → stall re-check → fetch (pending_exec overrides) →
/// execute → side-set ALWAYS → delay latch + PC advance if unstalled.
pub fn step(st: &mut NState, cfg: &NCfg, gpio_in: u32) {
    if st.delay_count > 0 {
        st.delay_count -= 1;
        return;
    }

    if st.stall != Stall::None {
        if still_stalled(st, cfg, gpio_in) {
            return;
        }
        st.stall = Stall::None;
        // Fall through: PC hasn't advanced, so the fetch below re-executes
        // the stalled slot with the condition now resolved.
    }

    let (insn, is_forced) = match st.pending_exec.take() {
        Some(forced) => (forced, true),
        None => (cfg.code[st.pc as usize], false),
    };

    let opcode = (insn >> 13) & 0x7;
    let operand = (insn & 0xFF) as u8;
    let (sideset, delay) = split_side_delay(((insn >> 8) & 0x1F) as u8, cfg);

    let pc_set = exec_op(st, cfg, opcode as u8, operand, gpio_in);

    // Side-set applies even when the instruction stalled, AFTER the op's
    // own pin writes (asserted side-set wins on a shared pin this cycle).
    if let Some(ss_val) = sideset {
        let value_bits = if cfg.side_en { cfg.side_count - 1 } else { cfg.side_count };
        if value_bits > 0 {
            if cfg.side_pindir {
                write_pin_field(&mut st.dir_latch, ss_val as u32, cfg.sideset_base, value_bits);
            } else {
                write_pin_field(&mut st.out_latch, ss_val as u32, cfg.sideset_base, value_bits);
            }
        }
    }

    if st.stall == Stall::None {
        st.delay_count = delay;
        if !is_forced && !pc_set {
            advance_pc(st, cfg);
        }
    }
}

/// Force-execute an instruction (SMn_INSTR setup idiom): clears any
/// stall/delay, then runs one cycle outside the clock divider.
pub fn force_exec(st: &mut NState, cfg: &NCfg, insn: u16, gpio_in: u32) {
    st.stall = Stall::None;
    st.delay_count = 0;
    st.pending_exec = Some(insn);
    step(st, cfg, gpio_in);
}

/// Execute one decoded instruction. Returns `true` when the instruction
/// set PC directly (taken JMP, OUT PC, MOV PC). Decode is TOTAL —
/// reserved codes behave exactly as the vendored emulator's fallthrough
/// arms (read 0 / no-op).
fn exec_op(st: &mut NState, cfg: &NCfg, opcode: u8, operand: u8, gpio_in: u32) -> bool {
    match opcode {
        // JMP
        0 => {
            let condition = (operand >> 5) & 0x7;
            let address = operand & 0x1F;
            let take = match condition {
                0 => true,
                1 => st.x == 0,
                2 => {
                    let was_nonzero = st.x != 0;
                    if was_nonzero {
                        st.x = st.x.wrapping_sub(1);
                    }
                    was_nonzero
                }
                3 => st.y == 0,
                4 => {
                    let was_nonzero = st.y != 0;
                    if was_nonzero {
                        st.y = st.y.wrapping_sub(1);
                    }
                    was_nonzero
                }
                5 => st.x != st.y,
                6 => (gpio_in >> (cfg.jmp_pin & 0x1F)) & 1 != 0,
                7 => st.osr_count < cfg.pull_threshold,
                _ => false,
            };
            if take {
                st.pc = address & 0x1F;
            }
            take
        }
        // WAIT
        1 => {
            let polarity = (operand >> 7) & 1 != 0;
            let source = (operand >> 5) & 0x3;
            let index = operand & 0x1F;
            match source {
                0 => {
                    if ((gpio_in >> (index & 31)) & 1 != 0) != polarity {
                        st.stall = Stall::WaitGpio { polarity, index };
                    }
                }
                1 => {
                    let pin = (cfg.in_base as u32 + index as u32) & 31;
                    if ((gpio_in >> pin) & 1 != 0) != polarity {
                        st.stall = Stall::WaitPin { polarity, index };
                    }
                }
                2 => {
                    let irq_idx = resolve_irq_index(index, cfg.sm_id);
                    if ((st.irq_flags >> (irq_idx & 7)) & 1 != 0) == polarity {
                        st.irq_flags &= !(1 << (irq_idx & 7)); // met: auto-clear
                    } else {
                        st.stall = Stall::WaitIrq { polarity, index: irq_idx };
                    }
                }
                _ => {} // JMPPIN stub — no-op, matching the vendored emulator
            }
            false
        }
        // IN
        2 => {
            let source = (operand >> 5) & 0x7;
            let bit_count = { let bc = operand & 0x1F; if bc == 0 { 32 } else { bc } };
            // Pending-autopush flush BEFORE shifting; a full RX FIFO stalls.
            if cfg.autopush && st.isr_count >= cfg.push_threshold {
                if st.rx.is_full() {
                    st.stall = Stall::Push;
                    return false;
                }
                st.rx.push(st.isr);
                st.isr = 0;
                st.isr_count = 0;
            }
            let src_val = match source {
                0 => gpio_in.rotate_right(cfg.in_base as u32),
                1 => st.x,
                2 => st.y,
                3 => 0,
                6 => st.isr,
                7 => st.osr,
                _ => 0,
            };
            let bc = bit_count as u32;
            let data = if bc >= 32 { src_val } else { src_val & ((1u32 << bc) - 1) };
            if cfg.in_shift_right {
                if bc >= 32 {
                    st.isr = data;
                } else {
                    st.isr = (st.isr >> bc) | (data << (32 - bc));
                }
            } else {
                if bc >= 32 {
                    st.isr = 0;
                } else {
                    st.isr <<= bc;
                }
                st.isr |= data;
            }
            st.isr_count = (st.isr_count + bit_count).min(32);
            // Post-shift autopush: threshold freshly reached and room now.
            if cfg.autopush && st.isr_count >= cfg.push_threshold && !st.rx.is_full() {
                st.rx.push(st.isr);
                st.isr = 0;
                st.isr_count = 0;
            }
            false
        }
        // OUT
        3 => {
            let destination = (operand >> 5) & 0x7;
            let bit_count = { let bc = operand & 0x1F; if bc == 0 { 32 } else { bc } };
            if cfg.autopull && st.osr_count >= cfg.pull_threshold {
                match st.tx.pop() {
                    Some(val) => {
                        st.osr = val;
                        st.osr_count = 0;
                    }
                    None => {
                        st.stall = Stall::Pull;
                        return false;
                    }
                }
            }
            let bc = bit_count as u32;
            let data = if cfg.out_shift_right {
                let d = if bc >= 32 { st.osr } else { st.osr & ((1u32 << bc) - 1) };
                if bc >= 32 {
                    st.osr = 0;
                } else {
                    st.osr >>= bc;
                }
                d
            } else {
                let d = if bc >= 32 { st.osr } else { st.osr >> (32 - bc) };
                if bc >= 32 {
                    st.osr = 0;
                } else {
                    st.osr <<= bc;
                }
                d
            };
            st.osr_count = (st.osr_count + bit_count).min(32);
            let pc_set = destination == 5;
            match destination {
                0 => {
                    let count = cfg.out_count.min(bit_count);
                    write_pin_field(&mut st.out_latch, data, cfg.out_base, count);
                }
                1 => st.x = data,
                2 => st.y = data,
                3 => {}
                4 => {
                    let count = cfg.out_count.min(bit_count);
                    write_pin_field(&mut st.dir_latch, data, cfg.out_base, count);
                }
                5 => st.pc = (data & 0x1F) as u8,
                6 => st.isr = data,
                7 => st.pending_exec = Some(data as u16),
                _ => {}
            }
            pc_set
        }
        // PUSH / PULL
        4 => {
            let is_pull = (operand >> 7) & 1 != 0;
            let if_x = (operand >> 6) & 1 != 0;
            let block = (operand >> 5) & 1 != 0;
            if is_pull {
                if st.tx.is_empty() {
                    if if_x {
                        st.osr = st.x;
                        st.osr_count = 0;
                        return false;
                    }
                    if block {
                        st.stall = Stall::Pull;
                        return false;
                    }
                    st.osr = st.x; // nonblocking empty PULL reads X
                    st.osr_count = 0;
                    return false;
                }
                st.osr = st.tx.pop().unwrap();
                st.osr_count = 0;
            } else {
                if st.rx.is_full() {
                    if if_x {
                        return false;
                    }
                    if block {
                        st.stall = Stall::Push;
                        return false;
                    }
                    // Nonblocking on full FIFO: push drops (FIFO refuses),
                    // ISR still clears below — vendored behavior.
                }
                st.rx.push(st.isr);
                st.isr = 0;
                st.isr_count = 0;
            }
            false
        }
        // MOV
        5 => {
            let destination = (operand >> 5) & 0x7;
            let op = (operand >> 3) & 0x3;
            let source = operand & 0x7;
            let mut val = match source {
                0 => {
                    let raw = gpio_in.rotate_right(cfg.in_base as u32);
                    if cfg.in_count == 0 || cfg.in_count >= 32 {
                        raw
                    } else {
                        raw & ((1u32 << cfg.in_count) - 1)
                    }
                }
                1 => st.x,
                2 => st.y,
                3 => 0,
                5 => {
                    let level = if cfg.status_sel { st.rx.level() } else { st.tx.level() };
                    if level < cfg.status_n { u32::MAX } else { 0 }
                }
                6 => st.isr,
                7 => st.osr,
                _ => 0,
            };
            val = match op {
                1 => !val,
                2 => val.reverse_bits(),
                _ => val,
            };
            let pc_set = destination == 5;
            match destination {
                0 => write_pin_field(&mut st.out_latch, val, cfg.out_base, cfg.out_count),
                1 => st.x = val,
                2 => st.y = val,
                3 => write_pin_field(&mut st.dir_latch, val, cfg.out_base, cfg.out_count),
                4 => st.pending_exec = Some(val as u16),
                5 => st.pc = (val & 0x1F) as u8,
                6 => st.isr = val,
                7 => st.osr = val,
                _ => {}
            }
            pc_set
        }
        // IRQ
        6 => {
            let clear = (operand >> 6) & 1 != 0;
            let wait = (operand >> 5) & 1 != 0;
            let irq_num = resolve_irq_index(operand & 0x1F, cfg.sm_id);
            if clear {
                st.irq_flags &= !(1 << (irq_num & 7));
            } else {
                st.irq_flags |= 1 << (irq_num & 7);
                if wait {
                    st.stall = Stall::IrqWait { index: irq_num };
                }
            }
            false
        }
        // SET
        _ => {
            let destination = (operand >> 5) & 0x7;
            let data = operand & 0x1F;
            match destination {
                0 => write_pin_field(&mut st.out_latch, data as u32, cfg.set_base, cfg.set_count),
                1 => st.x = data as u32,
                2 => st.y = data as u32,
                4 => write_pin_field(&mut st.dir_latch, data as u32, cfg.set_base, cfg.set_count),
                _ => {}
            }
            false
        }
    }
}

/// Compose the GPIO word the SM (and the observer) sees this cycle:
/// PIO-driven pins where OE is set, external stimulus overriding on
/// stimulated pins, 0 elsewhere. Mirrors the emulator's single-block
/// `compose_released_pins` (SIO contributes nothing in harness runs).
#[inline]
pub fn compose(st: &NState, ext_mask: u32, ext_value: u32) -> u32 {
    let driven = st.out_latch & st.dir_latch;
    (driven & !ext_mask) | (ext_value & ext_mask)
}

/// Per-cycle external input stimulus for [`run_with_stim`]: `values[i]`
/// is applied from cycle `i` on (the last entry holds — the harness's
/// latched `set_pin` semantics).
#[derive(Debug, Clone, Default)]
pub struct Stim {
    pub mask: u32,
    pub values: Vec<u32>,
}

impl Stim {
    fn at(&self, cycle: usize) -> u32 {
        if self.values.is_empty() {
            0
        } else {
            self.values[cycle.min(self.values.len() - 1)]
        }
    }
}

/// Threshold above which inputs are streamed into the TX FIFO instead of
/// pre-loaded — must equal `run::TX_FIFO_DEPTH` for trace parity.
const TX_FIFO_DEPTH: usize = 4;

/// Evaluate `program` under `spec`, returning the same per-cycle
/// `trace_pads` encoding as [`crate::run::run`] (bit `j` = level of
/// `capture_pins[j]`, bit `16+j` = its output-enable). Byte-identical to
/// the vendored-emulator path — pinned by `tests/narrow_diff.rs`.
pub fn run(program: &Program, spec: &RunSpec) -> Vec<u32> {
    run_with_stim(program, spec, &Stim::default())
}

/// [`run`] plus external pin stimulus (the vendored path drives this via
/// `Pio::set_pin`, which the FIFO-input-only `run::run` never uses; RX
/// specs need it).
pub fn run_with_stim(program: &Program, spec: &RunSpec, stim: &Stim) -> Vec<u32> {
    // Autopull word-boundary pad — mirror of `run::padded`.
    let mut inputs = spec.inputs.clone();
    if program.config.shift.autopull && spec.autopull_pad > 0 {
        inputs.extend(std::iter::repeat(0).take(spec.autopull_pad as usize));
    }

    let cfg = NCfg::from_program(program, spec.sm as u8);
    let mut st = NState::new(&cfg);

    // Setup: pin directions (the harness's exec'd `SET PINDIRS, 1` is
    // semantically a single direction-latch bit), then FIFO pre-load for
    // short input lists (parity with `run::run`'s fast path).
    for &p in &spec.output_pins {
        st.dir_latch |= 1u32 << p;
    }
    let streaming = inputs.len() > TX_FIFO_DEPTH;
    let mut next = 0usize;
    if !streaming {
        for &w in &inputs {
            st.tx.push(w);
        }
    }

    let mut out = Vec::with_capacity(spec.cycles as usize);
    for cycle in 0..spec.cycles as usize {
        if streaming {
            while next < inputs.len() && !st.tx.is_full() {
                st.tx.push(inputs[next]);
                next += 1;
            }
        }
        let ext = stim.at(cycle);
        let gpio_in = compose(&st, stim.mask, ext);
        if clock_tick(&mut st, &cfg) {
            step(&mut st, &cfg, gpio_in);
        }
        let levels = compose(&st, stim.mask, ext);
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
