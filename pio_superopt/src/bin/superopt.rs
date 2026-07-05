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
//!     superopt diagnose --trace PATH
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
       superopt diagnose --trace PATH\n\
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
        Some("diagnose") => diagnose(&args[1..]),
        _ => usage(),
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
