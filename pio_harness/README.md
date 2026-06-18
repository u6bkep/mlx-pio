# pio_harness

A thin, test-oriented harness over [`rp2350-emu`](https://crates.io/crates/rp2350-emu)
for developing and validating **RP2350 PIO programs** in software.

The emulator's native interface is register-poke at datasheet addresses. This
harness hides that behind a small typed API so a test — or a coding agent — can
load a program, configure a state machine with named fields, drive GPIO
stimulus cycle-by-cycle, and observe pins, FIFOs, PC, and diagnostic counters.

## Quick example

```rust
use pio_harness::{Pio, PinCtrl};

// Square wave on GPIO0: SET PINS,1 [1] / SET PINS,0 [1]
let prog = pio::pio_asm!(".wrap_target\n set pins,1 [1]\n set pins,0 [1]\n.wrap");
let code: Vec<u16> = prog.program.code.iter().copied().collect();

let mut sm = Pio::new(0, 0);                 // PIO block 0, state machine 0
sm.load(&code);
sm.pinctrl(PinCtrl { set_base: 0, set_count: 1, ..Default::default() });
sm.set_output(0);                            // drive GPIO0
sm.enable();

let trace = sm.trace_pin(0, 8);              // step 8 cycles, capture pin
assert_eq!(trace, "##__##__");
```

## API surface

**Construction** — one emulator can be shared by several `Pio` handles (needed
for multi-SM / multi-block scenarios like cross-SM IRQ handshakes):
- `Pio::new(block, sm)` — fresh emulator, one handle, `step_quantum = 1`.
- `Pio::from_shared(emu, block, sm)` — another handle on a shared emulator.
- `pio.emulator()` — clone the shared `Rc<RefCell<Emulator>>`.

**Configuration** (typed, no bit-twiddling at call sites):
- `load(code)` — load one program at offset 0.
- `load_at(offset, code, wrap_target, wrap_source)` — load at an offset,
  relocating JMP/wrap targets and setting the start PC. **Required when multiple
  SMs share a block** (see Gotchas).
- `pinctrl(PinCtrl { in_base, out_base/out_count, set_base/set_count, sideset_base/sideset_count })`
- `shiftctrl(ShiftCtrl { autopush, autopull, push_threshold, pull_threshold, in_dir, out_dir, fjoin_rx, fjoin_tx })`
- `sideset(opt, pindir)`, `jmp_pin(pin)`, `wrap(bottom, top)`, `clkdiv(int, frac)`
- `exec(insn)` — force-execute an instruction (the `exec_instr` setup idiom).
- `set_output(pin)` — mark a pin as a PIO output (force-executes `SET PINDIRS`).
- `enable()` — enable the SM (forces PC to the program start offset first).

**Stimulus & stepping:**
- `set_pin(pin, hi)` — drive a low-bank GPIO input.
- `step()`, `steps(n)`, `step_until(max, pred)`.

**Observation:**
- `gpio(pin)`, `trace_pin(pin, cycles)` → `_`/`#` string.
- `pc()`, `isr()`, `pc_visits()` (per-instruction hit histogram), `stall_cycles()`.
- `rx_empty()`, `rx_level()`, `rx_pop()`, `rx_push_success()`, `rx_fifo_drops()`.
- `tx_full()`, `tx_push(word)`.

## Gotchas (real PIO subtleties the harness encodes or exposes)

- **Instruction memory is per *block*** — all 4 SMs in a block share one 32-word
  memory. Two programs in the same block must occupy different offsets; use
  `load_at`, which relocates JMP/wrap targets. (`tx_a`+`tx_b` share a block
  because IRQ flags are per-block.)
- **Output pins read high before they're driven** — `shared_pin_values` resets
  to all-ones, so a pin reads `1` until a `SET`/side-set writes it. Init pin
  values if you assert default-low.
- **Input merge is end-of-step** — a `set_pin` becomes visible to the SM on the
  *next* `step()`, so stimulus must be cycle-aligned to the sampling instruction.
- **TX FIFO is shallow** (4, or 8 joined) — feed long streams incrementally
  (push when `!tx_full()`), mimicking DMA; pushing everything up front drops data.

## Tests

Run `cargo test -- --nocapture` to see the waveforms and traces.

| Test file | What it validates |
|-----------|-------------------|
| `tests/timestamp.rs` | The 10BASE-T1S hardware-timestamp SM: free-running counter, idle detection, capture-on-falling-edge, autopush. Golden assertions (monotonic counter, one capture per edge, instruction-budget check). |
| `tests/tx_waveform.rs` | The TX pair (`tx_b` encoder + `tx_a` line driver) on one block: cross-SM IRQ handshake + side-set + threshold-5 autopull → a differential-Manchester waveform on the data pin. |
| `tests/round_trip.rs` | **The full pipeline.** `clean_round_trip` sends 16 data line codes through `tx_b → tx_a → DME → wire → rx` and asserts they're recovered 1:1. Plus diagnostics: per-code symbol map, DME bit-period measurement, DME continuity, symbol-framing reconciliation. The "wire" is the emulator's GPIO merge (DI and RO on the same GPIO). |
| `tests/irq_repro.rs`, `tests/irq_trace.rs` | Minimal cross-SM `wait irq`/`irq set` handshake — the reproduction and regression for the emulator `WAIT IRQ` bug fixed in `vendor/`. |

## Dependency on the patched emulator

`Cargo.toml` has a `[patch.crates-io]` entry pointing `picoem-common` at the
patched copy in `../vendor/picoem-common`. Without that patch, the `WAIT IRQ`
bug makes the cross-SM TX handshake deadlock. See
[`../vendor/picoem-common/PATCH.md`](../vendor/picoem-common/PATCH.md).
