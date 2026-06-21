//! Emulator-reuse prototype for the superoptimizer hot path.
//!
//! The superoptimizer evaluates thousands of candidate PIO programs; each
//! eval currently calls `Pio::new`, which rebuilds an entire
//! `rp2350_emu::Emulator` (~200µs, the throughput bottleneck). This test
//! validates [`Pio::reset`]: resetting ONE emulator's PIO block between
//! evals instead of rebuilding.
//!
//!   cargo test -p pio_harness --test reset_reuse -- --nocapture
//!   cargo test -p pio_harness --test reset_reuse -- --ignored --nocapture  (bench)

use std::time::Instant;

use pio::{
    InstructionOperands as Op, MovDestination, MovOperation, MovSource, OutDestination,
    SetDestination,
};
use pio_harness::{PinCtrl, Pio, ShiftCtrl, ShiftDir};

const DATA: u8 = 0;
const CLK: u8 = 1;

/// Encode an instruction word with an explicit side-set value in the
/// delay/side-set field. `sideset_count = 1`, no `opt`, so the side value
/// occupies bit 12 and `delay` occupies bits [11:8].
fn with_side(base: u16, side: u16, delay: u16) -> u16 {
    base | (side << 12) | (delay << 8)
}

/// A self-contained evaluation spec: program words + full SM config +
/// stimulus + capture. Mirrors `pio_superopt::run::run`'s flow exactly.
#[derive(Clone)]
struct Eval {
    code: Vec<u16>,
    wrap_bottom: u8,
    wrap_top: u8,
    pinctrl: PinCtrl,
    shiftctrl: ShiftCtrl,
    side_en: bool,
    side_pindir: bool,
    jmp_pin: u8,
    clkdiv: (u16, u8),
    output_pins: Vec<u8>,
    capture_pins: Vec<u8>,
    inputs: Vec<u32>,
    cycles: u64,
}

/// Configure + run an already-constructed (and freshly reset, or freshly
/// built) `Pio`, returning the captured waveform. This is the exact
/// sequence `pio_superopt::run::run` performs.
fn configure_and_run(pio: &mut Pio, e: &Eval) -> Vec<u32> {
    pio.load_at(0, &e.code, e.wrap_bottom, e.wrap_top);
    pio.pinctrl(e.pinctrl);
    pio.sideset(e.side_en, e.side_pindir);
    pio.jmp_pin(e.jmp_pin);
    pio.clkdiv(e.clkdiv.0, e.clkdiv.1);
    pio.shiftctrl(e.shiftctrl);
    for &p in &e.output_pins {
        pio.set_output(p);
    }
    pio.enable();
    for &w in &e.inputs {
        pio.tx_push(w);
    }
    pio.trace_pins(&e.capture_pins, e.cycles)
}

/// The canonical SPI-TX golden program (the superopt's benchmark target):
/// `OUT PINS,1 side 0` ; `MOV Y,Y side 1`, wrap 0..1, MSB-first 8-bit,
/// autopull. Captures DATA + CLK for 16 cycles.
fn spi_golden() -> Eval {
    let out = with_side(
        Op::OUT { destination: OutDestination::PINS, bit_count: 1 }.encode(),
        0,
        0,
    );
    let mov = with_side(
        Op::MOV {
            destination: MovDestination::Y,
            op: MovOperation::None,
            source: MovSource::Y,
        }
        .encode(),
        1,
        0,
    );
    Eval {
        code: vec![out, mov],
        wrap_bottom: 0,
        wrap_top: 1,
        pinctrl: PinCtrl { out_base: DATA, out_count: 1, sideset_base: CLK, ..Default::default() },
        shiftctrl: ShiftCtrl {
            autopull: true,
            pull_threshold: 8,
            out_dir: ShiftDir::Left,
            ..Default::default()
        },
        side_en: false,
        side_pindir: false,
        jmp_pin: 0,
        clkdiv: (1, 0),
        output_pins: vec![DATA, CLK],
        capture_pins: vec![DATA, CLK],
        inputs: vec![0xA5 << 24],
        cycles: 16,
    }
}

/// A deliberately different program: drives DATA from a SET, clocks CLK
/// via side-set, different input/threshold — used to prove no state leaks
/// between back-to-back evals of *different* programs.
fn alt_program(seed: u32) -> Eval {
    // SET PINS, (seed & 1) side 1 ; SET PINS, ((seed>>1)&1) side 0
    let s_hi = with_side(
        Op::SET { destination: SetDestination::PINS, data: (seed & 1) as u8 }.encode(),
        1,
        (seed % 3) as u16,
    );
    let s_lo = with_side(
        Op::SET { destination: SetDestination::PINS, data: ((seed >> 1) & 1) as u8 }.encode(),
        0,
        0,
    );
    Eval {
        code: vec![s_hi, s_lo],
        wrap_bottom: 0,
        wrap_top: 1,
        pinctrl: PinCtrl {
            set_base: DATA,
            set_count: 1,
            sideset_base: CLK,
            ..Default::default()
        },
        shiftctrl: ShiftCtrl {
            autopull: true,
            pull_threshold: (1 + (seed % 32)) as u8,
            out_dir: ShiftDir::Right,
            ..Default::default()
        },
        side_en: false,
        side_pindir: false,
        jmp_pin: 0,
        clkdiv: (1 + (seed % 4) as u16, (seed % 7) as u8),
        output_pins: vec![DATA, CLK],
        capture_pins: vec![DATA, CLK],
        inputs: vec![seed.wrapping_mul(0x9E37_79B9)],
        cycles: 16,
    }
}

/// Build-fresh path: the status quo. One brand-new emulator per eval.
fn run_fresh(e: &Eval) -> Vec<u32> {
    let mut pio = Pio::new(0, 0);
    configure_and_run(&mut pio, e)
}

/// CORRECTNESS: the golden run, scored via the reuse path, must be
/// byte-identical to the fresh-build path.
#[test]
fn reuse_matches_fresh_for_golden() {
    let e = spi_golden();
    let fresh = run_fresh(&e);

    let mut pio = Pio::new(0, 0);
    for _ in 0..256 {
        pio.reset();
        let reused = configure_and_run(&mut pio, &e);
        assert_eq!(reused, fresh, "reuse path diverged from fresh build");
    }
}

/// CORRECTNESS / NO-LEAK: interleave a *different, dirty* program before
/// each golden eval on the reuse path. If `reset()` failed to wipe any
/// per-eval state (FIFOs, X/Y, OSR, pins, pc_visits, side-set latches…),
/// the golden waveform would pick up history and diverge from the
/// fresh-build reference. Also checks each alt program matches its own
/// fresh build.
#[test]
fn no_state_leak_between_different_programs() {
    let golden = spi_golden();
    let golden_fresh = run_fresh(&golden);

    let mut pio = Pio::new(0, 0);
    for seed in 0..512u32 {
        let alt = alt_program(seed);
        let alt_fresh = run_fresh(&alt);

        // Run the dirty/different program first on the shared emulator.
        pio.reset();
        let alt_reused = configure_and_run(&mut pio, &alt);
        assert_eq!(alt_reused, alt_fresh, "alt program seed={seed} reuse != fresh");

        // Now reset and run the golden program. It must be pristine.
        pio.reset();
        let golden_reused = configure_and_run(&mut pio, &golden);
        assert_eq!(
            golden_reused, golden_fresh,
            "golden leaked state from alt program seed={seed}"
        );
    }
}

/// BENCHMARK: time N evals of the golden run both ways. Ignored by
/// default (it's a benchmark, not a pass/fail test).
///   cargo test -p pio_harness --test reset_reuse -- --ignored --nocapture
#[test]
#[ignore = "benchmark; run with --ignored --nocapture"]
fn bench_rebuild_vs_reuse() {
    const N: usize = 5000;
    let e = spi_golden();

    // Warm up + capture reference.
    let reference = run_fresh(&e);

    // Path 1: rebuild per eval (status quo).
    let t0 = Instant::now();
    let mut acc = 0u32;
    for _ in 0..N {
        let w = run_fresh(&e);
        acc ^= w[w.len() - 1];
    }
    let rebuild = t0.elapsed();

    // Path 2: reuse one emulator, reset per eval.
    let mut pio = Pio::new(0, 0);
    let t1 = Instant::now();
    for _ in 0..N {
        pio.reset();
        let w = configure_and_run(&mut pio, &e);
        acc ^= w[w.len() - 1];
        debug_assert_eq!(w, reference);
    }
    let reuse = t1.elapsed();

    let rebuild_us = rebuild.as_secs_f64() * 1e6 / N as f64;
    let reuse_us = reuse.as_secs_f64() * 1e6 / N as f64;
    eprintln!("\n=== emulator-reuse benchmark (N={N}) ===");
    eprintln!("rebuild-per-eval : {rebuild_us:8.2} µs/eval  ({:?} total)", rebuild);
    eprintln!("reuse + reset    : {reuse_us:8.2} µs/eval  ({:?} total)", reuse);
    eprintln!("speedup          : {:8.2}x", rebuild_us / reuse_us);
    eprintln!("(acc={acc:#x})");
    assert!(reuse_us < rebuild_us, "reuse should be faster");
}
