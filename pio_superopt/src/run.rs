//! Run a [`Program`] in the emulator harness and capture its output
//! waveform. This is the bridge from the IR genome to a scoreable signal.

use crate::program::{Program, ShiftDir};
use pio_harness::{PinCtrl, Pio, ShiftCtrl, ShiftDir as HDir};

/// What to feed a program and what to observe.
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub block: usize,
    pub sm: usize,
    /// Words pushed to the TX FIFO before stepping.
    pub inputs: Vec<u32>,
    /// Pins to mark as PIO outputs (data, clock, …).
    pub output_pins: Vec<u8>,
    /// Pins to capture; bit `j` of each sample is `capture_pins[j]`.
    pub capture_pins: Vec<u8>,
    /// Number of PIO cycles to run/capture.
    pub cycles: u64,
}

fn hdir(d: ShiftDir) -> HDir {
    match d {
        ShiftDir::Left => HDir::Left,
        ShiftDir::Right => HDir::Right,
    }
}

/// Assemble, configure, run, and return the per-cycle waveform (one
/// bitmask per cycle, `capture_pins`-indexed). Deterministic.
pub fn run(program: &Program, spec: &RunSpec) -> Vec<u32> {
    let mut pio = Pio::new(spec.block, spec.sm);

    let code = program.assemble();
    // Slot index == instruction address: load at offset 0, no relocation.
    pio.load_at(0, &code, program.wrap_bottom, program.wrap_top);

    let c = &program.config;
    pio.pinctrl(PinCtrl {
        out_base: c.pins.out_base,
        out_count: c.pins.out_count,
        set_base: c.pins.set_base,
        set_count: c.pins.set_count,
        in_base: c.pins.in_base,
        sideset_base: c.pins.sideset_base,
        sideset_count: c.side.count,
    });
    pio.sideset(c.side.en, c.side_pindir);
    pio.jmp_pin(c.jmp_pin);
    pio.clkdiv(c.clkdiv_int, c.clkdiv_frac);
    pio.shiftctrl(ShiftCtrl {
        autopush: c.shift.autopush,
        autopull: c.shift.autopull,
        push_threshold: c.shift.push_threshold,
        pull_threshold: c.shift.pull_threshold,
        in_dir: hdir(c.shift.in_dir),
        out_dir: hdir(c.shift.out_dir),
        fjoin_rx: c.shift.fjoin_rx,
        fjoin_tx: c.shift.fjoin_tx,
    });

    for &p in &spec.output_pins {
        pio.set_output(p);
    }
    pio.enable();
    for &w in &spec.inputs {
        pio.tx_push(w);
    }

    pio.trace_pins(&spec.capture_pins, spec.cycles)
}
