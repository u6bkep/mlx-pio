# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-12 (eve).

## Emulator fidelity fixed (e4a4860, a810ec5) — verdicts re-established

RP2350 ch.11: MOV→ISR/OSR resets the shift counter, OUT→ISR sets it,
and PUSH IFFULL / PULL IFEMPTY are shift-count GUARDS. All three
layers (vendored, narrow twin, z3 mirror) shared the divergence —
fixed identically, pinned by unit tests + narrow_diff + smt fuzz.
`NOP_CANON` is now `mov x,x` (0xA021); ISR/OSR self-moves are real
ops. Old enumerate.rs ≤4-word impossibility proof carries a caveat
(it excluded those self-moves as nops). Production firmware and past
certified artifacts unaffected (journal 2026-07-12 eve).

**All small-bracket refutations HOLD under corrected semantics**:
L=1 18,357 items; L=2 0..0 / 0..1 (195.6M, 20s) / 1..1 (195.4M,
19.8s) at 28 threads.

## Ticket 009 CORE LANDED (a687b45) — lemma word quotient, ~1.9x

`word_canon` (lemma-verified per-config classes; battery gate
executes every respelling: tx_a folds 23,584/65,536 words) + lazy
per-mask partial-word class tables + fork-time sibling dedup. Both
L=1 censuses pass with it active. Brackets: L=1 13,501 items; L=2
0..1 102.7M/11.3s; 1..1 102.5M/11.6s (from ~195M/20s). Remaining
009: memo cond canonicalization (measure census first), lemmas for
battery-suggested-but-unproven classes.

## L=3 0..0 IN FLIGHT — two runs

- **Verdict run** (corrected semantics + quotient): `tx_a_l3_00_split`,
  search_split(28); quotient shrank the frontier 2052 → 1520 units.
  First (pre-quotient) attempt settled 2044 units in 10s then spent
  30min on 6 stragglers → relaunched on the quotient binary.
  `/data/pio_optimization/runs/txa_l3_split_run.log` (pre-quotient
  attempt kept as *.prequotient.log). Straggler-tail lesson: recursive
  unit splitting is the next driver lever if tails persist.
- **Census run** (OLD semantics, verdict tainted): instrumented
  sequential run, now past 3.78B items / 2 purges, snapshots written —
  `/data/pio_optimization/runs/txa_l3_run.log` + txa_l3_snaps/.

## Next (in expected-value order)

1. **Read the L=3 split verdict**; if refuted, ladder to L=4
   rediscovery.
2. **Read the census/snapshots** (miss composition → whether memo
   cond canonicalization is still worth building).
3. **Ticket 008 — outcome-grouped forking** (highest ceiling; also
   recovers the P1 nop-naming cost).
4. **Ticket 010 + multi-case specs** (flagship RX prereq).

## Flagship (unchanged): phase-invariant RX

Spec/oracle must quantify duty skew (±8ns measured / ±24 design) ×
parked phase; battery = pio_harness/tests/rx_bench_repro.rs +
ro_sampled fixtures. Engine needs multi-case specs first. (The
fidelity bug is NOT the deaf-RX culprit — firmware uses are benign.)

## Shard twin — COMPLETE (2a3a2e7); hand shard_pio/ to Christian

## Bench (idle) / paused

-0 pinger / -1 responder on [3][4][4][4], worktree
Raven-Firmware.single-sm-tx-bench UNPUSHED @350ede86. Paused: SMT
len-4, compress2, len-5 fleet, ticket 006 runner migration.
