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

// --- Ticket 011 stage (b): x/y Field tags, die-on-transform ---------
//
// Adversarial micro-specs written BEFORE the feature (red-green
// convention). The riskiest interactions identified in design:
//
//  (T1) TAG-AWARE STATE PROJECTION — a record whose subtree read a
//       register while it was TAGGED must pattern the tag IDENTITY,
//       never the placeholder u32 left in NState: a garbage-blind
//       projection lets pattern components recorded at tag-carrying
//       frames match value-equal CONCRETE probers (and vice versa)
//       whose futures differ by the undecided field. Red check:
//       weaken `project_state` to project the placeholder — the
//       memo on/off batteries below must catch a divergence.
//  (T2) JUNK-WALK TAG READS — the walk cannot collapse a tag; a walk
//       that treats a tagged register's placeholder as its value can
//       falsely co-refute a whole delay family (memo-INDEPENDENT
//       unsoundness). Red check: weaken the walk's value-read bail —
//       the widening/coverage tests below must catch lost champions.

const JMP_NOT_X_2: u16 = 0x0022; // jmp !x, 2
const MOV_PINS_X: u16 = 0xA001; // mov pins, x
const MOV_Y_X: u16 = 0xA041; // mov y, x (op none: tag copy)
const MOV_PINS_Y: u16 = 0xA002; // mov pins, y
const SET_PINS_0: u16 = 0xE000;
const SET_PINS_1: u16 = 0xE001;

/// Widening gate (ticket 011 (b) census): a provably-DEAD
/// `set x, imm` must close champions with the immediate UNDECIDED
/// (the don't-care prize), and a LIVE one must decide it. Champions
/// are additionally materialized at NONZERO don't-care assignments
/// (all-ones-under-mask + a seeded-random one) — the canonical-zero
/// materialization would miss a widening bug on exactly the newly
/// widened bits.
#[test]
fn tag_widening_dead_set_imm_stays_undecided() {
    let config = Config {
        pins: PinMap { set_base: 0, set_count: 1, out_base: 0, out_count: 2, ..PinMap::default() },
        ..Config::default()
    };
    // DEAD: slot0 `set x, <imm free>` (seeded dst, free data), slot1
    // `set pins, 1` (seeded) — x is never read; every imm reproduces
    // the trace, so the search must close ONE champion subspace with
    // the imm undecided instead of 32 decided ones.
    let mut spec = EngineSpec {
        cfg: cfg_for(config.clone(), 0, 1),
        slots: 2,
        cycles: 8,
        inputs: vec![],
        output_pins: vec![0, 1],
        capture_pins: vec![0, 1],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        seed: vec![(0, 0xFFE0, 0xE020), (1, 0xFFFF, SET_PINS_1)],
        memo_cap: 1 << 20,
    };
    let mut w = [NOP_WORD; 32];
    w[0] = 0xE022; // set x, 2 — any imm works
    w[1] = SET_PINS_1;
    spec.expected = run_spec(&spec, w);
    let r = search(&spec, 10_000);
    assert!(!r.champion_cap_hit);
    assert!(r.stats.champions_found > 0, "dead-set rig found nothing");
    assert!(
        r.champions.iter().all(|c| c.decided[0] & 0x001F == 0),
        "dead `set x, imm` decided its immediate — laziness lost: {:?}",
        r.champions.iter().map(|c| c.decided[0]).collect::<Vec<_>>()
    );
    assert_champions_sound_widened(&spec, &r.champions);

    // LIVE: slot1 becomes `mov pins, x` — the imm is pin-visible, so
    // champions must DECIDE it and only imm & 3 == 2 survives.
    let mut spec_live = spec.clone();
    spec_live.seed = vec![(0, 0xFFE0, 0xE020), (1, 0xFFFF, MOV_PINS_X)];
    let mut wl = [NOP_WORD; 32];
    wl[0] = 0xE022;
    wl[1] = MOV_PINS_X;
    spec_live.expected = run_spec(&spec_live, wl);
    let rl = search(&spec_live, 10_000);
    assert!(rl.stats.champions_found > 0, "live-set rig found nothing");
    assert!(
        rl.champions.iter().all(|c| c.decided[0] & 0x001F == 0x001F),
        "live `set x, imm` left its immediate undecided — unsound widening"
    );
    assert!(
        rl.champions.iter().all(|c| c.value[0] & 0x3 == 0x2),
        "live rig admitted a wrong immediate"
    );
    assert_champions_sound_widened(&spec_live, &rl.champions);
    eprintln!(
        "tag_widening: dead champs={} (imm undecided), live champs={} (imm decided)",
        r.champions.len(),
        rl.champions.len()
    );
}

/// Memo on/off battery over tag-exercising rigs (T1): SET-x + zero
/// test, SET-x + pin read (collapse), MOV tag-copy chain + read of
/// the copy, and a wrap-loop re-executing the tag-creating SET.
/// Champion LISTS must be identical with the memo on and off, and the
/// witness program covered. Any projection/cond hole in the tag memo
/// interplay shows up here as a divergence.
#[test]
fn tag_collapse_memo_on_off_battery() {
    let config = Config {
        pins: PinMap { set_base: 0, set_count: 1, out_base: 0, out_count: 2, ..PinMap::default() },
        ..Config::default()
    };
    // (rig, seeds, witness_slots): slot words for the witness run.
    let rigs: Vec<(&str, Vec<(u8, u16, u16)>, Vec<u16>, u8, u8)> = vec![
        // slot0 set x,<free>; slot1 jmp !x,2; slot2 set pins,0;
        // slot3 set pins,1; wrap 0..3. Zero test: taken iff imm == 0.
        (
            "zero-test",
            vec![
                (0, 0xFFE0, 0xE020),
                (1, 0xFFFF, JMP_NOT_X_2),
                (2, 0xFFFF, SET_PINS_0),
                (3, 0xFFFF, SET_PINS_1),
            ],
            vec![0xE022, JMP_NOT_X_2, SET_PINS_0, SET_PINS_1],
            0,
            3,
        ),
        // slot0 set x,<free>; slot1 mov pins,x — direct value read.
        (
            "pin-read",
            vec![(0, 0xFFE0, 0xE020), (1, 0xFFFF, MOV_PINS_X)],
            vec![0xE022, MOV_PINS_X],
            0,
            1,
        ),
        // slot0 set x,<free>; slot1 mov y,x (tag COPY); slot2
        // mov pins,y — the copy's tag is read one register removed.
        (
            "copy-read",
            vec![(0, 0xFFE0, 0xE020), (1, 0xFFFF, MOV_Y_X), (2, 0xFFFF, MOV_PINS_Y)],
            vec![0xE022, MOV_Y_X, MOV_PINS_Y],
            0,
            2,
        ),
        // Wrap loop re-executing the SET: slot0 set x,<free>; slot1
        // jmp !x,0 spins while x==0... witness imm=2: falls through
        // to slot2 set pins,0 then wraps. Exercises tag re-creation.
        (
            "loop-recreate",
            vec![
                (0, 0xFFE0, 0xE020),
                (1, 0xFFFF, 0x0020), // jmp !x, 0
                (2, 0xFFFF, SET_PINS_0),
            ],
            vec![0xE022, 0x0020, SET_PINS_0],
            0,
            2,
        ),
    ];
    for (name, seed, wit, wb, wt) in rigs {
        let mut spec = EngineSpec {
            cfg: cfg_for(config.clone(), wb, wt),
            slots: wit.len() as u8,
            cycles: 12,
            inputs: vec![],
            output_pins: vec![0, 1],
            capture_pins: vec![0, 1],
            stim: Stim::default(),
            irq_sets: vec![],
            expected: vec![],
            seed,
            memo_cap: 0,
        };
        let mut w = [NOP_WORD; 32];
        for (i, &ww) in wit.iter().enumerate() {
            w[i] = ww;
        }
        spec.expected = run_spec(&spec, w);
        let off = search(&spec, 100_000);
        assert!(!off.champion_cap_hit);
        assert!(
            covered(&off.champions, &w),
            "[{name}] witness not covered with memo off — rig broken"
        );
        assert_champions_sound_widened(&spec, &off.champions);
        spec.memo_cap = 1 << 20;
        let on = search(&spec, 100_000);
        assert_eq!(
            off.champions, on.champions,
            "[{name}] champion lists diverge between memo off/on (T1)"
        );
        eprintln!(
            "tag battery [{name}]: off items={} on items={} hits={} champs={}",
            off.stats.items,
            on.stats.items,
            on.stats.memo_hits,
            on.stats.champions_found
        );
    }
}

/// T2: the junk_walk must BAIL on a value read of a live tag instead
/// of walking the placeholder. Rig: slot0 `set x, <imm free>` with
/// FREE DELAY (the delay post-fork arms the walk right after the tag
/// is created), slot1 `jmp !x, 2`, slot2/3 emit different pins; wrap
/// parks at slot3. A placeholder-walking bug reads x == 0, takes the
/// branch for every family member, and co-refutes delay families that
/// contain the imm != 0 witnesses — losing champions that exist with
/// the walk disabled. Assert coverage of both an imm == 0 witness
/// trace and an imm != 0 one.
#[test]
fn tag_junk_walk_value_read_bails() {
    let config = Config {
        pins: PinMap { set_base: 0, set_count: 1, out_base: 0, out_count: 2, ..PinMap::default() },
        ..Config::default()
    };
    let base = |expected: Vec<u32>| EngineSpec {
        cfg: cfg_for(config.clone(), 0, 3),
        slots: 4,
        cycles: 10,
        inputs: vec![],
        output_pins: vec![0, 1],
        capture_pins: vec![0, 1],
        stim: Stim::default(),
        irq_sets: vec![],
        expected,
        seed: vec![
            (0, 0xE0E0, 0xE020), // set x, <imm free>, DELAY FREE
            (1, 0xFFFF, JMP_NOT_X_2),
            (2, 0xFFFF, SET_PINS_0),
            (3, 0xFFFF, 0xE001 | 0x0700), // set pins,1 [7] — park-ish
        ],
        memo_cap: 1 << 20,
    };
    // Witness A: imm = 5 (branch NOT taken; falls into slot2 then 3).
    let mut wa = [NOP_WORD; 32];
    wa[0] = 0xE025;
    wa[1] = JMP_NOT_X_2;
    wa[2] = SET_PINS_0;
    wa[3] = 0xE001 | 0x0700;
    let mut spec_a = base(vec![]);
    spec_a.expected = run_spec(&spec_a, wa);
    let ra = search(&spec_a, 100_000);
    assert!(
        covered(&ra.champions, &wa),
        "imm!=0 witness lost — junk_walk walked a tag placeholder (T2)"
    );
    assert_champions_sound_widened(&spec_a, &ra.champions);

    // Witness B: imm = 0 (branch taken; straight to slot2).
    let mut wb = wa;
    wb[0] = 0xE020;
    let mut spec_b = base(vec![]);
    spec_b.expected = run_spec(&spec_b, wb);
    if spec_b.expected != spec_a.expected {
        let rb = search(&spec_b, 100_000);
        assert!(
            covered(&rb.champions, &wb),
            "imm==0 witness lost — tag zero-test broken (T2)"
        );
        assert_champions_sound_widened(&spec_b, &rb.champions);
    }
    eprintln!("tag_junk_walk: witness A covered ({} champs)", ra.champions.len());

    // LATCH-QUIET arm — the actual walk hazard. The walk only refutes
    // through latch-quiet windows, so a placeholder-walking bug needs
    // a refutation that never touches a latch: OOB fall-through.
    // slot0 `set x, <imm free>, <delay free>`; slot1 `jmp x--, 0`
    // (seeded); idle trace; wrap top past the footprint so an untaken
    // jmp at slot1 FALLS THROUGH out of it. Placeholder x == 0 makes the
    // walk's `jmp x--` FALL THROUGH to an out-of-footprint fetch and
    // falsely co-refute the whole delay family — which contains every
    // imm >= 1 champion (the loop re-arms x each iteration and idles
    // forever). The correct walk bails at the tag-value read.
    let config_q = Config::default();
    let mut spec_q = EngineSpec {
        cfg: cfg_for(config_q, 0, 5),
        slots: 2,
        // Horizon must exceed the walk's OOB shift guard (max delay
        // 31) or the walk never concludes OOB for an undecided-delay
        // family and the hazard is unreachable.
        cycles: 40,
        inputs: vec![],
        output_pins: vec![],
        capture_pins: vec![0],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        seed: vec![(0, 0xE0E0, 0xE020), (1, 0xFFFF, 0x0040)],
        memo_cap: 1 << 20,
    };
    let mut wq = [NOP_WORD; 32];
    wq[0] = 0xE025; // set x, 5
    wq[1] = 0x0040; // jmp x--, 0
    spec_q.expected = run_spec(&spec_q, wq);
    let rq = search(&spec_q, 100_000);
    assert!(
        covered(&rq.champions, &wq),
        "imm>=1 loop champion lost — junk_walk co-refuted a tagged family \
         through the placeholder (T2, latch-quiet arm)"
    );
    assert_champions_sound_widened(&spec_q, &rq.champions);
    eprintln!("tag_junk_walk: latch-quiet arm covered ({} champs)", rq.champions.len());
}

/// Binding interplay: tags must collapse BEFORE a binding-relevant
/// event (ticket 011 §4). The pull_empty_binding_fork topology with
/// the x-loader's immediate left FREE: slot0 `set y, <imm free>`,
/// slot1 `pull noblock` (empty TX: osr <- physical X), slot2
/// `out pins, 1`. Both binding branches and all 32 immediates are in
/// play; the mirror trace requires the TWIN branch with imm = 3.
#[test]
fn tag_binding_collapse() {
    let config = Config {
        pins: PinMap { out_base: 0, out_count: 1, ..PinMap::default() },
        ..Config::default()
    };
    let mut spec = EngineSpec {
        cfg: cfg_for(config.clone(), 0, 2),
        slots: 3,
        cycles: 6,
        inputs: vec![],
        output_pins: vec![0],
        capture_pins: vec![0],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        // set y, <imm free> / pull noblock / out pins,1 — only the
        // immediate (and the binding) is searched.
        seed: vec![(0, 0xFFE0, 0xE040), (1, 0xFFFF, PULL_NOBLOCK), (2, 0xFFFF, 0x6001)],
        memo_cap: 1 << 20,
    };
    // The mirror of [set y,3 / pull / out pins,1]: set x,3 loads
    // physical X = 3, the pull reads it into OSR, the OUT emits 1.
    let w_expect_src = {
        let mut w = [NOP_WORD; 32];
        w[0] = SET_Y_3_B;
        w[1] = PULL_NOBLOCK;
        w[2] = 0x6001;
        w
    };
    spec.expected = run_spec(&spec, mirror_program(&w_expect_src, 3));
    let r = search(&spec, 100_000);
    assert!(!r.champion_cap_hit);
    assert_champions_sound_widened(&spec, &r.champions);
    // The twin champion must exist and carry imm = 3 decided (the
    // pull's X read is a value read through the binding).
    let mut w_twin = w_expect_src;
    for s in 0..3 {
        w_twin[s] = mirror_word(w_twin[s]);
    }
    assert!(
        covered(&r.champions, &w_twin),
        "twin champion with collapsed imm lost at the binding event"
    );
    eprintln!("tag_binding_collapse: champs={}", r.champions.len());
}

const SET_X_3: u16 = 0xE023;
const SET_Y_3_B: u16 = 0xE043;

/// Champion soundness incl. NONZERO don't-care materializations
/// (ticket 011 gate (ii)): canonical zeros, all-ones-under-mask, and
/// a seeded-random assignment must all reproduce the trace in-space.
fn assert_champions_sound_widened(
    spec: &EngineSpec,
    champions: &[pio_superopt::narrow::engine::Champion],
) {
    use pio_superopt::narrow::engine::run_spec_oob;
    let mut rng = 0x9E3779B97F4A7C15u64;
    for (i, ch) in champions.iter().enumerate() {
        let mut fills: Vec<[u16; 32]> = vec![ch.words()];
        let mut ones = ch.words();
        let mut rnd = ch.words();
        for s in 0..32 {
            ones[s] |= !ch.decided[s];
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            rnd[s] |= (rng >> 32) as u16 & !ch.decided[s];
        }
        fills.push(ones);
        fills.push(rnd);
        for (k, w) in fills.iter().enumerate() {
            let (trace, oob) = run_spec_oob(spec, *w);
            assert!(!oob, "champion {i} materialization {k} executes out of footprint");
            assert_eq!(
                trace, spec.expected,
                "champion {i} materialization {k} does not reproduce the trace \
                 (don't-care over-widening)"
            );
        }
        if ch.binding_free {
            let m = mirror_program(&ch.words(), spec.slots);
            assert_eq!(
                run_spec(spec, m),
                spec.expected,
                "champion {i} binding-free mirror diverges"
            );
        }
    }
}

// --- S7: P1-pruned spellings' reads must be consulted ----------------
//
// Finding S7 (s2-relaxation-review.md §5.2/§6): the P1 register-mirror
// prune discards Y-naming spellings with no accounting of the pruned
// spelling's reads, so records formed above unnamed naming forks
// generalize over Y while their champion-free proof silently leans on
// P1 coverage valid only where x == y. A binding-fork twin can arrive
// at the same core with y != 0, match the SC_Y-free pattern, and be
// refuted by a proof that does not cover it. On the reviewed engine no
// champion is lost — the one champion-bearing record is benefit-gated
// out (3-item frame vs min_benefit == 4) and the reachable twin values
// are capture-degenerate — so this test is GREEN either way; it is a
// CANARY that flips red if the benefit gate, the MovOp/field arities,
// or the pre-filter counting ever shift. The demonstrated
// wrong-transfer kills (SC_Y-free records refuting the y == 1 twin's
// items) are reproduced via the probe-log recipe in §6.3; the S7 fix
// (charge the pruned value's reads, mirroring the pin-prefilter's
// consume_reads) makes those probes state-miss instead.

const S1: u16 = 0x1000; // side-set value 1 (side_count=1, en=false)
const MOV_OSR_INV_NULL: u16 = 0xA0EB; // mov osr, ~null (osr=FFFF_FFFF, cnt=0)

fn p1_rig() -> (EngineSpec, [u16; 32]) {
    let config = Config {
        side: pio_superopt::ir::SideCfg { count: 1, en: false },
        pins: PinMap {
            out_base: 0,
            out_count: 2,
            set_base: 0,
            set_count: 2,
            sideset_base: 2,
            ..PinMap::default()
        },
        ..Config::default()
    };
    let mut spec = EngineSpec {
        cfg: cfg_for(config, 0, 5),
        slots: 4,
        cycles: 4,
        inputs: vec![],
        output_pins: vec![0, 1, 2],
        capture_pins: vec![0, 1, 2],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        seed: vec![(0, 0xFFFF, MOV_OSR_INV_NULL)], // registerless: root stays unnamed
        memo_cap: 0,
    };
    let mut w_target = [NOP_WORD; 32];
    w_target[0] = MOV_OSR_INV_NULL; // side 0
    w_target[1] = OUT_Y_1 | S1; // twin spelling; identity spells out x,1
    w_target[2] = PULL_NOBLOCK; // side 0; empty TX -> osr <- physical X
    w_target[3] = MOV_PINS_Y | S1; // y=1 -> pins 01, the differentiator
    spec.expected = run_spec(&spec, w_target);
    (spec, w_target)
}

#[test]
fn p1_pruned_reads_must_be_consulted() {
    let (mut spec, w_target) = p1_rig();
    eprintln!("expected: {:08x?}", &spec.expected);
    assert_eq!(spec.expected[3] & 0x7, 0x5, "target trace must end side=1 pins=01");
    assert_eq!(spec.expected[2] & 0x7, 0x3, "cycle-2: pins idle high, side 0");

    let off = search(&spec, 1_000_000);
    assert!(!off.champion_cap_hit);
    assert!(
        covered(&off.champions, &w_target),
        "memo-off search failed to find the twin champion at all — rig broken"
    );

    spec.memo_cap = 1 << 20;
    let on = search(&spec, 1_000_000);
    assert!(!on.champion_cap_hit);
    eprintln!(
        "p1 rig: off champs={} items={} | on champs={} items={} hits={} entries={}",
        off.champions.len(),
        off.stats.items,
        on.champions.len(),
        on.stats.items,
        on.stats.memo_hits,
        on.stats.memo_entries
    );
    for ch in &off.champions {
        if !on.champions.contains(ch) {
            eprintln!("  lost champion: v={:04x?} bf={}", &ch.value[..4], ch.binding_free);
        }
    }
    assert!(
        covered(&on.champions, &w_target),
        "P1 RED: memo record with unaccounted P1-pruned reads killed a bound twin prober"
    );
    assert_eq!(
        off.champions, on.champions,
        "P1 RED: champion lists diverge between memo off/on"
    );
}

/// Diagnostic: memo-on run of the P1 rig for env-driven dumps.
#[test]
#[ignore]
fn p1_rig_dump() {
    let (mut spec, w_target) = p1_rig();
    spec.memo_cap = 1 << 20;
    let on = search(&spec, 1_000_000);
    eprintln!(
        "diag: champs={} hits={} target_covered={}",
        on.champions.len(),
        on.stats.memo_hits,
        covered(&on.champions, &w_target)
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
