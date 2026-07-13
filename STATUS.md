# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-13.

## L=3 ladder: 3 of 6 REFUTED; monsters still gated on a real lever

0..0 (5.33B items/26min), 0..1 (784.0M/27s), 1..1 (716.3M/24s)
proven. Remaining: 2..2 / 1..2 / 0..2 (all-slots-live monsters).
2..2 stage-2-engine baseline: 27% settled at the 50-min gate, 23.3B
worker items (~3h full).

**Ops rules:** big searches serialized + gated — `systemd-run --user
-p MemoryMax=48G -p MemorySwapMax=0 -p RuntimeMaxSec=3000`. Any run
>50 min is too slow — build the lever instead. `systemctl --user`
checks are unreliable from background monitor shells — watch the run
log's mtime. **Magnitude gates need idle-box WALL-CLOCK, not item
counts alone** (3b's item halving hid a 2.8x slowdown behind a
contended baseline).

## Ticket 008 — stages 1+2 landed; 3b TRIED AND REVERTED

Stage 1 (12ec1cb): lazy JMP target demand. Stage 2 (89f97c9):
junk-window collapse, 4.4x at L=3; delay-agnostic walk records
(2a4c5fa). **Stage 3b (generalized subtree walk at cond-miss pops)
REVERTED: items halved but wall-clock 2.8x WORSE** (0..1 27s→76s,
2..2 10.3% vs 27% settled at 50min). Mechanism: the walk re-explores
subtrees without memo/quotient/canon sharing (~563 steps/kill vs
~100 break-even) and sibling pops re-walk overlapping futures. Full
post-mortem + firing-policy lessons in ticket 008 §3b.

**What stands from the mining (probes KEPT, 96f0372+8e69ac8):**
cross-opcode conflicts ≈ the whole post-stage-2 wall, and **84.5% of
cond-miss subtrees provably co-refute** (770K joint-fork races, 100%
latch-quiet). The transferability is real; the exploitable form is
RECORD-side (weaken conds so probers hit without re-proving), not
walk-side re-proof. Cheap candidate to size first: linear lock-step
race transfer at cond-miss (no forking, ~44cy, covers the 7.4%
flat-co_refute slice).

**Next in 008:** stage 4 ISR_CNT provenance (now active); then
one-shot Codex engine review (gpt-5.6-sol, single message).

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
