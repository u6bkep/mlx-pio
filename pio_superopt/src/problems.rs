//! Search problems: what the runner can point an engine at.
//!
//! A [`Problem`] bundles everything target-specific for the gated curriculum
//! ladder — the template/config, search space, the pooled multi-length
//! dataset, the default schedule, and the final gates — so the runner and
//! future engines stay problem-agnostic. Engine code never mentions DME;
//! problem code never mentions annealing.

use crate::fixtures::{
    dme_cfg, dme_corpus, dme_multilength_dataset, dme_spec_multilength_dataset, dme_validate,
    dme_validation_corpus, fmt_cert, spec_certify_corpus, SPEC_H, SPEC_PHI_MAX,
};
use crate::run::RunSpec;
use crate::search::{CurriculumHp, Genes, Space, Target};
use crate::{Program, SideCfg};

/// One gate verdict on a final champion: a label and a human-readable result.
/// `pass` drives the runner's exit reporting; the string carries the detail
/// (violation counts, Hamming distances).
pub struct Gate {
    pub label: &'static str,
    pub verdict: String,
    pub pass: bool,
}

/// A curriculum-ladder problem. `id` is the run-identity string pinned in the
/// trace header — changing what a problem means requires a new id, or resumes
/// would silently mix two different searches.
pub trait Problem {
    fn id(&self) -> &'static str;
    /// One-line banner describing the oracle/testbed shape.
    fn describe(&self) -> String;
    fn template(&self) -> Program;
    fn space(&self) -> Space;
    /// Schedule defaults for this problem (e.g. the spec oracle needs
    /// full-price spurious edges — see `dme_spec_densify_sweep`'s verdict).
    fn default_hp(&self) -> CurriculumHp;
    /// Pooled multi-length dataset + group tags for the gated ladder.
    fn dataset(&self, lengths: &[usize]) -> (Vec<(RunSpec, Target)>, Vec<usize>);
    /// Final gates on a champion (independent of the search metric).
    fn gates(&self, champ: &Program) -> Vec<Gate>;
}

/// Look up a problem by its header id.
pub fn by_id(id: &str) -> Option<Box<dyn Problem>> {
    match id {
        "dme-spec" => Some(Box::new(DmeSpec)),
        "dme-spec-ap" => Some(Box::new(DmeSpecAutopull)),
        "dme-spec-compress" => Some(Box::new(DmeSpecCompress)),
        "dme-wave" => Some(Box::new(DmeWave)),
        _ => None,
    }
}

/// [`DmeSpec`] with **autopull as a search gene** (ticket 005 step 4) — the
/// lever against the oracle-independent L=6 wall: the spec oracle doesn't
/// mandate `dme_ref`'s +1-cycle-per-word slip, so an autopull-on candidate can
/// drop the OSR-refill conjunction (`jmp NotOsrEmpty` + second `pull`) whose
/// basin the ladder can't finish wiring. The shared spec dataset/certifier
/// carry `autopull_pad = 2`, applied per-candidate (autopull-off candidates
/// see the same rows as `DmeSpec` byte-for-byte). Separate id so its traces
/// can never cross-resume with plain `dme-spec` runs.
pub struct DmeSpecAutopull;

impl Problem for DmeSpecAutopull {
    fn id(&self) -> &'static str {
        "dme-spec-ap"
    }
    fn describe(&self) -> String {
        format!("{} + autopull gene (pad 2)", DmeSpec.describe())
    }
    fn template(&self) -> Program {
        DmeSpec.template()
    }
    fn space(&self) -> Space {
        Space { genes: Genes { autopull: true, ..Genes::default() }, ..DmeSpec.space() }
    }
    fn default_hp(&self) -> CurriculumHp {
        DmeSpec.default_hp()
    }
    fn dataset(&self, lengths: &[usize]) -> (Vec<(RunSpec, Target)>, Vec<usize>) {
        DmeSpec.dataset(lengths)
    }
    fn gates(&self, champ: &Program) -> Vec<Gate> {
        DmeSpec.gates(champ)
    }
}

/// DME TX under the SPEC oracle (ticket 005): tolerance-band scoring against
/// the nominal 16-cycle cell (data at +8), gated by the independent certifier.
pub struct DmeSpec;

impl Problem for DmeSpec {
    fn id(&self) -> &'static str {
        "dme-spec"
    }
    fn describe(&self) -> String {
        format!("SPEC oracle: cell={} data@+{SPEC_H} phi_max={SPEC_PHI_MAX}", 2 * SPEC_H)
    }
    fn template(&self) -> Program {
        Program::empty(dme_cfg())
    }
    fn space(&self) -> Space {
        Space { slots: 10, side: SideCfg::NONE, search_wrap: true, genes: Genes::default() }
    }
    fn default_hp(&self) -> CurriculumHp {
        // densify_w = 1.0: full-price spurious edges kill the half-cell
        // toggler exploit (densify sweep, 2026-07-04).
        CurriculumHp { densify_w: 1.0, ..CurriculumHp::default() }
    }
    fn dataset(&self, lengths: &[usize]) -> (Vec<(RunSpec, Target)>, Vec<usize>) {
        dme_spec_multilength_dataset(lengths, 32)
    }
    fn gates(&self, champ: &Program) -> Vec<Gate> {
        let ct = spec_certify_corpus(champ, &dme_corpus());
        let cv = spec_certify_corpus(champ, &dme_validation_corpus());
        vec![
            Gate { label: "cert train", verdict: fmt_cert(ct), pass: ct == 0 },
            Gate { label: "cert held-out", verdict: fmt_cert(cv), pass: cv == 0 },
        ]
    }
}

/// COMPRESSION target (STOKE-style, 2026-07-05): shrink the hand-written
/// spec-shaped encoder `dme_spec_ref` (8 insns, autopull, certifies clean)
/// under the same spec dataset + certifier gates as [`DmeSpec`]. `template()`
/// is the SEED, not an empty program. Separate id: compression traces must
/// never cross-resume with synthesis runs.
pub struct DmeSpecCompress;

impl Problem for DmeSpecCompress {
    fn id(&self) -> &'static str {
        "dme-spec-compress"
    }
    fn describe(&self) -> String {
        format!("{} | COMPRESS seed=dme_spec_ref(8)", DmeSpec.describe())
    }
    fn template(&self) -> Program {
        crate::fixtures::dme_spec_ref()
    }
    fn space(&self) -> Space {
        // Autopull stays a gene: the seed uses it, but a smaller explicit-pull
        // program is a legitimate win — let the search choose.
        Space { genes: Genes { autopull: true, ..Genes::default() }, ..DmeSpec.space() }
    }
    fn default_hp(&self) -> CurriculumHp {
        DmeSpec.default_hp()
    }
    fn dataset(&self, lengths: &[usize]) -> (Vec<(RunSpec, Target)>, Vec<usize>) {
        DmeSpec.dataset(lengths)
    }
    fn gates(&self, champ: &Program) -> Vec<Gate> {
        DmeSpec.gates(champ)
    }
}

/// DME TX under the CYCLE-EXACT oracle: edge-cost against the reference
/// encoder's waveform (`dme_ref`, 14-cycle cell), gated by Hamming distance to
/// it. The retired primary testbed — kept runnable for spec-vs-wave A/Bs.
pub struct DmeWave;

impl Problem for DmeWave {
    fn id(&self) -> &'static str {
        "dme-wave"
    }
    fn describe(&self) -> String {
        let t = self.template();
        format!(
            "CYCLE-EXACT oracle vs dme_ref: autopull={} threshold={}",
            t.config.shift.autopull, t.config.shift.pull_threshold
        )
    }
    fn template(&self) -> Program {
        Program::empty(dme_cfg())
    }
    fn space(&self) -> Space {
        Space { slots: 10, side: SideCfg::NONE, search_wrap: true, genes: Genes::default() }
    }
    fn default_hp(&self) -> CurriculumHp {
        CurriculumHp::default()
    }
    fn dataset(&self, lengths: &[usize]) -> (Vec<(RunSpec, Target)>, Vec<usize>) {
        // pad = false: FIFO padding breaks the autopull-off conjunction crack
        // (see dme_multilength_dataset's doc).
        dme_multilength_dataset(lengths, 32, false)
    }
    fn gates(&self, champ: &Program) -> Vec<Gate> {
        let (vt, vh) = dme_validate(champ);
        vec![
            Gate { label: "hamming train", verdict: vt.to_string(), pass: vt == 0 },
            Gate { label: "hamming held-out", verdict: vh.to_string(), pass: vh == 0 },
        ]
    }
}
