//! Search runner: resumable gated-curriculum ladders over [`Problem`]s, plus
//! trace inspection.
//!
//!     superopt spec-ladder [opts]   # DME under the spec oracle (ticket 005)
//!     superopt spec-ap-ladder [opts]# spec oracle + autopull gene (ticket 005 step 4)
//!     superopt wave-ladder [opts]   # DME under the cycle-exact oracle
//!     superopt compress [opts]      # STOKE-style: shrink a certified seed (dme_spec_ref)
//!     superopt enumerate --len N    # exhaustive loop-body sweep (sharded, resumable)
//!     superopt serve --len N        # shard-lease coordinator for a worker fleet
//!     superopt work --server URL    # pull shards from a coordinator until drained
//!     superopt narrow-split [opts]  # narrowing-engine bracket search, unit-resumable
//!     superopt diagnose --trace PATH
//!     superopt smt-synth --len N    # CEGIS synthesis via z3 (--features smt)
//!
//! Ladder options: `--seed 0x5EED  --lengths 2..14  --restarts 32
//! --iters 4000000  --densify F  --trace PATH  --fresh`.
//!
//! The trace (default `runs/<cmd>-<seed>.jsonl`) doubles as the resume state:
//! a header row pins the run identity, and the engine appends a full
//! `GatedSnapshot` row at ~1/8-rung cadence, at attempt starts, and at the
//! checkpoint where a stop lands. Rerunning the same command resumes from the
//! last snapshot — byte-identical to never having stopped (locked by
//! `gene_search::tests::dme_spec_ladder_resume_is_byte_identical`). Ctrl-C
//! sets a stop flag; every restart quits at its next checkpoint barrier right
//! after that checkpoint's snapshot is written. On completion a
//! `<trace>.result.json` summary (champion, gates, git rev) is written next
//! to the trace.
//!
//! `diagnose` reads a trace's LAST snapshot mid-run or post-run: ladder
//! position, the best restart's champion, its structural class
//! (TOGGLER/CONJUNCTION/OTHER), and the problem's gates on it.

use pio_superopt::fixtures::spec_classify;
use pio_superopt::problems::{by_id, DmeSpec, DmeSpecAutopull, DmeSpecCompress, DmeWave, Problem};
use pio_superopt::search::{synthesize_curriculum_gated, TraceEvent};
use pio_superopt::trace::{header_json, scan_for_resume, trace_json, RunHeader};
use pio_superopt::Program;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

fn usage() -> ! {
    eprintln!(
        "usage: superopt <spec-ladder|spec-ap-ladder|wave-ladder|compress> [--seed HEX] [--lengths A..B] \
         [--restarts N] [--iters N] [--densify F] [--trace PATH] [--fresh]\n\
       superopt enumerate --len N [--shard-mod M --shard-rem R] [--threads T] [--out DIR]\n\
       superopt serve --len N [--out DIR] [--listen ADDR:PORT] [--lease-secs S]\n\
       superopt work --server URL [--threads T]\n\
       superopt narrow-split --spec tx-a --len L --wrap-lo A --wrap-hi B [--cycles 460]\n\
                          [--threads T] [--target N] [--champion-cap 5] [--memo-cap N]\n\
                          [--trace PATH] [--fresh]   (unit-level resume; see fn docs)\n\
       superopt diagnose --trace PATH\n\
       superopt smt-synth --len N [--side none|1|2en] [--side-pindir] [--no-autopull]\n\
                          [--max-iters N] [--trace PATH]   (needs --features smt)\n\
         \n\
         Ladders resume automatically from an existing trace (same parameters);\n\
         pass --fresh to discard it and start over. Ctrl-C saves a snapshot and\n\
         exits; rerun the same command to resume."
    );
    std::process::exit(2);
}

struct LadderOpts {
    seed: u64,
    lengths: Vec<usize>,
    restarts: usize,
    iters: u32,
    cycles: usize,
    densify: Option<f64>,
    trace: Option<String>,
    fresh: bool,
}

fn parse_ladder_opts(args: &[String]) -> LadderOpts {
    let mut o = LadderOpts {
        seed: 0x5EED,
        lengths: (2..=14).collect(),
        restarts: 32,
        iters: 4_000_000,
        cycles: 100_000,
        densify: None,
        trace: None,
        fresh: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage()).clone();
        match a.as_str() {
            "--seed" => {
                o.seed = u64::from_str_radix(val().trim_start_matches("0x"), 16).unwrap_or_else(|_| usage())
            }
            "--lengths" => {
                let v = val();
                let (a, b) = v.split_once("..").unwrap_or_else(|| usage());
                let lo: usize = a.parse().unwrap_or_else(|_| usage());
                let hi: usize = b.trim_start_matches('=').parse().unwrap_or_else(|_| usage());
                o.lengths = (lo..=hi).collect();
            }
            "--restarts" => o.restarts = val().parse().unwrap_or_else(|_| usage()),
            "--iters" => o.iters = val().parse().unwrap_or_else(|_| usage()),
            "--cycles" => o.cycles = val().parse().unwrap_or_else(|_| usage()),
            "--densify" => o.densify = Some(val().parse().unwrap_or_else(|_| usage())),
            "--trace" => o.trace = Some(val()),
            "--fresh" => o.fresh = true,
            _ => usage(),
        }
    }
    o
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("spec-ladder") => run_ladder(&DmeSpec, "spec-ladder", parse_ladder_opts(&args[1..])),
        Some("spec-ap-ladder") => run_ladder(&DmeSpecAutopull, "spec-ap-ladder", parse_ladder_opts(&args[1..])),
        Some("wave-ladder") => run_ladder(&DmeWave, "wave-ladder", parse_ladder_opts(&args[1..])),
        Some("compress") => {
            let mut o = parse_ladder_opts(&args[1..]);
            // Compression cycles are short reheat-and-cool passes from the
            // champion; the ladder's 4M default is a synthesis-rung budget.
            if !args[1..].iter().any(|a| a == "--iters") {
                o.iters = 200_000;
            }
            run_compress(&DmeSpecCompress, "compress", o)
        }
        Some("enumerate") => enumerate_cmd(&args[1..]),
        Some("serve") => serve_cmd(&args[1..]),
        Some("work") => work_cmd(&args[1..]),
        Some("narrow-split") => narrow_split_cmd(&args[1..]),
        Some("diagnose") => diagnose(&args[1..]),
        #[cfg(feature = "smt")]
        Some("smt-synth") => smt_synth_cmd(&args[1..]),
        #[cfg(not(feature = "smt"))]
        Some("smt-synth") => {
            eprintln!("smt-synth requires the `smt` feature: cargo run --release --features smt --bin superopt -- smt-synth …");
            std::process::exit(2);
        }
        _ => usage(),
    }
}

/// `superopt smt-synth --len N [--side none|1|2en] [--side-pindir]
/// [--no-autopull] [--max-iters N] [--trace PATH]` — CEGIS synthesis of a
/// len-N DME TX program (wrap 0..N-1) under the compression-seed config
/// (autopull ON threshold 5, shift right) with the chosen side-set mode.
/// Streams a heartbeat per iteration; JSONL trace defaults to
/// `runs/smt-synth-len<N>-<side>.jsonl`. NOT resumable (solver state is not
/// snapshottable) — the trace is observability, reruns start over.
#[cfg(feature = "smt")]
fn smt_synth_cmd(args: &[String]) {
    use pio_superopt::fixtures::dme_cfg;
    use pio_superopt::ir::SideCfg;
    use pio_superopt::smt::cegis::{cegis_dme, CegisOpts, Outcome};

    let mut len: Option<usize> = None;
    let mut side = SideCfg::NONE;
    let mut side_name = "none".to_string();
    let mut side_pindir = false;
    let mut autopull = true;
    let mut max_iters = 0usize;
    let mut trace: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage()).clone();
        match a.as_str() {
            "--len" => len = Some(val().parse().unwrap_or_else(|_| usage())),
            "--side" => {
                side_name = val();
                side = match side_name.as_str() {
                    "none" => SideCfg::NONE,
                    "1" => SideCfg { count: 1, en: false },
                    "2en" => SideCfg { count: 2, en: true },
                    _ => usage(),
                };
            }
            "--side-pindir" => side_pindir = true,
            "--no-autopull" => autopull = false,
            "--max-iters" => max_iters = val().parse().unwrap_or_else(|_| usage()),
            "--trace" => trace = Some(val()),
            _ => usage(),
        }
    }
    let len = len.unwrap_or_else(|| usage());
    let mut cfg = dme_cfg();
    cfg.side = side;
    cfg.side_pindir = side_pindir;
    cfg.shift.autopull = autopull;

    let trace = trace.unwrap_or_else(|| format!("runs/smt-synth-len{len}-{side_name}.jsonl"));
    eprintln!(
        "[smt-synth] len {len}, side {side_name}{}, autopull {}, trace {trace}",
        if side_pindir { " (pindir)" } else { "" },
        if autopull { "on(5)" } else { "off" },
    );
    let opts = CegisOpts { max_iters, trace: Some(trace.into()), verbose: true };
    let report = cegis_dme(&cfg, &vec![None; len], &opts);
    match report.outcome {
        Outcome::Found(p) => {
            println!("FOUND (battery-certified, {} iters): {}", report.iters, p.brief());
            println!("words: {:04x?}", &p.assemble()[..len]);
        }
        Outcome::Unsat => {
            println!(
                "UNSAT after {} iters / {} examples: no len-{len} program in the modeled \
                 subset (side {side_name}, autopull {autopull}). Verdict rests on mirror \
                 fidelity — rerun differential_fuzz before trusting it.",
                report.iters,
                report.examples.len()
            );
        }
        Outcome::MaxIters => {
            println!("stopped at --max-iters {} ({} examples)", report.iters, report.examples.len());
            std::process::exit(1);
        }
    }
}

fn run_ladder(problem: &dyn Problem, cmd: &str, o: LadderOpts) {
    let mut hp = problem.default_hp();
    if let Some(d) = o.densify {
        hp.densify_w = d;
    }
    let header = RunHeader {
        problem: problem.id().into(),
        seed: o.seed,
        lengths: o.lengths.clone(),
        restarts: o.restarts,
        rung_iters: o.iters,
        hp,
    };
    let path = std::path::PathBuf::from(
        o.trace.unwrap_or_else(|| format!("runs/{cmd}-{:#x}.jsonl", o.seed)),
    );
    std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new("."))).expect("create trace dir");

    // Resolve fresh-vs-resume BEFORE opening the file for append.
    let resume = if path.exists() && !o.fresh {
        let scan = scan_for_resume(&path).unwrap_or_else(|e| {
            eprintln!("cannot resume from {}: {e}\n(pass --fresh to discard it)", path.display());
            std::process::exit(1);
        });
        if scan.header != header {
            eprintln!(
                "{} belongs to a different run (header mismatch):\n  file: {:?}\n  args: {:?}\n\
                 rerun with the original parameters, another --trace, or --fresh.",
                path.display(),
                scan.header,
                header
            );
            std::process::exit(1);
        }
        match scan.snapshot {
            Some(s) => {
                eprintln!(
                    "resuming {} at rung#{} attempt {} iter {}",
                    path.display(),
                    s.frontier,
                    s.attempt,
                    s.iter
                );
                Some(s)
            }
            None => {
                eprintln!(
                    "{} has a matching header but no snapshot yet; pass --fresh to restart it.",
                    path.display()
                );
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(resume.is_some())
        .truncate(resume.is_none())
        .write(true)
        .open(&path)
        .expect("open trace file");
    let sink = std::sync::Mutex::new(std::io::BufWriter::new(file));
    if resume.is_none() {
        let mut w = sink.lock().unwrap();
        writeln!(w, "{}", header_json(&header)).unwrap();
        w.flush().unwrap();
    }

    // Ctrl-C -> stop flag: the engine snapshots at the next checkpoint barrier
    // and returns. The handler stays installed, so a second Ctrl-C (impatient
    // user, wedged run) exits hard from the handler itself.
    static STOP: AtomicBool = AtomicBool::new(false);
    ctrlc::set_handler(|| {
        if STOP.swap(true, Ordering::SeqCst) {
            eprintln!("\nsecond Ctrl-C — exiting immediately (snapshot may be stale)");
            std::process::exit(130);
        }
        eprintln!("\nCtrl-C — stopping at the next checkpoint (snapshot will be saved)...");
    })
    .expect("install Ctrl-C handler");

    let (dataset, groups) = problem.dataset(&o.lengths);
    eprintln!(
        "=== {cmd} === {} | lengths {:?} ({} seqs) densify={} seed={:#x}",
        problem.describe(),
        o.lengths,
        dataset.len(),
        hp.densify_w,
        o.seed
    );
    eprintln!("trace -> {}", path.display());

    let space = problem.space();
    let template = problem.template();
    let lens = o.lengths.clone();
    let on_trace = |ev: &TraceEvent| {
        let line = trace_json(ev);
        let mut w = sink.lock().unwrap();
        let _ = writeln!(w, "{line}");
        // Snapshots are the resume state — make each one durable immediately,
        // so even a SIGKILL after this point loses nothing.
        if matches!(ev, TraceEvent::Snapshot { .. }) {
            let _ = w.flush();
        }
    };
    let (champ, cost, solved) = synthesize_curriculum_gated(
        &template,
        &space,
        &dataset,
        &groups,
        o.lengths.len(),
        &hp,
        o.restarts,
        o.iters,
        0.0,
        o.seed,
        Some(&on_trace),
        resume,
        Some(&STOP),
        |frontier, rc, front_err, ok| {
            eprintln!(
                "{cmd} RUNG L={:<2} {} front_err={front_err:6.1} size={} {}",
                lens[frontier],
                if ok { "SOLVED" } else { "STALL " },
                rc.size(),
                rc.brief()
            );
        },
    );
    sink.lock().unwrap().flush().unwrap();

    if STOP.load(Ordering::SeqCst) {
        eprintln!(
            "interrupted — snapshot saved in {}; rerun the same command to resume.",
            path.display()
        );
        std::process::exit(130);
    }

    let gates = problem.gates(&champ);
    let gates_str: Vec<String> = gates.iter().map(|g| format!("{}={}", g.label, g.verdict)).collect();
    eprintln!(
        "{cmd} DONE solved {solved}/{} cost={cost:.1} size={} [{}] {}",
        o.lengths.len(),
        champ.size(),
        gates_str.join(" "),
        champ.brief()
    );

    // Self-describing result summary next to the trace.
    let git_rev = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    let result = serde_json::json!({
        "run": header,
        "solved": solved,
        "n_groups": o.lengths.len(),
        "full_cost": cost,
        "champion": {
            "brief": champ.brief(),
            "size": champ.size(),
            "class": spec_classify(&champ),
            "words": champ.assemble().iter().map(|w| format!("{w:#06x}")).collect::<Vec<_>>(),
            "wrap": [champ.wrap_bottom, champ.wrap_top],
        },
        "gates": gates.iter().map(|g| serde_json::json!({
            "label": g.label, "verdict": g.verdict, "pass": g.pass,
        })).collect::<Vec<_>>(),
        "git_rev": git_rev,
    });
    let result_path = path.with_extension("result.json");
    std::fs::write(&result_path, serde_json::to_string_pretty(&result).unwrap()).expect("write result");
    eprintln!("result -> {}", result_path.display());
}

fn run_compress(problem: &dyn Problem, cmd: &str, o: LadderOpts) {
    use pio_superopt::fixtures::{dme_corpus, spec_certify_corpus};
    use pio_superopt::search::synthesize_compress;
    let mut hp = problem.default_hp();
    if let Some(d) = o.densify {
        hp.densify_w = d;
    }
    let header = RunHeader {
        problem: problem.id().into(),
        seed: o.seed,
        lengths: o.lengths.clone(),
        restarts: o.restarts,
        rung_iters: o.iters,
        hp,
    };
    let path = std::path::PathBuf::from(
        o.trace.unwrap_or_else(|| format!("runs/{cmd}-{:#x}.jsonl", o.seed)),
    );
    std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new("."))).expect("create trace dir");

    let resume = if path.exists() && !o.fresh {
        let scan = scan_for_resume(&path).unwrap_or_else(|e| {
            eprintln!("cannot resume from {}: {e}\n(pass --fresh to discard it)", path.display());
            std::process::exit(1);
        });
        if scan.header != header {
            eprintln!(
                "{} belongs to a different run (header mismatch):\n  file: {:?}\n  args: {:?}\n\
                 rerun with the original parameters, another --trace, or --fresh.",
                path.display(),
                scan.header,
                header
            );
            std::process::exit(1);
        }
        match scan.snapshot {
            Some(s) => {
                eprintln!(
                    "resuming {} at cycle#{} iter {} (champ size {})",
                    path.display(),
                    s.frontier,
                    s.iter,
                    s.champ.as_ref().map(|c| c.size()).unwrap_or(0)
                );
                Some(s)
            }
            None => {
                eprintln!(
                    "{} has a matching header but no snapshot yet; pass --fresh to restart it.",
                    path.display()
                );
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(resume.is_some())
        .truncate(resume.is_none())
        .write(true)
        .open(&path)
        .expect("open trace file");
    let sink = std::sync::Mutex::new(std::io::BufWriter::new(file));
    if resume.is_none() {
        let mut w = sink.lock().unwrap();
        writeln!(w, "{}", header_json(&header)).unwrap();
        w.flush().unwrap();
    }

    static STOP: AtomicBool = AtomicBool::new(false);
    ctrlc::set_handler(|| {
        if STOP.swap(true, Ordering::SeqCst) {
            eprintln!("\nsecond Ctrl-C — exiting immediately (snapshot may be stale)");
            std::process::exit(130);
        }
        eprintln!("\nCtrl-C — stopping at the next checkpoint (snapshot will be saved)...");
    })
    .expect("install Ctrl-C handler");

    let (dataset, groups) = problem.dataset(&o.lengths);
    let seed_prog = problem.template();
    eprintln!(
        "=== {cmd} === {} | lengths {:?} ({} seqs) densify={} seed={:#x} | seed program size={} [{}]",
        problem.describe(),
        o.lengths,
        dataset.len(),
        hp.densify_w,
        o.seed,
        seed_prog.size(),
        seed_prog.brief()
    );
    eprintln!("trace -> {}", path.display());

    let space = problem.space();
    let on_trace = |ev: &TraceEvent| {
        let line = trace_json(ev);
        let mut w = sink.lock().unwrap();
        let _ = writeln!(w, "{line}");
        if matches!(ev, TraceEvent::Snapshot { .. }) {
            let _ = w.flush();
        }
    };
    // The champion gate: the TRAIN certifier corpus only — held-out stays an
    // independent end-of-run report, never a selection signal.
    let corpus = dme_corpus();
    let certify = move |p: &pio_superopt::Program| spec_certify_corpus(p, &corpus) == 0;
    let champ = synthesize_compress(
        &seed_prog,
        &space,
        &dataset,
        &groups,
        o.lengths.len(),
        &hp,
        o.restarts,
        o.iters,
        o.cycles,
        o.seed,
        Some(&on_trace),
        resume,
        Some(&STOP),
        &certify,
        |cycle, champ, improved| {
            if improved {
                eprintln!(
                    "{cmd} CYCLE {cycle:<5} SHRUNK size={} [{}]",
                    champ.size(),
                    champ.brief()
                );
            } else if cycle % 10 == 0 {
                eprintln!("{cmd} CYCLE {cycle:<5} champ size={} (no change)", champ.size());
            }
        },
    );
    sink.lock().unwrap().flush().unwrap();

    if STOP.load(Ordering::SeqCst) {
        eprintln!(
            "interrupted — snapshot saved in {}; rerun the same command to resume.",
            path.display()
        );
        std::process::exit(130);
    }

    let gates = problem.gates(&champ);
    let gates_str: Vec<String> = gates.iter().map(|g| format!("{}={}", g.label, g.verdict)).collect();
    eprintln!(
        "{cmd} DONE size {} -> {} [{}] {}",
        seed_prog.size(),
        champ.size(),
        gates_str.join(" "),
        champ.brief()
    );

    let git_rev = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    let result = serde_json::json!({
        "run": header,
        "seed_size": seed_prog.size(),
        "champion": {
            "brief": champ.brief(),
            "size": champ.size(),
            "class": spec_classify(&champ),
            "words": champ.assemble().iter().map(|w| format!("{w:#06x}")).collect::<Vec<_>>(),
            "wrap": [champ.wrap_bottom, champ.wrap_top],
        },
        "gates": gates.iter().map(|g| serde_json::json!({
            "label": g.label, "verdict": g.verdict, "pass": g.pass,
        })).collect::<Vec<_>>(),
        "git_rev": git_rev,
    });
    let result_path = path.with_extension("result.json");
    std::fs::write(&result_path, serde_json::to_string_pretty(&result).unwrap()).expect("write result");
    eprintln!("result -> {}", result_path.display());
}

/// Exhaustive loop-body sweep (see `enumerate.rs` module doc). Sharded by
/// first-slot op; one JSON per completed shard in --out, existing shards are
/// skipped (resume), --shard-mod/--shard-rem split shards across machines.
fn enumerate_cmd(args: &[String]) {
    use pio_superopt::enumerate::{alphabet, run_shard};
    let (mut len, mut shard_mod, mut shard_rem, mut out): (usize, usize, usize, Option<String>) =
        (4, 1, 0, None);
    let mut threads: usize = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage()).clone();
        match a.as_str() {
            "--len" => len = val().parse().unwrap_or_else(|_| usage()),
            "--shard-mod" => shard_mod = val().parse().unwrap_or_else(|_| usage()),
            "--shard-rem" => shard_rem = val().parse().unwrap_or_else(|_| usage()),
            "--threads" => threads = val().parse().unwrap_or_else(|_| usage()),
            "--out" => out = Some(val()),
            _ => usage(),
        }
    }
    assert!(len >= 2 && len <= 8, "--len must be in 2..=8");
    assert!(shard_rem < shard_mod, "--shard-rem must be < --shard-mod");
    let dir = std::path::PathBuf::from(out.unwrap_or_else(|| format!("runs/enum-len{len}")));
    std::fs::create_dir_all(&dir).expect("create out dir");

    let ops = alphabet(len);
    let todo: Vec<usize> = (0..ops.len())
        .filter(|s| s % shard_mod == shard_rem)
        .filter(|s| !dir.join(format!("shard-{s:04}.json")).exists())
        .collect();
    let total_mine = (0..ops.len()).filter(|s| s % shard_mod == shard_rem).count();
    eprintln!(
        "=== enumerate === len={len} alphabet={} shards {}/{} to do (mod {shard_mod} rem {shard_rem}) threads={threads}",
        ops.len(),
        todo.len(),
        total_mine
    );
    eprintln!("out -> {}  (one JSON per shard; rerun to resume; Ctrl-C safe between shards)", dir.display());

    // Graceful shutdown: first signal aborts in-flight shards (their work is
    // discarded; completed shard files are durable), second exits hard.
    static ESTOP: AtomicBool = AtomicBool::new(false);
    ctrlc::set_handler(|| {
        if ESTOP.swap(true, Ordering::SeqCst) {
            std::process::exit(130);
        }
        eprintln!("\nsignal — aborting in-flight shards (completed shards are saved; rerun to resume)...");
    })
    .expect("install signal handler");

    let queue = std::sync::Mutex::new(todo.into_iter().collect::<std::collections::VecDeque<usize>>());
    let done = std::sync::atomic::AtomicUsize::new(0);
    let t0 = std::time::Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| loop {
                if ESTOP.load(Ordering::SeqCst) {
                    return;
                }
                let shard = match queue.lock().unwrap().pop_front() {
                    Some(s) => s,
                    None => return,
                };
                let res = match run_shard(shard, len, &ops, Some(&ESTOP)) {
                    Some(r) => r,
                    None => return, // aborted mid-shard — nothing written
                };
                let path = dir.join(format!("shard-{shard:04}.json"));
                let tmp = dir.join(format!("shard-{shard:04}.json.tmp"));
                std::fs::write(&tmp, serde_json::to_string_pretty(&res).unwrap()).expect("write shard");
                std::fs::rename(&tmp, &path).expect("finalize shard");
                let d = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                eprintln!(
                    "[shard {shard:04}] {} structures, {} screened, {} pattern-pass, {} timing evals, {} SURVIVORS  ({d} shards done, {:.0}s elapsed)",
                    res.structures, res.screened, res.pattern_pass, res.timing_evals, res.survivors.len(),
                    t0.elapsed().as_secs_f64()
                );
                for sv in &res.survivors {
                    eprintln!("  SURVIVOR size={} delays={:?} {}", sv.size, sv.delays, sv.brief);
                }
            });
        }
    });

    // Aggregate every shard file present (this machine's and any copied in).
    let (mut st, mut sc, mut pp, mut te, mut sv, mut shards) = (0u64, 0u64, 0u64, 0u64, 0usize, 0usize);
    for e in std::fs::read_dir(&dir).expect("read out dir") {
        let path = e.expect("dir entry").path();
        if path.extension().and_then(|x| x.to_str()) == Some("json") {
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            shards += 1;
            st += v["structures"].as_u64().unwrap_or(0);
            sc += v["screened"].as_u64().unwrap_or(0);
            pp += v["pattern_pass"].as_u64().unwrap_or(0);
            te += v["timing_evals"].as_u64().unwrap_or(0);
            sv += v["survivors"].as_array().map(|a| a.len()).unwrap_or(0);
        }
    }
    eprintln!(
        "enumerate len={len}: {shards}/{} shards present | {st} structures, {sc} screened, {pp} pattern-pass, {te} timing evals, {sv} survivors",
        ops.len()
    );
}

/// Shard-lease coordinator for a worker fleet (see `coord.rs` module doc and
/// `docs/fleet.md`). Run this on the box that should end up holding all the
/// shard files; point any number of `superopt work` hosts at it. All durable
/// state is the shard files in --out — Ctrl-C just exits, restart rescans.
fn serve_cmd(args: &[String]) {
    use pio_superopt::coord::{serve, Coordinator, ServeCfg};
    use pio_superopt::enumerate::alphabet;
    let (mut len, mut out, mut listen): (usize, Option<String>, String) =
        (4, None, "0.0.0.0:7787".into());
    let mut lease_secs: u64 = 43_200;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage()).clone();
        match a.as_str() {
            "--len" => len = val().parse().unwrap_or_else(|_| usage()),
            "--out" => out = Some(val()),
            "--listen" => listen = val(),
            "--lease-secs" => lease_secs = val().parse().unwrap_or_else(|_| usage()),
            _ => usage(),
        }
    }
    assert!(len >= 2 && len <= 8, "--len must be in 2..=8");
    let dir = std::path::PathBuf::from(out.unwrap_or_else(|| format!("runs/enum-len{len}")));
    std::fs::create_dir_all(&dir).expect("create out dir");

    let ops = alphabet(len);
    let mut coord = Coordinator::new(ops.len(), std::time::Duration::from_secs(lease_secs));
    // Startup rescan: existing shard files are the durable state.
    let mut existing = 0usize;
    for e in std::fs::read_dir(&dir).expect("read out dir") {
        let path = e.expect("dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(num) = name.strip_prefix("shard-").and_then(|s| s.strip_suffix(".json")) {
            if let Ok(shard) = num.parse::<usize>() {
                if shard < ops.len() {
                    coord.mark_done(shard);
                    existing += 1;
                }
            }
        }
    }
    eprintln!(
        "=== serve === len={len} alphabet={} | {existing}/{} shards already done | lease {lease_secs}s | out -> {}",
        ops.len(),
        ops.len(),
        dir.display()
    );
    let server = tiny_http::Server::http(&listen)
        .unwrap_or_else(|e| panic!("cannot listen on {listen}: {e}"));
    eprintln!(
        "listening on {listen} (durable state = shard files; Ctrl-C any time, restart rescans)"
    );
    serve(&server, &mut coord, &ServeCfg { len, alphabet: ops.len(), out_dir: dir });
}

/// ureq call with exponential backoff on TRANSPORT errors (server down /
/// restarting), forever — a worker holding a finished result must be able to
/// outwait a server restart. HTTP status errors are returned to the caller.
fn retry_transport<T>(
    what: &str,
    mut f: impl FnMut() -> Result<T, ureq::Error>,
) -> Result<T, u16> {
    let mut delay = std::time::Duration::from_secs(1);
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(ureq::Error::Status(code, resp)) => {
                eprintln!("{what}: HTTP {code}: {}", resp.into_string().unwrap_or_default());
                return Err(code);
            }
            Err(ureq::Error::Transport(t)) => {
                eprintln!("{what}: {t}; retrying in {}s", delay.as_secs());
                std::thread::sleep(delay);
                delay = (delay * 2).min(std::time::Duration::from_secs(120));
            }
        }
    }
}

/// Worker: pull shard leases from a `superopt serve` coordinator, run them,
/// post results back. Exits 0 once the server reports nothing left to lease.
/// Join/leave freely — an abandoned lease expires server-side and requeues.
fn work_cmd(args: &[String]) {
    use pio_superopt::enumerate::{alphabet, run_shard};
    let mut server: Option<String> = None;
    let mut threads: usize = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage()).clone();
        match a.as_str() {
            "--server" => server = Some(val()),
            "--threads" => threads = val().parse().unwrap_or_else(|_| usage()),
            _ => usage(),
        }
    }
    let base = server.unwrap_or_else(|| usage()).trim_end_matches('/').to_string();

    // Learn len + alphabet size from the server, then check the contract:
    // our alphabet(len) must be the same size (same-commit invariant; the
    // ORDER can't be checked cheaply, so ship one binary — see fleet.md).
    let status = retry_transport("GET /status", || {
        ureq::get(&format!("{base}/status")).call()
    })
    .unwrap_or_else(|code| {
        eprintln!("cannot fetch {base}/status (HTTP {code})");
        std::process::exit(1);
    });
    let v: serde_json::Value =
        serde_json::from_str(&status.into_string().expect("status body")).expect("status JSON");
    let len = v["len"].as_u64().expect("status.len") as usize;
    let srv_alpha = v["alphabet"].as_u64().expect("status.alphabet") as usize;
    let ops = alphabet(len);
    if ops.len() != srv_alpha {
        eprintln!(
            "contract mismatch: server len={len} alphabet={srv_alpha}, this binary's alphabet({len}) = {} \
             — mixed revisions silently mislabel shards; run the same commit/binary everywhere.",
            ops.len()
        );
        std::process::exit(1);
    }
    eprintln!(
        "=== work === server {base} | len={len} alphabet={srv_alpha} | done {} leased {} remaining {} | threads={threads}",
        v["done"], v["leased"], v["remaining"]
    );

    // GRACEFUL SHUTDOWN (SIGINT/SIGTERM/SIGHUP):
    //   1st signal -> DRAIN: stop leasing; in-flight shards finish and post.
    //   2nd signal -> ABORT: in-flight shards stop (~20ms), their leases are
    //                 RELEASED back to the server so no shard sits blocked
    //                 for the lease TTL.
    //   3rd signal -> hard exit.
    static DRAIN: AtomicBool = AtomicBool::new(false);
    static ABORT: AtomicBool = AtomicBool::new(false);
    ctrlc::set_handler(|| {
        if ABORT.load(Ordering::SeqCst) {
            std::process::exit(130);
        }
        if DRAIN.swap(true, Ordering::SeqCst) {
            ABORT.store(true, Ordering::SeqCst);
            eprintln!("\nsignal #2 — aborting in-flight shards and releasing their leases...");
        } else {
            eprintln!("\nsignal — draining: no new leases; in-flight shards will finish and upload (signal again to abort them)");
        }
    })
    .expect("install signal handler");

    let lease_body = format!("{{\"len\":{len},\"alphabet\":{}}}", ops.len());
    let done_count = std::sync::atomic::AtomicUsize::new(0);
    let t0 = std::time::Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| loop {
                if DRAIN.load(Ordering::SeqCst) {
                    return;
                }
                let resp = match retry_transport("POST /lease", || {
                    ureq::post(&format!("{base}/lease")).send_string(&lease_body)
                }) {
                    Ok(r) => r,
                    Err(409) => std::process::exit(1), // contract refused mid-run
                    Err(_) => {
                        // Unexpected status; don't spin hot against a broken server.
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        continue;
                    }
                };
                if resp.status() == 204 {
                    return; // drained — nothing leasable right now or ever
                }
                let v: serde_json::Value =
                    serde_json::from_str(&resp.into_string().expect("lease body"))
                        .expect("lease JSON");
                let shard = v["shard"].as_u64().expect("lease.shard") as usize;
                let res = match run_shard(shard, len, &ops, Some(&ABORT)) {
                    Some(r) => r,
                    None => {
                        // Aborted: hand the lease back so the shard is
                        // immediately leasable elsewhere (best-effort — if
                        // the server is unreachable the TTL still covers it).
                        let _ = ureq::post(&format!("{base}/release?shard={shard}")).send_string("");
                        eprintln!("[shard {shard:04}] aborted, lease released");
                        return;
                    }
                };
                let body = serde_json::to_string_pretty(&res).unwrap();
                if let Err(code) = retry_transport(&format!("POST /done?shard={shard}"), || {
                    ureq::post(&format!("{base}/done?shard={shard}")).send_string(&body)
                }) {
                    eprintln!("[shard {shard:04}] server refused result (HTTP {code}) — dropping; the lease will expire and requeue");
                    continue;
                }
                let d = done_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                eprintln!(
                    "[shard {shard:04}] {} structures, {} screened, {} pattern-pass, {} timing evals, {} SURVIVORS  ({d} shards done, {:.0}s elapsed)",
                    res.structures, res.screened, res.pattern_pass, res.timing_evals, res.survivors.len(),
                    t0.elapsed().as_secs_f64()
                );
                for sv in &res.survivors {
                    eprintln!("  SURVIVOR size={} delays={:?} {}", sv.size, sv.delays, sv.brief);
                }
            });
        }
    });
    eprintln!(
        "work drained: {} shards completed by this host in {:.0}s",
        done_count.load(std::sync::atomic::Ordering::SeqCst),
        t0.elapsed().as_secs_f64()
    );
}

// ---------------------------------------------------------------------
// narrow-split: the narrowing engine's bracket searches (tx_a wrap
// brackets), migrated from the #[ignore] test path with UNIT-LEVEL
// resume. Same computation as `narrow::engine::search_split` — phase 1
// (`split_units`) deterministically enumerates truncated-spec frontier
// units, phase 2 runs each unit as an independent seeded search
// (`search_unit`) — but the runner writes one JSONL line per settled
// unit and, on restart with the same arguments, re-enumerates the
// frontier and skips every unit already in the trace. The final
// aggregate (item total, champion set, verdict) is byte-identical to
// an uninterrupted run because unit verdicts are scheduling-independent
// and the merge is in unit-id order.
//
// Trace records (one JSON object per line):
//   line 1   {"narrow_split":{spec identity + engine git rev}}  — the
//            resume contract; a mismatched header REFUSES to resume.
//   unit     {"unit":i,"ms":..,"items":..,"champions":[..],"stats":{..}}
//   telem    {"telem":{..}} every ~60s: settled-aggregate item/fork/memo
//            counters for tail analysis (distinct type; resume skips it).
//   done     {"done":{..}} final aggregate.
// Lines are written whole under a mutex and flushed line-atomically;
// fsync every ~5s and at signal exit. SIGINT/SIGTERM: settled units are
// already durable, in-flight units are abandoned (they simply re-run on
// resume — partial unit lines are never written), the trace is synced,
// exit 130. A torn final line (SIGKILL mid-write) is truncated away on
// the next resume scan.

/// FNV-1a 64 over the spec's Debug rendering: a stable, dependency-free
/// spec fingerprint for the resume header (belt and braces on top of
/// the explicit len/wrap/cycles fields — catches fixture drift).
fn fnv1a64(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn git_rev_dirty() -> (String, bool) {
    let rev = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain", "-uno"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    (rev, dirty)
}

/// One settled unit as recovered from the trace.
struct NarrowUnitRec {
    champions: Vec<pio_superopt::narrow::engine::Champion>,
    stats: pio_superopt::narrow::engine::Stats,
    cap_hit: bool,
}

/// Scan a narrow-split trace: returns (header, settled units, byte
/// length of the last complete line). A torn FINAL line is tolerated
/// (reported via the returned length, truncated by the caller before
/// appending); torn interior lines or duplicate unit ids are errors.
fn scan_narrow_trace(
    path: &std::path::Path,
) -> Result<(serde_json::Value, std::collections::BTreeMap<usize, NarrowUnitRec>, u64), String> {
    let data = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut header: Option<serde_json::Value> = None;
    let mut units = std::collections::BTreeMap::new();
    let mut good_len = 0u64;
    let mut offset = 0usize;
    for line in data.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let complete = line.ends_with('\n');
        let body = line.trim_end();
        if body.is_empty() {
            if complete {
                good_len = offset as u64;
            }
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => {
                if offset == data.len() {
                    // Torn tail (killed mid-write) — resume just before it.
                    eprintln!(
                        "note: {} has a torn final line ({} bytes) — dropping it (its unit re-runs)",
                        path.display(),
                        line.len()
                    );
                    break;
                }
                return Err(format!("{}: bad line at byte {start}: {e}", path.display()));
            }
        };
        if !complete && offset == data.len() {
            // Parsed but unterminated: treat as torn (an append could
            // otherwise glue the next record onto it).
            eprintln!("note: {} final line unterminated — dropping it", path.display());
            break;
        }
        good_len = offset as u64;
        if header.is_none() {
            if v.get("narrow_split").is_none() {
                return Err(format!("{}: not a narrow-split trace (bad header)", path.display()));
            }
            header = Some(v);
            continue;
        }
        if let Some(u) = v.get("unit").and_then(|u| u.as_u64()) {
            let champions = serde_json::from_value(v["champions"].clone())
                .map_err(|e| format!("unit {u}: bad champions: {e}"))?;
            let stats = serde_json::from_value(v["stats"].clone())
                .map_err(|e| format!("unit {u}: bad stats: {e}"))?;
            let cap_hit = v["cap_hit"].as_bool().unwrap_or(false);
            if units
                .insert(u as usize, NarrowUnitRec { champions, stats, cap_hit })
                .is_some()
            {
                return Err(format!(
                    "{}: unit {u} appears twice — was this trace written by two processes at once?",
                    path.display()
                ));
            }
        }
        // telem / done records: observability only, nothing to recover.
    }
    let header = header.ok_or_else(|| format!("{}: empty trace (pass --fresh)", path.display()))?;
    Ok((header, units, good_len))
}

/// The trace sink: whole-line writes under one lock, flushed per line
/// (a line present in the file is complete), fsynced at most every ~5s
/// plus at exit — a SIGKILL can tear only the final line, which the
/// resume scan drops.
struct NarrowSink {
    w: std::io::BufWriter<std::fs::File>,
    last_sync: std::time::Instant,
}

impl NarrowSink {
    fn line(&mut self, s: &str) {
        writeln!(self.w, "{s}").expect("write trace line");
        self.w.flush().expect("flush trace");
        if self.last_sync.elapsed().as_secs() >= 5 {
            let _ = self.w.get_ref().sync_data();
            self.last_sync = std::time::Instant::now();
        }
    }
    fn sync(&mut self) {
        let _ = self.w.flush();
        let _ = self.w.get_ref().sync_data();
    }
}

/// `superopt narrow-split --spec tx-a --len 3 --wrap-lo 1 --wrap-hi 2
/// [--cycles 460] [--threads T] [--target N] [--champion-cap 5]
/// [--memo-cap 2097152] [--trace PATH] [--fresh]`
///
/// Reproduces the #[ignore]-test bracket searches (`tx_a_l3_*` in
/// tests/narrow_engine.rs) through the resumable runner. The header
/// pins spec identity (spec/len/wrap/cycles/caps/target/spec-hash) AND
/// the engine git rev — resuming across a mismatched header is refused
/// (unit verdicts from different engine revisions must never be mixed;
/// pass --fresh or another --trace). `--threads` is deliberately NOT
/// part of the identity: unit verdicts are scheduling-independent, so a
/// resume may use a different thread count — but the unit DECOMPOSITION
/// depends on `--target` (default max(128*threads, 512), the frontier
/// unit target), so resuming with different threads requires pinning
/// --target to the original run's value (recorded in the header).
/// Env-gated engine probes (PIO_NARROW_*) behave exactly as under the
/// test path's `search_split`: active in instrumented sequential
/// searches only — split workers run uninstrumented by design.
fn narrow_split_cmd(args: &[String]) {
    use pio_superopt::fixtures::{tx_a_narrow_spec, tx_a_narrow_words};
    use pio_superopt::narrow::engine::{
        merge_stats, run_spec, search_unit, split_units, SplitPlan, Stats, WordQuotient,
    };
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    let mut spec_name = "tx-a".to_string();
    let (mut len, mut wrap_lo, mut wrap_hi): (Option<u8>, Option<u8>, Option<u8>) = (None, None, None);
    let mut cycles: u32 = 460;
    let mut threads: usize = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    let mut target: usize = 0; // 0 = auto
    let mut champion_cap: usize = 5;
    let mut memo_cap: usize = 1 << 21;
    let mut trace: Option<String> = None;
    let mut fresh = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage()).clone();
        match a.as_str() {
            "--spec" => spec_name = val(),
            "--len" => len = Some(val().parse().unwrap_or_else(|_| usage())),
            "--wrap-lo" => wrap_lo = Some(val().parse().unwrap_or_else(|_| usage())),
            "--wrap-hi" => wrap_hi = Some(val().parse().unwrap_or_else(|_| usage())),
            "--cycles" => cycles = val().parse().unwrap_or_else(|_| usage()),
            "--threads" => threads = val().parse().unwrap_or_else(|_| usage()),
            "--target" => target = val().parse().unwrap_or_else(|_| usage()),
            "--champion-cap" => champion_cap = val().parse().unwrap_or_else(|_| usage()),
            "--memo-cap" => memo_cap = val().parse().unwrap_or_else(|_| usage()),
            "--trace" => trace = Some(val()),
            "--fresh" => fresh = true,
            _ => usage(),
        }
    }
    let (len, wrap_lo, wrap_hi) = match (len, wrap_lo, wrap_hi) {
        (Some(l), Some(a), Some(b)) => (l, a, b),
        _ => usage(),
    };
    assert!(wrap_lo <= wrap_hi && wrap_hi < len, "need wrap-lo <= wrap-hi < len");
    let threads = threads.max(1);
    // Same formula as search_split — byte-identical decomposition.
    let target = if target == 0 { (threads * 128).max(512) } else { target };

    // Spec construction, mirroring the test path exactly: expected
    // trace from the reference program at its own shape (L=4 wrap
    // 0..1), then the bracket spec at --len/--wrap over that trace.
    let spec = match spec_name.as_str() {
        "tx-a" => {
            let (mut spec4, side) = tx_a_narrow_spec(cycles);
            spec4.expected = run_spec(&spec4, tx_a_narrow_words(&side));
            let (mut s, _) = tx_a_narrow_spec(cycles);
            s.slots = len;
            s.cfg.wrap_bottom = wrap_lo;
            s.cfg.wrap_top = wrap_hi;
            s.expected = spec4.expected;
            s.memo_cap = memo_cap;
            s
        }
        other => {
            eprintln!("unknown --spec {other:?} (available: tx-a)");
            std::process::exit(2);
        }
    };

    let (git_rev, dirty) = git_rev_dirty();
    let header = serde_json::json!({ "narrow_split": {
        "spec": spec_name,
        "len": len,
        "wrap": [wrap_lo, wrap_hi],
        "cycles": cycles,
        "champion_cap": champion_cap,
        "memo_cap": memo_cap,
        "target": target,
        "spec_hash": format!("fnv1a:{:016x}", fnv1a64(&format!("{spec:?}"))),
        "git_rev": git_rev,
        "dirty": dirty,
    }});

    let path = std::path::PathBuf::from(trace.unwrap_or_else(|| {
        format!("runs/narrow-split-{spec_name}-l{len}-w{wrap_lo}-{wrap_hi}.jsonl")
    }));
    std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new("."))).expect("create trace dir");

    // Resolve fresh-vs-resume BEFORE opening for append.
    let mut settled = std::collections::BTreeMap::new();
    let mut resume = false;
    if path.exists() && !fresh && std::fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false) {
        let (old_header, units, good_len) = scan_narrow_trace(&path).unwrap_or_else(|e| {
            eprintln!("cannot resume from {}: {e}\n(pass --fresh to discard it)", path.display());
            std::process::exit(1);
        });
        if old_header != header {
            eprintln!(
                "{} belongs to a different run (header mismatch — unit verdicts must not be \
                 mixed across specs or engine revisions):\n  file: {old_header}\n  args: {header}\n\
                 rerun with the original parameters/binary, another --trace, or --fresh.",
                path.display()
            );
            std::process::exit(1);
        }
        if header["narrow_split"]["dirty"].as_bool() == Some(true) {
            eprintln!(
                "warning: resuming with a DIRTY working tree — the header's git rev cannot \
                 prove the engine is unchanged; verdict validity is on you."
            );
        }
        // Drop a torn tail so appended lines start clean.
        if good_len < std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) {
            let f = std::fs::OpenOptions::new().write(true).open(&path).expect("open for truncate");
            f.set_len(good_len).expect("truncate torn tail");
        }
        settled = units;
        resume = true;
    }

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(resume)
        .truncate(!resume)
        .write(true)
        .open(&path)
        .expect("open trace file");
    let sink = std::sync::Arc::new(std::sync::Mutex::new(NarrowSink {
        w: std::io::BufWriter::new(file),
        last_sync: std::time::Instant::now(),
    }));
    if !resume {
        let mut s = sink.lock().unwrap();
        s.line(&header.to_string());
        s.sync();
    }

    // SIGINT/SIGTERM: settled units are already durable (per-line
    // flush); sync the trace and exit — in-flight units are abandoned
    // and re-run on resume. A second signal exits without the sync.
    static STOP: AtomicBool = AtomicBool::new(false);
    {
        let sink = sink.clone();
        let shown = path.display().to_string();
        ctrlc::set_handler(move || {
            if STOP.swap(true, Ordering::SeqCst) {
                std::process::exit(130);
            }
            eprintln!(
                "\nsignal — abandoning in-flight units (their trace lines were never written); \
                 settled units are durable in {shown}. Rerun the same command to resume."
            );
            if let Ok(mut s) = sink.lock() {
                s.sync();
            }
            std::process::exit(130);
        })
        .expect("install signal handler");
    }

    eprintln!(
        "=== narrow-split === {spec_name} L={len} wrap {wrap_lo}..{wrap_hi} cycles={cycles} \
         | champion_cap={champion_cap} memo_cap={memo_cap} | target {target} on {threads} threads"
    );
    eprintln!("trace -> {}", path.display());

    let t0 = std::time::Instant::now();
    eprintln!("phase 1: enumerating the frontier (deterministic; re-derived on every resume)...");
    let su = match split_units(&spec, target) {
        SplitPlan::Sequential => {
            // Frontier never widened to target: one unit IS the whole
            // search (an empty seed = the full seeded space).
            eprintln!("phase 1: frontier never reached target — running as a single unit");
            pio_superopt::narrow::engine::SplitUnits {
                seeds: vec![(vec![], vec![])],
                phase1: Stats::default(),
                frontier_cycle: 0,
                pre_mirror: 0,
            }
        }
        SplitPlan::Refuted(r) => {
            eprintln!(
                "phase 1: REFUTED on a trace prefix — the full space is empty \
                 (items={} in {:.1}s)",
                r.stats.items,
                t0.elapsed().as_secs_f64()
            );
            narrow_split_finish(
                &path, &sink, &header, &spec, spec_name.as_str(), len, wrap_lo, wrap_hi, threads,
                champion_cap, Vec::new(), r.stats, false, t0,
            );
            return;
        }
        SplitPlan::Units(su) => su,
    };
    let n_units = su.seeds.len();
    if let Some((&bad, _)) = settled.iter().find(|(&i, _)| i >= n_units) {
        eprintln!(
            "{}: trace has unit {bad} but the re-enumerated frontier has only {n_units} units \
             — same header yet different frontier (engine drift?); refusing. Pass --fresh.",
            path.display()
        );
        std::process::exit(1);
    }
    let todo: Vec<usize> = (0..n_units).filter(|i| !settled.contains_key(i)).collect();
    eprintln!(
        "phase 1: {n_units} units (frontier cycle {}, {} pre-mirror) in {:.1}s | {} settled in trace, {} to run",
        su.frontier_cycle,
        su.pre_mirror,
        t0.elapsed().as_secs_f64(),
        settled.len(),
        todo.len()
    );

    // Running aggregate over everything settled (phase 1 + resumed +
    // this run), for telemetry lines; the FINAL aggregate is re-merged
    // in unit order below for determinism.
    let mut agg0 = su.phase1.clone();
    for r in settled.values() {
        merge_stats(&mut agg0, &r.stats);
    }
    let agg = std::sync::Mutex::new(agg0);
    let results: std::sync::Mutex<Vec<(usize, pio_superopt::narrow::engine::SearchResult)>> =
        std::sync::Mutex::new(Vec::with_capacity(todo.len()));
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(settled.len());
    let live = AtomicU64::new(0);
    let t1 = std::time::Instant::now();
    let start_settled = settled.len();
    let spec_ref = &spec;
    let seeds = &su.seeds;
    let todo_ref = &todo;

    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                // Config-only scratch: build once per worker, lend to
                // every unit (see search_split).
                let mut wq = WordQuotient::build(&spec_ref.cfg);
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&i) = todo_ref.get(k) else { break };
                    let tu = std::time::Instant::now();
                    let r = search_unit(spec_ref, &seeds[i], champion_cap, Some(&live), &mut wq);
                    let line = serde_json::json!({
                        "unit": i,
                        "ms": tu.elapsed().as_millis() as u64,
                        "items": r.stats.items,
                        "refuted": r.stats.refuted,
                        "champions_found": r.stats.champions_found,
                        "memo_hits": r.stats.memo_hits,
                        "memo_entries": r.stats.memo_entries,
                        "cap_hit": r.champion_cap_hit,
                        "champions": r.champions,
                        "stats": r.stats,
                    });
                    // One whole line per settled unit, atomically; a
                    // unit is either fully in the trace or absent.
                    sink.lock().unwrap().line(&line.to_string());
                    merge_stats(&mut agg.lock().unwrap(), &r.stats);
                    results.lock().unwrap().push((i, r));
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        // Coordinator: stderr heartbeat (~10s) + trace telemetry (~60s).
        let mut last_hb = std::time::Instant::now();
        let mut last_tel = std::time::Instant::now();
        while done.load(Ordering::Relaxed) < n_units {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let d = done.load(Ordering::Relaxed);
            if last_hb.elapsed().as_secs() >= 10 {
                let el = t1.elapsed().as_secs_f64();
                let this_run = d - start_settled;
                let rate = this_run as f64 / el; // units/s
                let eta = if rate > 0.0 {
                    let s = (n_units - d) as f64 / rate;
                    if s > 5400.0 { format!("~{:.1}h", s / 3600.0) } else { format!("~{:.0}m", s / 60.0) }
                } else {
                    "?".into()
                };
                eprintln!(
                    "narrow-split: {d}/{n_units} units settled, ~{} worker items (live) | {:.1} u/min, ETA {eta}, elapsed {:.0}s",
                    live.load(Ordering::Relaxed),
                    rate * 60.0,
                    el
                );
                last_hb = std::time::Instant::now();
            }
            if last_tel.elapsed().as_secs() >= 60 {
                let a = agg.lock().unwrap().clone();
                let tel = serde_json::json!({ "telem": {
                    "t_s": t1.elapsed().as_secs_f64(),
                    "settled": d,
                    "total": n_units,
                    "live_items": live.load(Ordering::Relaxed),
                    "items": a.items,
                    "refuted": a.refuted,
                    "champions_found": a.champions_found,
                    "fork_kinds": a.fork_kinds,
                    "memo_hits": a.memo_hits,
                    "memo_core_matches": a.memo_core_matches,
                    "memo_entries": a.memo_entries,
                    "memo_purges": a.memo_purges,
                }});
                sink.lock().unwrap().line(&tel.to_string());
                last_tel = std::time::Instant::now();
            }
        }
    });

    // Deterministic merge in unit-id order: resumed units from the
    // trace + this run's results are indistinguishable from an
    // uninterrupted run's per-unit results.
    for (i, r) in results.into_inner().unwrap() {
        settled.insert(
            i,
            NarrowUnitRec { champions: r.champions, stats: r.stats, cap_hit: r.champion_cap_hit },
        );
    }
    assert_eq!(settled.len(), n_units, "settled unit count mismatch after merge");
    let mut champions = Vec::new();
    let mut stats = su.phase1;
    let mut cap_hit = false;
    for rec in settled.values() {
        champions.extend(rec.champions.iter().copied());
        merge_stats(&mut stats, &rec.stats);
        cap_hit |= rec.cap_hit;
    }
    if champions.len() > champion_cap {
        champions.truncate(champion_cap);
        cap_hit = true;
    }
    narrow_split_finish(
        &path, &sink, &header, &spec, spec_name.as_str(), len, wrap_lo, wrap_hi, threads,
        champion_cap, champions, stats, cap_hit, t0,
    );
}

/// Shared tail: append the `done` record, print the verdict in the
/// test-path format (directly comparable to the #[ignore] runs), and
/// write `<trace>.result.json`.
#[allow(clippy::too_many_arguments)]
fn narrow_split_finish(
    path: &std::path::Path,
    sink: &std::sync::Arc<std::sync::Mutex<NarrowSink>>,
    header: &serde_json::Value,
    spec: &pio_superopt::narrow::engine::EngineSpec,
    spec_name: &str,
    len: u8,
    wrap_lo: u8,
    wrap_hi: u8,
    threads: usize,
    champion_cap: usize,
    champions: Vec<pio_superopt::narrow::engine::Champion>,
    stats: pio_superopt::narrow::engine::Stats,
    cap_hit: bool,
    t0: std::time::Instant,
) {
    let secs = t0.elapsed().as_secs_f64();
    let verdict = if stats.champions_found == 0 { "REFUTED" } else { "SATISFIABLE" };
    let done_rec = serde_json::json!({ "done": {
        "verdict": verdict,
        "items": stats.items,
        "forks": stats.forks,
        "refuted": stats.refuted,
        "champions_found": stats.champions_found,
        "cap_hit": cap_hit,
        "memo_hits": stats.memo_hits,
        "secs": secs,
        "champions": champions,
    }});
    {
        let mut s = sink.lock().unwrap();
        s.line(&done_rec.to_string());
        s.sync();
    }
    eprintln!(
        "narrow-split DONE {spec_name} L={len} wrap {wrap_lo}..{wrap_hi} split({threads}): \
         items={} forks={} refuted={} memo_hit={} champions={} cap_hit={cap_hit} in {:.0}s -> {verdict}",
        stats.items, stats.forks, stats.refuted, stats.memo_hits, stats.champions_found, secs
    );
    eprintln!("  benefit_hist: {}", stats.benefit_hist_compact());
    for (i, ch) in champions.iter().take(5).enumerate() {
        let w: Vec<String> =
            ch.words()[..len as usize].iter().map(|w| format!("{w:#06x}")).collect();
        eprintln!("  champion {i}: words {} binding_free={}", w.join(" "), ch.binding_free);
    }
    let result = serde_json::json!({
        "run": header["narrow_split"],
        "verdict": verdict,
        "items": stats.items,
        "champion_cap": champion_cap,
        "cap_hit": cap_hit,
        "champions": champions,
        "champion_words": champions.iter()
            .map(|c| c.words()[..spec.slots as usize].iter().map(|w| format!("{w:#06x}")).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        "stats": stats,
        "secs": secs,
    });
    let result_path = path.with_extension("result.json");
    std::fs::write(&result_path, serde_json::to_string_pretty(&result).unwrap()).expect("write result");
    eprintln!("result -> {}", result_path.display());
}

/// Inspect a trace's LAST snapshot: where the ladder is, and how good the best
/// restart's champion currently is (structural class + the problem's gates).
/// Works mid-run (the engine flushes every snapshot) and post-run.
fn diagnose(args: &[String]) {
    let mut trace: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--trace" => trace = it.next().cloned(),
            _ => usage(),
        }
    }
    let path = std::path::PathBuf::from(trace.unwrap_or_else(|| usage()));
    let scan = scan_for_resume(&path).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let problem = by_id(&scan.header.problem).unwrap_or_else(|| {
        eprintln!("unknown problem id {:?} in header", scan.header.problem);
        std::process::exit(1);
    });
    let Some(snap) = scan.snapshot else {
        eprintln!("{}: no snapshot yet", path.display());
        std::process::exit(1);
    };
    println!(
        "run: {} seed={:#x} lengths {:?} ({}x{} iters)",
        scan.header.problem, scan.header.seed, scan.header.lengths, scan.header.restarts, scan.header.rung_iters
    );
    println!(
        "position: rung#{} (L={}) attempt {} iter {}/{}  solved_through={}",
        snap.frontier,
        scan.header.lengths.get(snap.frontier).copied().unwrap_or(0),
        snap.attempt,
        snap.iter,
        scan.header.rung_iters,
        snap.solved_through
    );
    if let Some(c) = &snap.champ {
        println!("rung warm base (last promoted champion): size={} {}", c.size(), c.brief());
    }
    println!("warm pool {} | macro lib {} | minima {}", snap.pool.len(), snap.lib.len(), snap.minima.len());

    let report = |tag: &str, p: &Program| {
        println!("{tag}: size={} class={} {}", p.size(), spec_classify(p), p.brief());
        for g in problem.gates(p) {
            println!("  {} = {}{}", g.label, g.verdict, if g.pass { "" } else { "  (failing)" });
        }
    };
    match snap.restarts.iter().min_by(|a, b| a.best_sel.partial_cmp(&b.best_sel).unwrap()) {
        Some(best) => report(&format!("best restart (sel={:.1})", best.best_sel), &best.best),
        None => {
            if let Some(c) = &snap.champ {
                report("champion", c);
            }
        }
    }
}
