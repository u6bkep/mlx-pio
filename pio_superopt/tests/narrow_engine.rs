//! Gate for the needed-narrowing fork loop (`narrow::engine`).
//!
//! The sanity tests run small exhaustive searches whose answers are
//! knowable by hand; the tx_a validation (the engine's first real
//! target) rediscovers the shipped 4-instruction IRQ→DI toggler from
//! its trace and proves nothing shorter matches — run it with:
//! `cargo test --release --test narrow_engine -- --ignored tx_a --nocapture`

use pio_superopt::encode::encode_insn;
use pio_superopt::ir::{Insn, JmpCond, Op, SetDst, SideCfg, WaitSrc};
use pio_superopt::narrow::engine::{mirror_word, run_spec, search, EngineSpec};
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
        assert_eq!(
            run_spec(spec, ch.words()),
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

/// The canonical respelling of a word under P2/P4 (L=1, so the JMP
/// fallthrough of slot 0 is `wrap_bottom`): pure-no-op spellings map to
/// NOP_CANON with delay preserved.
fn canon_l1_word(w: u16, spec: &EngineSpec) -> u16 {
    use pio_superopt::narrow::engine::NOP_CANON;
    let delay = w & 0x1F00;
    match (w >> 13) & 0x7 {
        0 => {
            let cond = (w >> 5) & 0x7;
            let ft = if 0 == spec.cfg.wrap_top { spec.cfg.wrap_bottom } else { 1 } as u16;
            if cond != 2 && cond != 4 && (w & 0x1F) == ft {
                return NOP_CANON | delay;
            }
            w
        }
        5 => {
            let (dst, op, src) = ((w >> 5) & 0x7, (w >> 3) & 0x3, w & 0x7);
            if op == 0 && dst == src && matches!(dst, 1 | 2 | 6 | 7) {
                return NOP_CANON | delay;
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
        let matches = run_spec(spec, code) == spec.expected;
        n_match += matches as u32;
        if matches && !in_l1_space(w, spec.slots) {
            continue; // documented space exclusion, not a claim
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
            if op == 0 && dst == src && matches!(dst, 1 | 2 | 6 | 7) {
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
                    "L={slots} wrap {wb}..{wt}: items={} forks={} refuted={} champions={}",
                    r.stats.items, r.stats.forks, r.stats.refuted, r.stats.champions_found
                );
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
