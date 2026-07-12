# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-12 (early am).

## Narrowing engine — ALL LEVERS LANDED; L=3 wall pushed back, not down

All four planned levers shipped, census-gated (0381fc4..fe610b3):
pin-write pre-filter (exact one-cycle lookahead), P1-full register
symmetry (always-on, binding forks; seeding added), P2/P3/P4 canon
filters, and the consulted-set memo — upgraded mid-session to
consulted-STATE keys (ticket 007, done): core + read-masked patterns,
segment-local X/Y provenance, two-level index. A conflict-scope bug
that had silently poisoned EVERY fork-frame record since the memo
landed was found via a measurement gate and fixed (389bbfa).

**tx_a ladder (460-cycle oracle), all verdicts reproduce (0 champions):**
- L=1: 14,141 items (was 16,735) · L=2 0..0: 632,537 (was 1,080,991)
- L=2 0..1: 153.3M (was 220.4M) · **L=2 1..1: 23.9M (was 220.4M — 9.2x,
  memo-driven; wrap-invariance is dead, the memo sees structure DFS can't)**
- **L=3 0..0 STILL OPEN.** v4 (1M-entry memo) killed at 1.42B items,
  hits flatlined at 2.57M once purges hit bar 1024; v5 (8M entries, x2
  purge) tripled hit density (7.4M hits @ 230M items) but ALSO
  flatlined ~200M and ran at 17K items/s. Verdict: capacity is not the
  lever — deep regions stop matching under value-exact patterns.

## Next (in expected-value order)

1. **Mine the dumps** — runs/txa_l3_v5_hits.jsonl + v4 (309K hit-pair
   lines): which components/fields block sharing in hot clusters.
2. **Predicate-valued patterns** — records condition on x's VALUE where
   the subtree only zero-tested it (`jmp !x`); condition on the
   predicate class. Same soundness shape as consulted-state.
3. **Probe throughput** — 64-165K items/s vs 800K plain; fast hasher,
   projection reuse. Memo must also stop LOSING wall-clock at L=2 0..1.
4. **ISR/OSR provenance; cond-lazy JMP targets** (fork-width lever,
   tx_a is JMP-heavy).
5. **Multi-case specs** (sequential trace concatenation + state reset)
   — the engine feature the flagship RX resynthesis needs.

## Flagship (unchanged): phase-invariant RX

Spec/oracle must quantify duty skew (±8ns measured / ±24 design) ×
parked phase; battery = pio_harness/tests/rx_bench_repro.rs +
ro_sampled fixtures. Engine now has seeding + impossibility proofs;
needs multi-case specs (above) before it can attack this.

## Shard twin — COMPLETE (2a3a2e7); hand shard_pio/ to Christian

## Bench (idle) / paused

-0 pinger / -1 responder on [3][4][4][4], worktree
Raven-Firmware.single-sm-tx-bench UNPUSHED @350ede86. Paused: SMT
len-4, compress2, len-5 fleet, ticket 006 runner migration.
