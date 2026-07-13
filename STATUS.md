# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-13.

## L=3 ladder: 3 of 6 REFUTED; monsters under attack with stage 3b

0..0 (5.33B items/26min), 0..1, 1..1 proven under corrected semantics
+ OOB refutation. Remaining: 2..2 / 1..2 / 0..2 (all-slots-live
monsters). Pre-3b 2..2 baseline: 27% settled at the 50-min gate,
23.3B worker items (~3h full). Post-3b 2..2 attempt in flight
(pio-sw-gate2, txa_l3_swalk_gate2.log).

**Ops rules:** big searches serialized + gated — `systemd-run --user
-p MemoryMax=48G -p MemorySwapMax=0 -p RuntimeMaxSec=3000`. Any run
>50 min is too slow — build the lever instead. `systemctl --user`
checks are unreliable from background monitor shells — watch the
run's log mtime instead.

## Ticket 008 — stages 1+2+3b landed

Stage 1 (12ec1cb): lazy JMP target demand. Stage 2 (89f97c9):
junk-window collapse, 4.4x at L=3; delay-agnostic walk records
(2a4c5fa). **Stage 3b (a1477f3): generalized subtree walk — 2x on
top of stage 2.** Mined via PairRace probe (96f0372 flat, 8e69ac8
recursive joint-fork races): cross-opcode conflicts = ~all of the
wall; **84.5% of cond-miss subtrees fully co-refute** (770K deep
races, 100% latch-quiet, true divergence 37, budget-bind 29). Lever =
bounded concrete DFS of the item's future at a COND-MISS pop, full
value enumeration (superset of engine children ⇒ no theorem needed),
768-step budget, 128-cycle per-BRANCH depth cap. 89% kill rate in
situ. **L=3 0..1: 784.0M→388.1M items (76s); 1..1: 716.3M→353.1M
(68s); verdicts hold; all 12 gates pass.**

**Firing-policy lesson (measured):** the SAME walk is a catastrophic
regression fired ungated at fork sites (18% kills, 416 avg steps,
walks out-cost the main loop) and a 2x lever fired at cond-miss pops
with a per-branch depth cap. Failure cost = proving a branch
survives; bound it by depth (~2x mined leaf depth 63cy), and fire on
the population the mining actually measured.

**Next in 008:** stage 3c adaptive walk budget (user: runtime tunable
via kill-rate×cost heuristic + occasional deep probe walks); stage 4
ISR_CNT provenance; then one-shot Codex engine review (gpt-5.6-sol,
single message). Re-mine the wall census post-3b before building 3c.

## Emulator fidelity fixed (e4a4860, a810ec5) — holds

MOV→ISR/OSR resets shift counter, OUT→ISR sets it, PUSH IFFULL/PULL
IFEMPTY are guards. NOP_CANON = mov x,x. Old ≤4-word impossibility
proof carries a caveat.

## Next (in expected-value order)

1. **2..2 / monsters under stage 3b** → all-six-refuted ⇒ L=4 ladder.
2. **008 stage 3c/4** + Codex engine review.
3. **Ticket 010 + multi-case specs** (flagship RX prereq).

## Flagship (unchanged): phase-invariant RX

Spec/oracle must quantify duty skew (±8ns measured / ±24 design) ×
parked phase; battery = pio_harness/tests/rx_bench_repro.rs +
ro_sampled fixtures. Engine needs multi-case specs first.

## Shard twin — COMPLETE (2a3a2e7); hand shard_pio/ to Christian

## Bench (idle) / paused

-0 pinger / -1 responder on [3][4][4][4], worktree
Raven-Firmware.single-sm-tx-bench UNPUSHED @350ede86. Paused: SMT
len-4, compress2, len-5 fleet, ticket 006 runner migration.
