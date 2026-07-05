//! Spec search metric — the LOOSE, smooth tier of the spec-level oracle
//! (ticket 005).
//!
//! Where the cycle-exact metric (`cost.rs`) scores a candidate against one
//! pinned reference waveform, this scores it against the NEAREST spec-compliant
//! DME encoding of the expected bits, granting the freedoms clause 147 allows
//! but the reference trace forbids:
//!
//!   * **Polarity: free.** We work entirely in transition space — we extract
//!     the cycles where the capture changes level and never look at the level
//!     itself, so an inverted waveform scores identically (DME decodes by
//!     transitions).
//!   * **Phase: free up to `phi_max`.** The ideal grid can start at any first-
//!     boundary cycle `phi` in `[0, phi_max]`; the cost is the MINIMUM over a
//!     small set of `phi` hypotheses.
//!   * **T: pinned.** The bit-cell is a fixed `2*h` cycles (no rate fitting;
//!     ±100 ppm is sub-resolution here, per the ticket's reality-check).
//!
//! The metric reuses the banded-edit-distance shape of `cost::align_edges`
//! (missing ideal edge = 1.0, spurious candidate edge = `spurious_w`, a matched
//! pair = `delta/(window+1)`), so it plugs into the exact same curriculum
//! apparatus the cycle-exact oracle drives: `window = 0` + `spurious_w = 1`
//! approximates the certifier's strictness, wider windows densify the gradient.
//!
//! This is the assumed-GAMEABLE search tier. It is never the ground truth — a
//! champion is only ever trusted once the independent certifier (`certify.rs`)
//! passes it. In particular this module shares no code with the certifier: it is
//! a distance-to-a-charitable-interpretation, the certifier is a strict decoder.

/// Transition times (POSITIONS only) of one bit-channel: the cycles `t` (>= 1)
/// at which bit `bit` differs from the previous cycle.
///
/// Direction is deliberately DISCARDED and the implicit pre-start level is not
/// counted (scanning starts at `t = 1` from `wave[0]`, exactly as the certifier
/// does): DME polarity is free, so a candidate that starts high and one that
/// starts low are the same encoding and must yield the same transition list.
/// (This is the one substantive difference from `cost::channel_edges`, which
/// keeps direction and an implicit-0 pre-start because the cycle-exact metric
/// compares levels.)
fn transition_times(wave: &[u32], bit: u32, n: usize) -> Vec<usize> {
    let mut out = Vec::new();
    if n == 0 {
        return out;
    }
    let mut prev = (wave.first().copied().unwrap_or(0) >> bit) & 1;
    for i in 1..n {
        let v = (wave.get(i).copied().unwrap_or(0) >> bit) & 1;
        if v != prev {
            out.push(i);
            prev = v;
        }
    }
    out
}

/// Banded edit distance between two sorted transition-time sequences, matching
/// on POSITION only (no direction — see [`transition_times`]). A matched pair
/// within `window` cycles costs `Δ/(window+1)`; a deleted ideal edge (missing)
/// costs 1; an inserted candidate edge (spurious) costs `spurious_w`. This is
/// `cost::align_edges` with the direction gate removed and is otherwise the
/// same two-row DP (one alloc pair per call).
fn align_positions(ideal: &[usize], cand: &[usize], window: usize, spurious_w: f64) -> f64 {
    let (n, m) = (ideal.len(), cand.len());
    let denom = window as f64 + 1.0;
    let mut prev: Vec<f64> = (0..=m).map(|j| j as f64 * spurious_w).collect(); // 0 ideal vs j spurious
    let mut cur = vec![0.0f64; m + 1];
    for i in 1..=n {
        cur[0] = i as f64; // i ideal edges deleted (missing), cost 1 each
        let gc = ideal[i - 1];
        for j in 1..=m {
            let cc = cand[j - 1];
            let d = (gc as isize - cc as isize).unsigned_abs();
            let match_cost = if d <= window { d as f64 / denom } else { f64::INFINITY };
            cur[j] = (prev[j - 1] + match_cost)
                .min(prev[j] + 1.0) // delete ideal edge i (missing)
                .min(cur[j - 1] + spurious_w); // insert candidate edge j (spurious)
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m] // after the final swap the last row lives in `prev` (n=0: j*spurious_w)
}

/// **Spec search cost.** Distance from the capture `wave` (bit 0 — the single TX
/// pin) to the nearest spec-compliant DME encoding of `bits`, cell = `2*h`,
/// data transition at `+h`, phase `phi` free in `[0, phi_max]`, polarity free.
///
/// Cost = MIN over `phi` hypotheses of [`align_positions`] between the ideal
/// transition grid at `phi` (clock at `phi + k*2h` for every `k`; a data
/// transition at `phi + k*2h + h` iff `bits[k]`) and the candidate's transition
/// times. `window`/`spurious_w` are the gradient-shaping knobs (window 0 +
/// spurious 1 ≈ strict; see the module doc).
///
/// **Phi-hypothesis strategy (deterministic, cheap).** Rather than scan every
/// `phi` in `0..=phi_max` (a full DP each), we test only `phi = 0` plus each of
/// the first four candidate transition times, clamped to `phi_max`, deduped.
/// Rationale: a perfect match forces `phi` to equal the candidate's first
/// transition (the ideal grid's earliest edge is the bit-0 clock at exactly
/// `phi`), so that hypothesis is always in the set whenever a compliant reading
/// exists with `phi <= phi_max`; the extra early candidates catch the
/// off-by-a-runt-edge cases, and `phi = 0` anchors the silence/degenerate case.
/// So a compliant capture always scores 0 at its true phase, while cost stays a
/// deterministic function of the capture (no RNG, no wall-clock) at ~a handful
/// of DPs per eval.
///
/// Silence (no candidate transitions) saturates at the all-ideal-edges-missing
/// maximum (every ideal edge deleted), never at a small residual — the DP with
/// an empty candidate returns `n_ideal`.
///
/// **Frame scope (don't-care tail).** Only transitions in `[·, phi + n_bits·cell)`
/// — the `n_bits`-cell frame — are scored; anything at or after `frame_end` is
/// dropped, exactly as the cycle-exact curriculum's tight per-length window
/// truncates the capture at `len` cells. This is REQUIRED, not cosmetic: with
/// `dme_cfg`'s `pull_threshold = 5`, an L-bit curriculum sequence (`L < 5`) packs
/// into one 5-bit word, so a correct program emits 5 DME cells (L data cells plus
/// `5-L` trailing zero-bit cells). Those trailing clock toggles are outside the
/// L-cell ideal grid; charging them as spurious would put an immovable floor
/// under the frontier error and the ladder could never reach 0. Runaway toggling
/// past the data is instead caught by the strict-tail CERTIFIER on full-word
/// corpora at the gate — the loose search tier need not police it.
/// Transitions BEFORE `phi` stay in scope (genuine pre-frame junk is spurious).
pub fn spec_cost(wave: &[u32], bits: &[bool], h: usize, phi_max: usize, window: usize, spurious_w: f64) -> f64 {
    let n = wave.len();
    let cand = transition_times(wave, 0, n);
    let cell = 2 * h;
    // Ideal grid RELATIVE to phi (ascending): clock at k*cell, data at +h iff bit.
    let mut offsets: Vec<usize> = Vec::with_capacity(bits.len() * 2);
    for (k, &b) in bits.iter().enumerate() {
        offsets.push(k * cell);
        if b {
            offsets.push(k * cell + h);
        }
    }
    // Phi hypotheses: 0 plus the first few candidate transitions (clamped).
    let mut phis: Vec<usize> = Vec::with_capacity(5);
    phis.push(0);
    for &t in cand.iter().take(4) {
        phis.push(t.min(phi_max));
    }
    phis.sort_unstable();
    phis.dedup();

    let frame_len = bits.len() * cell; // frame_end = phi + frame_len
    let mut ideal: Vec<usize> = Vec::with_capacity(offsets.len());
    let mut in_frame: Vec<usize> = Vec::with_capacity(cand.len());
    let mut best = f64::INFINITY;
    for &phi in &phis {
        ideal.clear();
        ideal.extend(offsets.iter().map(|o| phi + o));
        // Drop the don't-care tail: transitions at/after the frame end.
        let frame_end = phi + frame_len;
        in_frame.clear();
        in_frame.extend(cand.iter().copied().take_while(|&t| t < frame_end));
        let c = align_positions(&ideal, &in_frame, window, spurious_w);
        if c < best {
            best = c;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certify::{certify_dme, DmeParams};

    const H: usize = 8;
    const PHI_MAX: usize = 32;

    /// Build a compliant DME capture (packed bit 0) directly from the grid
    /// rules — independent of both `spec_cost`'s decode and `dme_ref`. Mirrors
    /// `certify.rs`'s `synth` but packs into `u32` samples the way a real
    /// capture is packed.
    fn synth(bits: &[bool], phi: usize, initial: bool, capture: usize) -> Vec<u32> {
        let cell = 2 * H;
        let mut toggles = vec![false; capture];
        for (k, &b) in bits.iter().enumerate() {
            let clock = phi + k * cell;
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
                level as u32
            })
            .collect()
    }

    fn bits(v: &[u8]) -> Vec<bool> {
        v.iter().map(|&b| b != 0).collect()
    }

    #[test]
    fn compliant_is_zero_any_phase_any_polarity() {
        // phi >= 1: a boundary edge at cycle 0 is unobservable (no prior cycle to
        // differ from), so — like `certify.rs` — compliant frames start at >= 1.
        let b = bits(&[1, 0, 1, 1, 0, 0, 1]);
        for phi in [1usize, 2, 7, 16, 32] {
            for initial in [false, true] {
                let cap = phi + b.len() * 2 * H + H;
                let w = synth(&b, phi, initial, cap);
                let c = spec_cost(&w, &b, H, PHI_MAX, 0, 1.0);
                assert_eq!(c, 0.0, "phi={phi} initial={initial} should be exactly compliant");
            }
        }
    }

    #[test]
    fn each_mutant_class_costs_more() {
        let b = bits(&[1, 0, 1, 1, 0]);
        let phi = 5usize;
        let cap = phi + b.len() * 2 * H + H;
        let good = synth(&b, phi, false, cap);
        let base = spec_cost(&good, &b, H, PHI_MAX, 0, 1.0);
        assert_eq!(base, 0.0);

        // Wrong data bit: same waveform, different expectation.
        let mut wrong = b.clone();
        wrong[2] = !wrong[2];
        assert!(spec_cost(&good, &wrong, H, PHI_MAX, 0, 1.0) > base, "wrong-bit mutant");

        // Missing clock transition at bit 2: cancel that toggle.
        let mut miss = good.clone();
        let clock2 = phi + 2 * 2 * H;
        for v in &mut miss[clock2..] {
            *v ^= 1; // flipping the tail cancels the transition at clock2
        }
        assert!(spec_cost(&miss, &b, H, PHI_MAX, 0, 1.0) > base, "missing-clock mutant");

        // Off-grid (jittered) data edge: +1 cycle late is out of grid at window 0.
        let off = {
            let cell = 2 * H;
            let mut toggles = vec![false; cap];
            for (k, &bit) in b.iter().enumerate() {
                toggles[phi + k * cell] = true;
                if bit {
                    toggles[phi + k * cell + H + 1] = true; // +1 late
                }
            }
            let mut level = false;
            (0..cap).map(|t| { if toggles[t] { level = !level; } level as u32 }).collect::<Vec<u32>>()
        };
        assert!(spec_cost(&off, &b, H, PHI_MAX, 0, 1.0) > base, "off-grid mutant");

        // Silence: saturates at the all-edges-missing maximum.
        let silent = vec![0u32; cap];
        let n_ideal = b.len() + b.iter().filter(|&&x| x).count();
        assert_eq!(spec_cost(&silent, &b, H, PHI_MAX, 0, 1.0), n_ideal as f64, "silence == all missing");
    }

    #[test]
    fn gradient_repairing_toward_compliant_decreases() {
        // Expectation is all-ones; the candidate starts with its data
        // transitions missing and gains one more real data edge each step
        // (`keep` = how many leading cells carry their mid-bit transition). Cost
        // must fall monotonically toward 0 as the wave is repaired — the
        // gradient the curriculum climbs.
        let b = bits(&[1, 1, 1, 1, 1, 1]);
        let phi = 1usize; // >= 1: an edge at cycle 0 is unobservable (see other tests)
        let cap = phi + b.len() * 2 * H + H;
        let mut prev_cost = f64::INFINITY;
        for keep in 0..=b.len() {
            let partial = bits(&(0..b.len()).map(|k| (k < keep) as u8).collect::<Vec<_>>());
            let w = synth(&partial, phi, false, cap);
            let c = spec_cost(&w, &b, H, PHI_MAX, 2, 1.0);
            assert!(c <= prev_cost + 1e-9, "cost must not increase as the wave gains real bits (keep={keep}, c={c}, prev={prev_cost})");
            prev_cost = c;
        }
        assert_eq!(prev_cost, 0.0, "fully repaired wave is exactly compliant");
    }

    /// Certifier ↔ metric consistency: strict spec-cost == 0 must imply the
    /// independent certifier passes. Property-style over synthesized frames and
    /// a few mutant classes.
    #[test]
    fn strict_zero_implies_certifies() {
        let params = DmeParams { half_cell: H, phi_max: PHI_MAX, strict_tail: true };
        let cases: &[&[u8]] = &[
            &[1, 0, 1, 1, 0, 0, 1],
            &[0, 0, 0, 0],
            &[1, 1, 1, 1, 1],
            &[0, 1, 0, 1, 0, 1],
        ];
        for case in cases {
            let b = bits(case);
            for phi in [1usize, 3, 9, 32] {
                for initial in [false, true] {
                    let cap = phi + b.len() * 2 * H + 2 * H;
                    let w = synth(&b, phi, initial, cap);
                    let strict = spec_cost(&w, &b, H, PHI_MAX, 0, 1.0);
                    let levels: Vec<bool> = (0..cap).map(|t| w[t] & 1 == 1).collect();
                    let cert = certify_dme(&levels, &b, &params);
                    if strict == 0.0 {
                        assert!(cert.pass, "strict spec-cost 0 but certifier failed: {:?}", cert.violations);
                    }
                    // A wrong-bit mutant: strict must be > 0 AND certifier fails.
                    if !b.is_empty() {
                        let mut wrong = b.clone();
                        let last = wrong.len() - 1;
                        wrong[last] = !wrong[last];
                        let s2 = spec_cost(&w, &wrong, H, PHI_MAX, 0, 1.0);
                        let c2 = certify_dme(&levels, &wrong, &params);
                        if s2 == 0.0 {
                            assert!(c2.pass);
                        } else {
                            assert!(!c2.pass, "metric flagged a mutant the certifier accepted");
                        }
                    }
                }
            }
        }
    }
}
