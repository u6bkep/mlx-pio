//! CEGIS driver for DME TX synthesis: the solver proposes programs that
//! satisfy the spec on an accumulated set of example frames (∃), the real
//! emulator + certifier battery refutes candidates (∀), and every refutation
//! becomes a new example constraint.
//!
//! ## Trust model
//!
//! * A **Found** program was certified by [`crate::fixtures::spec_certify_corpus`]
//!   in the REAL emulator on the whole battery — it does not rest on the
//!   symbolic mirror at all.
//! * An **Unsat** verdict says: no program in the modeled subset (this
//!   length, this config, wrap = (0, len-1), [`super::legal_word`]) satisfies
//!   even the accumulated examples — which soundly implies none satisfies the
//!   full spec, PROVIDED the mirror ([`super::step`]) and the frame encoding
//!   ([`assert_frame`]) are faithful. Rerun the `differential_fuzz` tier
//!   before believing it.
//! * The loop cross-checks the two worlds every iteration: a candidate the
//!   battery rejected must also violate its new example constraint in the
//!   mirror, so the solver can never propose it again. If it does, mirror and
//!   certifier disagree — the loop ABORTS loudly instead of spinning.
//!
//! ## Frame encoding = the certifier in bitvectors
//!
//! [`assert_frame`] restates `certify_dme` (clause 147.4.2, bare-cell scope)
//! over a symbolic waveform with ONE free variable per example — the frame
//! phase φ: transitions exactly at `φ + k*cell` (clock) and `φ + k*cell + H`
//! iff bit k, nowhere else (this includes the strict quiet tail), `1 ≤ φ ≤
//! phi_max`. Polarity freedom is inherent (only transitions are constrained).
//! The capture window is sized exactly like `spec_certify_corpus`
//! (`phi_max + n*cell + cell`), so the biconditional per cycle is the whole
//! certifier: first transition IS φ (earlier cycles admit no grid position),
//! missing clocks / wrong data / spurious or tail edges all falsify it.

use std::io::Write as _;
use std::time::Instant;

use z3::ast::{Bool, BV};

use super::{bt, bvu, legal_word, supported_config, unroll_interned, SymProgram};
use crate::fixtures::{
    dme_corpus, dme_validation_corpus, spec_certify_corpus, SPEC_H, SPEC_PHI_MAX,
};
use crate::ir::Insn;
use crate::program::{Config, Program};

/// One CEGIS example: a TX word stream (5-bit line codes; expected bits are
/// its LSB-first concatenation, 5 per word — word granularity, exactly like
/// the certifier battery).
pub type Example = Vec<u32>;

/// Expected line bits of a word stream (5 LSB-first bits per word).
fn frame_bits(words: &[u32]) -> Vec<bool> {
    words.iter().flat_map(|&w| (0..5).map(move |i| (w >> i) & 1 == 1)).collect()
}

/// Capture window for `n` expected bits — MUST stay equal to
/// `spec_certify_corpus`'s sizing or Found/Unsat drift from the certifier.
fn frame_window(n_bits: usize) -> usize {
    let cell = 2 * SPEC_H;
    SPEC_PHI_MAX + n_bits * cell + cell
}

/// Assert onto `solver` that `prog` emits a spec-compliant DME frame for
/// `words`. `tag` uniquely names this example's phase variable.
fn assert_frame(solver: &z3::Solver, prog: &SymProgram, cfg: &Config, words: &[u32], tag: usize) {
    let bits = frame_bits(words);
    let n = bits.len();
    let cell = 2 * SPEC_H;
    let window = frame_window(n);
    let tr = unroll_interned(solver, prog, cfg, words, window, tag);

    let phi = BV::new_const(format!("phi{tag}"), 8);
    solver.assert(&phi.bvuge(&bvu(1, 8)));
    solver.assert(&phi.bvule(&bvu(SPEC_PHI_MAX as u64, 8)));

    // Per cycle t >= 1 (the certifier's transition domain): a transition iff
    // t sits on the φ-grid. For concrete t each grid line contributes at most
    // one admissible φ value, so the RHS is a small disjunction of φ == c.
    for t in 1..window {
        let mut grid: Vec<Bool> = Vec::new();
        for k in 0..n {
            let clock_off = k * cell;
            // Clock transition of bit k: t == φ + k*cell.
            if t >= clock_off + 1 && t - clock_off <= SPEC_PHI_MAX {
                grid.push(phi._eq(&bvu((t - clock_off) as u64, 8)));
            }
            // Data transition of bit k (present iff the bit is 1).
            let data_off = clock_off + SPEC_H;
            if bits[k] && t >= data_off + 1 && t - data_off <= SPEC_PHI_MAX {
                grid.push(phi._eq(&bvu((t - data_off) as u64, 8)));
            }
        }
        let on_grid = match grid.len() {
            0 => bt(false),
            1 => grid.pop().unwrap(),
            _ => grid.iter().skip(1).fold(grid[0].clone(), |acc, g| acc | g),
        };
        let trans = (&tr.levels[t]).xor(&tr.levels[t - 1]);
        solver.assert(&trans.iff(&on_grid));
    }
}

/// The ∀-oracle: certify `p` in the real emulator against a deterministic
/// battery, cheapest frames first. Returns the first failing word stream, or
/// `None` if everything passes (Found). Battery: all 32 single words, all
/// 1024 word pairs, the training + held-out corpora, 16 seeded random
/// 8-word streams.
pub fn verify_battery(p: &Program) -> Option<Example> {
    for w in 0..32u32 {
        if spec_certify_corpus(p, &[w]) != 0 {
            return Some(vec![w]);
        }
    }
    for a in 0..32u32 {
        for b in 0..32u32 {
            if spec_certify_corpus(p, &[a, b]) != 0 {
                return Some(vec![a, b]);
            }
        }
    }
    for corpus in [dme_corpus(), dme_validation_corpus()] {
        if spec_certify_corpus(p, &corpus) != 0 {
            return Some(corpus);
        }
    }
    let mut rng = crate::rng::Rng::new(0xBA77_E12E);
    for _ in 0..16 {
        let stream: Vec<u32> = (0..8).map(|_| rng.below(32)).collect();
        if spec_certify_corpus(p, &stream) != 0 {
            return Some(stream);
        }
    }
    None
}

/// How a CEGIS run ended.
pub enum Outcome {
    /// Battery-certified in the real emulator (mirror-independent).
    Found(Program),
    /// No program in the modeled subset satisfies the accumulated examples —
    /// spec-level impossibility at this length/config, MODULO mirror fidelity.
    Unsat,
    /// Iteration cap hit (opts.max_iters) before a verdict.
    MaxIters,
}

pub struct CegisOpts {
    /// 0 = unlimited.
    pub max_iters: usize,
    /// JSONL trace path (one row per iteration); `None` = no trace file.
    pub trace: Option<std::path::PathBuf>,
    /// Stderr heartbeat (the long-run observability directive).
    pub verbose: bool,
}

impl Default for CegisOpts {
    fn default() -> Self {
        CegisOpts { max_iters: 0, trace: None, verbose: true }
    }
}

pub struct CegisReport {
    pub outcome: Outcome,
    pub iters: usize,
    pub examples: Vec<Example>,
}

/// Run CEGIS for a DME TX program over `template` slots (`None` = free,
/// `Some(word)` = pinned; wrap = (0, len-1)) under `cfg`.
pub fn cegis_dme(cfg: &Config, template: &[Option<u16>], opts: &CegisOpts) -> CegisReport {
    supported_config(cfg).expect("unsupported config for the smt model");
    let len = template.len();
    let sym = SymProgram::with_holes(template, &cfg.side);
    // Everything here is quantifier-free bit-vectors; pin the QF_BV strategy
    // (pre-simplify → bit-blast → CDCL SAT) instead of letting z3 guess, and
    // let its parallel cube-and-conquer use the idle cores. Measured on the
    // len-4 full-free probe (2026-07-06): the default single-core solver
    // spent 16 min on iteration 2 and 6+ h on iteration 3.
    z3::set_global_param("parallel.enable", "true");
    let solver = z3::Solver::new_for_logic("QF_BV").unwrap_or_else(z3::Solver::new);
    for (i, slot) in template.iter().enumerate() {
        if slot.is_none() {
            solver.assert(&legal_word(&sym.words[i], len as u8));
        }
    }
    // Sound pruning: a program with NO instruction that can drive the pin
    // (level or direction, incl. an asserted side-set) cannot encode
    // anything — the same generation-time reject enumerate.rs uses. Concrete
    // template slots participate as constants, so this never excludes a
    // valid completion.
    let any_pin_write = (0..len)
        .map(|i| writes_pin_word(&sym.words[i], cfg))
        .reduce(|a, b| a | b)
        .expect("len >= 1");
    solver.assert(&any_pin_write);

    let mut trace_file = opts.trace.as_ref().map(|p| {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).expect("create trace dir");
        }
        std::fs::OpenOptions::new().create(true).append(true).open(p).expect("open trace")
    });
    let mut emit = |row: serde_json::Value| {
        if let Some(f) = trace_file.as_mut() {
            writeln!(f, "{row}").and_then(|_| f.flush()).expect("write trace");
        }
    };

    // Seed with the cheapest example; every further example is a real
    // counterexample from the battery.
    let mut examples: Vec<Example> = vec![vec![0]];
    assert_frame(&solver, &sym, cfg, &examples[0], 0);

    let mut prev_words: Option<Vec<u16>> = None;
    let mut iters = 0usize;
    loop {
        iters += 1;
        if opts.max_iters > 0 && iters > opts.max_iters {
            return CegisReport { outcome: Outcome::MaxIters, iters: iters - 1, examples };
        }
        if opts.verbose {
            eprintln!(
                "[cegis] iter {iters}: solving (len {len}, {} examples, {} frame bits)…",
                examples.len(),
                examples.iter().map(|e| e.len() * 5).sum::<usize>(),
            );
        }
        let t0 = Instant::now();
        let res = solver.check();
        let solve_ms = t0.elapsed().as_millis();
        match res {
            z3::SatResult::Unsat => {
                if opts.verbose {
                    eprintln!("[cegis] UNSAT after {iters} iters ({solve_ms} ms final solve) — no len-{len} program in the modeled subset");
                }
                emit(serde_json::json!({
                    "iter": iters, "solve_ms": solve_ms, "result": "unsat",
                    "examples": examples.len(),
                }));
                return CegisReport { outcome: Outcome::Unsat, iters, examples };
            }
            z3::SatResult::Unknown => {
                panic!("solver returned Unknown (reason: {:?})", solver.get_reason_unknown());
            }
            z3::SatResult::Sat => {
                let model = solver.get_model().expect("sat without model");
                let words: Vec<u16> = (0..len)
                    .map(|i| {
                        model.eval(&sym.words[i], true).unwrap().as_u64().unwrap() as u16
                    })
                    .collect();
                let candidate = program_from_words(&words, cfg, len);
                if opts.verbose {
                    eprintln!(
                        "[cegis] iter {iters}: candidate ({solve_ms} ms) {}",
                        candidate.brief()
                    );
                }
                match verify_battery(&candidate) {
                    None => {
                        if opts.verbose {
                            eprintln!("[cegis] CERTIFIED after {iters} iters: {}", candidate.brief());
                        }
                        emit(serde_json::json!({
                            "iter": iters, "solve_ms": solve_ms, "result": "found",
                            "words": words, "program": candidate.brief(),
                            "examples": examples.len(),
                        }));
                        return CegisReport { outcome: Outcome::Found(candidate), iters, examples };
                    }
                    Some(cex) => {
                        // Divergence guard: the new constraint must rule this
                        // candidate out in the mirror too. If the solver hands
                        // back the identical words, mirror and certifier
                        // disagree — never spin on that.
                        if prev_words.as_deref() == Some(&words[..]) {
                            panic!(
                                "CEGIS divergence: candidate {words:04x?} was refuted by the \
                                 certifier on {cex:?} but still satisfies the mirror encoding. \
                                 Mirror or frame encoding is unfaithful — run differential_fuzz.",
                            );
                        }
                        if opts.verbose {
                            eprintln!(
                                "[cegis] iter {iters}: battery FAILED on {cex:?} — adding example {}",
                                examples.len()
                            );
                        }
                        emit(serde_json::json!({
                            "iter": iters, "solve_ms": solve_ms, "result": "counterexample",
                            "words": words, "program": candidate.brief(),
                            "cex": cex, "examples": examples.len() + 1,
                        }));
                        assert_frame(&solver, &sym, cfg, &cex, examples.len());
                        examples.push(cex);
                        prev_words = Some(words);
                    }
                }
            }
        }
    }
}

/// Can this instruction word drive the observed pin — OUT/MOV/SET to
/// Pins/PinDirs, or an asserted side-set (when the config has side-set
/// value pins)? Mirrors `enumerate::writes_pin`, extended with side-set.
fn writes_pin_word(word: &BV, cfg: &Config) -> Bool {
    let opcode = word.extract(15, 13);
    let dst = word.extract(7, 5);
    let out_pins = opcode._eq(&bvu(3, 3)) & (dst._eq(&bvu(0, 3)) | dst._eq(&bvu(4, 3)));
    let mov_pins = opcode._eq(&bvu(5, 3)) & (dst._eq(&bvu(0, 3)) | dst._eq(&bvu(3, 3)));
    let set_pins = opcode._eq(&bvu(7, 3)) & (dst._eq(&bvu(0, 3)) | dst._eq(&bvu(4, 3)));
    let mut writes = out_pins | mov_pins | set_pins;
    if cfg.side.count.saturating_sub(cfg.side.en as u8) > 0 {
        // With SIDE_EN the field's bit 4 (word bit 12) is the per-insn
        // enable; without it every instruction asserts side-set.
        let side_asserted = if cfg.side.en {
            word.extract(12, 12)._eq(&bvu(1, 1))
        } else {
            bt(true)
        };
        writes = writes | side_asserted;
    }
    writes
}

/// Decode solver words into a runnable [`Program`] (wrap = (0, len-1)).
/// `legal_word` keeps every free word inside the decodable subset.
fn program_from_words(words: &[u16], cfg: &Config, len: usize) -> Program {
    let mut p = Program::empty(*cfg);
    for (i, &w) in words.iter().enumerate() {
        let insn: Insn = crate::decode::decode_insn(w, &cfg.side)
            .unwrap_or_else(|e| panic!("model word {w:#06x} does not decode: {e:?}"));
        p.slots[i] = Some(insn);
    }
    p.wrap_bottom = 0;
    p.wrap_top = (len - 1) as u8;
    p.validate().expect("decoded candidate fails Program::validate");
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{dme_ref, dme_spec_ref, DME_H};

    /// Frame-encoding ⇔ certifier on CONCRETE programs: with all words fixed,
    /// SAT means "some phase certifies this waveform" — exactly
    /// `spec_certify_corpus == 0`.
    fn frame_sat(p: &Program, words: &[u32]) -> bool {
        let sym = SymProgram::from_program(p);
        let solver = z3::Solver::new();
        assert_frame(&solver, &sym, &p.config, words, 0);
        match solver.check() {
            z3::SatResult::Sat => true,
            z3::SatResult::Unsat => false,
            z3::SatResult::Unknown => panic!("unknown"),
        }
    }

    /// Positive + negative agreement with the real certifier: the
    /// spec-shaped seed passes exactly where the certifier passes; the
    /// non-spec-shaped reference encoder and a mutated seed fail.
    #[test]
    fn frame_encoding_matches_certifier() {
        let seed = dme_spec_ref();
        for corpus in [vec![0x00u32], vec![0x1F], vec![0x15, 0x09], dme_corpus()] {
            assert_eq!(
                spec_certify_corpus(&seed, &corpus),
                0,
                "precondition: seed certifies on {corpus:?}"
            );
            assert!(frame_sat(&seed, &corpus), "encoding rejects certified seed on {corpus:?}");
        }

        // dme_ref: 14-cycle cell, off-center data — certifier-rejected.
        let reff = dme_ref(DME_H).lower();
        let corpus = vec![0x1Eu32];
        assert_ne!(spec_certify_corpus(&reff, &corpus), 0, "precondition: dme_ref fails cert");
        assert!(!frame_sat(&reff, &corpus), "encoding accepts the non-spec dme_ref");

        // Mutated seed (drop one delay): timing grid breaks, both reject.
        let mut broken = dme_spec_ref();
        for s in broken.slots.iter_mut().flatten() {
            if s.delay > 0 {
                s.delay -= 1;
                break;
            }
        }
        assert_ne!(spec_certify_corpus(&broken, &corpus), 0);
        assert!(!frame_sat(&broken, &corpus), "encoding accepts a broken seed");
    }

    /// End-to-end CEGIS: pin 7 of the seed's 8 words, free ONE slot, and let
    /// the loop rediscover a word that battery-certifies. Exercises solve →
    /// decode → emulator battery → counterexample growth → certified exit.
    #[test]
    fn cegis_refills_one_seed_slot() {
        let seed = dme_spec_ref();
        let words = seed.assemble();
        // Free slot 1 (`mov Y, !Y` — the boundary-edge toggle).
        let template: Vec<Option<u16>> =
            (0..8).map(|i| if i == 1 { None } else { Some(words[i]) }).collect();
        let opts = CegisOpts { max_iters: 40, trace: None, verbose: true };
        let report = cegis_dme(&seed.config, &template, &opts);
        match report.outcome {
            Outcome::Found(p) => {
                assert_eq!(verify_battery(&p), None, "Found must be battery-certified");
                eprintln!(
                    "[test] refilled in {} iters ({} examples): {}",
                    report.iters,
                    report.examples.len(),
                    p.brief()
                );
            }
            Outcome::Unsat => panic!("hole is refillable (the seed word itself) — UNSAT is wrong"),
            Outcome::MaxIters => panic!("did not converge in 40 iters"),
        }
    }
}
