//! word_canon lemma re-certification through the equivalence driver
//! (`smt::equiv`) — the first real customer of the proof engine.
//!
//! For every word `w` with `word_canon(w, cfg) != w` where BOTH
//! spellings sit inside the mirror's modeled subset, prove
//! `equiv(&[w], &[canon(w)], cfg, H, ∅, TraceAndFinalState)` — a
//! ∀-state, ∀-FIFO one-slot interchangeability proof, strictly
//! stronger than the 48-state × 4-gpio battery of
//! `word_canon_battery_sound`. Words/configs outside the mirror
//! subset stay battery-only and are REPORTED as such, never assumed.
//!
//! Expected split (see the per-lemma tally the tests print):
//! * PROVEN-universal — under the `one_pin` config: lemma 9 (window-
//!   coincident SET PINS/PINDIRS all-0/all-1 ≡ MOV from NULL/!NULL),
//!   lemma 10 (SET X/Y, 0 ≡ MOV X/Y, NULL), lemma 5's data-masking arm
//!   (SET PINS/PINDIRS data folded to `data & set_mask` — same SET
//!   respelled), and lemma 3's X/Y self-move arm (`mov y,y` ≡ nop rep).
//! * BATTERY-ONLY (out-of-subset words) — lemmas 1/11 (WAIT, IRQ),
//!   lemma 4 (MOV from STATUS), lemma 6 (PULL/PUSH operand low bits;
//!   only the canonical PULL encoding is subset-legal), lemma 7 (MOV
//!   '::'), lemma 8 (MOV to PC / JMP outside a len-1 footprint).
//! * BATTERY-ONLY (unsupported config) — everything under the tx_a,
//!   autopush, and sm_id=1 battery configs (out_count 0 / autopush /
//!   non-zero bases), including lemmas 2, 3 (count-0 arm), and 5's
//!   count-0 arm, which only arm on those configs.

#![cfg(feature = "smt")]

use std::collections::BTreeMap;
use std::time::Instant;

use pio_superopt::ir::SideCfg;
use pio_superopt::narrow::engine::word_canon;
use pio_superopt::narrow::NCfg;
use pio_superopt::program::{Config, PinMap, Program};
use pio_superopt::smt::equiv::{equiv, EquivVerdict, Tier};

/// One execution of a single-slot program plus delay margin: enough to
/// cover the instruction's effect, a re-execution after wrap, and the
/// residual delay/stall state (single-slot lemmas re-execute from a
/// state inside the quantified family, so one covered execution
/// generalizes).
const HORIZON: usize = 6;

fn ncfg_of(cfg: &Config, sm_id: u8) -> NCfg {
    NCfg::from_program(&Program::empty(*cfg), sm_id)
}

/// The battery's `one_pin`: SET and OUT windows both = pin 0 — the
/// only battery config inside the mirror's modeled contract.
fn one_pin_cfg() -> Config {
    Config {
        pins: PinMap { out_count: 1, set_count: 1, ..PinMap::default() },
        ..Config::default()
    }
}

/// The battery's `tx_a`: out/set_count 0, optional side-set, jmp_pin 8,
/// in_base 16 — outside the mirror contract (reported, not proven).
fn tx_a_cfg() -> Config {
    Config {
        side: SideCfg { count: 2, en: true },
        pins: PinMap { sideset_base: 0, in_base: 16, ..PinMap::default() },
        jmp_pin: 8,
        ..Config::default()
    }
}

/// The battery's `auto`: one_pin + autopush/autopull, thresholds 8 —
/// autopush puts it outside the mirror contract.
fn auto_cfg() -> Config {
    let mut c = one_pin_cfg();
    c.shift.autopush = true;
    c.shift.autopull = true;
    c.shift.push_threshold = 8;
    c.shift.pull_threshold = 8;
    c
}

/// Which word_canon lemma produced this (w, canon) pair — mirrors the
/// dispatch structure of `word_canon` itself, for the per-lemma tally.
fn lemma_of(w: u16, c: u16, cfg: &NCfg) -> &'static str {
    match (w >> 13) & 0x7 {
        0 => "L08 jmp31=mov-pc-!null",
        1 => "L01 wait-irq rel-fold",
        3 => "L02 out-count0=out-null",
        4 => "L06 pull/push dead low bits",
        5 => {
            let (dst, op, src) = ((w >> 5) & 7, (w >> 3) & 3, w & 7);
            if (dst == 0 || dst == 3) && cfg.out_count == 0 {
                "L03 mov-pins-count0=nop"
            } else if op == 0 && dst == src && (dst == 1 || dst == 2) {
                "L03 x/y self-move=nop"
            } else if dst == 5 {
                "L08 mov-pc-null=jmp0"
            } else if src == 5 {
                "L04 mov-status0=null"
            } else if src == 3 && op == 2 {
                "L07 ::null=null"
            } else {
                "L04+L07 combined"
            }
        }
        6 => {
            if w & 0x0060 == 0x0060 {
                "L11 irq clear kills wait"
            } else {
                "L01 irq rel-fold/dead bit7"
            }
        }
        7 => {
            let dst = (w >> 5) & 7;
            if dst == 1 || dst == 2 {
                "L10 set-x/y-0=mov-null"
            } else if cfg.set_count == 0 {
                "L05 set-count0=nop"
            } else if (c >> 13) & 0x7 == 5 {
                "L09 set-window=mov-null/!null"
            } else {
                "L05 set data mask"
            }
        }
        _ => "unclassified",
    }
}

#[derive(Default)]
struct Tally {
    proven: usize,
    out_of_subset: usize,
    unsupported: usize,
    other_unknown: usize,
}

/// Sweep every changed (w, canon) pair of `cfg`, prove where possible,
/// and return per-lemma tallies. Panics on Refuted or MirrorDivergence
/// — either one means a word_canon lemma or the mirror is WRONG.
fn certify_config(
    name: &str,
    cfg: &Config,
    sm_id: u8,
    filter: impl Fn(u16) -> bool,
) -> BTreeMap<&'static str, Tally> {
    let ncfg = ncfg_of(cfg, sm_id);
    let mut tally: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let t0 = Instant::now();
    let mut pairs = 0usize;
    for w in 0..=0xFFFFu16 {
        if !filter(w) {
            continue;
        }
        let c = word_canon(w, &ncfg);
        if c == w {
            continue;
        }
        pairs += 1;
        let entry = tally.entry(lemma_of(w, c, &ncfg)).or_default();
        match equiv(&[w], &[c], cfg, HORIZON, &[], Tier::TraceAndFinalState) {
            EquivVerdict::Proven => entry.proven += 1,
            EquivVerdict::Unknown(r) if r.contains("outside the modeled subset") => {
                entry.out_of_subset += 1
            }
            EquivVerdict::Unknown(r) if r.contains("unsupported config") => {
                entry.unsupported += 1
            }
            EquivVerdict::Unknown(r) => {
                eprintln!("[{name}] {w:04x}->{c:04x}: Unknown({r})");
                entry.other_unknown += 1;
            }
            EquivVerdict::Refuted(cex) => panic!(
                "[{name}] word_canon merged behaviorally distinct words \
                 {w:04x} -> {c:04x}: {cex:?}"
            ),
            EquivVerdict::MirrorDivergence(msg) => panic!(
                "[{name}] MIRROR DIVERGENCE on {w:04x} -> {c:04x}: {msg}"
            ),
        }
    }
    eprintln!(
        "[{name}] {pairs} pairs in {:.1}s — per-lemma (proven / out-of-subset / \
         unsupported-config / other):",
        t0.elapsed().as_secs_f64()
    );
    for (lemma, t) in &tally {
        eprintln!(
            "  {lemma:<32} {:>5} / {:>5} / {:>5} / {:>3}",
            t.proven, t.out_of_subset, t.unsupported, t.other_unknown
        );
    }
    tally
}

/// Fast spot check (delay/side field = 0 only) on the one supported
/// battery config. Un-ignored: this is the regression gate.
#[test]
fn word_canon_recertify_spot() {
    let cfg = one_pin_cfg();
    let tally = certify_config("one_pin ds=0", &cfg, 0, |w| w & 0x1F00 == 0);
    // The in-subset lemmas must be PROVEN-universal, not merely counted.
    // (No "L05 set data mask" pairs exist here: with set_count == 1 the
    // masked data is always all-0s or all-1s, so every SET PINS/PINDIRS
    // pair goes through lemma 9 instead.)
    for lemma in [
        "L09 set-window=mov-null/!null",
        "L10 set-x/y-0=mov-null",
        "L03 x/y self-move=nop",
    ] {
        let t = tally.get(lemma).unwrap_or_else(|| panic!("no pairs for {lemma}"));
        assert!(t.proven > 0, "{lemma}: expected proofs, got none");
        assert_eq!(
            t.out_of_subset + t.unsupported + t.other_unknown,
            0,
            "{lemma}: expected fully in-subset"
        );
    }
    // Nothing may fail for an unexpected reason on a supported config.
    for (lemma, t) in &tally {
        assert_eq!(t.unsupported, 0, "{lemma}: one_pin is a supported config");
        assert_eq!(t.other_unknown, 0, "{lemma}: unexpected Unknown reason");
    }
}

/// Full re-certification: every word (all delay/side spellings), all
/// four battery configs — 2,144 solver proofs, ~4 s in release. The
/// three non-one_pin configs are outside the mirror contract and tally
/// as unsupported — that is the honest battery-only remainder, printed
/// per lemma.
#[test]
fn word_canon_recertify_full() {
    let one_pin = one_pin_cfg();
    let tally = certify_config("one_pin", &one_pin, 0, |_| true);
    let in_subset_proven: usize = tally.values().map(|t| t.proven).sum();
    assert!(in_subset_proven > 0);

    for (name, cfg, sm_id) in [
        ("tx_a", tx_a_cfg(), 0u8),
        ("auto", auto_cfg(), 0),
        ("sm1", tx_a_cfg(), 1),
    ] {
        let tally = certify_config(name, &cfg, sm_id, |_| true);
        for (lemma, t) in &tally {
            assert_eq!(t.proven, 0, "[{name}] {lemma}: config should be unsupported");
        }
    }
}
