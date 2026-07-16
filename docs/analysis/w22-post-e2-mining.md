# 2..2 post-E1+E2 re-run + mining — 2026-07-16

Purpose: re-confirm the 2..2 verdict under the new engine and produce
the fresh wall census the 013 recut discussion needs. Trace:
`/data/pio_optimization/runs/narrow-split-l3-w22-post-e2.jsonl`;
seeds joined with post-E2 `dump_seeds` (280,384 units, constraints
included — the frontier carries E2's `{no-side, side1}` class on
slot 0 from birth).

## Verdict + magnitude

**REFUTED** (again): 28.03B items, 1.22B memo hits, champions=0,
cap_hit=false, 1,969s wall (32m49s), 17.4 core-hours, 31.8/32
effective cores. vs pre-E1 (2026-07-15 wave 2: 132.0B / 2h55m):
**4.71x fewer items, 5.34x faster wall.** This is the E1+E2 idle-box
magnitude gate at monster-adjacent scale.

## Fork wall (013's question)

Delay **76.18%**, BitCount 10.31%, MovSrc 4.51%, WaitIdx 2.29%,
SetData 1.64%, Side 0.14%, rest <1.5%.

The in-unit Delay wall is untouched by E1/E2 (they attack wait-index
and side bits) and now stands even more alone. This is 013 v1's
target — but per the 2026-07-13 lesson, its collapsible fraction must
be sized by DelayPair race census, not fork attribution, before
building.

## Split-layer redundancy after E1+E2

Duplicate-CPU fraction **41.5%** (was 63.4–71.1% in the pre-E1
monsters) and its composition inverted:

- **side-only class: 0 groups.** E2 removed its own class completely
  (was 18–29%). delay-only: still 0 (never existed).
- What remains: 55.3% shape-varies + 44.7% multi-bit alphabets. The
  top groups are now FLAT mass — 76,426 copies × 82ms and
  34,955 × 67ms (~45K items each): cheap byte-identical units a seed
  quotient would fold, but each copy is two orders of magnitude
  cheaper than the old 126-orbit copies (~190s). The heavy end is 14
  near-twin units (ids 0–13, ~111M items / ~240s each, stats close
  but NOT byte-identical — the old exact-duplicate monsters are gone).
- Census artifact: 6 "identical seeds" groups are benign — the census
  script keys on `seed` only and ignores the `constraints` field that
  now also distinguishes units.

## Scheduling

Largest unit 247s = 12.6% of wall; top 1% of units hold 71% of CPU.
Skew grew in relative terms (wall shrank around the stubborn units).
Unchanged conclusion: deeper frontier cycle at L=4 before any
recursive-split machinery.

## Implications for the 013 ruling

1. v1's target (Delay 76% of forks) is real and now dominant, but the
   split-layer motivations are all dead — v1 stands or falls on
   IN-UNIT delay-ladder collapse. Size it with a DelayPair race
   census on this bracket before building (the stage-2 walk-bail
   stats + pair races, ~15 min instrumented run).
2. The seed-quotient lever's remaining prize halved and flattened:
   41.5% dup, dominated by very cheap copies — per-copy overhead of
   any quotient machinery matters much more than it would have
   pre-E2.
3. E2's constraint transport works end-to-end at the split layer
   (frontier seeds carry the side class; Side forks are 0.14%).
