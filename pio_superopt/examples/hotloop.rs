//! Tight eval hot-loop for `perf` analysis (ticket 004). Runs the locked DME
//! eval (run + edge_cost) N times so a process-wide `perf stat`/`perf record`
//! is dominated by the hot path.
//!
//!   cargo build --release --example hotloop
//!   perf stat -d ./target/release/examples/hotloop 2000000
//!   perf record -g ./target/release/examples/hotloop 2000000 && perf report

use pio_superopt::cost::edge_cost_w;
use pio_superopt::fixtures::{dme_golden, dme_plateau_gene, dme_ref, DME_H};
use pio_superopt::run;

fn main() {
    let iters: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1_000_000);
    // Optional 2nd arg: number of parallel worker threads (default 1). Use the
    // core count to reproduce the search's SMT cache contention (ticket 004).
    let threads: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(1);

    let (es, bs, ps, ss) = pio_harness::type_sizes();
    eprintln!("struct sizes (bytes): Emulator={es}  Bus={bs}  PioBlock={ps}  StateMachine={ss}");
    eprintln!("threads={threads}  iters/thread={iters}  (L1d 32 KiB/core, shared by SMT siblings)");

    let work = || {
        // Each thread owns its golden/program (the search runs one per island)
        // and its own thread-local emulator (RUNNER), matching deployment.
        let (spec, golden, mask) = dme_golden();
        let program = dme_ref(DME_H).lower();
        let cand = dme_plateau_gene().lower();
        let mut acc = 0.0f64;
        for i in 0..iters {
            let p = if i & 1 == 0 { &program } else { &cand };
            let wave = run(p, &spec);
            acc += edge_cost_w(&golden, &wave, &mask, 4, 0.5);
            acc += wave.len() as f64 * 1e-9;
        }
        acc
    };

    let total: f64 = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads).map(|_| s.spawn(work)).collect();
        handles.into_iter().map(|h| h.join().unwrap()).sum()
    });
    println!("threads={threads} iters/thread={iters} checksum={total:.6}");
}
