//! Adversarial micro-specs for the memo's interaction with P1
//! register-mirror symmetry (soundness findings S2/S3, Codex review
//! 2026-07-13). Each red test constructs a space where a memo record
//! wrongly refutes an item and asserts memo-on == memo-off champion
//! lists — red on the unfixed engine, green after the binding-domain
//! fixes.

use pio_superopt::narrow::engine::{mirror_word, run_spec, search, EngineSpec};
use pio_superopt::narrow::{NCfg, Stim};
use pio_superopt::program::{Config, PinMap, Program};

fn cfg_for(config: Config, wrap_bottom: u8, wrap_top: u8) -> NCfg {
    let mut p = Program::empty(config);
    p.wrap_bottom = wrap_bottom;
    p.wrap_top = wrap_top;
    NCfg::from_program(&p, 0)
}

fn mirror_program(words: &[u16; 32], slots: u8) -> [u16; 32] {
    let mut m = *words;
    for s in 0..slots as usize {
        m[s] = mirror_word(m[s]);
    }
    m
}

fn covered(champions: &[pio_superopt::narrow::engine::Champion], words: &[u16; 32]) -> bool {
    champions
        .iter()
        .any(|ch| (0..32).all(|s| words[s] & ch.decided[s] == ch.value[s]))
}

const SET_X_1: u16 = 0xE021;
const SET_X_2: u16 = 0xE022;
const SET_Y_1: u16 = 0xE041;
const SET_Y_2: u16 = 0xE042;
const OUT_X_1: u16 = 0x6021; // out x, 1
const OUT_Y_1: u16 = 0x6041; // out y, 1 (mirror)
const MOV_OSR_NULL: u16 = 0xA0E3; // mov osr, null (resets osr + count)
const PULL_NOBLOCK: u16 = 0x8080;
const MOV_PINS_OSR: u16 = 0xA007; // mov pins, osr
const NOP_WORD: u16 = 0xA042; // mov y, y — engine filler for slots >= slots

/// The S3 rig, shared by the red test and the diagnostic dump.
///
/// Topology (slot 1 is the only fully free slot; slot 2's DELAY is the
/// only other undecided field — it exists to place a recordable delay
/// fork at the cycle-3 core, past the OSR scrub):
///
///   slot0  set x, 2       (seed)             x <- 2
///   slot1  <searched>
///   slot2  mov osr, null  (seed, delay free) osr <- 0, osr_count <- 0
///   slot3  set x, 1       (seed)             x <- 1
///   slot4  pull noblock   (seed)             TX empty: osr <- physical X
///   slot5  mov pins, osr  (seed)             observable: pins <- osr & 3
///
/// RECORDER: the slot1 = PULL child binding-forks at cycle 1
/// (x=2 != y=0). Its BOUND identity crosses the slot4 pull WITHOUT
/// forking (osr <- x = 1, pins 01, refuted); the slot2-delay fork under
/// it closes champion-free and records at the cycle-3 core (pc=3,
/// x=2 unread — the pull's X read is provenance-accounted to the slot3
/// seed — pattern {tx, y=0, osr=0}). Its twin champions OUTSIDE that
/// frame, at the cycle-1 binding frame, so the record survives.
///
/// PROBER: slot1 = OUT X,k (opcode 3 pops AFTER pull's 4) zeroes X, so
/// its slot2-delay children reach the same core UNBOUND with
/// x == y == 0 — the existing mirror guard (x != y) does not fire —
/// and the record's conds/pattern all match. The unfixed memo kills
/// them BEFORE their slot4 binding fork; the lost twin
/// [set y,2 / out y,k / mov osr,null / set y,1 / pull / mov pins,osr]
/// pulls physical X = 0 and drives pins 00 at cycle 5 — the expected
/// trace. Memo-off finds it; the unfixed memo-on loses it.
fn s3_rig() -> (EngineSpec, [u16; 32]) {
    let config = Config {
        pins: PinMap { out_base: 0, out_count: 2, ..PinMap::default() },
        ..Config::default()
    };
    let mut spec = EngineSpec {
        cfg: cfg_for(config, 0, 5),
        slots: 6,
        cycles: 6,
        inputs: vec![],
        output_pins: vec![0, 1],
        capture_pins: vec![0, 1],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        seed: vec![
            (0, 0xFFFF, SET_X_2),
            (2, 0xE0FF, MOV_OSR_NULL), // delay bits undecided
            (3, 0xFFFF, SET_X_1),
            (4, 0xFFFF, PULL_NOBLOCK),
            (5, 0xFFFF, MOV_PINS_OSR),
        ],
        memo_cap: 0,
    };
    // The lost champion: the register-mirror of the OUT X,1 prober.
    // Reachable ONLY through a binding-fork twin — the seeds pin the
    // identity spelling, so no independently generated item carries
    // the mirrored words.
    let mut w_target = [NOP_WORD; 32];
    w_target[0] = SET_Y_2;
    w_target[1] = OUT_Y_1;
    w_target[2] = MOV_OSR_NULL;
    w_target[3] = SET_Y_1;
    w_target[4] = PULL_NOBLOCK;
    w_target[5] = MOV_PINS_OSR;
    spec.expected = run_spec(&spec, w_target);
    (spec, w_target)
}

/// S3 red: a record whose proof ran under a BOUND item (its subtree
/// crossed the binding-asymmetric slot4 pull without forking) must not
/// refute an UNBOUND prober — the prober stands for its register-mirror
/// twin too, and the twin's future (which diverges at the asymmetric
/// event) is unproven by the record. The x==y prober slips past the
/// existing `mirror_blocked` guard.
#[test]
fn s3_bound_record_must_not_refute_unbound_prober() {
    let (mut spec, w_target) = s3_rig();
    // Sanity: the trace has structure (idle-high 11 turning 00 at the
    // end), and the identity spelling does NOT reproduce it.
    assert_eq!(spec.expected[0] & 0x3, 0x3, "pins idle high");
    assert_eq!(spec.expected[5] & 0x3, 0x0, "target trace must end pins=00");
    let ident = mirror_program(&w_target, 6);
    assert_ne!(run_spec(&spec, ident), spec.expected, "identity spelling must refute");

    let off = search(&spec, 100_000);
    assert!(!off.champion_cap_hit);
    assert!(
        covered(&off.champions, &w_target),
        "memo-off search failed to find the twin champion at all — rig broken"
    );

    spec.memo_cap = 1 << 20;
    let on = search(&spec, 100_000);
    assert!(!on.champion_cap_hit);
    eprintln!(
        "s3 rig: off champs={} items={} | on champs={} items={} hits={} entries={}",
        off.champions.len(),
        off.stats.items,
        on.champions.len(),
        on.stats.items,
        on.stats.memo_hits,
        on.stats.memo_entries
    );
    for ch in &off.champions {
        if !on.champions.contains(ch) {
            eprintln!("  lost champion: v={:04x?} bf={}", &ch.value[..6], ch.binding_free);
        }
    }
    assert!(
        covered(&on.champions, &w_target),
        "S3 RED: memo hit killed an unbound x==y prober; its twin champion was lost"
    );
    assert_eq!(
        off.champions, on.champions,
        "S3 RED: champion lists diverge between memo off/on"
    );
}

/// Diagnostic: memo-on run of the S3 rig for env-driven dumps
/// (PIO_NARROW_DUMP / PIO_NARROW_PROBE_LOG). Development only.
#[test]
#[ignore]
fn s3_rig_dump() {
    let (mut spec, w_target) = s3_rig();
    spec.memo_cap = 1 << 20;
    let on = search(&spec, 100_000);
    eprintln!(
        "diag: champs={} hits={} target_covered={}",
        on.champions.len(),
        on.stats.memo_hits,
        covered(&on.champions, &w_target)
    );
}
