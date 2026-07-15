//! Dump the phase-1 frontier seeds of a narrow-split decomposition as
//! JSONL, one line per unit: {"unit": i, "seed": [[slot, decided, value], ...]}.
//!
//! Seed order is the trace's unit-id order (split_units is
//! deterministic for a given spec + target), so this joins 1:1 with a
//! narrow-split trace produced at the same engine rev and parameters.
//! Analysis companion to tools/mine_narrow_split.py.
//!
//! Usage: dump_seeds --len 3 --wrap-lo 1 --wrap-hi 2 \
//!            [--cycles 460] [--target 3584] [--memo-cap 2097152]

use pio_superopt::fixtures::{tx_a_narrow_spec, tx_a_narrow_words};
use pio_superopt::narrow::engine::{run_spec, split_units, SplitPlan};
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut len, mut wrap_lo, mut wrap_hi): (Option<u8>, Option<u8>, Option<u8>) =
        (None, None, None);
    let mut cycles: u32 = 460;
    let mut target: usize = 3584;
    let mut memo_cap: usize = 1 << 21;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().expect("flag needs a value").clone();
        match a.as_str() {
            "--len" => len = Some(val().parse().unwrap()),
            "--wrap-lo" => wrap_lo = Some(val().parse().unwrap()),
            "--wrap-hi" => wrap_hi = Some(val().parse().unwrap()),
            "--cycles" => cycles = val().parse().unwrap(),
            "--target" => target = val().parse().unwrap(),
            "--memo-cap" => memo_cap = val().parse().unwrap(),
            other => panic!("unknown flag {other}"),
        }
    }
    let (len, wrap_lo, wrap_hi) =
        (len.expect("--len"), wrap_lo.expect("--wrap-lo"), wrap_hi.expect("--wrap-hi"));

    // Spec construction copied verbatim from narrow_split_cmd — the
    // decomposition must be bit-identical to the runner's.
    let (mut spec4, side) = tx_a_narrow_spec(cycles);
    spec4.expected = run_spec(&spec4, tx_a_narrow_words(&side));
    let (mut spec, _) = tx_a_narrow_spec(cycles);
    spec.slots = len;
    spec.cfg.wrap_bottom = wrap_lo;
    spec.cfg.wrap_top = wrap_hi;
    spec.expected = spec4.expected;
    spec.memo_cap = memo_cap;

    let su = match split_units(&spec, target) {
        SplitPlan::Units(su) => su,
        SplitPlan::Sequential => panic!("frontier never reached target — no units"),
        SplitPlan::Refuted(_) => panic!("refuted on a trace prefix — no units"),
    };
    let out = std::io::stdout();
    let mut w = std::io::BufWriter::new(out.lock());
    eprintln!(
        "{} units (frontier cycle {}, {} pre-mirror)",
        su.seeds.len(),
        su.frontier_cycle,
        su.pre_mirror
    );
    for (i, seed) in su.seeds.iter().enumerate() {
        let line = serde_json::json!({ "unit": i, "seed": seed });
        writeln!(w, "{line}").unwrap();
    }
}
