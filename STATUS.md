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

## L=3 0..0 IN FLIGHT — two runs

- **Verdict run** (corrected semantics): `tx_a_l3_00_split`,
  search_split(28), 2052 frontier units —
  `/data/pio_optimization/runs/txa_l3_split_run.log`.
- **Census run** (OLD semantics, verdict tainted): the instrumented
  sequential run from midday, kept for probe census + purge
  snapshots + as a throughput benchmark (~1.5M items/s sustained,
  blew past v4's 1.42B kill point) —
  `/data/pio_optimization/runs/txa_l3_run.log`.

## Next (in expected-value order)

1. **Read the L=3 split verdict**; if refuted, L=3 walls are down —
   ladder to L=4 rediscovery.
2. **Ticket 009 — word behavioral quotient**: design settled this
   session (see ticket): battery digest PROPOSES classes, only
   lemma-verified merges prune (SMT mirror too narrow to prove the
   full space); fork-time sibling dedup by signature is the sound
   insertion point. Real wins identified: SET-pins data masking
   (16x on 1-pin configs), config-dependent aliasing.
3. **Ticket 008 — outcome-grouped forking** (highest ceiling; also
   recovers the P1 nop-naming cost noted in the journal).
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
