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

## RX bug refinement (2026-07-10 night, K2L on bus)

K2L (enp8s0u1u1u3u3) decodes board TX perfectly; boards fail against the
K2L too. Production clock pairing (150<->125) reproduced on bench: still
0 valid frames. New failure mode vs K2L: full 926-byte assemblies failing
only CRC — alignment CAN lock; residual = mid-frame bit errors. So the
RX bug is >=2-factor (phase-dependent alignment + mid-frame corruption).
Timeline: embassy bump (51dff4da, not in any release) precedes R6-1
bringup by 2 days — R6-1 only ever ran post-bump code. v0.1.3 worktree
staged at /tmp/claude-1000/rf-v013 (submodules cloned) for a released-
firmware A/B if needed. K2L PLCA config unchecked.

## RX MECHANISM REPRODUCED (waveform replay, 725ff8b)

pio_harness/tests/wave_replay.rs replays the real Saleae capture through
the emulated slow RX. Ideal sampling: decodes perfectly at EVERY phase.
With a 1.5ns sync-FF aperture model (edges near sample instants resolve
randomly): 10/16 phases corrupt every frame — only ~2.5ns clean window
per 8ns cycle. Explains everything: ~1ppm-matched bench boards PARK at
one phase for seconds (bad phase = dead link, our bench); same-clock
echo parks at a good phase (works); production 150/125 grid mismatch +
clkdiv-1.2 TX jitter smears each frame across phases (mostly-good +
retries = "fairly reliable"). Fix spec is now QUANTITATIVE: maximize
sampling distance from expected edges, phase-invariant — prime superopt
target. Raven changes now live on branch `single-sm-tx-bench`
(Raven-Firmware.single-sm-tx-bench, commit 825d829a); main is clean.
Logic2 DME decoder being built by a subagent (tools/logic2-dme-decoder).

## RX fixes implemented (branch ca183707) — PARTIAL bench success

Both fixes built + flashed: (1) PIO margin patches (found_low +1cy,
mid-bit sample 56ns; certified rx_fix.rs battery incl. 133MHz) and
(2) BitRealigner software re-frame in RXProcessor (J-H-H prefix lock at
any bit offset; validated reframe_proto.rs; replaces the all-data batch
path — perf note for Christian). MILESTONE: board -1 now decodes the
K2L's 926-byte frames WITH VALID CRC (first cross-clock accept ever on
this bench). STILL FAILING: peer pneumatics NS frames — RX captures ONLY
the trailing R symbol (wire Saleae-verified perfect). Learned the hard
way: retiming the startup foundit_next_high path ([7][7]->[9][9], which
the 133MHz battery wanted) makes hardware MISS whole frames — the
input-sync delay shifts scan-branch decisions a whole cycle vs the
emulator; reverted. Suspect the remaining mode is the startup scan vs
the pneumatics TX frame lead-in (DE-assert timing / first-transition
shape differs from K2L) under sync timing the emulator doesn't model.

## Next

1. Model the 2-FF input-sync DELAY (not just aperture) in the replay
   harness; replay the real NS capture with RX state carried across
   repeated frames (bench does frame-after-frame; replay was fresh-boot).
   Find why the scan misses pneumatics frames but not K2L frames.
2. Then: bench green -> single-SM TX ping-through finale.
3. Narrowing engine (evaluator MUST model sync delay + aperture — now
   proven load-bearing); tx_a optimality, phase-invariant RX flagship.
4. Paused: SMT len-4 probe, compress2, len-5 fleet (benchmark tier).
