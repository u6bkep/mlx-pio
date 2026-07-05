//! Search runner. First subcommand: the spec-oracle gated curriculum ladder
//! (the long-run workload), with inline-JSONL resumability.
//!
//!     superopt spec-ladder [--seed 0x5EED] [--lengths 2..14] [--restarts 32]
//!                          [--iters 4000000] [--densify 1.0] [--trace PATH]
//!                          [--fresh]
//!
//! The trace (default `runs/spec-ladder-<seed>.jsonl`) doubles as the resume
//! state: a header row pins the run identity, and the engine appends a full
//! [`GatedSnapshot`] row at heartbeat cadence, at every attempt start, and at
//! the checkpoint where a stop lands. Starting the same command again scans
//! the trace and resumes from the last snapshot — byte-identical to never
//! having stopped (locked by `gene_search::tests::dme_spec_ladder_resume_*`).
//! Ctrl-C sets a stop flag; every restart quits at its next checkpoint
//! barrier right after that checkpoint's snapshot is written.

use pio_superopt::fixtures::{
    dme_cfg, dme_corpus, dme_spec_multilength_dataset, dme_validation_corpus, fmt_cert,
    spec_certify_corpus, SPEC_H, SPEC_PHI_MAX,
};
use pio_superopt::search::{synthesize_curriculum_gated, CurriculumHp, Genes, Space, TraceEvent};
use pio_superopt::trace::{header_json, scan_for_resume, trace_json, RunHeader};
use pio_superopt::{Program, SideCfg};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

fn usage() -> ! {
    eprintln!(
        "usage: superopt spec-ladder [--seed HEX] [--lengths A..B] [--restarts N] \
         [--iters N] [--densify F] [--trace PATH] [--fresh]\n\
         \n\
         Runs the spec-oracle gated curriculum ladder. If the trace file already\n\
         exists (same parameters), the run RESUMES from its last snapshot; pass\n\
         --fresh to discard it and start over."
    );
    std::process::exit(2);
}

fn parse_args() -> (u64, Vec<usize>, usize, u32, f64, Option<String>, bool) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("spec-ladder") {
        usage();
    }
    let (mut seed, mut lo, mut hi, mut restarts, mut iters, mut densify) =
        (0x5EEDu64, 2usize, 14usize, 32usize, 4_000_000u32, 1.0f64);
    let mut trace_path: Option<String> = None;
    let mut fresh = false;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage()).clone();
        match a.as_str() {
            "--seed" => {
                let v = val();
                seed = u64::from_str_radix(v.trim_start_matches("0x"), 16).unwrap_or_else(|_| usage());
            }
            "--lengths" => {
                let v = val();
                let (a, b) = v.split_once("..").unwrap_or_else(|| usage());
                lo = a.parse().unwrap_or_else(|_| usage());
                hi = b.trim_start_matches('=').parse().unwrap_or_else(|_| usage());
            }
            "--restarts" => restarts = val().parse().unwrap_or_else(|_| usage()),
            "--iters" => iters = val().parse().unwrap_or_else(|_| usage()),
            "--densify" => densify = val().parse().unwrap_or_else(|_| usage()),
            "--trace" => trace_path = Some(val()),
            "--fresh" => fresh = true,
            _ => usage(),
        }
    }
    ((seed), (lo..=hi).collect(), restarts, iters, densify, trace_path, fresh)
}

fn main() {
    let (seed, lengths, restarts, rung_iters, densify_w, trace_path, fresh) = parse_args();
    let hp = CurriculumHp { densify_w, ..CurriculumHp::default() };
    let header = RunHeader {
        problem: "dme-spec".into(),
        seed,
        lengths: lengths.clone(),
        restarts,
        rung_iters,
        hp,
    };
    let path = std::path::PathBuf::from(
        trace_path.unwrap_or_else(|| format!("runs/spec-ladder-{seed:#x}.jsonl")),
    );
    std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new("."))).expect("create trace dir");

    // Resolve fresh-vs-resume BEFORE opening the file for append.
    let resume = if path.exists() && !fresh {
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

    let (dataset, groups) = dme_spec_multilength_dataset(&lengths, 32);
    eprintln!(
        "=== spec-ladder (SPEC oracle) === lengths {lengths:?} ({} seqs), cell={} data@+{SPEC_H} \
         phi_max={SPEC_PHI_MAX} densify={densify_w} seed={seed:#x}",
        dataset.len(),
        2 * SPEC_H
    );
    eprintln!("trace -> {}", path.display());

    let space = Space { slots: 10, side: SideCfg::NONE, search_wrap: true, genes: Genes::default() };
    let template = Program::empty(dme_cfg());
    let lens = lengths.clone();
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
        lengths.len(),
        &hp,
        restarts,
        rung_iters,
        0.0,
        seed,
        Some(&on_trace),
        resume,
        Some(&STOP),
        |frontier, rc, front_err, ok| {
            eprintln!(
                "spec-ladder RUNG L={:<2} {} front_err={front_err:6.1} size={} {}",
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

    let ct = spec_certify_corpus(&champ, &dme_corpus());
    let cv = spec_certify_corpus(&champ, &dme_validation_corpus());
    eprintln!(
        "spec-ladder DONE solved {solved}/{} cost={cost:.1} size={} [cert train={} held-out={}] {}",
        lengths.len(),
        champ.size(),
        fmt_cert(ct),
        fmt_cert(cv),
        champ.brief()
    );
}
