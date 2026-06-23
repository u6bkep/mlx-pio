---
status: open
priority: medium
created: 2026-06-22
source: reference/mlx86 (*_hyperparameters.cpp)
---

# 002 — Meta-optimize search hyperparameters

## Problem this targets

We hand-sweep starting radii and hand-pick the portfolio schedules, having
concluded "no single schedule is best" (SCRATCH.md). The k-anneal radii, temps,
length cap, and per-move weights are all hand-tuned. With ~1µs/eval there's room
to tune them automatically.

## What mlx86 does

Each `*_hyperparameters.cpp` makes the **hyperparameters the genome** and scores
a candidate set by running a mini-trial of the actual solver on a test problem:

```c
parallel_tempering(meta_problem, (Hyperparameters*)data, ..., 10000, &results);
return results.score - results.trial_count / 1000.0;   // quality minus cost
```

Two tactical details worth stealing:
- **Multiplicative (scale-invariant) perturbation** — multiply a param by a
  uniform factor in ~[0.5, 2), so one mutation operator works across params
  spanning orders of magnitude:
  ```c
  double m = (fast_rand()/32767.0) + 1;        // [1,2)
  if (fast_rand()%2) m = 1/m;                   // -> [0.5,1)
  ```
  with hard post-scramble bound clamps.
- **Average over multiple test problems** (different seeds/targets) so the tuner
  doesn't overfit one instance. For us: tune across both UART targets +
  data_loop, not one.
- Deterministic fixed seed for the inner trial so candidates are compared fairly.

## The idea for us

An outer search over Params (k-schedule radii, temps, length cap, move weights)
scored by `inner_solve_quality − normalized_compute`, averaged across a small
target suite, inner trials fixed-seeded.

## Why sequence AFTER 001

This is a force-multiplier on a search we're still changing structurally (001).
Tuning a moving target wastes the tuning. Do it once the search shape settles.

## Open questions

- Which params to expose as the meta-genome (full set risks a huge space).
- Compute budget — meta-search is inner-trials-deep; needs a tight inner cap.
- Risk of overfitting the tuner to the current corpus (the recurring
  overfitting hazard applies one level up).

## Acceptance criteria

- A meta-tuner that proposes a Params set beating the current hand-tuned
  portfolio on the target suite at equal inner compute.
