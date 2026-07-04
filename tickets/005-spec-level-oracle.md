# 005 — Spec-level oracle (tolerance-band search metric + independent certifier)

Status: `open` (design phase — DECISIONS PENDING, do not start code until the
open questions at the bottom are settled with the user)
Priority: high
Opened: 2026-07-04

## Motivation

The current oracle is a **cycle-exact reference trace**: `dme_ref` runs in the
emulator at a pinned clkdiv, and a candidate is scored by (banded, edit-distance
smoothed) agreement with that one waveform (`cost.rs`). This is the single
largest source of human bias in the system — larger than macros, window size, or
the single-SM harness — because it defines *correct* as *"matches my hand-written
reference's timing, cycle for cycle."* Everything the project exists to find
(retimed loops, oversampled clocks, fractional-clkdiv tricks, phase-shifted
schedules that free up instructions) is **rejected by construction**, not
because the protocol forbids it but because the reference didn't happen to do it.

Concrete freedoms the DME/10BASE-T1S *spec* grants that the reference trace forbids:

- **Phase/latency**: when the first bit-cell starts is (within framing limits)
  free; the trace pins it to the reference's startup latency.
- **Polarity**: DME is transition-encoded; an inverted waveform decodes
  identically. The trace pins one polarity.
- **Clock realization**: any instruction schedule + clkdiv whose *effective*
  bit-cell period lands in the receiver's tolerance band is compliant — including
  fractional clkdiv with delta-sigma jitter (which the emulator models
  faithfully; see the divider-fidelity findings). The trace pins one integer
  schedule.
- **Edge placement**: receivers tolerate bounded jitter per edge (eye diagram);
  the trace demands exact positions.

Secondary motivation: the **config-gene negative result** (ticket-adjacent,
commit 0d72f1d — searchable autopull/threshold/clkdiv degraded the search
monotonically) was measured *under the cycle-exact oracle*, where only the
reference's exact clkdiv scores at all: the correct config is a needle with no
gradient toward it. Under a spec oracle, *every* clkdiv whose period lands in
the band scores, so config genes acquire a gradient. The negative result should
be re-tested after this lands.

## Architecture: two tiers + hardware, never conflated

The **Thompson hazard** governs everything here: in this project's own history,
every softened metric got gamed (densify weights bred edge-spam; masked Hamming
bred level-drivers). A tolerance-band metric is *maximally* gameable because it
actively searches for the interpretation most charitable to the candidate. So:

1. **SEARCH METRIC** (`spec_cost`, loose, smooth, assumed-gameable). Used only
   inside the anneal for gradient. May share code with nothing safety-critical.
2. **CERTIFIER** (`certify`, strict, boolean + violation report). An
   *independent decoder written from the protocol spec*, sharing **no code** with
   the search metric or with `dme_ref` — not the reference encoder inverted, a
   receiver. Gates promotion out of the search exactly like today's held-out
   Hamming gate (`dme_validate`), on held-out **and freshly sampled random**
   corpora. Every champion the system ever *reports* has passed the certifier;
   search-metric scores are never reported as results.
3. **HW validation** (later): run certified champions on a real RP2350 against a
   real 10BASE-T1S link partner / scope capture. Out of scope here, but the
   certifier's tolerance parameters should be written down in one place so they
   can be cross-checked against IEEE 802.3cg numbers when we get there.

## The spec, formalized (DME TX v1)

Real numbers from IEEE 802.3-2022 Clause 147 (extracts + citations in
`docs/802.3-clause147-dme-timing.md`): bit-cell **T2 = 80 ns ± 100 ppm**
(clock-to-clock transition), mid-cell data transition **T3 = 40 ns, band
38–42 ns**, transmitter jitter **< 5 ns symbol-to-symbol**, inter-transmission
gap **T1 ≥ 480 ns**, idle = one extra DME `0` then high-Z / diff-0V.

**Resolution reality check.** At the testbed's 8 cycles/cell one sys-cycle is
10 ns; at a real 150 MHz target, 6.67 ns. Every per-edge freedom above (±2 ns
on T3, 5 ns jitter) is *sub-cycle* at these resolutions, and ±100 ppm is
crystal-class rate accuracy, not slack. So a compliant PIO transmitter must
hit the exact nominal integer grid — the spec-level oracle's payoff on TX is
**not edge fuzz** but the *family* of compliant waveforms: phase/startup
latency, polarity, and the schedule/clkdiv realization of the grid. (It also
bounds the oversampling dream: fractional-clkdiv ±1-cycle wander on the bit
clock is only in-spec when one sys-cycle < 5 ns, i.e. sys clock > 200 MHz.)
The looser `eps` in the *search metric* below is therefore pure gradient
shaping — intermediate candidates get partial credit for near-miss edges —
while the certifier enforces spec-true placement at the target resolution.
The full tolerance bands become load-bearing in the **RX direction**, where a
receiver program must decode worst-case compliant input (±100 ppm, 5 ns
jitter, T3 anywhere in 38–42 ns) — there the oracle generates the adversarial
input family instead of accepting an output family.

A captured waveform `w` (per-cycle pin samples, sys-clock resolution) is
**compliant for data bits `b[0..n]`** iff there exist parameters within bands —

| param | meaning | band |
|---|---|---|
| `T` | bit-cell period, cycles | spec-true: nominal integer grid (80 ns ↔ 8 cells at DME_H=4); ±100 ppm is sub-resolution |
| `phi` | phase of first boundary edge | `[0, phi_max)` — bounded startup latency |
| `pol` | polarity | free: `{normal, inverted}` |
| `eps` | per-edge budget, cycles | certifier: spec-true (0 at ≤ 500 MHz); search metric: a gradient-shaping knob |

— such that the transition sequence of `w` decodes to `b`:

- a transition within `eps` of every cell boundary `phi + k·T`,
- a transition within `eps` of cell center `phi + (k+½)·T` iff `b[k] = 1`,
- **no other transitions** (spurious edges are violations, not don't-cares),
- **no runt pulses** below the minimum pulse width (`T/2 − 2·eps` floor),
- `T` consistent across the whole capture (single fit — no per-cell re-fitting,
  which would let a drifting waveform pass piecewise).

Polarity falls out for free (decode only looks at transitions). Note `eps` is in
*sys-clock cycles*: real 802.3cg jitter specs are in ps/ppm, far below our
simulation resolution, so `eps` is our sim-resolution stand-in for the
receiver's eye tolerance — one more reason the HW tier exists.

## Search metric: fitted-grid edge distance (recommendation)

Reuse the machinery that already works, aimed at a *fitted* target instead of a
pinned one:

1. Extract the candidate's transition list (`channel_edges` — exists).
2. **Fit `(T, phi, pol)`** to the candidate's own edges: coarse grid over the
   `T` band × phase, refine by least squares on boundary-classified edges.
   Cheap (edge lists are short), deterministic, no RNG.
3. Build the **ideal transition grid** for the expected bits under the fitted
   parameters, and score with the existing banded edit distance
   (`align_edges`-style: missing edge 1.0, spurious `spurious_w`, matched edges
   cost `Δ/(eps+1)` — i.e. *inside* the jitter budget deviation still trickles a
   tiny gradient toward center, outside it ramps to a miss).
4. Add the fit residual and out-of-band overflow as smooth penalty terms so the
   fitter can't paper over drift.

Why not the alternative (score = decoded-bit Hamming after a real decode): it is
closer to the certifier in spirit but *flat* — a candidate that almost forms an
edge scores identically to one with nothing there. The gated ladder lives on
dense gradients; we know flat conjunctive landscapes stall it. The fitted-grid
edge distance keeps today's gradient shape (the whole curriculum apparatus
ports unchanged) while granting the spec freedoms. Both can coexist behind the
trait; A/B them on the testbed.

## Known exploit surface (each needs a mitigation + a mutant test)

- **Degenerate fit**: an edge-dense garbage waveform gives the fitter many
  candidate grids → bound the `T`/`phi` bands hard; fit is over *boundary*
  structure, not best-of-all-interpretations; spurious edges always cost.
- **Silence**: no transitions → nothing to fit → cost must saturate at the
  all-edges-missing maximum, never at a small fit residual.
- **Piecewise drift**: freq slightly off, resynced per cell → single global `T`
  fit, drift accumulates into misses at the tail. (This is why fit is global.)
- **Runt/double pulses inside the jitter window**: two edges within `eps` of one
  grid point → at most one edge matches a grid point; extras are spurious.
- **Band-edge camping**: champions that only pass at the extreme of every band →
  certifier can re-check at a *tighter* band (margin report), and HW validation
  is the backstop.

**Certifier hardening**: mutant tests (bit-flip encoder, half-period encoder,
polarity-glitch, jittered-past-eps variants must all FAIL); property-based
random corpora; `dme_ref` (and its fractional-clkdiv variants, which the
cycle-exact oracle could never accept) must PASS with reported margin.

## Portability (what stays, what changes)

The search engine is already oracle-agnostic in the ways that matter: the gated
ladder, retries, cross-pollination/mined macros (commit 333747f), trace sink,
and curriculum grouping all operate on `dataset` rows + a cost function. The
change is the row type: today a row is `(RunSpec, golden_wave, mask)`; it becomes
`(RunSpec, SpecInstance)` where `SpecInstance` carries expected bits + bands —
behind a small trait so the cycle-exact oracle remains available as an instance
(it *is* the `eps=0, tol_T=0, pol` fixed degenerate case, which is also the
migration test: the new metric with zero-width bands must reproduce today's
results on the ladder).

```rust
trait Oracle: Sync {
    fn search_cost(&self, wave: &[u32]) -> f64;          // loose, smooth
    fn certify(&self, wave: &[u32]) -> CertReport;       // strict, independent
}
```

`weighted_multidata_cost` / `group_edge_errors` route through `search_cost`;
`dme_validate` is replaced by certifier gating. The mask machinery
(sub-waveform curricula) generalizes to per-cell weighting in the ideal grid.

## Migration / validation plan (order matters)

1. Certifier first (it is the ground truth everything else is measured against),
   with the mutant + property test suite.
2. `Oracle` trait + wrap the existing cycle-exact metric as an instance; ladder
   regression-run (zero-band spec metric ≡ today).
3. Fitted-grid search metric; A/B on the DME testbed: cycle-exact vs spec
   metric, same seeds/budget — does the spec metric (a) still climb the ladder,
   (b) admit new champion families (jittered clkdiv, inverted polarity)?
4. Re-test config genes (clkdiv/autopull) under the spec oracle.
5. Only then: 32-slot window, multi-SM, real rs10base-t1s targets (per the
   sequencing in SCRATCH.md).

## Open questions (USER INPUT NEEDED before code)

1. ~~Band numbers~~ **RESOLVED 2026-07-04**: real numbers extracted from IEEE
   802.3-2022 Table 147–2 + 147.5.4.3 (see `docs/802.3-clause147-dme-timing.md`
   and the resolution reality-check above). The residual knob is the *search
   metric's* `eps` (pure gradient shaping, certifier unaffected) — an
   engineering choice to tune, not a spec question.
2. ~~Is `T` itself searchable?~~ **RESOLVED 2026-07-04 (user): clkdiv stays OUT
   of the genes for v1.** The spec pins the rate to ±100 ppm; fractional-div
   wander only fits the 5 ns jitter budget above ~200 MHz sys clock, and while
   some RP2350s overclock well past that, "needs an overclocked part" doesn't
   make a good PIO program. v1 grants phase/polarity/schedule freedom at pinned
   clkdiv. Revisit only alongside a deliberate high-sys-clock target.
3. **Scope of v1**: DME TX bit-cells only (current testbed), or include
   10BASE-T1S framing realities (SYNC/beacon, idle line state, BEACON/COMMIT)
   where phase freedom interacts with the protocol state machine?
4. **Certifier independence standard**: separate module written from the spec
   text is the plan — is "same author (this project), different code" enough, or
   should the certifier eventually be cross-validated against an external
   implementation / captured real-PHY waveforms before we trust champions?
