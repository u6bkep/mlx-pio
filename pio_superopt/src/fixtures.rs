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
use crate::rng::Rng;
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
        autopull_pad: 0,
    }
}

/// Hand-written SPEC-SHAPED DME encoder — the compression seed (ticket 004
/// follow-on, 2026-07-05). `dme_ref` can never certify (14-cycle cell, data
/// at +6, +1 slip per word from the in-loop `pull` — see
/// `dme_ref_is_not_spec_shaped`), so compression needs a seed built for the
/// certifier's grid: 16-cycle cell, boundary edge at cell+2, data edge at
/// +8, both bit paths balanced to 16 cycles, AUTOPULL (threshold 5) so word
/// refill costs zero cycles. 8 instructions.
///
/// Cycle layout per cell (edge = `mov Pins,Y` visible):
/// ```text
///   c0 out X,1   c1 mov Y,!Y   c2 mov Pins,!Y[5]  (boundary edge @c2)
/// Drives are `mov Pins, Invert Y` (not plain Y): the emulator pad idles
/// HIGH, and the first drive must land on the OPPOSITE level or the opening
/// boundary edge is invisible and the certifier's phase lock lands on the
/// wrong grid. Polarity itself is spec-free.
///   c8 jmp X--,5
///   bit=1: c9 mov Y,!Y  c10 mov Pins,Y (mid edge @c2+8)  c11 mov Pins,Y[4]
///   bit=0: c9 jmp 7[1]                                   c11 mov Pins,Y[4]
///   wrap -> next c0 at c16
/// ```
pub fn dme_spec_ref() -> Program {
    use crate::ir::{Insn, JmpCond, MovDst, MovOp, MovSrc, Op, OutDst};
    let mut cfg = dme_cfg();
    cfg.shift.autopull = true;
    let mut p = Program::empty(cfg);
    let ins = |op, delay| Some(Insn { op, delay, sideset: None });
    p.slots[0] = ins(Op::Out { dst: OutDst::X, count: 1 }, 0);
    p.slots[1] = ins(Op::Mov { dst: MovDst::Y, op: MovOp::Invert, src: MovSrc::Y }, 0);
    p.slots[2] = ins(Op::Mov { dst: MovDst::Pins, op: MovOp::Invert, src: MovSrc::Y }, 5);
    p.slots[3] = ins(Op::Jmp { cond: JmpCond::XPostDec, target: 5 }, 0);
    p.slots[4] = ins(Op::Jmp { cond: JmpCond::Always, target: 7 }, 1);
    p.slots[5] = ins(Op::Mov { dst: MovDst::Y, op: MovOp::Invert, src: MovSrc::Y }, 0);
    p.slots[6] = ins(Op::Mov { dst: MovDst::Pins, op: MovOp::Invert, src: MovSrc::Y }, 0);
    p.slots[7] = ins(Op::Mov { dst: MovDst::Pins, op: MovOp::Invert, src: MovSrc::Y }, 4);
    p.wrap_bottom = 0;
    p.wrap_top = 7;
    p
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

// ===================== SPEC-ORACLE TESTBED (ticket 005) =====================
//
// Spec-oracle counterparts of the cycle-exact fixtures above: the decided v1
// cell shape (16 cycles, data at +8), the pooled multi-length SpecBits
// dataset, and certifier gating. Promoted from the gene_search test module
// so the runner binary drives the same testbed the experiments did.

/// Pack `n_bits` (bit i = the i-th emitted line bit, LSB-first) into 5-bit
/// DME words for the TX FIFO.
pub fn seq_words(bits: u64, n_bits: usize) -> Vec<u32> {
    let words = (n_bits + 4) / 5;
    (0..words.max(1)).map(|w| ((bits >> (5 * w)) & 0x1F) as u32).collect()
}

/// Half-cell for the spec testbed (`DME_H` analogue): nominal cell = `2*8`
/// cycles, mid-bit data transition at `+8`.
pub const SPEC_H: usize = 8;
/// Startup-phase bound: the first boundary edge may land anywhere in
/// `[1, SPEC_PHI_MAX]`. Generous, so the search keeps phase freedom.
pub const SPEC_PHI_MAX: usize = 32;

/// Build a pooled multi-length SPEC curriculum dataset: exhaustive length-L
/// bit sequences (vary the first `len` bits) while `2^len <= cap`, else `cap`
/// sampled — the SAME enumeration and RNG discipline as
/// `dme_multilength_dataset` (in the gene_search tests; seed
/// `0xDA7A_5EED ^ len`), so determinism is
/// identical. Rows carry the expected BITS directly (not a golden waveform):
/// `Target::SpecBits { bits, h = SPEC_H, phi_max = SPEC_PHI_MAX }`. The
/// RunSpec packs those bits LSB-first into 5-bit words exactly like
/// `seq_words` (config `dme_cfg`: pull_threshold 5, autopull off, clkdiv
/// pinned). Capture window = `phi_max` slack + `len * cell` frame + a
/// half-cell tail, so a compliant frame at any admissible phase fits and a
/// runaway loop's post-frame toggles show up as spurious edges.
pub fn dme_spec_multilength_dataset(lengths: &[usize], cap: u64) -> (Vec<(RunSpec, crate::search::Target)>, Vec<usize>) {
    use crate::search::Target;
    let cell = 2 * SPEC_H;
    let mut dataset: Vec<(RunSpec, Target)> = Vec::new();
    let mut groups: Vec<usize> = Vec::new();
    for (gi, &len) in lengths.iter().enumerate() {
        let win = (SPEC_PHI_MAX + len * cell + SPEC_H) as u64;
        let exhaustive = (1u64 << len) <= cap;
        let n = if exhaustive { 1u64 << len } else { cap };
        let mut drng = Rng::new(0xDA7A_5EED ^ len as u64);
        for s in 0..n {
            let bitval = if exhaustive { s } else { drng.below(1u32 << len) as u64 };
            let bits: Vec<bool> = (0..len).map(|i| (bitval >> i) & 1 == 1).collect();
            // autopull_pad keeps autopull-ON candidates' prefetch fed at word
            // boundaries; autopull-off programs never see the pad (per-config,
            // see `RunSpec::autopull_pad`).
            let sp = RunSpec { inputs: seq_words(bitval, len), cycles: win, autopull_pad: 2, ..dme_spec(0) };
            dataset.push((sp, Target::SpecBits { bits, h: SPEC_H, phi_max: SPEC_PHI_MAX }));
            groups.push(gi);
        }
    }
    (dataset, groups)
}

/// Certify `champ` against the spec at the testbed's 16-cycle cell on one
/// data corpus. Returns the certifier's violation count (0 = PASS). The
/// capture is sized `phi_max + n_bits*cell + cell` so `strict_tail` catches a
/// loop that keeps clocking after the data ends. Expected bits are the 5-bit
/// line codes LSB-first (clause 147.4.2 / `dme_cfg`'s pull_threshold 5).
pub fn spec_certify_corpus(champ: &Program, corpus: &[u32]) -> usize {
    use crate::certify::{certify_dme, channel_levels, DmeParams};
    let cell = 2 * SPEC_H;
    let mut expected: Vec<bool> = Vec::new();
    for &w in corpus {
        for i in 0..5 {
            expected.push((w >> i) & 1 == 1);
        }
    }
    let cycles = (SPEC_PHI_MAX + expected.len() * cell + cell) as u64;
    // NO autopull_pad here: pad symbols are extra data an autopull champion
    // dutifully transmits, guaranteeing strict-tail violations. The last real
    // word drains fine unpadded (refill only matters mid-stream), verified by
    // `dme_spec_ref_certifies`.
    let sp = RunSpec { inputs: corpus.to_vec(), ..dme_spec(cycles) };
    let wave = run(champ, &sp);
    let levels = channel_levels(&wave, 0, cycles as usize);
    let p = DmeParams { half_cell: SPEC_H, phi_max: SPEC_PHI_MAX, strict_tail: true };
    certify_dme(&levels, &expected, &p).violations.len()
}

/// `PASS` or `FAIL(n)` for a certifier violation count.
pub fn fmt_cert(n: usize) -> String {
    if n == 0 { "PASS".into() } else { format!("FAIL({n})") }
}

// ============ cycle-exact (wave) testbed + champion classification ==========

/// Bit-0 transition edges of a waveform: `(cycle, rising?)`.
pub fn dme_edges01(wave: &[u32]) -> Vec<(usize, bool)> {
    let mut out = Vec::new();
    let mut prev = 0u32;
    for (i, &s) in wave.iter().enumerate() {
        let v = s & 1;
        if v != prev {
            out.push((i, v == 1));
            prev = v;
        }
    }
    out
}

/// Build a pooled multi-length DME curriculum dataset: exhaustive sequences
/// (vary the first `len` bits) at each length in `lengths` while `2^len <=
/// cap`, else `cap` sampled sequences, each windowed to its length's boundary
/// grid +3. Returns `(dataset, group-tags)` where group `i` = `lengths[i]`.
///
/// `pad` appends two spare `0` symbols past the scored window to keep the FIFO
/// FED. This is REQUIRED for any autopull retry (autopull's prefetch fetches the
/// next word as the threshold-th bit shifts out and starves at a word boundary
/// without spare symbols), but it BREAKS the autopull-off conjunction crack that
/// the gated ladder relies on, so autopull-off callers must pass `false`.
/// A/B (same seed 0x5EED, 32 restarts, 4M iters): padded stalls at L=2 (0/13,
/// front_err=1.5) — can't even crack the conjunction; unpadded solves L=2..5 and
/// stalls at L=6 (front_err=63). Why padding poisons the autopull-off crack is
/// unexplained; keep the knob for future autopull experiments but default it off.
pub fn dme_multilength_dataset(lengths: &[usize], cap: u64, pad: bool) -> (Vec<(RunSpec, crate::search::Target)>, Vec<usize>) {
    let reff = dme_ref(DME_H).lower();
    let gsp = RunSpec { inputs: vec![0, 0, 0, 0], cycles: 320, ..dme_spec(0) };
    let grid: Vec<usize> = dme_edges01(&run(&reff, &gsp)).iter().map(|&(c, _)| c).collect();
    let mut dataset: Vec<(RunSpec, crate::search::Target)> = Vec::new();
    let mut groups: Vec<usize> = Vec::new();
    for (gi, &len) in lengths.iter().enumerate() {
        let win = (grid[len.min(grid.len() - 1)] + 3) as u64;
        let exhaustive = (1u64 << len) <= cap;
        let n = if exhaustive { 1u64 << len } else { cap };
        let mut drng = Rng::new(0xDA7A_5EED ^ len as u64);
        for s in 0..n {
            let bits = if exhaustive { s } else { drng.below(1u32 << len) as u64 };
            // Optionally keep the FIFO FED: append spare symbols past the `len`
            // cells the window scores. The window (grid[len]+3) captures only
            // the first `len` cells and the golden is generated on the SAME
            // padded input, so padding never changes the score — it only feeds
            // autopull's prefetch (which fetches the next word as the
            // threshold-th bit shifts out and would otherwise starve at a word
            // boundary). REQUIRED for autopull retries; breaks the autopull-off
            // conjunction crack, so autopull-off callers pass `pad = false`.
            let mut inputs = seq_words(bits, len);
            if pad {
                inputs.extend_from_slice(&[0, 0]);
            }
            let sp = RunSpec { inputs, cycles: win, ..dme_spec(0) };
            let g = run(&reff, &sp);
            let m = vec![u32::MAX; g.len()];
            dataset.push((sp, crate::search::Target::Wave { golden: g, mask: m }));
            groups.push(gi);
        }
    }
    (dataset, groups)
}

/// Classify a spec-ladder champion by INSPECTING its wrap (loop) region, not
/// its cost — the sweep needs to tell the data-independent exploit apart from
/// a real encoder even when both score near 0.
///
/// * `TOGGLER` — the wrap region neither CONSUMES data (`out`, or `mov`-from-
///   OSR) nor branches on the DATA VALUE. This is the half-cell toggler: it
///   emits transitions on a fixed schedule regardless of the bits.
/// * `CONJUNCTION` — the region both consumes data AND branches on the value
///   in X/Y (`jmp !X / x-- / !Y / y-- / x!=y`). That is read+branch+toggle:
///   the data-driven encoder.
/// * `OTHER` — anything mixed (consumes but never branches on the value, or
///   vice-versa).
///
/// Value-branch conditions exclude `Always` (unconditional loop) and
/// `NotOsrEmpty` (a REFILL test on OSR occupancy, not the bit value) — both of
/// which a pure toggler legitimately uses.
pub fn spec_classify(champ: &Program) -> &'static str {
    use crate::ir::{JmpCond, MovSrc, Op};
    let (b, t) = (champ.wrap_bottom as usize, (champ.wrap_top as usize).min(31));
    let mut consumes = false;
    let mut value_branch = false;
    for i in b..=t {
        let Some(ins) = &champ.slots[i] else { continue };
        match &ins.op {
            Op::Out { .. } => consumes = true,
            Op::Mov { src: MovSrc::Osr, .. } => consumes = true,
            Op::Jmp { cond, .. } => {
                if matches!(
                    cond,
                    JmpCond::NotX | JmpCond::XPostDec | JmpCond::NotY | JmpCond::YPostDec | JmpCond::XneY
                ) {
                    value_branch = true;
                }
            }
            _ => {}
        }
    }
    match (consumes, value_branch) {
        (false, false) => "TOGGLER",
        (true, true) => "CONJUNCTION",
        _ => "OTHER",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The compression seed must CERTIFY — train and held-out, zero
    /// violations — or compression has no certified starting point.
    #[test]
    fn dme_spec_ref_certifies() {
        let seed = dme_spec_ref();
        let ct = spec_certify_corpus(&seed, &dme_corpus());
        let cv = spec_certify_corpus(&seed, &dme_validation_corpus());
        assert_eq!((ct, cv), (0, 0), "seed must certify on train + held-out");
    }

    /// The seed must also score ZERO on the search dataset (all lengths) —
    /// the compression anneal's correctness term starts at 0.
    #[test]
    fn dme_spec_ref_scores_zero_all_lengths() {
        use crate::run::run;
        let seed = dme_spec_ref();
        let lengths: Vec<usize> = (2..=14).collect();
        let (dataset, _groups) = dme_spec_multilength_dataset(&lengths, 32);
        let total: f64 = dataset.iter().map(|(sp, t)| t.search_cost(&run(&seed, sp), 0, 1.0)).sum();
        assert_eq!(total, 0.0, "seed must have zero spec error at every length");
    }
}
