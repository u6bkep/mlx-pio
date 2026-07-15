# narrow-split w12 (L=3 wrap 1..2) — per-unit trace mining

**Date:** 2026-07-15 (evening). **Trace:** `/data/pio_optimization/runs/narrow-split-l3-w12.jsonl`
(1.09 GB, header rev 024bb2a, clean). Mining scripts ran out-of-tree
(scratchpad); this doc is the durable output.

## Run summary

REFUTED. 363.97B items (363.96B in units + ~1.1M phase-1), 318.06B
refuted, 657.1M memo hits, champions=0, cap_hit=false, 34,943s wall
(9.7h) on 28 threads. 1,383,452 units (frontier cycle 2, 1,002,852
pre-mirror), all settled, none abandoned. Ladder: **5 of 6 L=3
brackets proven**; only 0..2 remains (running as of this writing).

Prior context: the pre-tags/pre-relaxation engine reached 60.5%
settled in ~7h (~235B items live) on this bracket and was lost as
unresumable. Tonight's engine finished the whole space in 9.7h with
full unit-level durability.

## 1. Scheduling verdict: recursive unit split is NOT needed at this scale

- sum(per-unit ms) = 976,706s = **28.0 effective cores** over a
  34,943s wall on 28 threads — utilization is saturated; there is no
  straggler tail to recover.
- Largest single unit = 1,087s = **3.1% of wall**. The work-stealing
  pool over 1.38M units absorbs the skew completely.

The recursive-split driver would buy nothing here. It becomes relevant
only when max-unit approaches wall/threads (~21 min at this wall —
the top unit is already 18 min, so at L=4 unit sizes it WILL bind
unless the frontier is taken deeper (cycle 3+) or units get split
recursively. Cheaper first lever: deeper frontier cycle.)

## 2. Tail shape (per-unit)

| metric | ms | items |
|---|---|---|
| p50 | 58 | 30,557 |
| p90 | 133 | 74,565 |
| p99 | 168 | 99,445 |
| p99.9 | 148,896 | 49.1M |
| max | 1,087,235 | 344.2M |

Violently bimodal: between p99 and p99.9 the unit cost jumps ~900x.
Top 1% of units hold **92.0% of total CPU**; bottom 50% hold 1.6%.
~290K units (21%) are 1-item instant refutes (their seed dies on
first consult).

## 3. Headline: 71% of CPU is byte-identical repeat work

Grouping units by the stat fingerprint (items, refuted, cycles_run,
walk_cycles, tags_created) — exact equality on five independent
counters, so collisions are implausible above trivial sizes:

- **6,925 distinct fingerprints across 1,383,452 units.**
- Redundant copies (all but one per group) = **694,660s = 71.1% of
  total CPU**. Dedup-perfect execution would have been ~78 core-hours
  → ~2.8h wall on 28 threads, a 3.5x.
- Group sizes: max 213,544 (53ms trivial units), p50 = 4. The
  mid-tail is systematic: **seven groups of exactly 126 copies at
  ~122s/copy (~43.3M items each)** — those 882 units alone are ~11%
  of the run's CPU. The multiplicity 126 recurring exactly says the
  copies are a spelling orbit of one subtree (delay/field respellings
  of the seed prefix), not coincidence.
- Mirror twins are the innermost factor-2: the two top units
  (1,087s/1,055s) are a binding-free mirror pair with identical
  items/hits, as are most of the top 20 (id pairs like 12/691738).

Caveats: (1) the trace does not record seed words, so identifying
WHAT the 126-orbits respell requires re-deriving phase-1 seeds
(deterministic, cheap) and joining on unit index — queued follow-up.
(2) Identical stats prove the work was isomorphic, not that a sound
engine could have known it in advance; the sound exploitation is a
seed-level quotient (prove seed equivalence via the equiv()/rule
library, run one representative, derive the rest), i.e. exactly the
CL-lemma / 011+012 lattice direction, applied at unit granularity.
Champion-time mirroring is known-unsound (PULL-on-empty reads
physical X), so the mirror factor-2 needs the same care.

## 4. Fork-kind attribution (sum over all units, 363.96B forks)

| kind | share |
|---|---|
| Delay | 73.07% |
| BitCount | 11.83% |
| WaitIdx | 4.79% |
| MovSrc | 3.16% |
| SetData | 2.16% |
| JmpTarget | 1.77% |
| everything else (13 kinds) | 3.2% |

Delay forks dominate the monster bracket even after the 008 stage-2
junk-window collapse — the wall the collapse didn't reach is INSIDE
windows that fail its cleanliness conditions (latch activity /
external reads / fork edges). BitCount second at 11.8% is consistent
with the dead-demand census (shift chains re-read in-loop, ~0% dead —
not superposition fodder; needs 012-style outcome predicates).

## 5. Operational finding: header rev-pinning is over-strict

`git_rev_dirty()` runs at RESUME time against the repo HEAD, and the
resume gate compares the full header (git_rev + dirty included). Any
doc-only commit — or even a dirty tracked file (`-uno` ignores
untracked) — after launch makes resume refuse, even though the binary
is unchanged. Recovery if w02 needs a resume after this doc lands:

```
git stash && git checkout 024bb2a   # relaunch commit
<rerun the exact narrow-split command>   # resumes cleanly
```

Fix candidate (small): pin a build-time rev (compiled into the
binary) instead of runtime HEAD, or add `--assume-same-engine` that
downgrades the rev mismatch to a warning when the binary mtime
predates the trace.
