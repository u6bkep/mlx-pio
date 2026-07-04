//! Independent DME certifier — the strict tier of the spec-level oracle
//! (ticket 005).
//!
//! This is a **receiver**, written from the IEEE 802.3-2022 Clause 147 rules
//! (see `docs/802.3-clause147-dme-timing.md`): it decodes a captured waveform
//! by its transitions and compares the *decoded bits* against the expected
//! data. It deliberately shares no code with the reference encoder
//! (`fixtures::dme_ref`) or the search metric (`cost.rs`) — those are the
//! things it exists to check (the Thompson hazard: a certifier derived from
//! the encoder inherits the encoder's mistakes; a certifier derived from the
//! search metric inherits its exploits).
//!
//! Clause 147.4.2, restated (bare bit-cell scope — no T1 gap / idle / SYNC
//! framing, per the v1 scope decision):
//!   * a clock transition at the start of EVERY bit-cell,
//!   * a data transition at the cell midpoint iff the bit is `1`,
//!   * no other transitions.
//!
//! Tolerances: at PIO resolutions every per-edge band in Table 147-2 (±2 ns
//! on T3, 5 ns symbol-to-symbol jitter, ±100 ppm rate) is SUB-CYCLE, so the
//! certifier demands the exact nominal integer grid. The freedoms it grants
//! over a pinned reference trace are the *compliant family*: startup phase
//! (any first-transition time ≤ `phi_max`) and polarity (decode never looks
//! at levels, only at transitions).

/// Spec parameters for certification, all in sys-clock cycles.
#[derive(Clone, Copy, Debug)]
pub struct DmeParams {
    /// Half a bit-cell (the testbed's `DME_H`): cell period is `2*half_cell`,
    /// the data transition sits `half_cell` after the clock transition.
    pub half_cell: usize,
    /// Latest cycle at which the first clock transition may occur (startup
    /// latency bound). Phase below this is free.
    pub phi_max: usize,
    /// Require silence after the last bit-cell (no transitions between frame
    /// end and capture end). Bare-cell strictness: a looping program that
    /// keeps toggling after the data ends is emitting out-of-spec noise a
    /// real receiver would see.
    pub strict_tail: bool,
}

/// Certification verdict plus everything needed to understand a failure.
#[derive(Clone, Debug)]
pub struct CertReport {
    pub pass: bool,
    /// Cycle of the first clock transition (frame phase), if any activity.
    pub phi: Option<usize>,
    /// The bits the receiver decoded (may be shorter than expected on a
    /// truncated/malformed frame).
    pub decoded: Vec<bool>,
    /// Human-readable spec violations, empty on pass.
    pub violations: Vec<String>,
}

/// Extract one pin's per-cycle level from packed capture samples (bit `bit`
/// of each `u32` sample, first `n` cycles) — adapter from
/// [`crate::run::run`]-style captures to the certifier's input.
pub fn channel_levels(wave: &[u32], bit: u32, n: usize) -> Vec<bool> {
    (0..n).map(|i| (wave.get(i).copied().unwrap_or(0) >> bit) & 1 == 1).collect()
}

/// Certify that `levels` is a spec-compliant DME encoding of `expected`.
///
/// Decode procedure (receiver's view):
/// 1. Transitions are the cycles `t` where `levels[t] != levels[t-1]`.
/// 2. The first transition is the frame phase `phi` (it must be the bit-0
///    clock transition — any earlier activity would itself be a transition,
///    and bare-cell scope allows none). `phi > phi_max` fails.
/// 3. The ideal grid is clock transitions at `phi + k*2H` and data positions
///    at `phi + k*2H + H`, `k` in `0..expected.len()`.
/// 4. Every clock position must hold a transition; each bit decodes as
///    "transition present at its data position"; every in-frame transition
///    not on the grid is a spurious-edge violation.
/// 5. The decoded bits must equal `expected`.
pub fn certify_dme(levels: &[bool], expected: &[bool], p: &DmeParams) -> CertReport {
    let mut violations: Vec<String> = Vec::new();
    let transitions: Vec<usize> =
        (1..levels.len()).filter(|&t| levels[t] != levels[t - 1]).collect();

    let n_bits = expected.len();
    let cell = 2 * p.half_cell;

    let Some(&phi) = transitions.first() else {
        return CertReport {
            pass: n_bits == 0,
            phi: None,
            decoded: Vec::new(),
            violations: if n_bits == 0 {
                Vec::new()
            } else {
                vec!["no line activity (no transitions in capture)".into()]
            },
        };
    };
    if phi > p.phi_max {
        violations.push(format!("first transition at cycle {phi} exceeds phi_max {}", p.phi_max));
    }
    let frame_end = phi + n_bits * cell;
    if levels.len() < frame_end {
        violations.push(format!(
            "capture ends at {} but the frame needs {frame_end} cycles (truncated)",
            levels.len()
        ));
    }

    // Classify every transition against the grid.
    use std::collections::HashSet;
    let tset: HashSet<usize> = transitions.iter().copied().collect();
    let mut decoded = Vec::with_capacity(n_bits);
    for k in 0..n_bits {
        let clock = phi + k * cell;
        let data = clock + p.half_cell;
        if clock < levels.len() && !tset.contains(&clock) {
            violations.push(format!("missing clock transition at bit {k} (cycle {clock})"));
        }
        if data < levels.len() {
            decoded.push(tset.contains(&data));
        }
    }
    for &t in &transitions {
        if t >= frame_end {
            if p.strict_tail {
                violations.push(format!("activity after frame end (transition at cycle {t})"));
            }
            continue;
        }
        let off = (t - phi) % cell;
        if off != 0 && off != p.half_cell {
            violations.push(format!("spurious transition at cycle {t} (off-grid by {off})"));
        }
    }

    // Decoded bits must match the expected data.
    for (k, (&d, &e)) in decoded.iter().zip(expected.iter()).enumerate() {
        if d != e {
            violations.push(format!("bit {k} decodes to {} expected {}", d as u8, e as u8));
        }
    }
    if decoded.len() < n_bits {
        violations.push(format!("only {} of {n_bits} bits decodable", decoded.len()));
    }

    CertReport { pass: violations.is_empty(), phi: Some(phi), decoded, violations }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: usize = 4;
    const P: DmeParams = DmeParams { half_cell: H, phi_max: 64, strict_tail: true };

    /// Build a compliant DME waveform directly from the clause-147 rules —
    /// independent of both the certifier's decode loop and `dme_ref`.
    fn synth(bits: &[bool], phi: usize, initial: bool, capture: usize) -> Vec<bool> {
        let mut toggles = vec![false; capture];
        for (k, &b) in bits.iter().enumerate() {
            let clock = phi + k * 2 * H;
            if clock < capture {
                toggles[clock] = true;
            }
            let data = clock + H;
            if b && data < capture {
                toggles[data] = true;
            }
        }
        let mut level = initial;
        (0..capture)
            .map(|t| {
                if toggles[t] {
                    level = !level;
                }
                level
            })
            .collect()
    }

    fn bits(v: &[u8]) -> Vec<bool> {
        v.iter().map(|&b| b != 0).collect()
    }

    #[test]
    fn compliant_frame_passes() {
        let b = bits(&[1, 0, 1, 1, 0, 0, 1]);
        let w = synth(&b, 5, false, 5 + b.len() * 2 * H + 3);
        let r = certify_dme(&w, &b, &P);
        assert!(r.pass, "violations: {:?}", r.violations);
        assert_eq!(r.phi, Some(5));
        assert_eq!(r.decoded, b);
    }

    #[test]
    fn polarity_and_phase_are_free() {
        let b = bits(&[0, 1, 1, 0, 1]);
        for phi in [0usize, 1, 13, 64] {
            for initial in [false, true] {
                let w = synth(&b, phi.max(1), initial, phi.max(1) + b.len() * 2 * H);
                let r = certify_dme(&w, &b, &P);
                assert!(r.pass, "phi={phi} initial={initial}: {:?}", r.violations);
            }
        }
    }

    #[test]
    fn phase_beyond_phi_max_fails() {
        let b = bits(&[1, 0]);
        let w = synth(&b, 65, false, 65 + b.len() * 2 * H);
        assert!(!certify_dme(&w, &b, &P).pass);
    }

    #[test]
    fn mutants_fail() {
        let b = bits(&[1, 0, 1, 1, 0]);
        let cap = 5 + b.len() * 2 * H + 2;
        let good = synth(&b, 5, false, cap);
        assert!(certify_dme(&good, &b, &P).pass);

        // Wrong data: same waveform, different expectation.
        let mut wrong = b.clone();
        wrong[2] = !wrong[2];
        assert!(!certify_dme(&good, &wrong, &P).pass, "bit-flip mutant passed");

        // Missing clock transition at bit 2: cancel the toggle by re-toggling
        // one cycle later (also an off-grid spurious edge).
        let mut w = good.clone();
        let clock2 = 5 + 2 * 2 * H;
        for t in clock2..clock2 + 1 {
            w[t] = !w[t];
        }
        assert!(!certify_dme(&w, &b, &P).pass, "missing-clock mutant passed");

        // Data edge displaced by one cycle: out of spec at this resolution.
        let shifted: Vec<bool> = {
            let mut toggles = vec![false; cap];
            for (k, &bit) in b.iter().enumerate() {
                toggles[5 + k * 2 * H] = true;
                if bit {
                    toggles[5 + k * 2 * H + H + 1] = true; // +1 cycle late
                }
            }
            let mut level = false;
            (0..cap)
                .map(|t| {
                    if toggles[t] {
                        level = !level;
                    }
                    level
                })
                .collect()
        };
        assert!(!certify_dme(&shifted, &b, &P).pass, "jittered-edge mutant passed");

        // Half-period encoder (everything twice as fast): wrong grid.
        let fast = {
            let mut toggles = vec![false; cap];
            for (k, &bit) in b.iter().enumerate() {
                toggles[5 + k * H] = true;
                if bit {
                    toggles[5 + k * H + H / 2] = true;
                }
            }
            let mut level = false;
            (0..cap)
                .map(|t| {
                    if toggles[t] {
                        level = !level;
                    }
                    level
                })
                .collect::<Vec<bool>>()
        };
        assert!(!certify_dme(&fast, &b, &P).pass, "half-period mutant passed");

        // Silence.
        assert!(!certify_dme(&vec![false; cap], &b, &P).pass, "silence passed");

        // Truncated capture.
        let short = &good[..good.len() - 3 * 2 * H];
        assert!(!certify_dme(short, &b, &P).pass, "truncated capture passed");

        // Tail noise after the frame.
        let mut noisy = good.clone();
        let last = noisy.len() - 1;
        noisy[last] = !noisy[last];
        assert!(!certify_dme(&noisy, &b, &P).pass, "tail-noise mutant passed");
    }

    /// FINDING (2026-07-04), pinned as a test: **the reference encoder itself
    /// is not spec-shaped.** `dme_ref(4)`'s actual waveform (see the probe
    /// dump below) is a 14-cycle bit-cell with the data transition at +6 —
    /// NOT the mid-cell the spec's T3 (40 of 80 ns nominal) demands — plus a
    /// +1-cycle slip at every 5-bit word boundary (the in-loop `pull`
    /// latency; as jitter, 1/14 of 80 ns ≈ 5.7 ns > the 5 ns budget). So even
    /// certified at its own rate under the most charitable uniform symmetric
    /// grid (half_cell = 7), it must FAIL. Every cycle-exact champion to date
    /// has been forced to reproduce these implementation artifacts — the
    /// concrete case for the spec-level oracle. (A compliant TX must center
    /// its data transitions and hide the refill latency — e.g. balanced
    /// delay slots or autopull.)
    #[test]
    fn dme_ref_is_not_spec_shaped() {
        use crate::fixtures::{dme_ref, dme_spec, DME_CYCLES, DME_H};
        use crate::run::run;
        let corpus = crate::fixtures::dme_corpus();
        let spec = dme_spec(DME_CYCLES);
        let golden = run(&dme_ref(DME_H).lower(), &spec);
        let levels = channel_levels(&golden, 0, spec.cycles as usize);
        // Expected bits: 5-bit line codes, LSB first (147.4.2 / dme_cfg
        // pull_threshold = 5).
        let mut expected = Vec::new();
        for &w in &corpus {
            for i in 0..5 {
                expected.push((w >> i) & 1 == 1);
            }
        }
        let p = DmeParams { half_cell: 7, phi_max: 64, strict_tail: true };
        let r = certify_dme(&levels, &expected, &p);
        assert!(!r.pass, "dme_ref unexpectedly certified — was it made compliant?");
        // The failure must be the documented shape: off-center data
        // transitions land off the symmetric grid as spurious edges.
        assert!(
            r.violations.iter().any(|v| v.contains("spurious")),
            "expected off-center-data violations, got: {:?}",
            r.violations
        );
    }
}

/// Diagnostic dumps of the reference encoder's real waveform — the evidence
/// behind `dme_ref_is_not_spec_shaped` (transition list, spacings, head
/// levels). Run with `--ignored dump_dme_ref --nocapture`.
#[cfg(test)]
mod probe {
    #[test]
    #[ignore = "diagnostic dump"]
    fn dump_dme_ref_edges() {
        use crate::fixtures::{dme_ref, dme_spec, DME_CYCLES, DME_H};
        use crate::run::run;
        let spec = dme_spec(DME_CYCLES);
        let golden = run(&dme_ref(DME_H).lower(), &spec);
        let levels = super::channel_levels(&golden, 0, spec.cycles as usize);
        let tr: Vec<usize> = (1..levels.len()).filter(|&t| levels[t] != levels[t - 1]).collect();
        let deltas: Vec<usize> = tr.windows(2).map(|w| w[1] - w[0]).collect();
        eprintln!("transitions: {tr:?}");
        eprintln!("deltas: {deltas:?}");
        eprintln!("head levels: {:?}", levels[..40].iter().map(|&b| b as u8).collect::<Vec<_>>());
        eprintln!("corpus: {:?}", crate::fixtures::dme_corpus());
    }
}
