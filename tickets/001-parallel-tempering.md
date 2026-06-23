---
status: done (superseded by cross-breeding)
priority: high
created: 2026-06-22
source: reference/mlx86 (SolverParallelTempering)
---

# 001 — Parallel tempering with live solution exchange

## Cross-breeding A/B (2026-06-23) — does NOT help on DME (n=6 was noise)

Clean isolation of recombination's value: cooperative (crossover on) vs the SAME
32 islands run independently (crossover off, `poll_rate = 0`, take best), equal
compute, identical ladder/iters/seeds (`dme_breed_ab`).

An initial **n=6** run looked positive (cooperative best 17 vs 20), but **n=16
reversed it**:

```
cooperative: best 22  median 23  mean 23.3  breakthroughs(<=18) 0/16  [22-26]
independent: best 20  median 22  mean 21.8  breakthroughs(<=18) 0/16  [20-24]
=> independent is better on best, median, AND mean
```

The n=6 "best 17" was **noise**: seeds 0-5 are identical across the two runs, yet
seed 1 gave 17 in the first and 23 in the second — *same seed, same params,
different result*, because `synthesize_flat_breed` is non-deterministic
(board <-> thread timing). The noise was large enough to flip the conclusion.

**On DME, cross-breeding does not earn its keep** — independent restarts are
consistently ~1.5 lower. Likely cause: slot-range `crossover` is destructive
(breaks structure / JMP targets), and at DME complexity that disruption
outweighs the rare beneficial recombination.

Caveats / open: this does **not** refute the hypothesis that cooperation pays at
*higher* complexity (toward 10BASE-T1S; mlx86 needed it) — DME may be below that
threshold. And the old "cross-breeding cracked the DME wall" claim was never
isolated from densify+scale; this A/B suggests densify+scale did the work.

**Implications:**
1. Determinism (ticket 002) is now a *critical* blocker, not a nicety — it just
   flipped an A/B conclusion. No few-seed result is trustworthy without it.
2. The board is not justified at current complexity. Re-run this A/B on the
   harder target before assuming cooperation helps there.

## Verdict (2026-06-22)

**Within-stage island migration (copy a better peer) is a wash** — measured
neutral-to-negative on UART, flat-DME, and flat-DME-at-scale. The mechanism was
ported twice (gene `Migration`, flat `FlatMigration`) and never beat independent
chains at equal compute. **Superseded** by **cross-breeding**: the same island
board, but the exchange step *recombines* (slot-range `crossover`) instead of
copying — and that, with the densify objective + scale, cracked the DME mid-cell
wall (see SCRATCH "the cross-breeding pivot"). Copy-migration is retired; the
board lives on in `synthesize_flat_breed`. Kept here for the rationale trail.

## Problem this targets

Our stated remaining bottleneck (SCRATCH.md "Reliability"): **late-stage
barrier-crossing, not stage-1 diversity**. `synthesize_portfolio` runs
(schedule × seed) chains **fully independently** and keeps the strict-best —
**zero communication** between chains. Inside `synthesize_gene` the only
cross-talk is generational: the elite pool re-seeds the *next* stage (a barrier,
one-directional). Chains running concurrently never share mid-flight.

## What mlx86 does

`SolverParallelTempering`: N concurrent workers, each running its own SA. Every
`neighbor_poll_rate` (~500) iters, with `1/neighbor_poll_chance` (~1/10)
probability, a worker polls `num_neighbors` randomly-chosen peers and
**probabilistically copies a peer's whole solution** if the peer is better:

```cpp
if (((i % neighbor_poll_rate) == 0) && ((fast_rand() % neighbor_poll_chance) == 0)) {
    for (j < num_neighbors) {
        if (neighbor_syncs[j]->score > current_score) {
            float p = exp((neighbor_score - current_score) * scale / t);
            if (fast_rand() > 32767.0 * p) memcpy(data, neighbor[j].data, len);
        }
    }
}
```

Plus per-worker **elitism**: each worker tracks `best_score`/`best_data`
separately from its current state; the global best is reduced across workers at
the end. Topology is **random peers**, not a ring/grid (no comms bottleneck).

Note: mlx86's "temperatures" are actually the same cooling schedule per worker —
it's really island-model migration, not textbook replica-exchange. For us the
*real* PT idea (distinct temperature per chain: hot=explore, cold=exploit, with
exchange) is the more principled version and probably what we want.

## The idea for us

Let concurrently-running chains exchange whole genes mid-flight so a
hot/exploratory chain can hand a better basin to a cold/exploitative one — the
mechanism aimed directly at barrier-crossing. This is a natural evolution of the
portfolio: chains already run concurrently in `run_chains`; add lateral swaps.

## Open design questions (resolve before coding)

- **Replica-exchange vs. island migration.** True PT = a temperature ladder with
  Metropolis swap acceptance between adjacent rungs. Island model = same temp,
  occasional "copy the better one." Which fits our annealed-k + multistart
  structure? (Leaning replica-exchange on temperature, but k-annealing already
  occupies the "blurry→sharp" axis — interaction TBD.)
- **What is a "chain" here?** Currently `run_chains` spawns `params.restarts`
  threads per stage with a barrier between stages. PT wants long-lived chains
  with live exchange *across* what are now stage boundaries. Does PT replace the
  per-stage elitist GA loop, or run inside one stage?
- **Interaction with the k-anneal schedule.** Each chain currently walks the
  same k-schedule. Could instead spread chains over *both* temperature and k
  (2-D ladder). Risk of over-engineering.
- **Exchange acceptance rule** and migration rate/topology.
- **Cost metric for "better"** during exchange: smooth (k>0) cost vs strict.

## Acceptance criteria

- A PT/island variant that lets concurrent chains exchange solutions.
- Measured against `synthesize_portfolio` on the two UART targets and
  `data_loop`: solve rate and time-to-first-solve, same total chain budget.
- Keep it only if it beats independent portfolio at equal compute.

## Prototype + first A/B (2026-06-22)

Implemented async within-stage island migration: `Migration` blackboard in
`gene_search.rs`, `anneal_chain` posts current gene every `post_rate` and adopts
a strictly-better peer with prob `1 - exp(-intensity·gap/t)` (mlx86 rule, cools
into consensus). Opt-in via `Params.migrate: Option<MigrateCfg>` — `None` =
unchanged reproducible baseline. Wired through `run_chains`. A/B test
`migration_ab` (`cargo test --release -- --ignored migration_ab --nocapture`).

First run, equal compute (iters=5000, restarts=8, n=12 seeds, default knobs
post=20/poll=50/intensity=1.0):

| target | baseline solved | migration solved |
|---|---|---|
| UART k=4 | 0/12 (best corr 2) | 0/12 (best corr 3) |
| UART k=8 | 4/12 | 3/12 |

**Result: neutral-to-slightly-negative.** No evidence migration helps; weak
signal it hurts. Within run-to-run noise at n=12, but the direction is
consistent. Mechanism (hypothesis): late-stage consensus *collapses diversity*,
and on this problem diversity is exactly what works (SCRATCH: reliability comes
from independent portfolio parallelism, not per-chain cooperation). Within-stage
migration also overlaps the niche our per-stage elitism already fills — so it
adds consensus pressure without adding a new capability. Default knobs adopt
aggressively even early (gap≈w=64, t0=128 ⇒ ~0.4 adopt prob at start), so
diversity collapses before chains explore.

**Status: prototype landed, hypothesis not supported as-is.** Open options
before shelving: (a) gentler/late-only migration (intensity≪1, or gate polling
to the last fraction of iters); (b) migrate *best* not *current*, or cap how
many chains may converge; (c) conclude within-stage is the wrong altitude and
move migration to cross-*seed*/cross-schedule level, or pivot to true
replica-exchange (Option B). Decision pending discussion.

## Notes

mlx86 mechanisms that do NOT transfer (recorded so we don't cargo-cult): score
averaging/resampling (their objective is KVM-noisy; ours is deterministic),
watchdog/HLT-padding/RO-memory (real-silicon hazards we don't have), flat byte
genome (we deliberately moved to typed atomic-loop IR).
