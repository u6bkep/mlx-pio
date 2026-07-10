# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-10 (eve).

## Targets = real rs485-eth firmware (pivot 2026-07-10)

TX pair (tx_a 4 + tx_b 17 instr), RX (31/32, "32" comment is stale),
timestamp (10). Old single-SM DME scoreboard = benchmark suite only.
Deployed config: **clk_sys 125 MHz** (TX clkdiv exactly 1.0, SLOW RX
variant) — at the embassy default 150 MHz the RX is dead on hardware.

## Single-SM TX — HARDWARE-VALIDATED (wire-level, 2026-07-10)

`mov pins, !pins` dissolves the two-SM split. 17 instr, one SM, IRQ
freed, TX PIO 31→27. Emulator proof: `pio_harness/tests/tx_single_sm.rs`
(150/133/125 MHz equivalence + slow-RX round trips). Hardware proof:
Saleae wire captures on the R6-1 bench — single-SM frames structurally
identical to stock (J J H H prefix, clean 40/80ns runs, T R trailer,
990 bits, 12.5 MBd). Ping-through validation was impossible due to the
RX bug below (affects stock TX identically — verified baseline-first).

## DISCOVERY: shipped RX has a phase-dependent bit-alignment bug

On the 2-board bench (crystals ~1-25 ppm apart) NO frames decode
cross-board while own-echo decodes fine, at 100% rate, both directions.
Saleae shows the wire is perfect; the RX recovers every BIT correctly
but its 5-bit ISR grouping is offset (bench: +1 bit; emulator: +3 at
every phase — `pio_harness/tests/rx_diag.rs`) depending on startup-lock
phase. The firmware RXProcessor never re-frames → misaligned frames are
100% garbage (silently discarded, wrong-MAC path). The slow variant's
startup paths are also mistimed vs fast (16 cycles ≈ 1.6 bits where fast
skips 2.0). Fix candidates: (a) software re-framing in rs485-eth
RXProcessor (sync on J-J-H-H across 5 offsets), (b) resynthesize the RX
startup so alignment is phase-invariant — **this is now the concrete
flagship spec for the superopt RX target** (spec must quantify over
TX/RX phase, and the emulator needs sub-cycle-phase/sync-jitter modeling
to certify it — narrowing-engine evaluator requirement).

## Raven-Firmware.main (kalogon repo) — UNCOMMITTED, user reviews

- crates/rs485-eth: feature `single-sm-tx` (default-off, tx_a unused).
- rs485_eth_test: refreshed to current APIs; 125 MHz clock (required);
  `board-pneumatics-r6-1` pins GP18-21 no LED (GP25 = VBUS FET!);
  `pinger` (PING_TARGET env, default ff02::1 — NB bare smoltcp doesn't
  answer multicast echo, use unicast); MAC from OTP chipid.
- Bench: probes `2e8a:000c-{0,1}:E66368254F694937:0` (trailing :0
  required); one probe op at a time; Saleae Logic 2 automation on
  127.0.0.1:10430 (MCP on 10530 needs session restart), scripts in
  session scratchpad (saleae_cap/analyze/baud.py — DME wire decoder).

## Next

1. Discuss with user/Christian: RX bug disposition (product impact?
   main<->pneumatics may be phase-lucky or limping on retries).
2. RX re-frame fix (software) to get bench pings green, then single-SM
   TX ping-through as icing.
3. Narrowing engine (bit-field needed-narrowing, own evaluator w/
   sub-cycle phase modeling) — first targets tx_a optimality, then the
   phase-invariant RX resynthesis.
4. Paused: SMT len-4 probe, compress2, len-5 fleet (benchmark tier).
