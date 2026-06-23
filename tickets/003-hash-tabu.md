---
status: open
priority: low
created: 2026-06-22
source: reference/mlx86 (SolverTabuSearch)
---

# 003 — Hash-based tabu list

## Problem this targets

Pre-IR we saw the search "keep re-deranging the loop" — revisiting the same
structures. The first-class IR mostly fixed this, but cheap revisit-avoidance is
useful insurance, especially in the polish/late stages where the neighborhood is
small and cycling is easy.

## What mlx86 does

`SolverTabuSearch`: generate ~50 neighbors, hash each genome with murmur3, skip
any whose hash is in a small **circular tabu buffer** (FIFO, len ~10), move to
the best non-tabu neighbor, push its hash. O(1) membership, stores hashes not
solutions. No aspiration criterion, no diversification — deliberately minimal.

## The idea for us

A small rolling hash-set of recently-visited gene structures, consulted in the
greedy polish sweep (and optionally as a soft penalty in annealing) to avoid
re-evaluating/re-entering just-left states. Hash over the canonical gene
encoding.

## Open questions

- Hash over lowered `Program` bytes, or over the gene structure directly?
  (Gene-level better matches the "stop re-deranging the loop" intent.)
- Tabu in polish only (cheap, deterministic) vs. also annealing (needs a soft
  penalty so it doesn't break detailed balance).
- Buffer length and whether to key on full genome vs. structural skeleton.

## Acceptance criteria

- Polish (and/or anneal) consults a tabu set; measure whether time-to-solve or
  solve-rate improves on the target suite. Low-priority; pick up opportunistically.
