---
status: in-progress (3.4x real breed-step; near the single-SM ceiling)
priority: high
created: 2026-06-22
source: the shared bottleneck for scale (SCRATCH) + meta-tuning (002)
---

# 004 — Eval hot-path performance

## Deep dive round 2 (2026-06-23) — compute-bound, not cache-bound

Hypothesis going in was cache/L1. `perf` refuted it: single-thread IPC 4.2 with
~0.0% L1d misses; even at 32 threads L1d misses stay 0.2%. `Emulator` is 11.5
KiB and fits L1. The bottleneck is pure instruction count (~142K/eval), and at
32 threads IPC drops 4.2->2.6 from SMT execution-unit contention (16 physical
cores, ~5.2 IPC/core saturated) — so 32 islands give ~20x aggregate, not 32x;
physical core count is the real ceiling.

Wins came from cutting instructions, all guarded by `fast_step_matches_full`:
- **Leaner capture** (commit 4d70f7e): `tick_pio` drops the redundant per-cycle
  pre-`update_gpio` (hoisted to one `refresh_gpio`); `update_gpio_released`
  merges only released blocks; one merged-GPIO read/cycle instead of per-pin.
- **Fat LTO + codegen-units=1** (commit 761c53f): the hot path crosses 4 crate
  boundaries through thin pub fns that weren't inlined (~15% was call
  overhead). LTO inlines `tick_pio -> step_n_with_pins -> step_with_pins ->
  execute_cycle`. Incremental release rebuild ~4s; debug/test loop unaffected.

Cumulative vs the original baseline:
- emulator core (`run`): 16.79 -> 4.17us (4.0x)
- **real breed-step: 16.80 -> 4.88us (3.4x)**
- candidate eval (run+edge_cost): 30.66 -> 8.90us (3.4x)

Post-LTO the profile is genuine PIO emulation (execute_cycle / 4-SM clock loop /
pin merge). Going further would require single-SM specialization (skip the
4-SM loop) — declined, multi-SM/complementary-programs is a wanted feature — or
a JIT-scale rewrite. This is a good stopping point near the single-SM ceiling.

## Round 1 (2026-06-23) — 2.9x on the real per-candidate eval

Profile-first with a Criterion suite (`pio_superopt/benches/eval.rs`): full
eval, its decomposition, cost-by-mask-width, and the real breed-step. Findings
overturned the guesses below — per-eval setup (validate/assemble/reset) is ~2%,
not a target; `validate()` is 29ns and never short-circuits the local move.

Two wins, both verified byte-identical:

1. **edge_cost 3.9x** (12.3 -> 3.2us). A full all-ones mask made it scan all 32
   bit-channels when only 1 pin carries data (~10us wasted); intersect `care`
   with bits present in golden|candidate. Plus rolled the align_edges DP onto
   two reused rows (was one heap alloc per row). Commit c4041fe.
2. **Emulator core 3.1x** (16.5 -> 5.26us, ~59 -> ~19 ns/cycle). `emu.step()`
   ran both Cortex-M33 cores + all peripherals every PIO cycle; we run one SM,
   no firmware. Vendored rp2350-emu (commit 326e74f) and added
   `step_pio_only()` (commit c9bc555) — the PIO slice of step only. Gated by
   `fast_step_matches_full` (DME ref + plateau + 300 random programs, in the
   normal suite).

Net: candidate eval 30.7 -> 10.5us; **real breed-step 16.8 -> 6.45us (2.6x)**.

## Remaining levers (optional, diminishing)

- The fast step still calls `update_gpio` twice/cycle and iterates all 3 PIO
  blocks (2 in reset). Dropping the *pre*-step `update_gpio` (safe only when no
  input pin changes mid-run — true for TX-only DME) and skipping reset blocks
  would shave the ~19ns/cycle further, at the cost of the clean "byte-identical
  to full step in all cases" guarantee.
- `edge_cost`'s remaining ~4 small allocs/eval (channel_edges Vecs + DP rows)
  could move to a thread-local scratch buffer. Sub-1us; edge_cost is now ~15%
  of eval at most.
- The PIO step itself (`PioBlock::step_n_with_pins`, vendored picoem-common) is
  now the bulk of the per-cycle cost — profile if more is needed.

## Re-run 002 (meta-tune) at the now-affordable budget

With evals ~2.9x cheaper, the meta-tuner's inner trials can run far closer to
deployment scale — the fix for the transfer trap. 002's machinery is ready.

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
