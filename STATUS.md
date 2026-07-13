# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-12 (late night).

## L=3 ladder: 3 of 6 REFUTED; the other 3 await a faster engine

0..0 (5.33B items/26min), 0..1 (3.43B/65s), 1..1 (3.14B/59s) proven
under corrected semantics + OOB refutation (6b08a0b: out-of-footprint
execution is UB; space = programs staying within their L words).
The remaining three (2..2, 1..2, 0..2) are ALL monsters — they put
all 3 slots on the straight-line path (0..1/1..1 reach slot 2 only
via JMP); 1..2 hit 52B items at 6% settled, 2..2 extrapolated ~10h.
**They are now the 008 benchmark, not something to grind.**

**Ops rules:** (post-OOM) big searches run serialized and gated —
`systemd-run --user -p MemoryMax=48G -p MemorySwapMax=0
-p RuntimeMaxSec=3000`. (User policy 2026-07-12): **any run >50 min
is too slow** — the engine must get faster instead; a long run's
mining value is in its first minutes; session cache expires at 1h.

## Ticket 008 — stages 1+2 landed; stage 2 = junk-window collapse, 4.4x

Stage 1 (12ec1cb): lazy JMP target demand (consult-time, like delay).
Stage 2 (89f97c9): **time-shift-invariant refutation lookahead** at
the delay post-fork — one representative walk refutes the whole
delay-spelling family when the window to refutation has no latch
value changes, no external reads, no fork edges (mined: 96% of
delay-conflict pairs co-refute, 100% latch-quiet, 0 diverged; records
lose their undecided-delay conds). **L=3 0..1: 3.43B→785M items
(4.4x, 29s); 1..1: 3.14B→717M; L=2 1..1: 104.6M→40.2M.** OOB breaks
horizon-bounded per shift point (hole caught by the L=1 exact
census). Next: fresh mining pass on the stage-2 binary (the wall
census changed under the collapse), then stage 3 (cross-opcode
outcome records — was 41% of the old wall) + stage 4 (ISR_CNT
provenance). After all stages: one-shot Codex review (gpt-5.6-sol).

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
