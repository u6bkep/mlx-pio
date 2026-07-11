//! Differential gate for the narrowing evaluator (`narrow::run`) against
//! the vendored-emulator path (`run::run`): byte-identical `trace_pads`
//! output on the DME reference, random-program fleets across side-set
//! configs and config genes, streamed long inputs, and stimulus-driven
//! input programs. This suite is the trust anchor named in
//! `docs/evaluator-spec.md` §9 — any semantic change must keep it green.

use pio_harness::Pio;
use pio_superopt::fixtures::{dme_random_golden, dme_ref, dme_spec, DME_H};
use pio_superopt::ir::SideCfg;
use pio_superopt::narrow::{self, Stim};
use pio_superopt::program::Program;
use pio_superopt::rng::Rng;
use pio_superopt::run::{self, RunSpec};
use pio_superopt::search::{random_program, Genes, Space};

use pio_superopt::fixtures::dme_cfg;

#[test]
fn dme_reference_matches() {
    let r = dme_ref(DME_H).lower();
    for cycles in [1u64, 7, 140, 278, 1000] {
        let sp = dme_spec(cycles);
        assert_eq!(narrow::run(&r, &sp), run::run(&r, &sp), "DME reference @ {cycles} cycles");
    }
}

/// Random programs, no side-set, all config genes live (clkdiv incl.
/// fractional delta-sigma, pull threshold, shift dir, autopull).
#[test]
fn random_programs_match_no_sideset() {
    let space = Space {
        slots: 20,
        side: SideCfg::NONE,
        search_wrap: true,
        genes: Genes { clkdiv: true, pull_threshold: true, out_dir: true, autopull: true },
    };
    let template = Program::empty(dme_cfg());
    let sp = dme_spec(200);
    let mut rng = Rng::new(0x5EED_0001);
    for i in 0..400 {
        let p = random_program(&template, &space, &mut rng);
        assert_eq!(narrow::run(&p, &sp), run::run(&p, &sp), "program {i}: {}", p.brief());
    }
}

/// Side-set spaces: mandatory 2-bit, optional (enable-bit) 2-bit, and
/// optional 1-bit-enable-only, plus a pindir-driving variant. The
/// side-set pin is captured so a wrong split/apply shows in the trace.
#[test]
fn random_programs_match_sideset() {
    let variants: [(SideCfg, bool); 4] = [
        (SideCfg { count: 2, en: false }, false),
        (SideCfg { count: 2, en: true }, false),
        (SideCfg { count: 1, en: true }, false),
        (SideCfg { count: 2, en: false }, true), // side-set drives PINDIRS
    ];
    for (vi, (side, pindir)) in variants.iter().enumerate() {
        let mut cfg = dme_cfg();
        cfg.side = *side;
        cfg.side_pindir = *pindir;
        cfg.pins.sideset_base = 1;
        let space = Space { slots: 16, side: *side, search_wrap: true, genes: Genes::default() };
        let template = Program::empty(cfg);
        let sp = RunSpec {
            capture_pins: vec![0, 1, 2],
            output_pins: vec![0, 1, 2],
            ..dme_spec(200)
        };
        let mut rng = Rng::new(0x5EED_0002 + vi as u64);
        for i in 0..200 {
            let p = random_program(&template, &space, &mut rng);
            assert_eq!(
                narrow::run(&p, &sp),
                run::run(&p, &sp),
                "variant {vi} program {i}: {}",
                p.brief()
            );
        }
    }
}

/// RX-flavored configs: autopush ON across push thresholds, both IN
/// shift directions, and both FIFO joins — the IN/autopush semantics
/// (pre-shift flush, post-shift push, RX-full stall) and depth-0/8
/// FIFOs that the TX-shaped spaces above never reach. RX is never
/// drained mid-run, so the full-FIFO stall arms are hit too.
#[test]
fn random_programs_match_rx_flavors() {
    use pio_superopt::program::ShiftDir as SD;
    let mut vi = 0;
    for push_threshold in [1u8, 5, 8, 32] {
        for in_dir in [SD::Left, SD::Right] {
            for (fjoin_rx, fjoin_tx) in [(false, false), (true, false), (false, true)] {
                let mut cfg = dme_cfg();
                cfg.pins.in_base = 8;
                cfg.jmp_pin = 9;
                cfg.shift.autopush = true;
                cfg.shift.push_threshold = push_threshold;
                cfg.shift.in_dir = in_dir;
                cfg.shift.fjoin_rx = fjoin_rx;
                cfg.shift.fjoin_tx = fjoin_tx;
                let space =
                    Space { slots: 16, side: SideCfg::NONE, search_wrap: true, genes: Genes::default() };
                let template = Program::empty(cfg);
                let sp = RunSpec {
                    capture_pins: vec![0, 1, 8],
                    ..dme_spec(200)
                };
                let mut rng = Rng::new(0x5EED_0100 + vi as u64);
                for i in 0..60 {
                    let p = random_program(&template, &space, &mut rng);
                    assert_eq!(
                        narrow::run(&p, &sp),
                        run::run(&p, &sp),
                        "flavor {vi} (pt {push_threshold} {in_dir:?} rx{fjoin_rx} tx{fjoin_tx}) program {i}: {}",
                        p.brief()
                    );
                }
                vi += 1;
            }
        }
    }
}

/// Long input lists exercise the streaming refill path (and autopull's
/// word-boundary behavior against a fed FIFO).
#[test]
fn random_programs_match_streaming() {
    let (sp, _, _) = dme_random_golden(32, 0xFEED_BEEF);
    let space = Space {
        slots: 20,
        side: SideCfg::NONE,
        search_wrap: true,
        genes: Genes { clkdiv: true, pull_threshold: true, out_dir: true, autopull: true },
    };
    let template = Program::empty(dme_cfg());
    let mut rng = Rng::new(0x5EED_0003);
    for i in 0..150 {
        let p = random_program(&template, &space, &mut rng);
        assert_eq!(narrow::run(&p, &sp), run::run(&p, &sp), "program {i}: {}", p.brief());
    }
}

/// Dump differential test vectors for the shard PIO emulator (the twin
/// implementation of docs/evaluator-spec.md): one JSON object per line,
/// each a fully-decoded config + program + stimulus + the certified
/// per-cycle trace from the vendored emulator. A shard implementation
/// replays these and must reproduce every trace byte-identically.
/// Run with:
/// `cargo test --release --test narrow_diff -- --ignored dump_shard_vectors --nocapture`
#[test]
#[ignore]
fn dump_shard_vectors() {
    use std::io::Write;
    let out_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/shard_vectors.jsonl");
    let mut f = std::io::BufWriter::new(std::fs::File::create(out_path).unwrap());

    let mut emit = |name: &str, p: &Program, sp: &RunSpec, stim: &Stim| {
        let trace = narrow::run_with_stim(p, sp, stim);
        // The narrow path IS the certified one (differential gate above);
        // sanity-pin the no-stim cases against the vendored path anyway.
        if stim.mask == 0 {
            assert_eq!(trace, run::run(p, sp), "{name}: vendored disagreement");
        }
        let cfg = narrow::NCfg::from_program(p, sp.sm as u8);
        let row = serde_json::json!({
            "name": name,
            "code": cfg.code.to_vec(),
            "wrap_bottom": cfg.wrap_bottom, "wrap_top": cfg.wrap_top,
            "side_count": cfg.side_count, "side_en": cfg.side_en,
            "side_pindir": cfg.side_pindir,
            "jmp_pin": cfg.jmp_pin,
            "in_base": cfg.in_base,
            "out_base": cfg.out_base, "out_count": cfg.out_count,
            "set_base": cfg.set_base, "set_count": cfg.set_count,
            "sideset_base": cfg.sideset_base,
            "autopush": cfg.autopush, "autopull": cfg.autopull,
            "push_threshold": cfg.push_threshold,
            "pull_threshold": cfg.pull_threshold,
            "in_shift_right": cfg.in_shift_right,
            "out_shift_right": cfg.out_shift_right,
            "clkdiv_int": cfg.clkdiv_int, "clkdiv_frac": cfg.clkdiv_frac,
            "sm_id": cfg.sm_id,
            "tx_depth": cfg.tx_depth, "rx_depth": cfg.rx_depth,
            // Driver-level spec: inputs are pre-loaded when <= 4 words,
            // else streamed (refill-before-cycle); autopull_pad applied.
            "inputs": sp.inputs.clone(),
            "autopull_pad": if p.config.shift.autopull { sp.autopull_pad } else { 0 },
            "output_pins": sp.output_pins.clone(),
            "capture_pins": sp.capture_pins.clone(),
            "cycles": sp.cycles,
            "stim_mask": stim.mask,
            "stim_values": stim.values.clone(),
            "trace": trace,
        });
        writeln!(f, "{row}").unwrap();
    };

    // The DME reference — the one vector a human can eyeball.
    emit("dme_reference", &dme_ref(DME_H).lower(), &dme_spec(278), &Stim::default());

    // Random fleets from the same spaces the differential gate uses.
    let no_stim = Stim::default();
    let space = Space {
        slots: 20,
        side: SideCfg::NONE,
        search_wrap: true,
        genes: Genes { clkdiv: true, pull_threshold: true, out_dir: true, autopull: true },
    };
    let template = Program::empty(dme_cfg());
    let sp = dme_spec(200);
    let mut rng = Rng::new(0x5AAD_0001);
    for i in 0..40 {
        emit(&format!("plain_{i}"), &random_program(&template, &space, &mut rng), &sp, &no_stim);
    }

    let mut side_cfg = dme_cfg();
    side_cfg.side = SideCfg { count: 2, en: true };
    side_cfg.pins.sideset_base = 1;
    let side_space =
        Space { slots: 16, side: side_cfg.side, search_wrap: true, genes: Genes::default() };
    let side_template = Program::empty(side_cfg);
    let side_sp =
        RunSpec { capture_pins: vec![0, 1, 2], output_pins: vec![0, 1, 2], ..dme_spec(200) };
    for i in 0..30 {
        emit(
            &format!("sideset_{i}"),
            &random_program(&side_template, &side_space, &mut rng),
            &side_sp,
            &no_stim,
        );
    }

    let mut rx_cfg = dme_cfg();
    rx_cfg.pins.in_base = 8;
    rx_cfg.jmp_pin = 9;
    rx_cfg.shift.autopush = true;
    rx_cfg.shift.push_threshold = 5;
    let rx_space = Space { slots: 16, side: SideCfg::NONE, search_wrap: true, genes: Genes::default() };
    let rx_template = Program::empty(rx_cfg);
    for i in 0..30 {
        let p = random_program(&rx_template, &rx_space, &mut rng);
        let mut values = Vec::with_capacity(300);
        let mut cur = 0u32;
        for c in 0..300usize {
            if c % 3 == 0 {
                cur = (rng.below(16) as u32) << 8;
            }
            values.push(cur);
        }
        let stim = Stim { mask: 0xF << 8, values };
        let sp = RunSpec {
            block: 0,
            sm: 0,
            inputs: pio_superopt::fixtures::dme_corpus(),
            output_pins: vec![0, 1, 2],
            capture_pins: vec![0, 1, 2, 8, 9],
            cycles: 300,
            autopull_pad: 0,
        };
        emit(&format!("stim_{i}"), &p, &sp, &stim);
    }

    println!("wrote {out_path}");
}

/// Throughput comparison, narrow vs vendored path, on the locked DME
/// workload. Run with:
/// `cargo test --release --test narrow_diff -- --ignored eval_throughput --nocapture`
#[test]
#[ignore]
fn eval_throughput() {
    let r = dme_ref(DME_H).lower();
    let sp = dme_spec(278);
    let n = 20_000u32;

    let t0 = std::time::Instant::now();
    let mut acc = 0u32;
    for _ in 0..n {
        acc ^= run::run(&r, &sp).last().copied().unwrap_or(0);
    }
    let vendored = t0.elapsed();

    let t1 = std::time::Instant::now();
    for _ in 0..n {
        acc ^= narrow::run(&r, &sp).last().copied().unwrap_or(0);
    }
    let narrow_t = t1.elapsed();

    println!(
        "vendored: {:?}/eval   narrow: {:?}/eval   speedup: {:.1}x   (acc {acc})",
        vendored / n,
        narrow_t / n,
        vendored.as_secs_f64() / narrow_t.as_secs_f64()
    );
}

/// Drive the vendored emulator with per-cycle `set_pin` stimulus —
/// the reference for `narrow::run_with_stim` (WAIT PIN/GPIO, JMP PIN,
/// IN/MOV PINS all read externally-forced inputs).
fn harness_run_with_stim(program: &Program, spec: &RunSpec, stim: &Stim) -> Vec<u32> {
    assert!(spec.inputs.len() <= 4, "stim reference path pre-loads the FIFO");
    let mut pio = Pio::new(spec.block, spec.sm);
    run::configure(&mut pio, program, spec);
    let mut out = Vec::with_capacity(spec.cycles as usize);
    for cycle in 0..spec.cycles as usize {
        let v = if stim.values.is_empty() {
            0
        } else {
            stim.values[cycle.min(stim.values.len() - 1)]
        };
        for pin in 0..32u8 {
            if (stim.mask >> pin) & 1 != 0 {
                pio.set_pin(pin, (v >> pin) & 1 != 0);
            }
        }
        out.push(pio.trace_pads(&spec.capture_pins, 1)[0]);
    }
    out
}

/// Random programs against random input stimulus. IN_BASE and JMP_PIN
/// point into the stimulated pin group (8..=11), so input-consuming
/// instructions see live data; outputs stay on pins 0..=2.
#[test]
fn random_programs_match_with_stimulus() {
    let mut cfg = dme_cfg();
    cfg.pins.in_base = 8;
    cfg.jmp_pin = 9;
    let space = Space { slots: 16, side: SideCfg::NONE, search_wrap: true, genes: Genes::default() };
    let template = Program::empty(cfg);
    let mut rng = Rng::new(0x5EED_0004);
    for i in 0..120 {
        let p = random_program(&template, &space, &mut rng);
        // Random stimulus on pins 8..=11, changing every few cycles.
        let mut values = Vec::with_capacity(300);
        let mut cur = 0u32;
        for c in 0..300usize {
            if c % 3 == 0 {
                cur = (rng.below(16) as u32) << 8;
            }
            values.push(cur);
        }
        let stim = Stim { mask: 0xF << 8, values };
        let sp = RunSpec {
            block: 0,
            sm: 0,
            inputs: pio_superopt::fixtures::dme_corpus(),
            output_pins: vec![0, 1, 2],
            capture_pins: vec![0, 1, 2, 8, 9],
            cycles: 300,
            autopull_pad: 0,
        };
        assert_eq!(
            narrow::run_with_stim(&p, &sp, &stim),
            harness_run_with_stim(&p, &sp, &stim),
            "program {i}: {}",
            p.brief()
        );
    }
}
