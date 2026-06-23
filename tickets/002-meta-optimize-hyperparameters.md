---
status: open (re-run after 004; transfer trap persists — needs redesign)
priority: high
created: 2026-06-22
source: reference/mlx86 (*_hyperparameters.cpp)
---

# 002 — Meta-optimize search hyperparameters

## Re-run after 004 (2026-06-23) — trap persists at 3.75x inner; noise dominates

With evals ~3.4x cheaper, re-ran with inner_iters 40k→150k (3.75x, a much
larger fraction of the 800k deployment) and 3 inner seeds, AND added the
apples-to-apples test the original lacked: default vs tuned, each averaged over
3 seeds at full 800k scale.

```
inner 150k mini-cost:  default 24.3   tuned 20.0   (meta-anneal "wins" the trial)
@ full 800k (3 seeds): default mean 21.0 [23,18,22]
                       tuned   mean 22.3 [21,22,24]   => tuning does NOT transfer
```

Two findings:
1. **The transfer trap is structural, not just budget size.** Even at 150k inner
   the tuner picks `max_window=1` (a narrow ladder that sprints in a short
   budget) over the default's `max_window=8` — exactly wrong for 800k, where
   wide-window exploration crosses barriers. Bigger inner alone won't fix it;
   the ladder/window knobs are the overfitting lever.
2. **The objective is very noisy.** Full-scale spreads (18–24) are ~25% of the
   mean — comparable to the mini-trial "improvement" (24.3→20.0). The meta-anneal
   is partly chasing noise; `synthesize_flat_breed` is non-deterministic
   (cross-breeding board ↔ thread timing), so single-seed scores are unreliable.

### Redesign options (before another run)
- **Drop `max_window`/ladder shape from the meta-genome** — tune only the
  continuous knobs (w, t0, t_end, densify_w, poll_rate) and fix a good wide
  ladder. Removes the main overfit lever.
- **Make the engine deterministic** (seed the board exchange deterministically,
  or a deterministic island schedule) so the objective stops being noisy — the
  single biggest blocker to trustworthy meta-tuning.
- **Tune at deployment scale** once determinism + a JIT-class eval make 800k
  inner trials cheap enough (a follow-on to 004).

## Built + finding (2026-06-22)

## Built + finding (2026-06-22)

Implemented for the breeding engine: `BreedHp` (meta-genome: w, t0, t_end,
densify_w, post/poll rate, ladder max_window) + `meta_anneal` (SA over HPs,
multiplicative perturbation + clamps, fixed-seed mini-trial objective). `dme_meta_tune`.

**Works mechanically but the tuned HPs don't transfer.** A 40k-iter inner trial
drove mini-cost 24.5→21.5, but the winning knobs (narrow ladder `max_window=3`,
aggressive breeding `poll=18`) overfit short-budget *sprinting* and score **29 at
full 800k scale vs the default's 17** on the same seed. Classic meta-opt transfer
trap: mlx86 tuned and deployed at the same scale; our inner/deploy differ ~25×.
**Blocked on 004** — trustworthy meta-tuning needs inner trials near deployment
budget, which needs a faster eval. The machinery is correct and ready to re-run
once evals are cheap enough.

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
