# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-12 (midday).

## Narrowing engine — memo machinery was 87% of CPU; now 6.7x faster

perf (100s L=3 slices) found the probe's record-list rescan at ~55%
CPU, SipHash ~25%, insert dedup ~7%. Fixed (5807b93): contiguous
packed-conds RecList, FxHash maps, insert-side subsumption +
REC_LIST_CAP=32 (swept 32/64/256; equal hit density, 32 fastest
everywhere). Same-slice L=3 throughput 257K → **1.72M items/s**;
L=2 0..1 40min → 4.5min (within 1.35x of plain DFS, was 12x);
L=2 1..1 ~5min → 76s. All verdicts reproduce; L=1 / L=2 0..0 item
counts byte-identical. New stats: recs_avg/recs_max in heartbeat.

**Implication: v4's L=3 kill point (1.42B items) is now ~15 min of
wall-clock. An overnight run plausibly finishes the 0..0 bracket —
or shows whether deep-region throughput decay survives capped lists.**

## Instrumentation flags — landed (95710db), for the next long run

Env-driven, search-behavior-free (gated), big dumps →
`/data/pio_optimization/runs/` (2nd SSD, 1.3T free):
- `PIO_NARROW_PROBE_LOG=<path>` (+`_BYTES`, default 8GiB): per-cycle
  probe-outcome census (nocore/state_miss/cond_miss/hit) + sampled
  miss diagnostics (nearest-record component diff / first failing
  cond). Stride-doubles past half budget.
- `PIO_NARROW_SNAPSHOT=<dir>` (+`_MAX`): full memo-table JSONL dump
  before each purge + at end.

## Next (in expected-value order)

1. **Overnight instrumented L=3 0..0** with both flags — either the
   bracket closes, or the probe census + snapshots say exactly which
   state components block deep sharing.
2. **Predicate-valued patterns** (condition on the predicate class the
   subtree tested, e.g. `x==0` vs x's exact value) — build informed by
   the near-miss data.
3. **Wrap-bracket verdict-equivalence** — L=2's 0..1/1..1 theorem
   generalized could skip whole L=3 brackets; probe empirically first.
4. **ISR/OSR provenance; cond-lazy JMP targets** (fork-width lever).
5. **Multi-case specs** (sequential trace concat + reset) — needed by
   the flagship RX resynthesis.

## Flagship (unchanged): phase-invariant RX

Spec/oracle must quantify duty skew (±8ns measured / ±24 design) ×
parked phase; battery = pio_harness/tests/rx_bench_repro.rs +
ro_sampled fixtures. Engine needs multi-case specs (above) first.

## Shard twin — COMPLETE (2a3a2e7); hand shard_pio/ to Christian

## Bench (idle) / paused

-0 pinger / -1 responder on [3][4][4][4], worktree
Raven-Firmware.single-sm-tx-bench UNPUSHED @350ede86. Paused: SMT
len-4, compress2, len-5 fleet, ticket 006 runner migration.
