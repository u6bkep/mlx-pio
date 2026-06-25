//! Canonical search workloads, shared by the test suite and the Criterion
//! benches so both measure the *same* program the search actually evaluates.
//!
//! The DME (Differential Manchester / biphase-mark) reference is the locked
//! benchmark target (tickets 001 / the v2-IR motivation): a transition at every
//! bit boundary (the clock) plus a data-conditional mid-bit transition iff the
//! data bit is 1. `gene_search::tests::dme_reference_scores_zero` pins it.

use crate::gene::{CondKind, Gene, LoopCond, Node};
use crate::ir::{Insn, MovDst, MovOp, MovSrc, Op, OutDst};
use crate::program::{Config, PinMap, Program, ShiftCfg, ShiftDir};
use crate::run::{run, RunSpec};
use crate::ir::SideCfg;

/// The single output (TX) pin used by every DME workload.
pub const TX: u8 = 0;

/// Half-bit duration in PIO cycles for the locked DME reference.
pub const DME_H: u8 = 4;

/// Locked capture window: covers the 4-code corpus (active to cycle 272) with a
/// small tail, so "correctness" isn't inflated by a long constant stall.
pub const DME_CYCLES: u64 = 278;

/// SM config for the DME workloads: 5-bit 4B/5B line codes, LSB-first.
pub fn dme_cfg() -> Config {
    Config {
        side: SideCfg::NONE,
        clkdiv_int: 1,
        shift: ShiftCfg { pull_threshold: 5, out_dir: ShiftDir::Right, ..ShiftCfg::default() },
        pins: PinMap { out_base: TX, out_count: 1, set_base: TX, set_count: 1, ..PinMap::default() },
        ..Config::default()
    }
}

fn pull() -> Node {
    Node::Prim(Insn::plain(Op::Pull { if_empty: false, block: true }))
}
fn mov_y_inv() -> Node {
    Node::Prim(Insn::plain(Op::Mov { dst: MovDst::Y, op: MovOp::Invert, src: MovSrc::Y }))
}
fn drive(d: u8) -> Node {
    Node::Prim(Insn { op: Op::Mov { dst: MovDst::Pins, op: MovOp::None, src: MovSrc::Y }, delay: d, sideset: None })
}
fn out_x() -> Node {
    Node::Prim(Insn::plain(Op::Out { dst: OutDst::X, count: 1 }))
}

/// Reference biphase-mark DME encoder, half-bit = `h` cycles. Tracks level in Y.
/// Per bit: boundary transition + first-half hold, fetch the data bit, a
/// conditional mid-bit transition (`if x-- {toggle}`, `skip_delay=1` to balance
/// the 0/1 paths to equal duration), then the second-half hold.
pub fn dme_ref(h: u8) -> Gene {
    let cell = vec![
        out_x(),       // data bit -> X (consumed by the Cond)
        mov_y_inv(),   // boundary transition (clock edge)
        drive(h - 1),  // drive + hold first half (h cycles)
        Node::Cond {
            cond: CondKind::XPostDec, // taken iff bit == 1
            then: vec![mov_y_inv(), drive(0)], // mid-bit transition
            els: vec![],
            dispatch_delay: 0,
            skip_delay: 1, // balance: 0-path == 1-path duration
        },
        drive(h - 1), // second-half hold (re-drives current level, both paths)
    ];
    Gene {
        config: dme_cfg(),
        nodes: vec![
            pull(),
            Node::Loop { cond: LoopCond::UntilOsrEmpty, counter_init: None, body: cell, jmp_delay: 0 },
        ],
    }
}

/// A multi-code corpus (diverse 4B/5B data codes) so a thin oracle can't be
/// gamed. Processed back-to-back: the loop wraps to `pull` between codes.
pub fn dme_corpus() -> Vec<u32> {
    vec![0x1E, 0x0A, 0x15, 0x09] // codes 0,4,3,1; lsb bits 01111/01010/10101/10010
}

/// The locked RunSpec: feed the corpus, capture TX for `cycles`.
pub fn dme_spec(cycles: u64) -> RunSpec {
    RunSpec {
        block: 0,
        sm: 0,
        inputs: dme_corpus(),
        output_pins: vec![TX],
        capture_pins: vec![TX],
        cycles,
    }
}

/// The locked DME benchmark: `(spec, golden, full_mask)`. Golden is the
/// reference's own output under the locked window — a self-consistent oracle.
pub fn dme_golden() -> (RunSpec, Vec<u32>, Vec<u32>) {
    let sp = dme_spec(DME_CYCLES);
    let golden = run(&dme_ref(DME_H).lower(), &sp);
    let mask = vec![u32::MAX; golden.len()];
    (sp, golden, mask)
}

/// RANDOM-DATA training set — communication protocols make labeled data free:
/// run arbitrary inputs through the reference encoder and capture its output, so
/// we can synthesize as much (input, golden) data as we want. Returns `(spec,
/// golden, full_mask)` for `n_codes` random 5-bit codes drawn from `seed`, with
/// the capture window sized to the active region (last transition + a half-bit
/// tail) so a long constant stall can't inflate "correctness".
///
/// A larger, more-diverse corpus strips out the corpus-specific partial credit a
/// champion can earn by accident — a fixed-pattern replay or a level-driven
/// fake aligns with a *short* golden far more easily than with a long random
/// one. The held-out [`dme_validate`] gate is the independent generalization
/// check on top of this.
pub fn dme_random_golden(n_codes: usize, seed: u64) -> (RunSpec, Vec<u32>, Vec<u32>) {
    let mut rng = crate::rng::Rng::new(seed);
    let corpus: Vec<u32> = (0..n_codes).map(|_| rng.below(32)).collect();
    let lowered = dme_ref(DME_H).lower();
    // Probe generously, then trim the window to the last transition + tail.
    let probe = RunSpec { inputs: corpus.clone(), cycles: n_codes as u64 * 80 + 64, ..dme_spec(0) };
    let full = run(&lowered, &probe);
    let last = full.windows(2).rposition(|w| (w[0] ^ w[1]) & 1 != 0).map(|i| i + 1).unwrap_or(0);
    let cycles = (last + DME_H as usize + 2) as u64;
    let sp = RunSpec { inputs: corpus, cycles, ..dme_spec(0) };
    let golden = run(&lowered, &sp);
    let mask = vec![u32::MAX; golden.len()];
    (sp, golden, mask)
}

/// HELD-OUT validation corpus: 4 distinct 4B/5B data codes, none of which appear
/// in [`dme_corpus`], with a different mid-transition pattern. The search trains
/// on `dme_corpus`; this is the generalization oracle. A program that *overfits*
/// the training corpus — replays its specific 278-cycle waveform via fixed
/// delays rather than reading the data — scores 0 on train but nonzero here.
/// Only a genuinely data-driven DME encoder reproduces the reference on both.
pub fn dme_validation_corpus() -> Vec<u32> {
    vec![0x12, 0x16, 0x1B, 0x0F] // codes 8,A,D,7; lsb 01001/01101/11011/11110
}

/// The held-out validation benchmark: `(spec, golden, full_mask)` — same locked
/// window and config as [`dme_golden`] but driven by [`dme_validation_corpus`].
pub fn dme_validation_golden() -> (RunSpec, Vec<u32>, Vec<u32>) {
    let sp = RunSpec { inputs: dme_validation_corpus(), ..dme_spec(DME_CYCLES) };
    let golden = run(&dme_ref(DME_H).lower(), &sp);
    let mask = vec![u32::MAX; golden.len()];
    (sp, golden, mask)
}

/// VALIDATION GATE: strict correctness of `champ` on the training corpus and the
/// held-out validation corpus, as `(train, held_out)`. A champion is a *real*
/// DME solution only when BOTH are 0 — exact on training proves it fits the
/// objective, exact on held-out proves it generalizes (isn't an overfit replay).
/// Use this to qualify every cost-0 champion before trusting it.
pub fn dme_validate(champ: &Program) -> (u32, u32) {
    let (tsp, tg, _) = dme_golden();
    let (vsp, vg, _) = dme_validation_golden();
    let train = crate::cost::score(champ, &tg, &tsp).correctness;
    let held_out = crate::cost::score(champ, &vg, &vsp).correctness;
    (train, held_out)
}

/// A representative *imperfect* candidate: the reference with the data-conditional
/// mid-bit transition removed. This is the boundary-only "plateau" basin the
/// flat search gets stuck in — a realistic, edge-dense, misaligned waveform for
/// stressing the edge-cost DP aligner (not a degenerate all-zeros input).
pub fn dme_plateau_gene() -> Gene {
    let cell = vec![
        out_x(),
        mov_y_inv(),
        drive(DME_H - 1),
        drive(DME_H - 1),
    ];
    Gene {
        config: dme_cfg(),
        nodes: vec![
            pull(),
            Node::Loop { cond: LoopCond::UntilOsrEmpty, counter_init: None, body: cell, jmp_delay: 0 },
        ],
    }
}
