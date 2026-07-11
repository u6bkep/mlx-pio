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

## Fix state (raven `single-sm-tx-bench` @ 5a06f0eb, both boards flashed)

Polarity-asymmetric retiming [4][1][9][4] (low-side test ~fall+80ns,
high-side rise+40ns) + BitRealigner software re-frame: 7/7 sampled
captures decode in the harness (3 bit-perfect, rest >97%) vs 0/7 for
shipped AND the earlier aperture patch. Bench: board -1 went ~2 → 533
CRC-valid frames/min; first-ever cross-board NS decode + NA reply.
**Pings still red (0/71):** residual ~2% symbol errors (vanished
one-sample pulses — unrecoverable by any retiming) → P(clean 86-byte
frame) ≈ 0.98^172 ≈ 3%. Short frames (PTP 58B) decode often.

## The frontier: beat the 2% floor

1. **Analog question (user hands):** is the ±20ns skew intrinsic to the
   R6-1 (transceiver/RC on RO) or bench wiring (Saleae ground clip on
   one differential leg; termination/bias)? Unclip/rewire and rerun
   `ro_sampler` (2 min). If intrinsic, hardware ticket.
2. **RX resynthesis (THE flagship superopt target, now with real spec):**
   decode DME under {duty ±24ns, phase, aperture, vanished high pulses}.
   Fixtures = `pio_harness/tests/data/ro_sampled_*` (real signals, both
   boards) + `rx_bench_repro.rs` `distort()` model. Falling edges are
   trustworthy (lows never vanish) — an edge-position decoder keyed on
   falls may be phase/duty-invariant. Narrowing-engine evaluator must
   model duty + vanishing pulses (aperture/sync-delay alone certified a
   WRONG fix twice).
3. Main firmware module is now on the bus (production 150MHz, PTP +
   100ms telemetry) — realistic noise + a production peer to test
   against; all its frames decode FCS-clean offline.

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
