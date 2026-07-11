# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-11 (late).

## Narrowing engine — evaluator + fork loop LANDED; L=3 wall says: build the memo

- **Evaluator** (`pio_superopt/src/narrow/`, 4b9a364..42ecea0): flat Copy
  NState, total bit-field decode, vendored-exact; diff-gate ~2,500
  programs byte-identical; independently reviewed (zero divergences).
  Contract: `docs/evaluator-spec.md` (now incl. §9 driver contract).
- **Fork engine** (`narrow/engine.rs`, aa489b3, cf892d0): bit-field
  demand forking, trace refutation, don't-care champions; delay
  post-fork + side-set trace pre-filter (≈1.65x). Gates green
  (`tests/narrow_engine.rs`).
- **Proofs banked** (tx_a oracle, 0 champions, eager+lazy engines
  agree): footprint 1 and 2 CANNOT reproduce tx_a's trace (all wraps;
  L=2 0..1 = 220M items). L=1 period-3 duty impossible.
- **The wall**: L=3 first bracket abandoned >5.67B items/~90min of six
  brackets. Plain DFS has no cross-item sharing — the playground's
  lesson holds ("the memo is the whole game"). PIVOT (user call).

## Next (in expected-value order)

1. **Consulted-set memoization** — the real narrowing machinery;
   canonical-prefix hashing is the memo key (ties into CANON content
   addressing). Design against the playground's memo.
2. **Op-level pin-write pre-filter** — SET/OUT/MOV PINS to captured,
   OE-pinned pins determine levels like side-set does; kills 32-wide
   data forks early.
3. **Runner integration** — resumable brackets (spec-ladder pattern)
   before any more L=3+ compute.
4. Full P1 virtual registers: must model PULL's implicit X at the link
   binding (nonblocking/if_empty PULL on empty FIFO loads X — X/Y
   renaming is NOT a free symmetry; P1-lite is a default-off flag).

## Shard twin — COMPLETE (merged 2a3a2e7)

`shard_pio/`: evaluator-spec.md implemented in shard; 101/101 certified
vectors byte-identical; full closure checker-green, zero CANON
advisories. Run: `bin/shard_eval run runner.shard` from shard_pio/
(prebuilt binary at ~/Documents/programmingSync/computer-whisperer/
shard/bin — 40-60x faster than the Rust bootstrap). PIO semantics now
exist as CHECKED SHARD DEFINITIONS → Christian's 2-SM ≡ 1-SM equality
proof is statable. Proof arc needs (README design notes): Block-record
lift of shared latches/irq_flags, inter-SM intra-cycle ordering pinned
in spec + 2-SM vectors, then step2 bisimulation by induction over the
cycle list. Hand shard_pio/ to Christian for review.

## Flagship (unchanged): phase-invariant RX

Spec/oracle must quantify duty skew (±8ns measured / ±24 design) ×
parked phase; battery = pio_harness/tests/rx_bench_repro.rs +
ro_sampled fixtures. Fast-RX variant needed for production main.

## Bench (idle) / paused

-0 pinger / -1 responder on [3][4][4][4], worktree
Raven-Firmware.single-sm-tx-bench UNPUSHED @350ede86. Hardware
validation deferred until phase-invariant RX. Paused: SMT len-4,
compress2, len-5 fleet.
