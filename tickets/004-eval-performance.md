---
status: open
priority: high
created: 2026-06-22
source: the shared bottleneck for scale (SCRATCH) + meta-tuning (002)
---

# 004 — Eval hot-path performance

## Why

Two open levers both bottleneck here:
- **Scale** is the lever that's actually working on DME (edge-cost 22→17 with
  more islands/iters), but we're core-bound at 32 islands; going further needs
  faster per-eval, not more cores.
- **Meta-tuning (002)** overfits because affordable inner trials are far smaller
  than deployment; honest tuning needs deployment-scale inner trials, which needs
  cheaper evals.

The hot loop is `run()` (`run.rs`) — one emulator execution per candidate,
~28µs (278 cycles × ~100ns/cycle), called tens of millions of times per search.
mlx86's author spent real effort on perf to reach millions of trials; our problem
is fundamentally simpler (deterministic emulator, no KVM), so there should be
substantial headroom.

## Candidate wins (profile first — evidence before theories)

- **Per-eval overhead**: `Pio::reset` cost; `program.validate()` runs on every
  candidate before `run` (and again in cost) — dedup. `program.assemble()` +
  `load_at` per eval — cache when slots unchanged?
- **Capture less**: `trace_pads` captures all `capture_pins` for all `cycles`;
  we only score specific bits. Capture only what the metric reads.
- **Shorter horizons**: 278 cycles/eval for DME — is the full corpus needed every
  eval, or can early-out on divergence (bound the cost, skip the tail)?
- **The emulator core** (`picoem-common`): ~100ns/cycle. Profile the per-cycle
  step; the SM step is the inner-inner loop.
- **Edge metric cost**: `edge_cost` does per-channel DP alignment each eval —
  cheap vs the emulator, but check it's not O(n·m) blowing up on edge-dense
  candidates.

## Acceptance

- Profile `run()` + the cost path; identify where the ~28µs goes.
- A measured speedup (target: several×) on the per-eval hot path, verified to
  produce byte-identical waveforms (the emulator-reuse invariant).
- Re-run 002's meta-tune at a larger inner budget to check the transfer trap
  closes.

## Note

Verify any optimization preserves the reset-reuse invariant (pio_harness
`tests/reset_reuse.rs`: reset must equal a fresh build).
