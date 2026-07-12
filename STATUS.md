# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-12 (pm).

## L=3 0..0 IN FLIGHT — instrumented, detached, ~2.4M items/s

Launched 14:03 under `/data/pio_optimization/runs/txa_l3_*` (probe
census + snapshots; 8GiB detail budget burned in the shallow region
by design, census/snapshots cover depth). At this rate v4's 1.42B kill
point ≈ 10 min, v1's 5.67B ≈ 40 min — **the bracket may close**.
Watch: `tail -f /data/pio_optimization/runs/txa_l3_run.log`.

## Engine this session: 6.7x sequential + 25x parallel

- **Memo rework (5807b93)**: perf showed 87% CPU in memo machinery →
  RecList (contiguous packed conds), FxHash, insert-side subsumption +
  REC_LIST_CAP=32 (swept). 257K → 1.72M items/s sequential.
- **Parallel split driver (f673354)**: `search_split(spec, cap,
  threads)` — phase-1 truncated-spec champions = frontier work units
  (gentle cycle growth, straggler lesson from the playground); seeded
  workers, per-unit memos, deterministic; binding-free units mirror-
  expanded. Refutation verdicts exactly ≡ sequential (gated).
  **L=2 0..1 bracket: 40min → 4.5min → 11s (28 threads).**
- **Instrumentation flags (95710db)**: PIO_NARROW_PROBE_LOG census +
  near-miss diagnostics; PIO_NARROW_SNAPSHOT pre-purge table dumps.

## Next (in expected-value order)

1. **Read the L=3 verdict + dumps** (census: which components block
   deep sharing; snapshots: table composition at purges).
2. **Ticket 009 — word behavioral quotient** (digest classes):
   mechanized P2/P4 + memo cond canonicalization; attacks the
   dominant cond-miss class. Low risk, do before 008.
3. **Ticket 008 — outcome-grouped forking** (fork on consult OUTCOME,
   value-sets per field; unifies with predicate-valued memo records).
   Highest ceiling, big surgery.
4. **Ticket 010 — multi-SM factorization blueprint** (playground
   compile⊗exec arc; needs multi-case specs first).
5. **Multi-case specs** (trace concat + reset) — flagship RX prereq;
   include the determinism prefilter + cheapest-case-first ordering
   lessons from 010.

## Flagship (unchanged): phase-invariant RX

Spec/oracle must quantify duty skew (±8ns measured / ±24 design) ×
parked phase; battery = pio_harness/tests/rx_bench_repro.rs +
ro_sampled fixtures. Engine needs multi-case specs first.

## Shard twin — COMPLETE (2a3a2e7); hand shard_pio/ to Christian

## Bench (idle) / paused

-0 pinger / -1 responder on [3][4][4][4], worktree
Raven-Firmware.single-sm-tx-bench UNPUSHED @350ede86. Paused: SMT
len-4, compress2, len-5 fleet, ticket 006 runner migration (partly
subsumed by search_split's unit model).
