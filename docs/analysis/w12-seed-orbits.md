# w12 seed orbits — naming the 71% repeat work

**Date:** 2026-07-15 (evening). Companion to
`narrow-split-w12-unit-mining.md` (which found that 71% of the w12
run's CPU was byte-identical repeat subtrees). This analysis joins
those duplicate groups back to their phase-1 seeds and names the
respelling structure. Tools: `pio_superopt/src/bin/dump_seeds.rs`
(re-derives the exact seed list; verified 1,383,452 units / frontier
cycle 2 / 1,002,852 pre-mirror, matching the launch log) +
`tools/orbit_analysis.py`.

## Redundant-CPU census by seed-variation class

Weighted by redundant ms (all copies beyond one per group), 702,387s
total. Top classes — ALL dominated by slot 0, the prologue slot:

| share | groups | what varies across the group's seeds |
|---|---|---|
| 24.8% | 421 | s0: delay + op-hi |
| 18.1% | 1,222 | s0: delay only |
| 16.3% | 16 | s0: opcode + delay + op-hi + op-lo (the 126-orbits) |
| 14.4% | 161 | s0: delay + op-lo |
| 13.3% | 108 | s0: delay + op-hi + op-lo |
| 10.5% | 225 | mixed decided-structure (incl. mirror twins) |
| ~2.6% | rest | s0 op-field-only and s0×s1 combinations |

**~87% of the redundant CPU is slot-0 prologue respelling.** Slot-1/2
(loop body) cross-unit duplication is under 1% — the loop body's
respellings live INSIDE units, where the engine's memo/junk-window
machinery already works on them.

## Anatomy of the seven 126-orbits (11% of the run)

Each is a fully-decided single-word slot-0 seed class, 126 members,
~43.3M items each, byte-identical continuation counters. Decoding
orbit 1 (rep 0xb900) in full:

- **124 satisfied-condition WAITs** (CL3 class): `wait 0 gpio N` for
  every N that idles low, `wait 0 pin N` for the same set through
  in_base=16 (pin p ≡ gpio 16+p — the excluded indices match
  exactly), plus `wait 1` on the two high-idling views of gpio0. The
  exclusions are the config speaking: gpio0 (output/side pin, idles
  high), gpio8 (DE, externally driven and toggling — no wait on it is
  ever vacuous), and no IRQ waits at all (irq_sets stimulates flags
  at 23 cycles — never a vacuous wait).
- **2 self-moves** (CL6 class): `mov pins, pins`.
- Each member appears at exactly TWO delays, d and d+24: orbit 1 =
  {delay 1} ∪ {delay 25} (62+62+2 = 126). The seven orbits are the
  delay ladder d=1..7 (reps 0xb900..0xbf00, member sets shifted by
  +0x100 exactly).

So the orbit = "the prologue is a proven no-op; its spelling and its
cycle count MOD 24 don't matter." Three equivalence mechanisms
stacked:

1. **CL3** (satisfied-guard ops are no-ops) — needs the pin-idle map
   + external stimulus schedule as preconditions;
2. **CL1/CL6** (self-move writes back the read value);
3. **NEW — call it CL7 (phase congruence):** prologue cycle counts
   d and d+24 yield identical continuations. 24 is presumably the
   spec-trace period inside the pre-DE idle window (DE rises at
   cycle 60) — NOT yet verified against the expected trace; verify
   before building on it. The delay-only census class (18.1%) is the
   even stronger observed form: for many partially-decided prologues,
   EVERY surviving delay spelling continues identically — plausibly
   loops that re-synchronize via a stall/WAIT absorb the entry phase
   entirely (self-syncing continuation), but that mechanism is also
   unverified.

## Lever design implications

- The cross-unit redundancy is prologue-spelling redundancy that
  phase-1 CREATES by forking slot-0 fields at the frontier and then
  paying for each spelling's full continuation. A **seed-level
  quotient before phase 2** — canonicalize each seed's slot-0 word
  under proven no-op rules (CL3 with config preconditions, CL1/CL6),
  run one representative per class, replicate the verdict — attacks
  ~87% of the redundant mass. The proof obligations are exactly what
  the equiv() driver already does (CL1/CL2 proven; CL3 needs the
  supported_config extension — ANOTHER reason config coverage is
  queue priority for the proof engine, tx_a itself is blocked on it).
- CL7 (mod-period congruence) is spec-conditioned, not
  state-conditioned — a different proof shape (trace periodicity +
  horizon argument). Bigger payoff (folds the delay ladders), but
  don't lead with it; the state-conditioned rules are provable today.
- REFUTED-bracket caveat: replicating verdicts across a proven-
  equivalent seed class is sound for refutation AND for champions
  (champions of the representative rewrite to the class member by the
  same rule), but the champion rewrite path needs its own gate when a
  bracket ever produces champions.
- This also bounds what's left INSIDE units: the engine never sees
  cross-unit duplication, so the junk-window/memo levers were not
  missing anything here — the gap is purely in the phase-1
  decomposition layer.

## Open questions

- Verify 24 = expected-trace period in the idle window (dump
  spec.expected; cheap).
- The delay-only 18.1% class: confirm the self-syncing-continuation
  mechanism on one group (pick a group, disassemble two members, step
  the emulator).
- The mega-groups of trivial units (213K copies × 53ms, 6
  decided-structures) — worth naming only if the seed quotient
  design wants to cover the trivial mass too (it's 1.6% of CPU;
  probably not).
- Mirror twins (10.5% mixed-structure class) stay explicit — mirror
  verdict derivation remains unsound in general (PULL-on-empty reads
  physical X).
