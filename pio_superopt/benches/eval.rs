//! Eval hot-path benchmarks (ticket 004).
//!
//! The search evaluates tens of millions of candidates; each one is a full
//! emulator run plus a cost computation. These benches attribute the per-eval
//! time so we know where to spend optimization effort:
//!
//!   * `eval/run`        — the whole `run()`: assemble + load + configure +
//!                         enable + step/capture. The headline per-candidate cost.
//!   * `eval/edge_cost`  — the cost metric (per-channel DP edge alignment) on a
//!                         realistic golden-vs-imperfect waveform pair.
//!   * `eval/candidate`  — run + edge_cost: one complete candidate evaluation.
//!
//! And `run()` decomposed, to split the emulator core from per-eval setup:
//!
//!   * `run_parts/assemble`  — `Program::assemble()` (IR -> 32 machine words).
//!   * `run_parts/reset`     — `Pio::reset()` (the reuse path; vs ~200µs rebuild).
//!   * `run_parts/configure` — load + pinctrl/shift/clkdiv/... + enable + push.
//!   * `run_parts/trace`     — `trace_pads()` alone: the cycle-stepping core.
//!
//! All workloads are the locked DME target from `pio_superopt::fixtures`, so the
//! benches measure exactly what the search runs.

use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;
use std::time::{Duration, Instant};

use pio_harness::Pio;
use pio_superopt::cost::edge_cost_w;
use pio_superopt::fixtures::{dme_cfg, dme_golden, dme_plateau_gene, dme_ref, DME_H};
use pio_superopt::ir::SideCfg;
use pio_superopt::program::Program;
use pio_superopt::rng::Rng;
use pio_superopt::search::{edge_breed_cost, mutate, random_program, Genes, Params, Space};
use pio_superopt::{configure, run};

/// The window/spurious-weight the breeding engine actually evaluates at
/// (`densify_w` default = 0.5; a representative mid-ladder window).
const COST_WINDOW: usize = 4;
const SPURIOUS_W: f64 = 0.5;

fn bench_eval(c: &mut Criterion) {
    let (spec, golden, mask) = dme_golden();
    let program = dme_ref(DME_H).lower();
    // A realistic imperfect candidate: the boundary-only "plateau" basin —
    // edge-dense and misaligned vs golden, the worst case the DP aligner sees.
    let cand_wave = run(&dme_plateau_gene().lower(), &spec);

    let mut g = c.benchmark_group("eval");
    g.bench_function("run", |b| {
        b.iter(|| run(black_box(&program), black_box(&spec)))
    });
    g.bench_function("edge_cost", |b| {
        b.iter(|| {
            edge_cost_w(
                black_box(&golden),
                black_box(&cand_wave),
                black_box(&mask),
                COST_WINDOW,
                SPURIOUS_W,
            )
        })
    });
    g.bench_function("candidate", |b| {
        b.iter(|| {
            let w = run(black_box(&program), black_box(&spec));
            edge_cost_w(black_box(&golden), &w, black_box(&mask), COST_WINDOW, SPURIOUS_W)
        })
    });
    g.finish();

    // Evidence: does the full (all-32-bits) mask cost ~32x the channel scan vs a
    // mask naming only the bits that actually carry data? `care` = OR of the mask.
    let mask_full = mask.clone(); // u32::MAX everywhere -> care = all 32 bits
    let mask_bit0: Vec<u32> = vec![1; golden.len()]; // only the TX level channel
    let mask_present: Vec<u32> = vec![1 | (1 << 16); golden.len()]; // TX level + OE
    let mut g = c.benchmark_group("edge_cost_mask");
    for (name, m) in [("full32", &mask_full), ("present2", &mask_present), ("bit0", &mask_bit0)] {
        g.bench_function(name, |b| {
            b.iter(|| {
                edge_cost_w(black_box(&golden), black_box(&cand_wave), black_box(m), COST_WINDOW, SPURIOUS_W)
            })
        });
    }
    g.finish();
}

fn bench_run_parts(c: &mut Criterion) {
    let (spec, _golden, _mask) = dme_golden();
    let program = dme_ref(DME_H).lower();

    let mut g = c.benchmark_group("run_parts");

    g.bench_function("assemble", |b| {
        b.iter(|| black_box(&program).assemble())
    });

    // reset / configure: reuse one Pio (mirrors the thread-local in `run()`).
    g.bench_function("reset", |b| {
        let mut pio = Pio::new(spec.block, spec.sm);
        b.iter(|| {
            pio.reset();
            black_box(&pio);
        })
    });
    g.bench_function("configure", |b| {
        let mut pio = Pio::new(spec.block, spec.sm);
        b.iter(|| {
            pio.reset();
            configure(&mut pio, black_box(&program), black_box(&spec));
            black_box(&pio);
        })
    });

    // trace alone: each timed call needs a freshly reset+configured SM (stepping
    // is destructive), so reset+configure *untimed* and time only trace_pads.
    // One reused Pio keeps the cache hot, matching `run()`'s thread-local reuse
    // (a fresh Pio per call measures cold-cache and over-reports by ~50%).
    g.bench_function("trace", |b| {
        let mut pio = Pio::new(spec.block, spec.sm);
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                pio.reset();
                configure(&mut pio, &program, &spec);
                let t = Instant::now();
                black_box(pio.trace_pads(black_box(&spec.capture_pins), spec.cycles));
                total += t.elapsed();
            }
            total
        })
    });

    g.finish();
}

/// The real search inner loop, to confirm the micro-benches above add up to the
/// per-iteration cost the search actually pays — and to catch the `validate()`
/// the ticket flagged (it runs per candidate, inside `edge_breed_cost`, and is
/// not covered by `run`/`edge_cost`).
fn bench_breed(c: &mut Criterion) {
    let (spec, golden, mask) = dme_golden();
    // The exact DME breed configuration (gene_search::tests::dme_breed).
    let template = Program::empty(dme_cfg());
    let space = Space { slots: 20, side: SideCfg::NONE, search_wrap: true, genes: Genes::default() };
    let params = Params::default(); // w = 64.0, densify_w = 0.5
    let window = COST_WINDOW; // representative mid-ladder window

    // A representative valid starting candidate (regenerate until legal).
    let mut rng = Rng::new(0xB433);
    let cur = loop {
        let p = random_program(&template, &space, &mut rng);
        if p.validate().is_ok() {
            break p;
        }
    };

    // Diagnostic: how edge-dense is this candidate's waveform? (edge_cost scales
    // with edge count, so a sparse random candidate is the cheap end of the
    // range; an evolved/plateau candidate the dense end.)
    {
        let w = run(&cur, &spec);
        let ec = edge_cost_w(&golden, &w, &mask, window, params.densify_w);
        let edges = w.windows(2).filter(|x| (x[0] ^ x[1]) & 1 != 0).count();
        eprintln!("[breed/cost candidate] wave edges(bit0)={edges}  edge_cost={ec:.2}");
    }

    let mut g = c.benchmark_group("breed");

    g.bench_function("validate", |b| {
        b.iter(|| black_box(&cur).validate().is_ok())
    });

    g.bench_function("mutate", |b| {
        b.iter(|| mutate(black_box(&cur), black_box(&space), false, &mut rng))
    });

    // The full per-candidate objective: validate + run + edge_cost + size.
    g.bench_function("cost", |b| {
        b.iter(|| {
            edge_breed_cost(
                black_box(&cur),
                black_box(&golden),
                black_box(&mask),
                black_box(&spec),
                params.w,
                window,
                params.densify_w,
            )
        })
    });

    // One full local-move iteration of flat_breed_chain: mutate a fresh
    // candidate, then score it. (The crossover/poll path adds a second
    // edge_breed_cost every poll_rate≈50 iters — ~2% amortized, omitted.)
    g.bench_function("step", |b| {
        b.iter(|| {
            let cand = mutate(black_box(&cur), black_box(&space), false, &mut rng);
            edge_breed_cost(&cand, black_box(&golden), black_box(&mask), black_box(&spec), params.w, window, params.densify_w)
        })
    });

    g.finish();
}

criterion_group!(benches, bench_eval, bench_run_parts, bench_breed);
criterion_main!(benches);
