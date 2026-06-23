//! A thin, test-oriented harness around `rp2350-emu` for closed-loop
//! development of RP2350 PIO programs.
//!
//! The emulator's native interface is register-poke at datasheet
//! addresses. This harness hides that behind a small typed API so a
//! test (or a coding agent) can:
//!
//!   1. load an assembled program,
//!   2. configure one state machine with named fields,
//!   3. force-execute setup instructions (the `exec_instr` idiom),
//!   4. drive GPIO input stimulus cycle-by-cycle,
//!   5. step and observe pins, FIFOs, PC, and diagnostic counters.
//!
//! Everything is single-block / single-SM focused, but multiple `Pio`
//! handles can share one `Emulator` for cross-SM IRQ scenarios (see
//! [`Pio::from_shared`]).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;

use rp2350_emu::{Config, Emulator, EmulatorBuilder};

/// PIO block base addresses (RP2350 datasheet §11).
const PIO_BASE: [u32; 3] = [0x5020_0000, 0x5030_0000, 0x5040_0000];

// Block-level register offsets.
const CTRL: u32 = 0x000;
const TXF0: u32 = 0x010;
const RXF0: u32 = 0x020;
const INSTR_MEM0: u32 = 0x048;
// SM register file: starts at 0x0C8, stride 0x18.
const SM0: u32 = 0x0C8;
const SM_STRIDE: u32 = 0x18;
const R_CLKDIV: u32 = 0x00;
const R_EXECCTRL: u32 = 0x04;
const R_SHIFTCTRL: u32 = 0x08;
const R_INSTR: u32 = 0x10;
const R_PINCTRL: u32 = 0x14;

/// Shift direction for ISR/OSR.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ShiftDir {
    Left,
    Right,
}

/// SM0_PINCTRL fields (counts/bases). All default to 0.
#[derive(Default, Clone, Copy)]
pub struct PinCtrl {
    pub out_base: u8,
    pub out_count: u8,
    pub set_base: u8,
    pub set_count: u8,
    pub in_base: u8,
    pub sideset_base: u8,
    pub sideset_count: u8,
}

impl PinCtrl {
    fn encode(self) -> u32 {
        ((self.sideset_count as u32 & 0x7) << 29)
            | ((self.set_count as u32 & 0x7) << 26)
            | ((self.out_count as u32 & 0x3F) << 20)
            | ((self.in_base as u32 & 0x1F) << 15)
            | ((self.sideset_base as u32 & 0x1F) << 10)
            | ((self.set_base as u32 & 0x1F) << 5)
            | (self.out_base as u32 & 0x1F)
    }
}

/// SM0_SHIFTCTRL fields.
#[derive(Clone, Copy)]
pub struct ShiftCtrl {
    pub autopush: bool,
    pub autopull: bool,
    pub push_threshold: u8, // 1..=32 (32 encoded as 0)
    pub pull_threshold: u8, // 1..=32 (32 encoded as 0)
    pub in_dir: ShiftDir,
    pub out_dir: ShiftDir,
    pub fjoin_rx: bool,
    pub fjoin_tx: bool,
}

impl Default for ShiftCtrl {
    fn default() -> Self {
        ShiftCtrl {
            autopush: false,
            autopull: false,
            push_threshold: 32,
            pull_threshold: 32,
            in_dir: ShiftDir::Right,
            out_dir: ShiftDir::Right,
            fjoin_rx: false,
            fjoin_tx: false,
        }
    }
}

impl ShiftCtrl {
    fn encode(self) -> u32 {
        let thr = |t: u8| (t as u32) & 0x1F; // 32 -> 0
        ((self.fjoin_rx as u32) << 31)
            | ((self.fjoin_tx as u32) << 30)
            | (thr(self.pull_threshold) << 25)
            | (thr(self.push_threshold) << 20)
            | (((self.out_dir == ShiftDir::Right) as u32) << 19)
            | (((self.in_dir == ShiftDir::Right) as u32) << 18)
            | ((self.autopull as u32) << 17)
            | ((self.autopush as u32) << 16)
    }
}

/// Relocate a PIO instruction by `offset`: only JMP (opcode 000) carries
/// a target address (low 5 bits) that must shift when the program is
/// loaded away from address 0. All other opcodes are position-independent.
fn relocate(insn: u16, offset: u8) -> u16 {
    if (insn >> 13) & 0x7 == 0 {
        let addr = (insn & 0x1F) + offset as u16;
        (insn & !0x1F) | (addr & 0x1F)
    } else {
        insn
    }
}

/// A handle to one state machine on one PIO block, sharing an emulator.
pub struct Pio {
    emu: Rc<RefCell<Emulator>>,
    block: usize,
    sm: usize,
    wrap_bottom: u8,
    wrap_top: u8,
    jmp_pin: u8,
    side_en: bool,
    side_pindir: bool,
    pinctrl: PinCtrl,
    start_pc: u8,
}

impl Pio {
    /// Create a fresh emulator with one SM handle. PIO `block` is
    /// released from reset and the emulator steps one cycle per
    /// [`Pio::step`] (`step_quantum = 1`).
    pub fn new(block: usize, sm: usize) -> Self {
        let mut emu = EmulatorBuilder::new(Config::default())
            .step_quantum(1)
            .build()
            .unwrap();
        emu.bus.resets_state &= !(1u32 << (rp2350_emu::bus::RESET_PIO0 as u32 + block as u32));
        Self::from_shared(Rc::new(RefCell::new(emu)), block, sm)
    }

    /// Attach another SM handle (same or different block) to a shared
    /// emulator — for cross-SM IRQ handshake tests.
    pub fn from_shared(emu: Rc<RefCell<Emulator>>, block: usize, sm: usize) -> Self {
        {
            let mut e = emu.borrow_mut();
            e.bus.resets_state &= !(1u32 << (rp2350_emu::bus::RESET_PIO0 as u32 + block as u32));
        }
        Pio {
            emu,
            block,
            sm,
            wrap_bottom: 0,
            wrap_top: 0,
            jmp_pin: 0,
            side_en: false,
            side_pindir: false,
            pinctrl: PinCtrl::default(),
            start_pc: 0,
        }
    }

    /// Shared emulator handle, for constructing sibling SMs.
    pub fn emulator(&self) -> Rc<RefCell<Emulator>> {
        self.emu.clone()
    }

    /// Reset this handle's PIO block and the shared GPIO stimulus to the
    /// power-on state a freshly-built [`Pio::new`] would produce, WITHOUT
    /// rebuilding the [`Emulator`]. This lets one emulator be reused across
    /// many evaluations (the superoptimizer hot path), skipping the
    /// ~200µs `Bus`/core allocation of a rebuild.
    ///
    /// Clears all per-evaluation PIO state (via `PioBlock::reset`): every
    /// SM's PC / X / Y / ISR / OSR / shift counters / clkdiv accumulator /
    /// stall+delay flags / pending forced exec / FIFOs / side-set latches /
    /// pc_visits / stall_cycles; the block's instr_mem, irq_flags, shared
    /// pin value+dir latches, pad_out / pad_oe, enable mask, INT registers.
    /// Also clears the bus-level external GPIO stimulus so input forcing
    /// from a prior eval cannot leak, and resets the harness-side config
    /// shadow (wrap/jmp_pin/sideset/pinctrl/start_pc).
    ///
    /// Leaves the PIO RESET line deasserted (as `Pio::new` did). The CPU
    /// cores/SIO are untouched: with no firmware loaded they never drive
    /// PIO inputs, so they can't affect a captured waveform — verified
    /// byte-identical to the rebuild path in `tests/reset_reuse.rs`.
    ///
    /// Resets only THIS handle's block; multi-block scenarios must reset
    /// each handle.
    pub fn reset(&mut self) -> &mut Self {
        {
            let mut e = self.emu.borrow_mut();
            e.bus.pio[self.block].reset();
            e.bus.gpio_external_in.store(0, Ordering::Relaxed);
            e.bus.gpio_external_mask = 0;
            e.bus.gpio_external_in_hi.store(0, Ordering::Relaxed);
            e.bus.gpio_external_mask_hi = 0;
        }
        self.wrap_bottom = 0;
        self.wrap_top = 0;
        self.jmp_pin = 0;
        self.side_en = false;
        self.side_pindir = false;
        self.pinctrl = PinCtrl::default();
        self.start_pc = 0;
        self
    }

    fn base(&self) -> u32 {
        PIO_BASE[self.block]
    }
    fn sm_reg(&self, r: u32) -> u32 {
        self.base() + SM0 + (self.sm as u32) * SM_STRIDE + r
    }
    fn w(&self, addr: u32, val: u32) {
        self.emu.borrow_mut().bus.write32(addr, val, 0);
    }
    fn r(&self, addr: u32) -> u32 {
        self.emu.borrow_mut().bus.read32(addr, 0)
    }

    /// Load a single program at instruction-memory offset 0 (wrap =
    /// bottom 0, top last). For one SM per block.
    pub fn load(&mut self, code: &[u16]) -> &mut Self {
        let top = code.len().saturating_sub(1) as u8;
        self.load_at(0, code, 0, top)
    }

    /// Load a program at a given instruction-memory offset, relocating
    /// JMP targets and wrap by `offset`. REQUIRED when multiple SMs share
    /// one PIO block: instruction memory is per-block (32 words shared by
    /// all 4 SMs), so each program must occupy a distinct region.
    ///
    /// `wrap_target`/`wrap_source` are the program-relative indices from
    /// the assembler (`prog.program.wrap.target` / `.source`); they are
    /// shifted by `offset` here. The SM's start PC is set to `offset`.
    pub fn load_at(
        &mut self,
        offset: u8,
        code: &[u16],
        wrap_target: u8,
        wrap_source: u8,
    ) -> &mut Self {
        assert!(
            offset as usize + code.len() <= 32,
            "program at offset {offset} (+{} insns) overflows the 32-word PIO instruction memory",
            code.len()
        );
        for (i, &insn) in code.iter().enumerate() {
            let relocated = relocate(insn, offset);
            self.w(self.base() + INSTR_MEM0 + (offset as u32 + i as u32) * 4, relocated as u32);
        }
        self.wrap_bottom = wrap_target + offset;
        self.wrap_top = wrap_source + offset;
        self.start_pc = offset;
        self.push_execctrl();
        self
    }

    /// Override the wrap target/top (instruction indices).
    pub fn wrap(&mut self, bottom: u8, top: u8) -> &mut Self {
        self.wrap_bottom = bottom;
        self.wrap_top = top;
        self.push_execctrl();
        self
    }

    /// Set the pin that JMP PIN tests. (WAIT PIN / IN PINS use IN_BASE,
    /// configured via [`Pio::pinctrl`].)
    pub fn jmp_pin(&mut self, pin: u8) -> &mut Self {
        self.jmp_pin = pin;
        self.push_execctrl();
        self
    }

    /// Configure side-set mode. `opt` => the side-set MSB is an enable
    /// bit (`.side_set N opt`); `pindir` => side-set drives PINDIRS not
    /// PINS. The side-set base/count live in [`PinCtrl`].
    pub fn sideset(&mut self, opt: bool, pindir: bool) -> &mut Self {
        self.side_en = opt;
        self.side_pindir = pindir;
        self.push_execctrl();
        self
    }

    fn push_execctrl(&self) {
        let v = ((self.side_en as u32) << 30)
            | ((self.side_pindir as u32) << 29)
            | ((self.jmp_pin as u32 & 0x1F) << 24)
            | ((self.wrap_top as u32 & 0x1F) << 12)
            | ((self.wrap_bottom as u32 & 0x1F) << 7);
        self.w(self.sm_reg(R_EXECCTRL), v);
    }

    pub fn pinctrl(&mut self, pc: PinCtrl) -> &mut Self {
        self.pinctrl = pc;
        self.w(self.sm_reg(R_PINCTRL), pc.encode());
        self
    }

    pub fn shiftctrl(&mut self, sc: ShiftCtrl) -> &mut Self {
        self.w(self.sm_reg(R_SHIFTCTRL), sc.encode());
        self
    }

    /// Set the integer (and optional fractional) clock divider.
    pub fn clkdiv(&mut self, int: u16, frac: u8) -> &mut Self {
        self.w(self.sm_reg(R_CLKDIV), ((int as u32) << 16) | ((frac as u32) << 8));
        self
    }

    /// Force-execute an instruction immediately (the `exec_instr` idiom
    /// used for SM setup). Works whether or not the SM is enabled.
    pub fn exec(&mut self, insn: u16) -> &mut Self {
        self.w(self.sm_reg(R_INSTR), insn as u32);
        self
    }

    /// Enable this state machine. First forces the PC to the program's
    /// start offset (programs loaded at a nonzero offset must not begin
    /// executing at address 0).
    pub fn enable(&mut self) -> &mut Self {
        if self.start_pc != 0 {
            self.exec(self.start_pc as u16); // JMP <start_pc> (cond=always)
        }
        let cur = self.r(self.base() + CTRL);
        self.w(self.base() + CTRL, cur | (1 << self.sm));
        self
    }

    /// Mark a pin as a PIO output, equivalent to `set_pin_dirs(Out, ..)`.
    /// `pad_oe` is recomputed from `shared_pin_dirs` every merge, so we
    /// can't poke it directly — instead we force-execute `SET PINDIRS, 1`
    /// with SET_BASE pointed at `pin` (then restore PINCTRL), exactly as
    /// the firmware drives pin directions. Call before [`Pio::enable`].
    pub fn set_output(&mut self, pin: u8) -> &mut Self {
        let saved = self.pinctrl;
        let tmp = PinCtrl { set_base: pin, set_count: 1, ..Default::default() };
        self.w(self.sm_reg(R_PINCTRL), tmp.encode());
        self.exec(0xE081); // SET PINDIRS, 1
        self.w(self.sm_reg(R_PINCTRL), saved.encode());
        self
    }

    // --- stimulus ---------------------------------------------------

    /// Drive a low-bank (GPIO 0..31) input pin. The merge into the SM's
    /// view happens at end-of-step, so the SM sees it on the next step.
    pub fn set_pin(&mut self, pin: u8, hi: bool) -> &mut Self {
        let mut e = self.emu.borrow_mut();
        let bit = 1u32 << pin;
        e.bus.gpio_external_mask |= bit;
        let cur = e.bus.gpio_external_in.load(Ordering::Relaxed);
        let next = if hi { cur | bit } else { cur & !bit };
        e.bus.gpio_external_in.store(next, Ordering::Relaxed);
        drop(e);
        self
    }

    // --- stepping ---------------------------------------------------

    /// Advance one PIO cycle.
    pub fn step(&mut self) {
        self.emu.borrow_mut().step().unwrap();
    }

    /// Advance `n` cycles.
    pub fn steps(&mut self, n: u64) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Step until `pred(self)` is true or `max` cycles elapse. Returns
    /// cycles stepped, or `None` on timeout.
    pub fn step_until(&mut self, max: u64, mut pred: impl FnMut(&Pio) -> bool) -> Option<u64> {
        for i in 0..max {
            if pred(self) {
                return Some(i);
            }
            self.step();
        }
        if pred(self) {
            Some(max)
        } else {
            None
        }
    }

    // --- observation ------------------------------------------------

    pub fn gpio(&self, pin: u8) -> bool {
        self.emu.borrow().gpio_read(pin)
    }

    /// The PIO-driven output-enable (direction) of a low-bank pin on this
    /// block: true = the PIO is actively driving the pin as an output this
    /// cycle. Read from the block's merged `pad_oe`.
    pub fn gpio_oe(&self, pin: u8) -> bool {
        (self.emu.borrow().bus.pio[self.block].pad_oe >> pin) & 1 != 0
    }

    /// Capture `cycles` of a pin as a `_`/`#` string, stepping as it goes.
    pub fn trace_pin(&mut self, pin: u8, cycles: u64) -> String {
        let mut s = String::with_capacity(cycles as usize);
        for _ in 0..cycles {
            self.step();
            s.push(if self.gpio(pin) { '#' } else { '_' });
        }
        s
    }

    /// Capture `cycles` of several pins **synchronously**: one bitmask per
    /// cycle where bit `j` is the level of `pins[j]` that cycle. All pins
    /// are sampled at the same cycle (after each step), so a multi-signal
    /// waveform (e.g. SPI clock + data) stays phase-aligned — unlike
    /// calling [`Pio::trace_pin`] per pin, which re-runs different cycles.
    /// This is the capture the superoptimizer scores against a reference.
    pub fn trace_pins(&mut self, pins: &[u8], cycles: u64) -> Vec<u32> {
        let mut out = Vec::with_capacity(cycles as usize);
        for _ in 0..cycles {
            self.step();
            let mut mask = 0u32;
            for (j, &p) in pins.iter().enumerate() {
                if self.gpio(p) {
                    mask |= 1 << j;
                }
            }
            out.push(mask);
        }
        out
    }

    /// Capture `cycles` of several pins encoding BOTH level and direction:
    /// bit `j` is the level of `pins[j]`, bit `16 + j` is its output-enable.
    /// Scoring against this rejects programs that fake the observed level by
    /// toggling pin direction instead of driving values — a real transmitter
    /// keeps its pins driven. Up to 16 pins.
    pub fn trace_pads(&mut self, pins: &[u8], cycles: u64) -> Vec<u32> {
        assert!(pins.len() <= 16, "trace_pads supports up to 16 pins");
        // Fast path: borrow the emulator once and step only the PIO each cycle
        // (`step_pio_only` skips the CPU cores and non-PIO peripherals — ticket
        // 004). Byte-identical to `trace_pads_full` whenever the captured output
        // doesn't depend on the cores; `pio_superopt`'s `fast_step_matches_full`
        // test asserts that on real and random programs.
        let block = self.block;
        let mut e = self.emu.borrow_mut();
        let mut out = Vec::with_capacity(cycles as usize);
        for _ in 0..cycles {
            e.step_pio_only();
            let mut w = 0u32;
            for (j, &p) in pins.iter().enumerate() {
                if e.gpio_read(p) {
                    w |= 1 << j;
                }
                if (e.bus.pio[block].pad_oe >> p) & 1 != 0 {
                    w |= 1 << (16 + j);
                }
            }
            out.push(w);
        }
        out
    }

    /// Full-fidelity `trace_pads`: steps the entire emulator (CPU cores + all
    /// peripherals) each cycle. The reference the fast [`Self::trace_pads`] is
    /// validated against, and the path to use if a capture ever depends on the
    /// cores driving the PIO (firmware-in-the-loop scenarios).
    pub fn trace_pads_full(&mut self, pins: &[u8], cycles: u64) -> Vec<u32> {
        assert!(pins.len() <= 16, "trace_pads supports up to 16 pins");
        let mut out = Vec::with_capacity(cycles as usize);
        for _ in 0..cycles {
            self.step();
            let mut w = 0u32;
            for (j, &p) in pins.iter().enumerate() {
                if self.gpio(p) {
                    w |= 1 << j;
                }
                if self.gpio_oe(p) {
                    w |= 1 << (16 + j);
                }
            }
            out.push(w);
        }
        out
    }

    pub fn pc(&self) -> u8 {
        self.emu.borrow().bus.pio[self.block].sm[self.sm].pc()
    }
    pub fn isr(&self) -> u32 {
        self.emu.borrow().bus.pio[self.block].sm[self.sm].isr_value()
    }
    pub fn tx_full(&self) -> bool {
        self.emu.borrow().bus.pio[self.block].sm[self.sm].tx_fifo_full()
    }
    pub fn rx_empty(&self) -> bool {
        self.emu.borrow().bus.pio[self.block].sm[self.sm].rx_fifo_empty()
    }
    pub fn rx_level(&self) -> u8 {
        self.emu.borrow().bus.pio[self.block].sm[self.sm].rx_fifo_level()
    }
    pub fn rx_push_success(&self) -> u64 {
        self.emu.borrow().bus.pio[self.block].sm[self.sm].rx_push_success()
    }
    pub fn rx_fifo_drops(&self) -> u64 {
        self.emu.borrow().bus.pio[self.block].sm[self.sm].rx_fifo_drops()
    }
    pub fn stall_cycles(&self) -> u64 {
        self.emu.borrow().bus.pio[self.block].sm[self.sm].stall_cycles()
    }
    pub fn pc_visits(&self) -> [u64; 32] {
        *self.emu.borrow().bus.pio[self.block].sm[self.sm].pc_visits()
    }

    /// Pop one word from the RX FIFO (via the RXF register), or `None`.
    pub fn rx_pop(&mut self) -> Option<u32> {
        if self.rx_empty() {
            return None;
        }
        Some(self.r(self.base() + RXF0 + (self.sm as u32) * 4))
    }

    /// Push one word to the TX FIFO (via the TXF register).
    pub fn tx_push(&mut self, word: u32) {
        self.w(self.base() + TXF0 + (self.sm as u32) * 4, word);
    }
}
