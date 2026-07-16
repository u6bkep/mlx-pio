# 013 — junk-window cleanliness: known-constant reads don't dirty the window

**Status:** **MEASURED OUT 2026-07-16 — recommend CLOSE (see §Sizing
census). v1's collapsible class is 0.4–0.5% of walk bails on every
bracket; fork edges are 99.3–99.6%. Neither v1 nor v2 can reach the
Delay wall through this walk.**

## Sizing census (2026-07-16, post-E2 engine; commit 2277275)

Permanent `walk_bail_*` counters at junk_walk's nine bail sites, with
the v1 constancy predicate (stim-table/irq-schedule check over the
family shift envelope) applied at the external-read site.
Instrumentation-inert: L=2 0..1 and full 2..2 byte-identical items.

| bracket | walks refute | bail: fork edge | ext CONST (v1) | ext vary | latch |
|---|---:|---:|---:|---:|---:|
| L3 0..1 | 68.9% | 99.30% | 0.51% | 0.01% | 0.17% |
| L3 1..1 | 70.2% | 99.53% | 0.42% | 0.01% | 0.05% |
| L3 2..2 | 3.0% | **99.56%** | **0.42%** | 0.02% | 0.00% |

The walk does not die of dirty windows — it dies of UNDECIDED
NEIGHBORS (fetch-demand fork edges). Relaxing window cleanliness (v1)
or absorbing shifts at stalls (v2 — mechanism emulator-confirmed but
unreachable: the walk bails at a fork edge long before a stall can
absorb anything) addresses <0.5% of bails. The Delay fork wall (76%)
is generated at demand time against undecided slots; no junk-walk
amendment touches it.

Companion pair-race census (same session): the delay-only conflict
class is EXTINCT at the record layer (0 races found — stage 2's
delay-agnostic records already strip those conds); the surviving
cond-miss wall is cross-opcode with 80–82% full co-refutation, of
which only 26–34% are external-input-free — i.e. most co-refuting
record-sharing opportunities DO consult the environment and co-refute
anyway. That is a RECORD-SIDE (008 stage 3 / 012 outcome-conds)
signal, not a walk signal.
· **Source:** w12 seed-orbit analysis (docs/analysis/w12-seed-orbits.md);
companion to the 012 E1 amendment. This is an amendment to 008 stage 2
(junk-window collapse, 89f97c9), not new machinery.

## Evidence gate results (2026-07-16) — ran before building, per plan

Bin: `pio_superopt/src/bin/evidence013.rs`; full write-up:
docs/analysis/w02-mining-and-orbits.md §CORRECTION.

- **v2's mechanism is REAL:** at the emulator, a stalling WAIT
  downstream absorbs slot-0 delay shifts exactly (d=1..7, zero trace
  divergence); a visible write first exposes all 7. Shift-absorption
  is confirmed physics.
- **But v2's motivating class is DEAD:** the "delay-only" redundancy
  class (18.1% w12 / 28.8% w02) was a classifier artifact — under
  `.side_set 1 opt`, bits 12:11 of the "delay" region are SIDE-SET.
  Re-census with the bits separated: side-only spelling = the entire
  class (to the decimal); **true-delay-only = 0 groups in both
  brackets**. "CL7 d≡d+24" = side-1-vs-none (known-value write,
  latch-conditioned; ~30% of random completions diverge — unit-level
  identity comes from golden-trace conditioning).
- **Consequences for this ticket:** the split-layer evidence in §Why
  is void. v1's remaining justification is the IN-UNIT Delay fork
  wall (74% of fork mass — real, but its collapse magnitude through
  window-cleanliness relaxation is now unquantified). v2 keeps its
  proven mechanism but loses its named mass; treat as an in-unit
  lever candidate only.
- **The displaced lever:** a write-side E1 analog on the Side field
  (outcome partition against the concrete latch) now owns the named
  18–29% split-layer mass — candidate 012 stage E2, sketched in
  ticket 012. Ranking E2 vs this ticket's v1 is a user decision.

## Why

Delay forks are 73.1% of the 1..2 monster's fork mass — AFTER the
stage-2 collapse. The residue lives in windows stage 2 defines as
dirty: its cleanliness condition is "no latch VALUE changes, no
external reads, no fork edges", and WAIT / MOV-src-PINS / IN-PINS /
JMP-PIN are external reads CATEGORICALLY — even when the read
provably returns the same value at every shift. The w12 prologue is
the extreme case: satisfied-WAIT no-ops reading pins that idle
constant for the whole pre-DE window (DE rises at cycle 60), each
delay spelling paying a full continuation. The d ≡ d+24 orbit pairing
and the delay-only redundancy class (18.1% of w12's repeat CPU) are
this residue measured end-to-end.

## The rule change (v1)

Replace "no external reads" with: **no external reads whose observed
value varies across the shift envelope.** For a walk window with
shift range [t_min, t_max], each external read of signal s at cycle t
is clean iff s's value is constant over [t, t + (t_max − t_min)]:

- **Stim-driven signals** (stim mask, irq_sets): constancy is a table
  interval check against the KNOWN schedule — computable at walk time
  with no new state.
- **Self-driven latches**: already constant inside a clean window
  (the latch-quiet condition), so shifts read the same value —
  no new check needed beyond mapping which pins are self-driven.
- **Mixed/unknown provenance** for the read pin: dirty (bail), as
  today.

A satisfied WAIT under a constant read is met at every shift — the
time-shift theorem's premise (identical sequence, shifted) holds.
An UNSATISFIED wait stays dirty in v1 (it stalls; behavior is not a
pure shift — see v2 below).

## v2 candidate (separate, bigger claim — do not build in v1)

A stalling WAIT on a stim edge at absolute cycle T is
**shift-absorbing**: every delay spelling entering the window
converges to the same post-wait state at T. This is the suspected
mechanism behind the delay-only 18.1% class (unverified — see the
w12-seed-orbits open questions). It collapses delay respellings even
across windows with different shift totals, which the shift theorem
cannot. It needs its own soundness story (the wait's own delay field
and the post-T remainder still shift) and its own red-green rig.
Evidence gate before building: confirm the self-sync mechanism on one
delay-only group by stepping the emulator.

## Soundness notes

- OOB near horizon: unchanged — the existing per-shift-point bound
  (the L=1 exact-census hole's fix) applies verbatim; constancy of
  reads does not extend the horizon argument.
- Record conds: a walk certified under the relaxed rule additionally
  depends on the READ VALUES being what the schedule says — but the
  schedule is spec-constant (same for every item), so no new cond
  component is needed. The only new soundness input is the constancy
  computation itself; it must use the same stim/irq tables the
  emulator executes (single source of truth — reuse, don't mirror).
- E1 composition (012): after E1, the met-WAIT branch carries a
  value-set constraint on WaitIdx. The walk's constancy check must
  hold for EVERY pin in the allowed set (all-met ⇒ all constant over
  the envelope is exactly the met-now-at-every-shift condition).
  Cheap: ≤32 table checks.

## Gates

- Red-green pair: (a) a spec where a WAIT reads a stim pin that
  CHANGES inside the shift envelope — the collapse must NOT fire
  (canary red if it does); (b) same spec with the edge moved outside
  the envelope — collapse fires, verdict identical to the unshifted
  enumeration (exact census balance).
- Standing gates: fast suite, L1/L2 censuses, memo on/off,
  determinism locks, proven-bracket verdict identity.
- Stat gate: stage-2 walk-bail frequency must drop (that is the
  point); junk-collapse kill rate must not regress on the 150s 2..2
  mine.
- Wall-clock magnitude on idle box / b-srv0, after merge (user
  ruling: implement first, measure when the box frees up).

## Effect expectation (honest)

Delay ladders in prologue/idle windows collapse (the w12 named mass
says this is large FOR PROLOGUE-BEARING BRACKETS and for the
real-firmware targets with setup preambles); loop-body delay residue
in busy windows is untouched. v1 does not touch the delay-only class
unless the reads in those windows happen to be constant — v2 is the
lever for the self-sync shape.
