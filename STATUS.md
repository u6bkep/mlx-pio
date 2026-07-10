# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-10.

## Direction pivot (2026-07-10)

Target is now the REAL rs485-eth firmware programs (`reference/rs485-eth`,
same code as Raven-Firmware.main/crates/rs485-eth): TX pair (tx_a 4 + tx_b
17 instr), RX (32/32 full), timestamp (10). The single-SM DME scoreboard
(≤4 impossible, 6-word champion) is demoted to a benchmark/validation
suite — the shipped firmware never used that encoder. Note the ≤4 proof
predates side-set in enumeration; side-set is now mandatory in specs.

## Single-SM TX — emulator-certified, hardware validation pending

`mov pins, !pins` (pin-as-state, our compress-track discovery) dissolves
the two-SM TX split (tx_a existed only because side-set can't XOR).
Transform: irq→mov toggle, parking irq→`mov pins ~null`. 17 instr, one SM,
IRQ freed, TX PIO 31→27. Proof: `pio_harness/tests/tx_single_sm.rs` —
DE cycle-exact, DI edge-identical (constant 3-cycle lead), both clkdivs,
multi-frame parking; shipped RX round-trips all 16 data codes.

**Hardware next (user)**: Raven-Firmware.main has UNCOMMITTED changes —
rs485-eth feature `single-sm-tx` + refreshed rs485_eth_test HIL firmware
(both variants build). Flash `--features single-sm-tx`, ping over the rig
vs known-good RX. If DI is dead on hardware, suspect pad input-enable on
the DI pin (mov ~pins readback) — emulator models no sync latency.

## Narrowing engine (planned third engine, not started)

From reference/shard-search-playground: superposed evaluation — holes =
instruction slots, demand = fetch, fork at BIT-FIELD granularity at the
cycle each field is consulted (side-set first: asserts even under stall →
trace-pinned), prune on per-cycle pin mismatch, unfetched slots =
don't-cares. Build our OWN demand-driven evaluator (SMT-mirror pattern:
diff-fuzzed accelerator, emulator+certifier = soundness authority). First
targets: tx_a ≤4 optimality (validation), then tx_b-single compression.

## Paused/running (user's shell)

- SMT len-4 probe: `runs/smt-synth-len4-none.log` (check `ps -C superopt`).
- compress2 + len-5 fleet: commands in journal 2026-07-06 / docs/fleet.md.
  Both now benchmark-tier priority.

## Watch out for

- SMT UNSAT trust = mirror diff-fuzz (rerun before believing any UNSAT).
- Harness `set_pin()` is external stimulus and OVERRIDES PIO output in the
  GPIO merge — preset output latches via exec'd `mov`, never `set_pin`.
- Metric gaming: size() is full footprint since 6b41592.
