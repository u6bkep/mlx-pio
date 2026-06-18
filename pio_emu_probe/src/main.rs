//! Hands-on probe of `rp2350-emu` for closed-loop PIO development.
//!
//! Goal: demonstrate the loop an agent would run --
//!   assemble .pio text  ->  load into emulator  ->  drive pin stimulus
//!   ->  step cycle-by-cycle  ->  capture an observable text trace.

use rp2350_emu::{Config, EmulatorBuilder, Emulator};
use std::sync::atomic::Ordering;

const PIO0_BASE: u32 = 0x5020_0000;
// SM0 register offsets (RP2350 datasheet ch.11)
const CTRL: u32 = 0x000;
const RXF0: u32 = 0x020;
const INSTR_MEM0: u32 = 0x048;
const SM0_EXECCTRL: u32 = 0x0CC;
const SM0_SHIFTCTRL: u32 = 0x0D0;
const SM0_INSTR: u32 = 0x0D8;
const SM0_PINCTRL: u32 = 0x0DC;

fn w(emu: &mut Emulator, off: u32, val: u32) {
    emu.bus.write32(PIO0_BASE + off, val, 0);
}
fn r(emu: &mut Emulator, off: u32) -> u32 {
    emu.bus.read32(PIO0_BASE + off, 0)
}

fn new_emu() -> Emulator {
    let mut emu = EmulatorBuilder::new(Config::default())
        .step_quantum(1) // one emu.step() == one PIO cycle
        .build()
        .unwrap();
    // Release PIO0 from reset so it actually ticks.
    emu.bus.resets_state &= !(1u32 << rp2350_emu::bus::RESET_PIO0);
    emu
}

fn load(emu: &mut Emulator, code: &[u16]) {
    for (i, &insn) in code.iter().enumerate() {
        w(emu, INSTR_MEM0 + (i as u32) * 4, insn as u32);
    }
}

/// Drive a low-bank GPIO input level. The merge into `gpio_in` happens
/// inside `step()` at end-of-quantum, so the SM sees this on the next step.
fn set_input(emu: &mut Emulator, pin: u8, hi: bool) {
    let bit = 1u32 << pin;
    emu.bus.gpio_external_mask |= bit; // overlay this pin from the harness
    let cur = emu.bus.gpio_external_in.load(Ordering::Relaxed);
    let next = if hi { cur | bit } else { cur & !bit };
    emu.bus.gpio_external_in.store(next, Ordering::Relaxed);
}

fn main() {
    demo_output_waveform();
    println!();
    demo_input_driven_capture();
}

// ---------------------------------------------------------------------------
// Demo A: an OUTPUT program. Toggle a pin; capture the waveform as text.
// ---------------------------------------------------------------------------
fn demo_output_waveform() {
    println!("== Demo A: output waveform (square wave on GPIO0) ==");

    let prog = pio::pio_asm!(
        "
        .wrap_target
            set pins, 1 [1]
            set pins, 0 [1]
        .wrap
        "
    );
    let code: Vec<u16> = prog.program.code.iter().copied().collect();
    println!("assembled {} instructions: {:04X?}", code.len(), code);

    let mut emu = new_emu();
    load(&mut emu, &code);

    // PINCTRL: set_base=0, set_count=1
    w(&mut emu, SM0_PINCTRL, (1 << 26) | (0 << 5));
    // EXECCTRL wrap: top = last instr index, bottom = 0
    w(&mut emu, SM0_EXECCTRL, ((code.len() as u32 - 1) << 12) | (0 << 7));
    // Force pindir out on pin 0: SET PINDIRS,1 = 0xE081
    w(&mut emu, SM0_INSTR, 0xE081);
    w(&mut emu, CTRL, 0x1); // enable SM0

    let mut trace = String::new();
    for _ in 0..16 {
        emu.step().unwrap();
        trace.push(if emu.gpio_read(0) { '#' } else { '_' });
    }
    println!("GPIO0 trace : {}", trace);
    println!("pc_visits   : {:?}", &emu.bus.pio[0].sm[0].pc_visits()[..code.len()]);
}

// ---------------------------------------------------------------------------
// Demo B: an INPUT program -- the shape that matters for 10BASE-T1S RX.
// Wait for the pin high, sample it into ISR, autopush to RX FIFO.
// We drive a pin waveform from the test side and read the captured words.
// ---------------------------------------------------------------------------
fn demo_input_driven_capture() {
    println!("== Demo B: input-driven sampling + autopush to RX FIFO ==");

    // Sample the input pin once every 3 cycles (1 exec + [2] delay),
    // shifting LSB-first, autopush a 4-bit word at threshold 4.
    let prog = pio::pio_asm!(
        "
        .wrap_target
            in pins, 1 [2]
        .wrap
        "
    );
    let code: Vec<u16> = prog.program.code.iter().copied().collect();
    println!("assembled {} instructions: {:04X?}", code.len(), code);

    let mut emu = new_emu();
    load(&mut emu, &code);

    // PINCTRL: in_base = 0.
    w(&mut emu, SM0_PINCTRL, 0 << 10);
    w(&mut emu, SM0_EXECCTRL, ((code.len() as u32 - 1) << 12) | (0 << 7));
    // SHIFTCTRL: IN_SHIFTDIR right (bit18, LSB-first), autopush (bit16),
    // push threshold = 4 (bits[24:20]).
    let shiftctrl = (1 << 18) | (1 << 16) | (4 << 20);
    w(&mut emu, SM0_SHIFTCTRL, shiftctrl);
    w(&mut emu, CTRL, 0x1);

    // Drive a 4-bit pattern. The input merge lands at end-of-step, so we
    // prime the first bit, then set each subsequent bit during the prior
    // bit's delay window -- the alignment work the closed loop iterates on.
    let bits = [1u8, 0, 1, 1];
    emu.bus.gpio_external_mask |= 1;
    emu.bus.gpio_in.store(bits[0] as u32, Ordering::Relaxed); // prime cycle 0
    emu.bus.gpio_external_in.store(bits[0] as u32, Ordering::Relaxed);

    let mut log = String::new();
    for i in 0..bits.len() {
        emu.step().unwrap(); // `in pins,1` samples the aligned bit
        let sampled = emu.gpio_read(0);
        if i + 1 < bits.len() {
            set_input(&mut emu, 0, bits[i + 1] == 1); // present next bit now
        }
        emu.step().unwrap();
        emu.step().unwrap(); // [2] delay
        log.push_str(&format!(
            "  cycle {}: drove={} sampled={} -> isr=0x{:X} shift_cnt={} rx_level={}\n",
            i, bits[i], sampled as u8,
            emu.bus.pio[0].sm[0].isr_value(),
            emu.bus.pio[0].sm[0].isr_shift_count(),
            emu.bus.pio[0].sm[0].rx_fifo_level(),
        ));
    }
    print!("{}", log);

    // Shift-right fills the ISR from the MSB down, so 4 bits (1,0,1,1
    // in sample order) land in the top nibble: 0b1101 << 28 = 0xD000_0000.
    let expected = 0xD000_0000u32;

    while !emu.bus.pio[0].sm[0].rx_fifo_empty() {
        let word = r(&mut emu, RXF0);
        let ok = if word == expected { "OK" } else { "MISMATCH" };
        println!(
            "  RX FIFO word: 0x{:08X}  expected 0x{:08X} [{}]",
            word, expected, ok
        );
    }
    println!(
        "diagnostics : rx_push_success={} rx_fifo_drops={} stall_cycles={}",
        emu.bus.pio[0].sm[0].rx_push_success(),
        emu.bus.pio[0].sm[0].rx_fifo_drops(),
        emu.bus.pio[0].sm[0].stall_cycles(),
    );
}
