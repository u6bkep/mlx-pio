# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-10 (late night).

## ROOT CAUSE FOUND: RX failure = receive-path duty-cycle distortion

`ro_sampler.rs` (raven branch, PIO1 raw-samples its own RO pin at
125 Msps) proved the signal the RX SM sees is duty-distorted ~±20 ns
(rising edges late; low runs 7-8/12-13 samples, high runs 1-3/7-8 —
some high pulses shrink to ONE 8ns sample). The WIRE is pristine
(Saleae + K2L + offline decoder all agree); the skew is in the R6-1
transceiver-RO/pad path, identical on both bench boards. Feeding the
sampled streams into the emulator reproduces the historic bench garbage
(`01 12 02...`) exactly — mechanism closed. The earlier aperture story
was a red herring for this failure; uniform sync delay provably cancels
(`rx_bench_repro.rs`).

Also: the "reverted [7][7] build still misses" observation was a
PHANTOM — flash-timeline audit showed the revert never reached the
board (cached-binary flash). ALWAYS verify `Compiling <crate>` in the
flash log after a source edit.

## RESOLVED: the ±20ns was the Saleae ground clip (user's call)

Ground clip moved off the differential leg → residual skew ~±4-8ns,
opposite sign, no vanishing pulses (lows 4-5/9-10, highs 5-6/10-11 at
8ns; identical both boards; fixtures ro_sampled2_*). New timing
**[3][4][4][4]** (raven @ 350ede86) selected by the harness: bit-perfect
on BOTH regimes' real captures, 156/160 over -12..+24ns synthetic duty
× phase. Flashed on both boards.

## Bench truth (2026-07-10 ~23:20)

- Both boards accept ~85-87% of the MAIN module's frames (production
  150MHz TX, clkdiv-1.2 delta-sigma jitter smears phase per frame).
- Parked-phase 125↔125 peer frames: ~10% decode at current (bad)
  parking — NS→NA works BOTH directions but lossy; pings still red.
  Mechanism: 1ppm-matched crystals park the sub-cycle phase; an 8ns
  sample grid vs ±6ns residual duty leaves ~1-cycle margins at some
  parkings. THIS is the resynthesis spec (below).
- **Production main (shipped fast RX) answered 0 of ~70 NS from the
  boards** — production firmware needs this fix class too (both RX
  variants). Note: host-side tshark can NOT arbitrate the bus (lan865x
  MAC-filters other nodes' multicast even in promiscuous mode).

## The frontier: phase-invariant RX (flagship, spec now concrete)

Decode DME on an 8ns grid under duty ±8ns (design for ±24) × all
parked phases × aperture. Fixtures: ro_sampled_* (clip era, extreme),
ro_sampled2_* (current), `distort()` + phase sweep in rx_bench_repro.rs.
Ideas: falling-edge-keyed decoding (falls are the cleaner polarity),
both-polarity redundant sampling, 2-SM oversampling. The narrowing
evaluator MUST model duty + parking (aperture-only certified wrong
fixes twice). Fast-RX (150MHz) variant needs the same treatment for
production main.

## Single-SM TX — HARDWARE-VALIDATED (unchanged, 8b6755f/825d829a)

17 instr, one SM, IRQ freed; emulator-certified vs shipped pair; Saleae
wire captures structurally identical. Ping-through finale still blocked
on RX (above).

## Bench state (left running)

-0 = duty-robust pinger (PING_TARGET=-1's LL addr), -1 = duty-robust
responder, both 125 MHz from worktree `Raven-Firmware.single-sm-tx-bench`
(branch UNPUSHED). Probes `2e8a:000c-{0,1}:E66368254F694937:0`, one op
at a time. `ro_sampler` diagnostic: `cargo run --bin ro_sampler`.
Saleae automation on 127.0.0.1:10430; offline decoder
`tools/logic2-dme-decoder/decode_csv.py` (FCS-validated).

## Next

1. User: analog check (ground clip / termination), then rerun sampler.
2. Resynthesis spec + narrowing evaluator with duty/vanish model;
   iterate candidates against `rx_bench_repro.rs` fixtures.
3. After bench green: single-SM TX ping-through finale.
4. Paused: SMT len-4 probe, compress2, len-5 fleet (benchmark tier).
