# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-11.

## Narrowing engine — UNDERWAY (evaluator v1 landed)

`pio_superopt/src/narrow/` (4b9a364): our own forkable evaluator —
flat Copy `NState` (~120B, fork checkpoint = memcpy), total bit-field
decode, vendored-exact cycle semantics. Contract: `docs/
evaluator-spec.md` (written to double as the shard twin's spec).
Differential gate: `tests/narrow_diff.rs` — DME reference + ~2,500
random programs (side-set configs, config genes, streaming, RX
flavors, pin stimulus), byte-identical to `run::run`. 2.8x the fused
vendored path. Contract facts the fuzz pinned: pin value latch idles
ALL-ONES; osr_count resets 32. A sub-agent semantic review of
narrow/mod.rs vs vendored sm.rs was launched 2026-07-11 — check its
findings before building on edge-case semantics.

## Next: the narrowing layer itself

1. Hole representation + demand-driven forking at bit-field
   granularity (side-set bits first, then opcode, operands lazily,
   delay last); DFS + checkpoint prefix sharing (v1).
2. Canonicalization fork-filters, from Christian's CANON.md
   (`~/Documents/programmingSync/computer-whisperer/shard/docs/`):
   P1 virtual registers (candidates over r0/r1, link-time binding
   first-consulted→X, preloads are candidate holes and rename with the
   binding), P2 canonical nop, P3 delay-normal form, P4 vacuous
   control. Constraints: every rule LENGTH-NON-INCREASING on the
   representative (else ≤N impossibility proofs die); leaf filters
   only at fork time (sibling-content constraints stay out); license =
   fuzz-certified. Gate: len≤2 exactness census (every behavior class
   keeps exactly one representative) + per-rule differential fuzz.
3. First target: tx_a (4-instr) optimality as validation. Flagship:
   phase-invariant RX — the spec/oracle must quantify over duty skew
   (±8ns measured, design ±24) and parked sub-cycle phase; battery =
   pio_harness/tests/rx_bench_repro.rs + ro_sampled fixtures.

## Shard prover track (ratified direction, not started)

Shard PIO emulator implementing evaluator-spec.md (total step,
first-order — good shard fit), diff-fuzzed three ways; then the prize:
unbounded equality proof of shipped 2-SM TX pair ≡ our 1-SM TX
(delayed bisimulation, constant 3-cycle DI lead). Christian's actual
TX requirement (clarified 2026-07-11): implementations must MATCH —
they do per emulator cert incl. parking (tx_a parks DI high; our
mov pins,~null identical); the proof is what would fully convince.

## Bench (idle, carried from 2026-07-10)

-0 pinger / -1 responder on [3][4][4][4] duty-robust RX, worktree
Raven-Firmware.single-sm-tx-bench (branch UNPUSHED, @350ede86).
Parked-phase 125↔125 margins = the phase-invariant RX spec; production
main's shipped fast RX is deaf to boards (needs the fix class too).
Hardware validation deferred until phase-invariant RX lands.

## Paused

SMT len-4 probe, compress2, len-5 fleet (benchmark tier).
