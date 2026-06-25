//! Run a [`Program`] in the emulator harness and capture its output
//! waveform. This is the bridge from the IR genome to a scoreable signal.

use std::cell::RefCell;

use crate::program::{Program, ShiftDir};
use pio_harness::{PinCtrl, Pio, ShiftCtrl, ShiftDir as HDir};

thread_local! {
    /// One emulator reused across all evaluations on this thread. Rebuilding
    /// it (`Pio::new`) costs ~200µs and dominates eval time; resetting it is
    /// ~17x cheaper. `Pio::reset` is verified to produce byte-identical
    /// results to a fresh build (pio_harness `tests/reset_reuse.rs`).
    static RUNNER: RefCell<Option<(usize, usize, Pio)>> = const { RefCell::new(None) };
}

/// What to feed a program and what to observe.
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub block: usize,
    pub sm: usize,
    /// Words pushed to the TX FIFO before stepping.
    pub inputs: Vec<u32>,
    /// Pins to mark as PIO outputs (data, clock, …).
    pub output_pins: Vec<u8>,
    /// Pins to capture. Each per-cycle sample packs, for pin `j`, its level
    /// in bit `j` and its output-enable (direction) in bit `16 + j`.
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

/// PIO TX FIFO depth (RP2350, unjoined). A spec with more `inputs` than this
/// can't be pre-loaded — the surplus overflows and is dropped — so such specs
/// must be *streamed* (refill the FIFO as the SM drains it). See [`run`].
const TX_FIFO_DEPTH: usize = 4;

/// Assemble, configure, run, and return the per-cycle waveform (one
/// bitmask per cycle, `capture_pins`-indexed). Deterministic.
///
/// Specs whose `inputs` fit the TX FIFO are pre-loaded and stepped with the
/// fused fast path. Longer specs — arbitrary-length random data fed through the
/// reference encoder, the multi-corpus training workload — are **streamed**:
/// the FIFO is refilled every cycle as the SM consumes it, equivalent to an
/// infinite FIFO. Identical output to the fast path on any spec that fits
/// (pinned by `run::tests::stream_matches_fast`).
pub fn run(program: &Program, spec: &RunSpec) -> Vec<u32> {
    RUNNER.with(|cell| {
        let mut slot = cell.borrow_mut();
        // Rebuild only if the target block/SM changed (it doesn't, mid-search).
        if !matches!(&*slot, Some((b, s, _)) if *b == spec.block && *s == spec.sm) {
            *slot = Some((spec.block, spec.sm, Pio::new(spec.block, spec.sm)));
        }
        let pio = &mut slot.as_mut().unwrap().2;
        pio.reset();
        if spec.inputs.len() > TX_FIFO_DEPTH {
            stream_on(pio, program, spec)
        } else {
            run_on(pio, program, spec)
        }
    })
}

/// Run `program`/`spec` streaming the inputs through the TX FIFO regardless of
/// length — the explicit entry point [`run`] auto-selects for long specs. Public
/// so the equivalence test can compare it to the fast path on a short spec.
pub fn run_streaming(program: &Program, spec: &RunSpec) -> Vec<u32> {
    RUNNER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if !matches!(&*slot, Some((b, s, _)) if *b == spec.block && *s == spec.sm) {
            *slot = Some((spec.block, spec.sm, Pio::new(spec.block, spec.sm)));
        }
        let pio = &mut slot.as_mut().unwrap().2;
        pio.reset();
        stream_on(pio, program, spec)
    })
}

/// Configure (without pre-loading inputs), then step cycle-by-cycle, refilling
/// the TX FIFO from `spec.inputs` whenever it has room. With a refill before
/// every step the SM never underflows while data remains, so the captured
/// waveform equals what an unbounded FIFO would produce.
fn stream_on(pio: &mut Pio, program: &Program, spec: &RunSpec) -> Vec<u32> {
    configure_regs(pio, program, spec);
    let inputs = &spec.inputs;
    let mut next = 0usize;
    let mut out = Vec::with_capacity(spec.cycles as usize);
    for _ in 0..spec.cycles {
        while next < inputs.len() && !pio.tx_full() {
            pio.tx_push(inputs[next]);
            next += 1;
        }
        out.push(pio.trace_pads(&spec.capture_pins, 1)[0]);
    }
    out
}

/// Configure and run an already-reset `Pio`, returning the waveform.
fn run_on(pio: &mut Pio, program: &Program, spec: &RunSpec) -> Vec<u32> {
    configure(pio, program, spec);
    pio.trace_pads(&spec.capture_pins, spec.cycles)
}

/// Like [`run`], but captures via the full-fidelity emulator step (CPU cores +
/// all peripherals) instead of the PIO-only fast path. Equal to [`run`] for any
/// program whose output doesn't depend on the cores — the reference the fast
/// path is validated against (`fast_step_matches_full`). Not used in the search
/// hot loop.
pub fn run_full(program: &Program, spec: &RunSpec) -> Vec<u32> {
    RUNNER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if !matches!(&*slot, Some((b, s, _)) if *b == spec.block && *s == spec.sm) {
            *slot = Some((spec.block, spec.sm, Pio::new(spec.block, spec.sm)));
        }
        let pio = &mut slot.as_mut().unwrap().2;
        pio.reset();
        configure(pio, program, spec);
        pio.trace_pads_full(&spec.capture_pins, spec.cycles)
    })
}

/// Assemble, load, and configure an already-reset `Pio` for `program`/`spec`,
/// up to (and including) enabling the SM and pushing the inputs — everything a
/// single evaluation does *except* the cycle-stepping capture. Split out so the
/// per-eval setup cost can be benchmarked apart from the emulator core
/// (`trace_pads`); `run_on` is its only in-tree caller.
pub fn configure(pio: &mut Pio, program: &Program, spec: &RunSpec) {
    configure_regs(pio, program, spec);
    for &w in &spec.inputs {
        pio.tx_push(w);
    }
}

/// Everything [`configure`] does *except* pushing `spec.inputs` to the TX FIFO —
/// the streaming path ([`stream_on`]) feeds the FIFO during stepping instead.
fn configure_regs(pio: &mut Pio, program: &Program, spec: &RunSpec) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{dme_cfg, dme_ref, dme_spec, DME_H};
    use crate::ir::SideCfg;
    use crate::program::Program;
    use crate::rng::Rng;
    use crate::search::{random_program, Genes, Space};

    /// STREAM EQUIVALENCE: on a spec that fits the TX FIFO, the cycle-by-cycle
    /// streaming path must produce byte-identical output to the fused pre-load
    /// fast path. This is the correctness anchor the long-corpus (random-data)
    /// stream relies on — it can only refill an infinite FIFO faithfully if it
    /// already matches the finite one. Covers the DME reference and the search's
    /// actual diet of random programs.
    #[test]
    fn stream_matches_fast() {
        let sp = dme_spec(140); // the 4-code locked corpus — fits the FIFO
        let r = dme_ref(DME_H).lower();
        assert_eq!(run(&r, &sp), run_streaming(&r, &sp), "DME reference");

        let space = Space { slots: 20, side: SideCfg::NONE, search_wrap: true, genes: Genes::default() };
        let template = Program::empty(dme_cfg());
        let mut rng = Rng::new(0xABCD_1234);
        for i in 0..200 {
            let p = random_program(&template, &space, &mut rng);
            assert_eq!(run(&p, &sp), run_streaming(&p, &sp), "random program {i}");
        }
    }
}
