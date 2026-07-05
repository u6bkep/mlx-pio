//! JSONL trace serialization + resume-state parsing for the gated ladder.
//!
//! One JSON object per line. The small time-series events (checkpoint,
//! new_best, restart_end, attempt_end) are hand-rolled — compact, and their
//! `prog` field is a human-readable disassembly, not machine state. The
//! `snapshot` rows are full machine state ([`GatedSnapshot`], serde) — the
//! LAST one in a file is the resume point. A `header` row (first line) pins
//! the run identity so a resume can refuse a mismatched state file.

use crate::search::{CurriculumHp, GatedSnapshot, TraceEvent};
use std::io::BufRead;

/// Run identity, written as the first line of a trace and re-checked on
/// resume. Two runs with equal headers are the same deterministic search, so
/// resuming one from the other's snapshot is sound; any field differing means
/// the snapshot belongs to a different search and resume must refuse.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RunHeader {
    /// Problem/testbed id, e.g. "dme-spec".
    pub problem: String,
    pub seed: u64,
    /// Curriculum lengths (rungs), e.g. 2..=14.
    pub lengths: Vec<usize>,
    pub restarts: usize,
    pub rung_iters: u32,
    pub hp: CurriculumHp,
}

/// Serialize one trace event as a JSONL row.
pub fn trace_json(ev: &TraceEvent) -> String {
    use TraceEvent::*;
    // Bare finite number, or `null` (JSON has no inf/nan).
    fn jf(x: f64) -> String {
        if x.is_finite() { format!("{x}") } else { "null".into() }
    }
    fn esc(s: &str) -> String {
        let mut o = String::with_capacity(s.len() + 2);
        for c in s.chars() {
            match c {
                '"' => o.push_str("\\\""),
                '\\' => o.push_str("\\\\"),
                '\n' => o.push_str("\\n"),
                '\t' => o.push_str("\\t"),
                c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
                c => o.push(c),
            }
        }
        o
    }
    match ev {
        Checkpoint { frontier, attempt, r, iter, temp, cur_cost, best_sel_cost, best_frontier_err, best_size } => format!(
            "{{\"kind\":\"checkpoint\",\"rung\":{frontier},\"attempt\":{attempt},\"r\":{r},\"iter\":{iter},\"temp\":{},\"cur_cost\":{},\"best_sel\":{},\"best_fe\":{},\"best_size\":{best_size}}}",
            jf(*temp), jf(*cur_cost), jf(*best_sel_cost), jf(*best_frontier_err)
        ),
        NewBest { frontier, attempt, r, iter, program, sel_cost, frontier_err, size } => format!(
            "{{\"kind\":\"new_best\",\"rung\":{frontier},\"attempt\":{attempt},\"r\":{r},\"iter\":{iter},\"sel\":{},\"fe\":{},\"size\":{size},\"prog\":\"{}\"}}",
            jf(*sel_cost), jf(*frontier_err), esc(&program.brief())
        ),
        RestartEnd { frontier, attempt, r, program, sel_cost, frontier_err, size } => format!(
            "{{\"kind\":\"restart_end\",\"rung\":{frontier},\"attempt\":{attempt},\"r\":{r},\"sel\":{},\"fe\":{},\"size\":{size},\"prog\":\"{}\"}}",
            jf(*sel_cost), jf(*frontier_err), esc(&program.brief())
        ),
        AttemptEnd { frontier, attempt, program, solved, reheat, early_stop_checkpoint } => format!(
            "{{\"kind\":\"attempt_end\",\"rung\":{frontier},\"attempt\":{attempt},\"solved\":{solved},\"reheat\":{},\"early_stop\":{},\"prog\":\"{}\"}}",
            jf(*reheat),
            match early_stop_checkpoint { Some(v) => v.to_string(), None => "null".into() },
            esc(&program.brief())
        ),
        Snapshot { snap } => {
            let state = serde_json::to_string(snap).expect("GatedSnapshot serializes");
            format!("{{\"kind\":\"snapshot\",\"state\":{state}}}")
        }
    }
}

/// Serialize the header row.
pub fn header_json(h: &RunHeader) -> String {
    let body = serde_json::to_string(h).expect("RunHeader serializes");
    format!("{{\"kind\":\"header\",\"run\":{body}}}")
}

/// What a trace file yields for resume purposes.
pub struct ResumeScan {
    pub header: RunHeader,
    /// Last snapshot in the file, if any (no snapshot = start fresh but the
    /// file already exists; the caller decides whether that is an error).
    pub snapshot: Option<GatedSnapshot>,
}

/// Scan a trace file: parse the header row and the LAST snapshot row.
/// Errors on a missing/garbled header or a garbled final snapshot — a trace
/// we cannot interpret must not be silently restarted over.
pub fn scan_for_resume(path: &std::path::Path) -> Result<ResumeScan, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut header: Option<RunHeader> = None;
    let mut last_snapshot_line: Option<String> = None;
    for (n, line) in std::io::BufReader::new(f).lines().enumerate() {
        let line = line.map_err(|e| format!("read {}: {e}", path.display()))?;
        if line.starts_with("{\"kind\":\"header\"") {
            let v: serde_json::Value =
                serde_json::from_str(&line).map_err(|e| format!("line {}: bad header: {e}", n + 1))?;
            header = Some(
                serde_json::from_value(v["run"].clone()).map_err(|e| format!("line {}: bad header: {e}", n + 1))?,
            );
        } else if line.starts_with("{\"kind\":\"snapshot\"") {
            last_snapshot_line = Some(line);
        }
    }
    let header = header.ok_or_else(|| format!("{}: no header row — not a resumable trace", path.display()))?;
    let snapshot = match last_snapshot_line {
        None => None,
        Some(line) => {
            let v: serde_json::Value = serde_json::from_str(&line).map_err(|e| format!("bad final snapshot: {e}"))?;
            Some(serde_json::from_value(v["state"].clone()).map_err(|e| format!("bad final snapshot: {e}"))?)
        }
    };
    Ok(ResumeScan { header, snapshot })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{GatedSnapshot, RestartSnap};
    use crate::{Config, Program};

    #[test]
    fn snapshot_round_trips_through_jsonl() {
        let p = Program::empty(Config::default());
        let snap = GatedSnapshot {
            frontier: 3,
            attempt: 1,
            iter: 500_000,
            champ: Some(p.clone()),
            solved_through: 3,
            reheat: 0.575,
            pool: vec![p.clone()],
            lib: vec![],
            minima: vec![(p.clone(), 12.5, 2.0)],
            restarts: vec![RestartSnap { rng: 0xDEAD_BEEF, cur: p.clone(), best: p.clone(), best_sel: 7.0 }],
        };
        let line = trace_json(&TraceEvent::Snapshot { snap: &snap });
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        let back: GatedSnapshot = serde_json::from_value(v["state"].clone()).unwrap();
        assert_eq!(back.frontier, snap.frontier);
        assert_eq!(back.iter, snap.iter);
        assert_eq!(back.restarts[0].rng, 0xDEAD_BEEF);
        assert_eq!(back.restarts[0].cur, p);
        assert_eq!(back.champ, snap.champ);
        assert_eq!(back.minima[0].0, p);
    }

    #[test]
    fn header_round_trips_and_scan_finds_last_snapshot() {
        let hdr = RunHeader {
            problem: "dme-spec".into(),
            seed: 0x5EED,
            lengths: (2..=4).collect(),
            restarts: 4,
            rung_iters: 1000,
            hp: CurriculumHp::default(),
        };
        let p = Program::empty(Config::default());
        let mk_snap = |iter: u32| GatedSnapshot {
            frontier: 0,
            attempt: 0,
            iter,
            champ: None,
            solved_through: 0,
            reheat: 0.15,
            pool: vec![],
            lib: vec![],
            minima: vec![],
            restarts: vec![RestartSnap { rng: 1, cur: p.clone(), best: p.clone(), best_sel: 0.0 }],
        };
        let dir = std::env::temp_dir().join(format!("pio_superopt_trace_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let mut body = header_json(&hdr) + "\n";
        body += "{\"kind\":\"checkpoint\",\"rung\":0,\"attempt\":0,\"r\":0,\"iter\":0,\"temp\":1,\"cur_cost\":1,\"best_sel\":1,\"best_fe\":1,\"best_size\":0}\n";
        body += &(trace_json(&TraceEvent::Snapshot { snap: &mk_snap(100) }) + "\n");
        body += &(trace_json(&TraceEvent::Snapshot { snap: &mk_snap(200) }) + "\n");
        std::fs::write(&path, body).unwrap();
        let scan = scan_for_resume(&path).unwrap();
        assert_eq!(scan.header, hdr);
        assert_eq!(scan.snapshot.unwrap().iter, 200, "resume takes the LAST snapshot");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
