//! Gate for the needed-narrowing fork loop (`narrow::engine`).
//!
//! The sanity tests run small exhaustive searches whose answers are
//! knowable by hand; the tx_a validation (the engine's first real
//! target) rediscovers the shipped 4-instruction IRQ→DI toggler from
//! its trace and proves nothing shorter matches — run it with:
//! `cargo test --release --test narrow_engine -- --ignored tx_a --nocapture`

use pio_superopt::encode::encode_insn;
use pio_superopt::ir::{Insn, JmpCond, Op, SetDst, SideCfg, WaitSrc};
use pio_superopt::narrow::engine::{mirror_word, run_spec, run_spec_oob, search, EngineSpec};
use pio_superopt::narrow::{NCfg, Stim};
use pio_superopt::program::{Config, PinMap, Program};

/// NCfg for a searchable space: config fields from `Config`, wrap from
/// arguments, code irrelevant (the engine owns it).
fn cfg_for(config: Config, wrap_bottom: u8, wrap_top: u8) -> NCfg {
    let mut p = Program::empty(config);
    p.wrap_bottom = wrap_bottom;
    p.wrap_top = wrap_top;
    NCfg::from_program(&p, 0)
}

fn words_of(insns: &[Insn], side: &SideCfg) -> [u16; 32] {
    let mut code = [encode_insn(&Insn::nop_for(side), side); 32];
    for (i, ins) in insns.iter().enumerate() {
        code[i] = encode_insn(ins, side);
    }
    code
}

/// Mirror the searched slots of a program (the P1 register-rename twin).
fn mirror_program(words: &[u16; 32], slots: u8) -> [u16; 32] {
    let mut m = *words;
    for s in 0..slots as usize {
        m[s] = mirror_word(m[s]);
    }
    m
}

/// Every champion, materialized, must reproduce the spec's trace — the
/// engine's own don't-care claim checked against the concrete runner.
/// A binding-free champion additionally claims its register-mirror
/// reproduces the trace; check that claim too.
fn assert_champions_sound(spec: &EngineSpec, champions: &[pio_superopt::narrow::engine::Champion]) {
    for (i, ch) in champions.iter().enumerate() {
        let (trace, oob) = run_spec_oob(spec, ch.words());
        assert!(!oob, "champion {i} executes out of footprint (UB on hardware)");
        assert_eq!(
            trace,
            spec.expected,
            "champion {i} does not reproduce the spec trace"
        );
        if ch.binding_free {
            assert_eq!(
                run_spec(spec, mirror_program(&ch.words(), spec.slots)),
                spec.expected,
                "champion {i} is binding-free but its register-mirror diverges"
            );
        }
    }
}

/// Does some champion subspace cover this concrete program?
fn covered(champions: &[pio_superopt::narrow::engine::Champion], words: &[u16; 32]) -> bool {
    champions.iter().any(|ch| (0..32).all(|s| words[s] & ch.decided[s] == ch.value[s]))
}

/// Is this word inside the engine's enumerated space? The exclusions
/// are DOCUMENTED space choices (footprint semantics, stubbed or
/// reserved encodings whose behavior aliases an in-space spelling), not
/// soundness claims: JMP targets >= slots, WAIT src 3 (JMPPIN stub),
/// IN src 4/5 and MOV src 4 (reserved, alias src 3), MOV op 3 (reserved,
/// alias op 0), IRQ index bit 3 (dead), SET reserved dsts (no-ops).
fn in_l1_space(w: u16, slots: u8) -> bool {
    let f = (w >> 5) & 0x7;
    match (w >> 13) & 0x7 {
        0 => (w & 0x1F) < slots as u16,
        1 => (w >> 5) & 0x3 != 3,
        2 => !matches!(f, 4 | 5),
        5 => (w & 0x7) != 4 && (w >> 3) & 0x3 != 3,
        6 => w & 0x8 == 0,
        7 => matches!(f, 0 | 1 | 2 | 4),
        _ => true,
    }
}

/// The canonical respelling of a word under the mechanized quotient
/// (ticket 009 `word_canon`) plus the POSITIONAL P4 map (L=1, so the
/// JMP fallthrough of slot 0 is `wrap_bottom`): vacuous-jump no-op
/// spellings map to the config's no-op representative with delay
/// preserved.
fn canon_l1_word(w: u16, spec: &EngineSpec) -> u16 {
    use pio_superopt::narrow::engine::word_canon;
    let w = word_canon(w, &spec.cfg);
    match (w >> 13) & 0x7 {
        0 => {
            let cond = (w >> 5) & 0x7;
            let ft = if 0 == spec.cfg.wrap_top { spec.cfg.wrap_bottom } else { 1 } as u16;
            if cond != 2 && cond != 4 && (w & 0x1F) == ft {
                // The nop representative for this config, delay kept.
                return word_canon(0xA042 | (w & 0x1F00), &spec.cfg);
            }
            w
        }
        _ => w,
    }
}

/// The strong soundness gate for every generation-pruning lever: brute
/// force all 65,536 words in slot 0 of an L=1 space and check, both
/// directions, that an in-space word reproduces the trace IFF some
/// champion subspace covers it — directly, via its register-mirror (P1,
/// binding-free champions only), or via its canonical respelling
/// (P2/P4). An unsound filter (kills a survivor) fails
/// `matches && !covered`; a broken don't-care claim fails the converse.
fn census_l1(spec: &EngineSpec, side: &SideCfg, champions: &[pio_superopt::narrow::engine::Champion]) {
    let base = words_of(&[], side);
    let mut code = base;
    let covered_q = |w: u16| {
        let mut c = base;
        c[0] = w;
        if covered(champions, &c) {
            return true;
        }
        c[0] = mirror_word(w);
        champions
            .iter()
            .any(|ch| ch.binding_free && (0..32).all(|s| c[s] & ch.decided[s] == ch.value[s]))
    };
    let mut n_match = 0u32;
    for w in 0..=0xFFFFu16 {
        code[0] = w;
        let (trace, oob) = run_spec_oob(spec, code);
        let matches = trace == spec.expected;
        n_match += matches as u32;
        if matches && (oob || !in_l1_space(w, spec.slots)) {
            // Out of space: documented word exclusion, or execution
            // left the declared footprint (UB on hardware) — such a
            // word's trace only "matches" over the nop filler.
            continue;
        }
        let cov = covered_q(w) || covered_q(canon_l1_word(w, spec));
        assert_eq!(
            matches, cov,
            "census mismatch at word {w:04x}: reproduces-trace={matches} covered-by-champion={cov}"
        );
    }
    eprintln!("census_l1: {n_match} of 65536 words reproduce the trace (exact quotient coverage)");
}

/// A 2-slot square wave: `set pins,1 / set pins,0`, wrap 0..1. The
/// search must terminate, find champions, and one of them must cover
/// the hand-written program.
#[test]
fn square_wave_l2_rediscovered() {
    let config = Config {
        pins: PinMap { set_base: 0, set_count: 1, out_base: 0, out_count: 1, ..PinMap::default() },
        ..Config::default()
    };
    let cfg = cfg_for(config, 0, 1);
    let side = SideCfg::NONE;
    let reference = words_of(
        &[
            Insn::plain(Op::Set { dst: SetDst::Pins, data: 1 }),
            Insn::plain(Op::Set { dst: SetDst::Pins, data: 0 }),
        ],
        &side,
    );

    let mut spec = EngineSpec {
        cfg,
        slots: 2,
        cycles: 17,
        inputs: vec![],
        output_pins: vec![0],
        capture_pins: vec![0],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        seed: vec![],
        memo_cap: 1 << 20,
    };
    spec.expected = run_spec(&spec, reference);
    // Sanity of the oracle itself: a strict square wave after the first edge.
    assert!(spec.expected.windows(2).all(|w| w[0] != w[1]), "oracle is not a square wave");

    let result = search(&spec, 1_000_000);
    assert!(result.stats.champions_found > 0, "no champions found");
    assert!(!result.champion_cap_hit);
    assert_champions_sound(&spec, &result.champions);
    assert!(covered(&result.champions, &reference), "hand program not covered by any champion");
    // The pin-write pre-filter must actually fire on a SET-data space.
    assert!(result.stats.prefiltered > 0, "pin-write pre-filter never fired");
    eprintln!(
        "square_wave L2: items={} forks={} refuted={} prefilt={} champions={}",
        result.stats.items,
        result.stats.forks,
        result.stats.refuted,
        result.stats.prefiltered,
        result.stats.champions_found
    );
}

/// Impossibility smoke: a period-3 pattern (two cycles high, one low)
/// from a single instruction slot. One instruction can produce constant
/// output or a symmetric toggle (via pin loopback + delay), but not an
/// asymmetric duty cycle — expect ZERO champions, i.e. exhaustion as an
/// impossibility proof.
#[test]
fn period3_l1_impossible() {
    let config = Config {
        pins: PinMap {
            set_base: 0,
            set_count: 1,
            out_base: 0,
            out_count: 1,
            in_base: 0,
            ..PinMap::default()
        },
        ..Config::default()
    };
    let cfg = cfg_for(config, 0, 0);

    let mut expected = Vec::new();
    for c in 0..18u32 {
        let level = if c % 3 < 2 { 1 } else { 0 };
        expected.push(level | 1 << 16);
    }
    let spec = EngineSpec {
        cfg,
        slots: 1,
        cycles: 18,
        inputs: vec![],
        output_pins: vec![0],
        capture_pins: vec![0],
        stim: Stim::default(),
        irq_sets: vec![],
        expected,
        seed: vec![],
        memo_cap: 1 << 20,
    };
    let result = search(&spec, 10);
    assert_eq!(
        result.stats.champions_found, 0,
        "a single instruction allegedly produces a period-3 duty pattern: {:?}",
        result.champions.first().map(|c| c.value[0])
    );
    // Brute-force cross-validation of the impossibility itself.
    census_l1(&spec, &SideCfg::NONE, &result.champions);
    eprintln!(
        "period3 L1: items={} forks={} refuted={} prefilt={} (exhausted, 0 champions)",
        result.stats.items, result.stats.forks, result.stats.refuted, result.stats.prefiltered
    );
}

/// A satisfiable L=1 space with exact-coverage census: `mov pins, !pins`
/// on a loopback pin toggles every cycle; the engine must find champions
/// whose coverage is EXACTLY the set of matching words.
#[test]
fn mov_toggle_l1_census_exact() {
    let config = Config {
        pins: PinMap { out_base: 0, out_count: 1, in_base: 0, ..PinMap::default() },
        ..Config::default()
    };
    let cfg = cfg_for(config, 0, 0);
    let side = SideCfg::NONE;
    // MOV (op 5) dst=PINS(0) op=INVERT(1) src=PINS(0) = 0xA008.
    let mut reference = words_of(&[], &side);
    reference[0] = 0xA008;

    let mut spec = EngineSpec {
        cfg,
        slots: 1,
        cycles: 12,
        inputs: vec![],
        output_pins: vec![0],
        capture_pins: vec![0],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        seed: vec![],
        memo_cap: 1 << 20,
    };
    spec.expected = run_spec(&spec, reference);
    assert!(spec.expected.windows(2).all(|w| (w[0] ^ w[1]) & 1 != 0), "oracle is not a toggle");

    let result = search(&spec, 100_000);
    assert!(result.stats.champions_found > 0, "no champions found");
    assert!(!result.champion_cap_hit);
    assert_champions_sound(&spec, &result.champions);
    assert!(covered(&result.champions, &reference), "mov pins,!pins not covered");
    census_l1(&spec, &side, &result.champions);
    eprintln!(
        "mov_toggle L1: items={} forks={} refuted={} prefilt={} champions={}",
        result.stats.items,
        result.stats.forks,
        result.stats.refuted,
        result.stats.prefiltered,
        result.stats.champions_found
    );
}

/// The canonicity gate: a constant-HIGH trace is matched by EVERY
/// effect-free word — all nop spellings (P2 self-moves, P4 vacuous
/// jumps), met/never-met WAITs, IRQ sets, FIFO ops, register writes —
/// so the census exercises the P2/P4 quotient maps and the don't-care
/// machinery at full width.
#[test]
fn nop_l1_census_exact() {
    let config = Config {
        pins: PinMap {
            set_base: 0,
            set_count: 1,
            out_base: 0,
            out_count: 1,
            in_base: 0,
            ..PinMap::default()
        },
        ..Config::default()
    };
    let cfg = cfg_for(config, 0, 0);
    let side = SideCfg::NONE;
    let reference = words_of(&[], &side);

    let mut spec = EngineSpec {
        cfg,
        slots: 1,
        cycles: 8,
        inputs: vec![],
        output_pins: vec![0],
        capture_pins: vec![0],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        seed: vec![],
        memo_cap: 1 << 20,
    };
    spec.expected = run_spec(&spec, reference);
    assert!(spec.expected.iter().all(|&w| w == (1 | 1 << 16)), "oracle is not constant HIGH");

    let result = search(&spec, 1_000_000);
    assert!(!result.champion_cap_hit);
    assert_champions_sound(&spec, &result.champions);
    assert!(result.stats.canon_pruned > 0, "canonicity filters never fired");
    // The engine must NOT emit pruned spellings: no champion may decide
    // a MOV self-move other than the canonical nop, nor a vacuous JMP.
    use pio_superopt::narrow::engine::NOP_CANON;
    for ch in &result.champions {
        let w = ch.value[0];
        if ch.decided[0] & 0xE0FF == 0xE0FF && (w >> 13) == 5 {
            let (dst, op, src) = ((w >> 5) & 7, (w >> 3) & 3, w & 7);
            if op == 0 && dst == src && matches!(dst, 1 | 2) {
                assert_eq!(w & 0xE0FF, NOP_CANON, "non-canonical self-move emitted: {w:04x}");
            }
        }
    }
    census_l1(&spec, &side, &result.champions);
    eprintln!(
        "nop_l1: items={} forks={} refuted={} prefilt={} canon={} champions={}",
        result.stats.items,
        result.stats.forks,
        result.stats.refuted,
        result.stats.prefiltered,
        result.stats.canon_pruned,
        result.stats.champions_found
    );
}

/// P3 delay-normal form, targeted: seed slot 0 to the MOV opcode (so
/// the space stays small) on a constant trace and check nop-pair delay
/// distributions: the front-loaded spelling is covered, the equivalent
/// non-front-loaded one is not (its behavior is identical — asserted —
/// so only the spelling was pruned).
#[test]
fn p3_delay_normal_form() {
    use pio_superopt::narrow::engine::NOP_CANON;
    let config = Config {
        pins: PinMap { out_base: 0, out_count: 1, in_base: 0, ..PinMap::default() },
        ..Config::default()
    };
    let mut spec = EngineSpec {
        cfg: cfg_for(config, 0, 1),
        slots: 2,
        cycles: 5,
        inputs: vec![],
        output_pins: vec![0],
        capture_pins: vec![0],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        // Both slots pinned to the MOV family to keep the space small;
        // delays remain fully searchable, which is what P3 quotients.
        seed: vec![(0, 0xE000, 0xA000), (1, 0xE000, 0xA000)],
        memo_cap: 1 << 20,
    };
    let reference = words_of(&[], &SideCfg::NONE);
    spec.expected = run_spec(&spec, reference);

    let result = search(&spec, 1_000_000);
    assert!(!result.champion_cap_hit);
    assert_champions_sound(&spec, &result.champions);

    let pair = |d0: u16, d1: u16| {
        let mut c = reference;
        c[0] = NOP_CANON | d0 << 8;
        c[1] = NOP_CANON | d1 << 8;
        c
    };
    // Same behavior, two spellings: only the front-loaded one covered.
    assert_eq!(run_spec(&spec, pair(1, 1)), run_spec(&spec, pair(2, 0)));
    assert!(covered(&result.champions, &pair(2, 0)), "front-loaded nop pair not covered");
    assert!(covered(&result.champions, &pair(1, 0)), "sum-1 front-loaded pair not covered");
    assert!(
        !covered(&result.champions, &pair(1, 1)),
        "non-front-loaded nop pair covered — P3 not pruning"
    );
    eprintln!(
        "p3_delay_normal_form: items={} canon={} champions={}",
        result.stats.items, result.stats.canon_pruned, result.stats.champions_found
    );
}

/// Ticket 007 gate: consulted-STATE generalization. Slot 0 is seeded to
/// `set x, <data free>` (32 x-values); slot 1 is searched against an
/// impossible period-3 duty trace. Slot-1 subtrees that never read X
/// record state patterns without the X component, so the 31 later
/// x-children hit records the first child wrote — sharing a monolithic
/// state key can never exhibit (x differs at every fork state).
#[test]
fn consulted_state_shares_across_unread_register() {
    let config = Config {
        pins: PinMap {
            set_base: 0,
            set_count: 1,
            out_base: 0,
            out_count: 1,
            in_base: 0,
            ..PinMap::default()
        },
        ..Config::default()
    };
    let expected =
        (0..18u32).map(|c| if c % 3 < 2 { 1 | 1 << 16 } else { 1 << 16 }).collect();
    let mut spec = EngineSpec {
        cfg: cfg_for(config, 0, 1),
        slots: 2,
        cycles: 18,
        inputs: vec![],
        output_pins: vec![0],
        capture_pins: vec![0],
        stim: Stim::default(),
        irq_sets: vec![],
        expected,
        seed: vec![(0, 0xFFE0, 0xE020)], // set x, <data undecided>
        memo_cap: 0,
    };
    let off = search(&spec, 10);
    spec.memo_cap = 1 << 20;
    let on = search(&spec, 10);
    assert_eq!(off.champions, on.champions, "memo changed the verdict");
    assert_eq!(off.stats.champions_found, 0, "period-3 unexpectedly satisfiable at L=2");
    assert!(on.stats.memo_hits > 0, "no memo sharing at all");
    assert!(
        on.stats.items < off.stats.items / 2,
        "consulted-state memo lost its cross-register sharing: on={} off={}",
        on.stats.items,
        off.stats.items
    );
    eprintln!(
        "consulted_state gate: off items={} on items={} hits={} core_matches={} entries={}",
        off.stats.items,
        on.stats.items,
        on.stats.memo_hits,
        on.stats.memo_core_matches,
        on.stats.memo_entries
    );
}

/// The memo must be invisible in results: identical champion LISTS (set
/// and order) with the memo on and off, across satisfiable and
/// impossible spaces, including one with register naming and pull-empty
/// binding forks in play.
#[test]
fn memo_on_off_equivalence() {
    let mut specs: Vec<EngineSpec> = Vec::new();
    // Square wave (satisfiable L=2).
    {
        let config = Config {
            pins: PinMap { set_base: 0, set_count: 1, out_base: 0, out_count: 1, ..PinMap::default() },
            ..Config::default()
        };
        let mut s = EngineSpec {
            cfg: cfg_for(config, 0, 1),
            slots: 2,
            cycles: 17,
            inputs: vec![],
            output_pins: vec![0],
            capture_pins: vec![0],
            stim: Stim::default(),
            irq_sets: vec![],
            expected: vec![],
            seed: vec![],
            memo_cap: 0,
        };
        let reference = words_of(
            &[
                Insn::plain(Op::Set { dst: SetDst::Pins, data: 1 }),
                Insn::plain(Op::Set { dst: SetDst::Pins, data: 0 }),
            ],
            &SideCfg::NONE,
        );
        s.expected = run_spec(&s, reference);
        specs.push(s);
    }
    // Period-3 (impossible L=1, loopback reads in play).
    {
        let config = Config {
            pins: PinMap {
                set_base: 0,
                set_count: 1,
                out_base: 0,
                out_count: 1,
                in_base: 0,
                ..PinMap::default()
            },
            ..Config::default()
        };
        let expected =
            (0..18u32).map(|c| if c % 3 < 2 { 1 | 1 << 16 } else { 1 << 16 }).collect();
        specs.push(EngineSpec {
            cfg: cfg_for(config, 0, 0),
            slots: 1,
            cycles: 18,
            inputs: vec![],
            output_pins: vec![0],
            capture_pins: vec![0],
            stim: Stim::default(),
            irq_sets: vec![],
            expected,
            seed: vec![],
            memo_cap: 0,
        });
    }
    // Pull-empty binding forks (seeded L=3).
    {
        let config = Config {
            pins: PinMap { out_base: 0, out_count: 1, ..PinMap::default() },
            ..Config::default()
        };
        let mut s = EngineSpec {
            cfg: cfg_for(config, 0, 2),
            slots: 3,
            cycles: 6,
            inputs: vec![],
            output_pins: vec![0],
            capture_pins: vec![0],
            stim: Stim::default(),
            irq_sets: vec![],
            expected: vec![],
            seed: vec![(0, 0xFFFF, 0xE043), (1, 0xFFFF, 0x8080)],
            memo_cap: 0,
        };
        let mut w = words_of(&[], &SideCfg::NONE);
        w[0] = 0xE043;
        w[1] = 0x8080;
        w[2] = 0x6001;
        s.expected = run_spec(&s, w);
        specs.push(s);
    }
    for (i, spec) in specs.iter_mut().enumerate() {
        spec.memo_cap = 0;
        let off = search(spec, 1_000_000);
        spec.memo_cap = 1 << 20;
        let on = search(spec, 1_000_000);
        assert_eq!(
            off.champions, on.champions,
            "spec {i}: champion lists diverge between memo off/on"
        );
        assert_eq!(off.stats.champions_found, on.stats.champions_found, "spec {i}");
        assert_eq!(off.champion_cap_hit, on.champion_cap_hit, "spec {i}");
        eprintln!(
            "memo_equivalence spec {i}: off items={} on items={} hits={} entries={}",
            off.stats.items, on.stats.items, on.stats.memo_hits, on.stats.memo_entries
        );
    }
}

/// The P1 binding fork, exercised end to end. PULL nonblocking on an
/// empty TX FIFO reads PHYSICAL X — the one register asymmetry in the
/// ISA — so an item whose x != y must fork into its two register
/// bindings there. Seed `set y,3 / pull noblock` (slots 0,1) and search
/// slot 2 only:
/// - against the seeded spelling's own trace (`out pins,1` emits
///   OSR = x = 0, pin LOW): the IDENTITY branch must cover it, and the
///   mirror spelling (whose OSR would be 3, pin HIGH) must NOT be
///   covered;
/// - against the mirror's trace: the TWIN branch must produce a
///   champion whose words ARE the mirror spelling (set x,3), and the
///   seeded spelling must NOT be covered.
#[test]
fn pull_empty_binding_fork() {
    const SET_Y_3: u16 = 0xE043; // set y, 3
    const SET_X_3: u16 = 0xE023; // set x, 3 (the mirror)
    const PULL_NOBLOCK: u16 = 0x8080;
    const OUT_PINS_1: u16 = 0x6001;

    let side = SideCfg::NONE;
    let base_spec = || {
        let config = Config {
            pins: PinMap { out_base: 0, out_count: 1, ..PinMap::default() },
            ..Config::default()
        };
        EngineSpec {
            cfg: cfg_for(config, 0, 2),
            slots: 3,
            cycles: 6,
            inputs: vec![],
            output_pins: vec![0],
            capture_pins: vec![0],
            stim: Stim::default(),
            irq_sets: vec![],
            expected: vec![],
            seed: vec![(0, 0xFFFF, SET_Y_3), (1, 0xFFFF, PULL_NOBLOCK)],
            memo_cap: 1 << 20,
        }
    };
    let mut w_seeded = words_of(&[], &side);
    w_seeded[0] = SET_Y_3;
    w_seeded[1] = PULL_NOBLOCK;
    w_seeded[2] = OUT_PINS_1;
    let w_mirror = mirror_program(&w_seeded, 3);
    assert_eq!(w_mirror[0], SET_X_3);
    assert_eq!(w_mirror[1], PULL_NOBLOCK);

    // (a) The seeded spelling's trace: OSR <- x = 0, pin driven LOW.
    let mut spec_a = base_spec();
    spec_a.expected = run_spec(&spec_a, w_seeded);
    let ra = search(&spec_a, 100_000);
    assert!(!ra.champion_cap_hit);
    assert_champions_sound(&spec_a, &ra.champions);
    assert!(covered(&ra.champions, &w_seeded), "identity branch lost the seeded program");
    assert!(!covered(&ra.champions, &w_mirror), "mirror spelling wrongly covered — traces differ");

    // (b) The mirror's trace: its pull reads x = 3, pin stays HIGH.
    let mut spec_b = base_spec();
    spec_b.expected = run_spec(&spec_b, w_mirror);
    assert_ne!(spec_a.expected, spec_b.expected, "the two bindings must be distinguishable");
    let rb = search(&spec_b, 100_000);
    assert!(!rb.champion_cap_hit);
    assert_champions_sound(&spec_b, &rb.champions);
    assert!(covered(&rb.champions, &w_mirror), "twin branch failed to produce the mirror champion");
    assert!(!covered(&rb.champions, &w_seeded), "seeded spelling wrongly covered — traces differ");
    eprintln!(
        "pull_empty_binding_fork: a items={} champs={}, b items={} champs={}",
        ra.stats.items, ra.stats.champions_found, rb.stats.items, rb.stats.champions_found
    );
}

/// The shipped tx_a (rs485-eth dme_pio.rs): IRQ→DI toggler with the
/// jmp-PIN parity absorber, `.side_set 1 opt` on DI, wrap 0..1.
fn tx_a_words(side: &SideCfg) -> [u16; 32] {
    words_of(
        &[
            // low: wait 1 irq 0 side 1
            Insn { op: Op::Wait { polarity: true, src: WaitSrc::Irq, index: 0 }, delay: 0, sideset: Some(1) },
            // jmp PIN high(2)
            Insn::plain(Op::Jmp { cond: JmpCond::Pin, target: 2 }),
            // high: wait 1 irq 0 side 0
            Insn { op: Op::Wait { polarity: true, src: WaitSrc::Irq, index: 0 }, delay: 0, sideset: Some(0) },
            // jmp low(0)
            Insn::plain(Op::Jmp { cond: JmpCond::Always, target: 0 }),
        ],
        side,
    )
}

/// tx_a's environment and observable: DI on pin 0 (side-set base 0,
/// captured with OE), DE on pin 8 (external stimulus, jmp_pin), IRQ 0
/// pulses from the "sequencer". The schedule covers: idempotent
/// force-highs while DE is low (the parity absorber), toggling while DE
/// is high across varied gaps, and a DE drop mid-stream.
fn tx_a_spec(cycles: u32) -> (EngineSpec, SideCfg) {
    let side = SideCfg { count: 2, en: true }; // .side_set 1 opt
    let config = Config {
        side,
        pins: PinMap { sideset_base: 0, in_base: 16, ..PinMap::default() },
        jmp_pin: 8,
        ..Config::default()
    };
    let cfg = cfg_for(config, 0, 1);

    // DE (pin 8): low, then high through the "frame", low again, high.
    let mut stim_values = Vec::with_capacity(cycles as usize);
    for c in 0..cycles {
        let de_high = (60..300).contains(&c) || (340..cycles.saturating_sub(10)).contains(&c);
        stim_values.push(if de_high { 1u32 << 8 } else { 0 });
    }
    // IRQ pulses: parity-absorber hits while DE low, then bit-ish
    // cadence while high (varied gaps), one stray after the DE drop.
    let mut irq_sets = Vec::new();
    for c in [10u32, 25, 40, 70, 82, 90, 102, 118, 126, 140, 160, 168, 190, 210, 240, 265, 290, 310, 350, 365, 380, 400, 430] {
        if c < cycles {
            irq_sets.push((c, 1u8));
        }
    }

    let spec = EngineSpec {
        cfg,
        slots: 4,
        cycles,
        inputs: vec![],
        output_pins: vec![0],
        capture_pins: vec![0, 8],
        stim: Stim { mask: 1 << 8, values: stim_values },
        irq_sets,
        expected: vec![],
        seed: vec![],
        memo_cap: 1 << 20,
    };
    (spec, side)
}

/// The oracle must show real toggling (not a constant line) or the
/// search below proves nothing.
#[test]
fn tx_a_oracle_sanity() {
    let (mut spec, side) = tx_a_spec(460);
    let reference = tx_a_words(&side);
    spec.expected = run_spec(&spec, reference);
    let toggles = spec.expected.windows(2).filter(|w| (w[0] ^ w[1]) & 1 != 0).count();
    assert!(toggles >= 10, "DI toggled only {toggles} times — schedule too weak");
}

/// The L=3 wall bracket alone, with the big-memo configuration
/// (8M-entry cap, gentler purge) — the targeted experiment for memo
/// capacity policy. Run: `cargo test --release --test narrow_engine --
/// --ignored tx_a_l3_first --nocapture`
#[test]
#[ignore]
fn tx_a_l3_first_bracket() {
    let (mut spec4, side) = tx_a_spec(460);
    let reference = tx_a_words(&side);
    spec4.expected = run_spec(&spec4, reference);

    let (mut s, _) = tx_a_spec(460);
    s.slots = 3;
    s.cfg.wrap_bottom = 0;
    s.cfg.wrap_top = 0;
    s.expected = spec4.expected.clone();
    s.memo_cap = 1 << 23;
    let r = search(&s, 5);
    eprintln!(
        "L=3 wrap 0..0 (bigmemo): items={} forks={} refuted={} prefilt={} canon={} memo_hit={} memo_ent={} purges={} champions={}",
        r.stats.items,
        r.stats.forks,
        r.stats.refuted,
        r.stats.prefiltered,
        r.stats.canon_pruned,
        r.stats.memo_hits,
        r.stats.memo_entries,
        r.stats.memo_purges,
        r.stats.champions_found
    );
    eprintln!("  benefit_hist: {}", r.stats.benefit_hist_compact());
    assert_eq!(r.stats.champions_found, 0, "L=3 wrap 0..0 unexpectedly satisfiable");
}

/// The main event: rediscover tx_a from its trace at L=4 and prove
/// footprint <= 3 impossible. Long-running; #[ignore]d from CI.
#[test]
#[ignore]
fn tx_a_rediscovery_and_optimality() {
    let (mut spec4, side) = tx_a_spec(460);
    let reference = tx_a_words(&side);
    spec4.expected = run_spec(&spec4, reference);

    // Optimality: exhaust every shorter footprint (all wrap choices).
    for slots in 1..=3u8 {
        for wb in 0..slots {
            for wt in wb..slots {
                let (mut s, _) = tx_a_spec(460);
                s.slots = slots;
                s.cfg.wrap_bottom = wb;
                s.cfg.wrap_top = wt;
                s.expected = spec4.expected.clone();
                let r = search(&s, 5);
                eprintln!(
                    "L={slots} wrap {wb}..{wt}: items={} forks={} refuted={} prefilt={} canon={} memo_hit={} memo_ent={} purges={} champions={}",
                    r.stats.items,
                    r.stats.forks,
                    r.stats.refuted,
                    r.stats.prefiltered,
                    r.stats.canon_pruned,
                    r.stats.memo_hits,
                    r.stats.memo_entries,
                    r.stats.memo_purges,
                    r.stats.champions_found
                );
                eprintln!("  benefit_hist: {}", r.stats.benefit_hist_compact());
                assert_eq!(
                    r.stats.champions_found, 0,
                    "L={slots} wrap {wb}..{wt} unexpectedly matches tx_a's trace: {:04x?}",
                    r.champions.first().map(|c| &c.value[..slots as usize])
                );
            }
        }
    }

    // Rediscovery at tx_a's own shape (L=4, wrap 0..1).
    let r = search(&spec4, 100_000);
    eprintln!(
        "L=4 wrap 0..1: items={} forks={} refuted={} champions={} cap_hit={}",
        r.stats.items, r.stats.forks, r.stats.refuted, r.stats.champions_found, r.champion_cap_hit
    );
    assert!(r.stats.champions_found > 0, "tx_a's own shape found no champions");
    assert_champions_sound(&spec4, &r.champions);
    assert!(covered(&r.champions, &reference), "tx_a itself is not covered by any champion");
}

/// The probe-log and snapshot instrumentation flags must be
/// side-effect free w.r.t. the search: champions and every stat are
/// bit-identical with them on, and the files appear with parseable
/// content. Uses a small memo cap so purges (and the pre-purge
/// snapshot) actually fire.
#[test]
fn instrumentation_flags_do_not_change_search() {
    let config = Config {
        pins: PinMap { set_base: 0, set_count: 1, out_base: 0, out_count: 1, ..PinMap::default() },
        ..Config::default()
    };
    let cfg = cfg_for(config, 0, 1);
    let side = SideCfg::NONE;
    let reference = words_of(
        &[
            Insn::plain(Op::Set { dst: SetDst::Pins, data: 1 }),
            Insn::plain(Op::Set { dst: SetDst::Pins, data: 0 }),
        ],
        &side,
    );
    let mut spec = EngineSpec {
        cfg,
        slots: 2,
        cycles: 17,
        inputs: vec![],
        output_pins: vec![0],
        capture_pins: vec![0],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        seed: vec![],
        memo_cap: 64,
    };
    spec.expected = run_spec(&spec, reference);

    let baseline = search(&spec, 1_000_000);

    let dir = std::env::temp_dir().join(format!("narrow_instr_gate_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let probe_path = dir.join("probe.jsonl");
    let snap_dir = dir.join("snaps");
    std::env::set_var("PIO_NARROW_PROBE_LOG", &probe_path);
    std::env::set_var("PIO_NARROW_PROBE_BYTES", "8000000");
    std::env::set_var("PIO_NARROW_SNAPSHOT", &snap_dir);
    std::env::set_var("PIO_NARROW_SNAPSHOT_MAX", "4");
    let instrumented = search(&spec, 1_000_000);
    std::env::remove_var("PIO_NARROW_PROBE_LOG");
    std::env::remove_var("PIO_NARROW_PROBE_BYTES");
    std::env::remove_var("PIO_NARROW_SNAPSHOT");
    std::env::remove_var("PIO_NARROW_SNAPSHOT_MAX");

    assert_eq!(baseline.champions, instrumented.champions, "instrumentation changed champions");
    assert_eq!(
        format!("{:?}", baseline.stats),
        format!("{:?}", instrumented.stats),
        "instrumentation changed stats"
    );

    // Probe log: non-empty, has census rows and the end summary.
    // (Line-level format checks stay lenient: concurrent tests may
    // append to the same file while the env vars are set.)
    let probe = std::fs::read_to_string(&probe_path).unwrap();
    assert!(probe.lines().any(|l| l.starts_with("{\"census_cycle\"")), "no census rows");
    assert!(probe.lines().any(|l| l.starts_with("{\"probe_log_end\"")), "no end summary");
    if instrumented.stats.memo_state_misses + instrumented.stats.memo_cond_misses > 0 {
        assert!(
            probe.lines().any(|l| l.starts_with("{\"probe\":")),
            "misses occurred but no detail lines were sampled"
        );
    }

    // Snapshots: the end snapshot always exists; a purge snapshot
    // exists iff purges fired.
    let names: Vec<String> = std::fs::read_dir(&snap_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n.contains("-end-")), "no end snapshot: {names:?}");
    if instrumented.stats.memo_purges > 0 {
        assert!(names.iter().any(|n| n.contains("-purge-")), "purges fired but no purge snapshot");
    }
    // Every snapshot line is a JSON object with the expected leaders.
    for n in &names {
        let s = std::fs::read_to_string(snap_dir.join(n)).unwrap();
        assert!(s.lines().next().unwrap().starts_with("{\"snapshot\""), "{n}: bad header");
        assert!(
            s.lines().skip(1).all(|l| l.starts_with("{\"cycle\"")),
            "{n}: bad record line"
        );
    }

    let sm = instrumented.stats.memo_state_misses;
    let cm = instrumented.stats.memo_cond_misses;
    eprintln!(
        "instr gate: items={} core_matches={} state_miss={sm} cond_miss={cm} hits={} purges={} snaps={}",
        instrumented.stats.items,
        instrumented.stats.memo_core_matches,
        instrumented.stats.memo_hits,
        instrumented.stats.memo_purges,
        names.len()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The split driver must agree with the sequential engine. Refutation
/// verdicts are exactly equivalent (that is the driver's job — L=3+
/// brackets); champion-bearing spaces agree on the COVERED SET, with
/// representation allowed to differ (mirror expansion, seed-boundary
/// delay spellings): every split champion must reproduce the trace and
/// be covered by the sequential quotient, and every sequential
/// champion's representative (and its register-mirror when
/// binding-free) must be covered by the split set. Two same-config
/// runs must be bit-identical (scheduling independence).
#[test]
fn split_agrees_with_sequential() {
    use pio_superopt::narrow::engine::search_split;

    // Refutation: the real L=2 0..0 tx_a bracket (632,537 items).
    let (spec4, _) = tx_a_spec(460);
    let mut expected4 = spec4.clone();
    expected4.expected = run_spec(&spec4, tx_a_words(&tx_a_spec(460).1));
    let (mut s, _) = tx_a_spec(460);
    s.slots = 2;
    s.cfg.wrap_bottom = 0;
    s.cfg.wrap_top = 0;
    s.expected = expected4.expected.clone();
    let seq = search(&s, 5);
    let par = search_split(&s, 5, 8);
    assert_eq!(seq.stats.champions_found, 0);
    assert_eq!(par.stats.champions_found, 0, "split found champions where sequential refuted");
    let par2 = search_split(&s, 5, 8);
    assert_eq!(par.champions, par2.champions);
    assert_eq!(format!("{:?}", par.stats), format!("{:?}", par2.stats), "split not deterministic");

    // Champions: the square wave.
    let config = Config {
        pins: PinMap { set_base: 0, set_count: 1, out_base: 0, out_count: 1, ..PinMap::default() },
        ..Config::default()
    };
    let cfg = cfg_for(config, 0, 1);
    let side = SideCfg::NONE;
    let reference = words_of(
        &[
            Insn::plain(Op::Set { dst: SetDst::Pins, data: 1 }),
            Insn::plain(Op::Set { dst: SetDst::Pins, data: 0 }),
        ],
        &side,
    );
    let mut spec = EngineSpec {
        cfg,
        slots: 2,
        cycles: 17,
        inputs: vec![],
        output_pins: vec![0],
        capture_pins: vec![0],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        seed: vec![],
        memo_cap: 1 << 20,
    };
    spec.expected = run_spec(&spec, reference);
    let seq = search(&spec, 1_000_000);
    let par = search_split(&spec, 1_000_000, 8);
    assert!(!par.champion_cap_hit);
    assert_champions_sound(&spec, &par.champions);
    // Split -> sequential: every split champion's representative lives
    // inside the sequential quotient (identity or mirror side).
    for (i, sp) in par.champions.iter().enumerate() {
        let w = sp.words();
        let ok = covered(&seq.champions, &w)
            || covered(&seq.champions, &mirror_program(&w, spec.slots));
        assert!(ok, "split champion {i} not covered by sequential quotient");
    }
    // Sequential -> split: representatives (and binding-free mirrors)
    // are covered by the split set.
    for (i, ch) in seq.champions.iter().enumerate() {
        let w = ch.words();
        assert!(covered(&par.champions, &w), "sequential champion {i} not covered by split");
        if ch.binding_free {
            let m = mirror_program(&w, spec.slots);
            let ok = covered(&par.champions, &m)
                || par.champions.iter().any(|sp| {
                    sp.binding_free
                        && (0..32).all(|s| m[s] & sp.decided[s] == mirror_program(&sp.words(), spec.slots)[s] & sp.decided[s])
                });
            assert!(ok, "sequential champion {i}'s mirror not covered by split");
        }
    }
    // The hand-written program must be found by both.
    assert!(covered(&par.champions, &reference));
    eprintln!(
        "split gate: refutation ok; square wave seq {} champions, split {} champions",
        seq.champions.len(),
        par.champions.len()
    );
}

/// Re-establish the small tx_a bracket verdicts (L=1 0..0 and L=2
/// 1..1; L=2 0..0 lives in `split_agrees_with_sequential`, L=2 0..1 in
/// `tx_a_l2_01_split`). Run after any semantics change:
/// `cargo test --release --test narrow_engine -- --ignored
/// tx_a_small_brackets --nocapture`
#[test]
#[ignore]
fn tx_a_small_brackets_split() {
    use pio_superopt::narrow::engine::search_split;
    let (spec4, side) = tx_a_spec(460);
    let reference = tx_a_words(&side);
    let mut expected = spec4.clone();
    expected.expected = run_spec(&spec4, reference);
    for (slots, wb, wt) in [(1u8, 0u8, 0u8), (2, 1, 1)] {
        let (mut s, _) = tx_a_spec(460);
        s.slots = slots;
        s.cfg.wrap_bottom = wb;
        s.cfg.wrap_top = wt;
        s.expected = expected.expected.clone();
        let t = std::time::Instant::now();
        let r = search_split(&s, 5, 28);
        eprintln!(
            "L={slots} wrap {wb}..{wt} split(28): items={} champions={} in {:.1}s",
            r.stats.items,
            r.stats.champions_found,
            t.elapsed().as_secs_f64()
        );
        assert_eq!(
            r.stats.champions_found, 0,
            "L={slots} wrap {wb}..{wt} unexpectedly satisfiable"
        );
    }
}

/// The remaining five L=3 wrap brackets (0..0 fell 2026-07-12: 5.33B
/// items, 26 min at 28 threads). All six refuted = footprint <= 3
/// impossible for tx_a under the narrow evaluator. Detached:
/// `cargo test --release --test narrow_engine -- --ignored
/// tx_a_l3_rest --nocapture`
#[test]
#[ignore]
fn tx_a_l3_rest_brackets() {
    use pio_superopt::narrow::engine::search_split;
    let (mut spec4, side) = tx_a_spec(460);
    let reference = tx_a_words(&side);
    spec4.expected = run_spec(&spec4, reference);
    for (wb, wt) in [(0u8, 1u8), (0, 2), (1, 1), (1, 2), (2, 2)] {
        let (mut s, _) = tx_a_spec(460);
        s.slots = 3;
        s.cfg.wrap_bottom = wb;
        s.cfg.wrap_top = wt;
        s.expected = spec4.expected.clone();
        s.memo_cap = 1 << 21;
        let t = std::time::Instant::now();
        let r = search_split(&s, 5, 28);
        eprintln!(
            "L=3 wrap {wb}..{wt} split(28): items={} refuted={} memo_hit={} champions={} in {:.0}s",
            r.stats.items,
            r.stats.refuted,
            r.stats.memo_hits,
            r.stats.champions_found,
            t.elapsed().as_secs_f64()
        );
        assert_eq!(r.stats.champions_found, 0, "L=3 wrap {wb}..{wt} unexpectedly satisfiable");
    }
    eprintln!("L=3 COMPLETE: all six brackets refuted");
}

/// The L=3 0..0 bracket under the split driver — the corrected-
/// semantics verdict run. Detached:
/// `cargo test --release --test narrow_engine -- --ignored
/// tx_a_l3_00_split --nocapture`
#[test]
#[ignore]
fn tx_a_l3_00_split() {
    use pio_superopt::narrow::engine::search_split;
    let (mut spec4, side) = tx_a_spec(460);
    let reference = tx_a_words(&side);
    spec4.expected = run_spec(&spec4, reference);
    let (mut s, _) = tx_a_spec(460);
    s.slots = 3;
    s.cfg.wrap_bottom = 0;
    s.cfg.wrap_top = 0;
    s.expected = spec4.expected.clone();
    s.memo_cap = 1 << 21; // per-unit memos; 28 workers share RAM
    let t = std::time::Instant::now();
    let r = search_split(&s, 5, 28);
    eprintln!(
        "L=3 wrap 0..0 split(28): items={} forks={} refuted={} memo_hit={} champions={} in {:.0}s",
        r.stats.items,
        r.stats.forks,
        r.stats.refuted,
        r.stats.memo_hits,
        r.stats.champions_found,
        t.elapsed().as_secs_f64()
    );
    eprintln!("  benefit_hist: {}", r.stats.benefit_hist_compact());
    assert_eq!(r.stats.champions_found, 0, "L=3 wrap 0..0 unexpectedly satisfiable");
}

/// Wall-clock measurement of the split driver on the L=2 0..1 bracket
/// (sequential: ~270s). `cargo test --release --test narrow_engine --
/// --ignored tx_a_l2_01_split --nocapture`
#[test]
#[ignore]
fn tx_a_l2_01_split() {
    use pio_superopt::narrow::engine::search_split;
    let (spec4, side) = tx_a_spec(460);
    let reference = tx_a_words(&side);
    let mut expected = spec4.clone();
    expected.expected = run_spec(&spec4, reference);
    let (mut s, _) = tx_a_spec(460);
    s.slots = 2;
    s.cfg.wrap_bottom = 0;
    s.cfg.wrap_top = 1;
    s.expected = expected.expected.clone();
    let t = std::time::Instant::now();
    let r = search_split(&s, 5, 28);
    eprintln!(
        "L=2 0..1 split(28): items={} memo_hit={} champions={} in {:.1}s",
        r.stats.items, r.stats.memo_hits, r.stats.champions_found, t.elapsed().as_secs_f64()
    );
    assert_eq!(r.stats.champions_found, 0);
}

/// Ticket 009 lemma gate: `word_canon` merges spellings ONLY when they
/// are behaviorally identical. Every non-identity respelling is
/// executed against its representative from a diverse state battery ×
/// several gpio words, on configs chosen to arm every lemma: the tx_a
/// config (out/set_count 0, side-set, sm_id 0), a 1-pin SET/OUT
/// config (data masking), an autopush/autopull config (counter
/// visibility), and an sm_id=1 variant (rel-IRQ folding must NOT
/// collapse the same way). Also checks idempotence on all 65,536
/// words. This is the "battery verifies, lemmas prove" contract: a
/// lemma bug shows up here as a behavioral diff.
#[test]
fn word_canon_battery_sound() {
    use pio_superopt::narrow::engine::word_canon;
    use pio_superopt::narrow::{step, NState};

    fn battery(cfg: &NCfg) -> Vec<NState> {
        let xs = [0u32, 1, 2, 5, 31, 32, 0x8000_0000, 0xFFFF_FFFF, 0xA5A5_5A5A];
        let mut states = Vec::new();
        for i in 0..24usize {
            let mut st = NState::new(cfg);
            st.x = xs[i % xs.len()];
            st.y = xs[(i * 7 + 3) % xs.len()];
            st.isr = xs[(i * 3 + 1) % xs.len()];
            st.osr = xs[(i * 5 + 2) % xs.len()];
            st.isr_count = [0u8, 1, 4, 7, 8, 31, 32][i % 7];
            st.osr_count = [32u8, 0, 1, 7, 8, 31, 16][i % 7];
            st.irq_flags = [0u8, 1, 0x80, 0xFF, 2, 0x55][i % 6];
            st.out_latch = xs[(i * 11 + 4) % xs.len()];
            st.dir_latch = [0u32, u32::MAX, 0x0000_FFFF][i % 3];
            st.pc = (i % 4) as u8;
            for k in 0..(i % 5) {
                st.tx.push(0x1111_1111u32.wrapping_mul(k as u32 + 1));
            }
            for k in 0..(i % 4) {
                st.rx.push(0x2222_2222u32.wrapping_mul(k as u32 + 1));
            }
            states.push(st);
        }
        states
    }

    fn step_word(cfg: &NCfg, st0: &NState, w: u16, gpio: u32) -> NState {
        let mut c = cfg.clone();
        c.code = [w; 32];
        let mut st = *st0;
        step(&mut st, &c, gpio);
        st
    }

    let tx_a = tx_a_spec(460).0.cfg;
    let one_pin = cfg_for(
        Config {
            pins: PinMap {
                set_base: 0,
                set_count: 1,
                out_base: 0,
                out_count: 1,
                in_base: 0,
                ..PinMap::default()
            },
            ..Config::default()
        },
        0,
        31,
    );
    let mut auto = one_pin.clone();
    auto.autopush = true;
    auto.autopull = true;
    auto.push_threshold = 8;
    auto.pull_threshold = 8;
    let mut sm1 = tx_a.clone();
    sm1.sm_id = 1;

    for (name, cfg) in [("tx_a", &tx_a), ("one_pin", &one_pin), ("auto", &auto), ("sm1", &sm1)] {
        let states = battery(cfg);
        let mut merged = 0u32;
        for w in 0..=0xFFFFu16 {
            let c = word_canon(w, cfg);
            assert_eq!(word_canon(c, cfg), c, "[{name}] canon not idempotent at {w:04x}");
            if c == w {
                continue;
            }
            merged += 1;
            for st in &states {
                for gpio in [0u32, u32::MAX, 0x0000_0101, 1 << 8] {
                    assert_eq!(
                        step_word(cfg, st, w, gpio),
                        step_word(cfg, st, c, gpio),
                        "[{name}] word_canon merged behaviorally distinct words: {w:04x} -> {c:04x}"
                    );
                }
            }
        }
        eprintln!("word_canon[{name}]: {merged} of 65536 words respelled, battery-verified");
    }
}
