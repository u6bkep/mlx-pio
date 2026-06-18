# pio_optimization — software-emulated RP2350 PIO for closed-loop development

Goal: enable a coding agent (or a human) to iterate on **RP2350 PIO programs**
against a software emulator instead of hardware. PIO assembly is hand-written,
hard to test, and easy to overrun the 32-instruction-per-block memory limit.
Running it on a faithful emulator gives fast, text-observable, assertable
feedback.

The concrete driving target is a custom **10BASE-T1S** (single-pair Ethernet)
PIO implementation: a differential-Manchester (DME) TX encoder/line-driver pair
and a hand-tuned RX sampler/decoder.

## What's here

| Path | What it is |
|------|------------|
| `pio_harness/` | **The deliverable.** A thin, test-oriented harness over `rp2350-emu` plus a suite of tests that exercise real PIO programs, up to a full TX→RX round-trip. See [`pio_harness/README.md`](pio_harness/README.md). |
| `vendor/picoem-common/` | A **patched copy** of the emulator's PIO core. Contains a one-line correctness fix for a `WAIT IRQ` bug we found, plus regression tests. See [`vendor/picoem-common/PATCH.md`](vendor/picoem-common/PATCH.md). |
| `pio_emu_probe/` | The initial exploratory probe — a `main.rs` that assembles a `.pio` program, runs it, and prints a text waveform + FIFO trace. Kept as a minimal "does the emulator work for an agent?" demonstration. |
| `docs/rp2350 datasheet/` | RP2350 datasheet (markdown). Chapter 11 (PIO) is the reference for register layouts and instruction semantics. |

## Emulator survey conclusion

Several PIO emulators exist; we evaluated them for agent-fit (headless,
text/structured output, assertable):

- **`rp2350-emu`** (Rust, `0x4D44/picoem`) — chosen. Native to the target stack,
  models everything this code needs (cross-SM IRQ, side-set, autopush/autopull
  thresholds, fractional clock dividers, the GPIO merge), with rich diagnostic
  accessors (`pc_visits`, `stall_cycles`, FIFO levels, ISR state).
- `pioemu` (Python, `NathanY3G/rp2040-pio-emulator) — great for unit tests but
  no IRQ and single-SM only; can't model this TX path.
- Browser sims (`ice458`, Wokwi) — excellent for humans, no headless/assert API.

**Verdict:** `rp2350-emu`, *with the one-line `WAIT IRQ` fix in `vendor/`*, is
faithful enough that a complete 10BASE-T1S TX→RX round-trip decodes correctly in
software (all 16 data line codes recovered exactly). Suitable for closed-loop
agent iteration.

## Building & testing

```sh
cd pio_harness
cargo test            # all harness tests, incl. the full round-trip
cargo test -- --nocapture   # see the text waveforms / RX symbol traces
```

The harness depends on the patched `vendor/picoem-common` via a
`[patch.crates-io]` entry in `pio_harness/Cargo.toml`, so the fix is applied
automatically.
