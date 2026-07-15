# 013 — junk-window cleanliness: known-constant reads don't dirty the window

**Status:** design (small ticket-let; sequenced AFTER 012/E1 merges —
user ruling 2026-07-15) · **Source:** w12 seed-orbit analysis
(docs/analysis/w12-seed-orbits.md); companion to the 012 E1 amendment.
This is an amendment to 008 stage 2 (junk-window collapse, 89f97c9),
not new machinery.

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
