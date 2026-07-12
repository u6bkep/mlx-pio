# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-12 (late night).

## L=3 ladder: 3 of 6 brackets REFUTED, rest in flight (gated unit)

0..0 (5.33B items/26min), 0..1 (3.43B/65s), 1..1 (3.14B/59s) proven
under corrected semantics + OOB refutation (6b08a0b: out-of-footprint
execution is UB; space = programs staying within their L words).
Remaining: 2..2 (fast), then the two monsters 1..2 (52B+ items at 6%
settled when the first attempt died) and 0..2. Running detached as
systemd unit `pio-l3rest` (txa_l3_rest2.log), MemoryMax=48G.
**Ops rule (post-OOM): big searches run serialized and gated —
`systemd-run --user -p MemoryMax=48G -p MemorySwapMax=0`.** All six
refuted ⇒ tx_a footprint ≤ 3 impossible ⇒ L=4 rediscovery next.

## Ticket 008 in progress — stage 1 of 4 landed (12ec1cb)

Lazy JMP target demand (consult-time fork, like delay): L=3 0..1
−1.4%, 1..1 −1.6% items; L=2 +2% (bounded vacuous-walk cost); scales
with L. **Probe-log measurement reshaped the ticket**: target-only
conflicts are 0.33% of the deep-memo wall; the wall is **JMP
delay-bit conflicts (44% — same word, different delay spelling) and
cross-opcode pairs (41%)**; cond-bit conflicts 2.5%, side ~0 (the
fork-time side pre-filter kills them). Remaining stages: (2)
outcome-grouped forking (value-set items, u32-bitmask sets per
≤5-bit field, lazy refinement at re-consult), (3) predicate-valued
memo records on stage-2 partitions, (4) ISR_CNT provenance. Stage
2/3 design must center timing/outcome sharing, not cond grouping.
After all stages: one-shot Codex review (gpt-5.6-sol) of the engine.

## Emulator fidelity fixed (e4a4860, a810ec5) — holds

MOV→ISR/OSR resets shift counter, OUT→ISR sets it, PUSH IFFULL/PULL
IFEMPTY are guards; all 3 layers fixed identically, gates pinned.
NOP_CANON = mov x,x. Old ≤4-word impossibility proof carries a caveat.

## Next (in expected-value order)

1. **L=3 rest verdicts** (in flight) → L=4 rediscovery ladder.
2. **008 stages 2–4** (see above) + Codex engine review.
3. **Ticket 010 + multi-case specs** (flagship RX prereq).

## Flagship (unchanged): phase-invariant RX

Spec/oracle must quantify duty skew (±8ns measured / ±24 design) ×
parked phase; battery = pio_harness/tests/rx_bench_repro.rs +
ro_sampled fixtures. Engine needs multi-case specs first.

## Shard twin — COMPLETE (2a3a2e7); hand shard_pio/ to Christian

## Bench (idle) / paused

-0 pinger / -1 responder on [3][4][4][4], worktree
Raven-Firmware.single-sm-tx-bench UNPUSHED @350ede86. Paused: SMT
len-4, compress2, len-5 fleet, ticket 006 runner migration. Old
census run died (OOM) at 6.65B items pre-census — loss accepted.
