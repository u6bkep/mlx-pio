# Local patch: `picoem-common` 0.2.1

This is a vendored copy of [`picoem-common`](https://crates.io/crates/picoem-common)
0.2.1 (the PIO core of the `rp2350-emu` / `0x4D44/picoem` emulator), with one
**correctness fix** and accompanying **regression tests**. `pio_harness`
consumes it via `[patch.crates-io]`.

Not yet filed upstream.

## The bug: `WAIT IRQ` that stalls can never complete

A `WAIT n IRQ` instruction that *stalls* (the flag isn't set yet) and is *later*
satisfied never completes — it consumes the flag and re-stalls forever. It only
works if the flag is already set on the wait's very first (non-stalled)
execution, making it execution-order-dependent and breaking normal cross-SM IRQ
handshakes (e.g. one SM does `irq set 0`, another does `wait 1 irq 0`).

### Root cause

The emulator resolves a stall by having `check_stall()` report "no longer
stalled", then `execute_cycle()` falls through and **re-executes** the parked
instruction. For self-clearing waits this double-processes: `check_stall`'s
`WaitIrq` arm *cleared the flag*, and then the re-executed `exec_wait` saw it
cleared and stalled again.

### The fix (`src/pio/sm.rs`, `check_stall`)

Make the `WaitIrq` arm a *pure predicate* — report whether the stall persists,
without mutating the flag. The single clear-and-complete is then owned solely by
`exec_wait` on re-execute (which already handled the never-stalled case
correctly):

```rust
// before
StallKind::WaitIrq { polarity, index } => {
    let flag_set = (*irq_flags >> (index & 7)) & 1 != 0;
    if flag_set == polarity {
        *irq_flags &= !(1 << (index & 7));   // <-- buggy side effect
        false
    } else { true }
}
// after
StallKind::WaitIrq { polarity, index } => {
    let flag_set = (*irq_flags >> (index & 7)) & 1 != 0;
    flag_set != polarity
}
```

### Datasheet correctness

Verified against RP2350 datasheet §11.4.3.2 (a satisfied `WAIT` retires like a
1-cycle instruction; `WAIT 1 IRQ` clears the selected flag exactly once on
satisfaction) and §11.2.5 (a stalled instruction does not advance PC). The fix
clears the flag exactly once, on the resolving cycle, and advances — matching
silicon. The `WaitGpio`/`WaitPin`/`Pull`/`Push`/`IrqWait` arms are unchanged
(none is self-clearing).

### Regression tests added

- `src/pio/sm.rs::wait_irq_completes_after_stall_then_flag_set` — bug demo:
  fails on the pre-fix code, passes after.
- `src/pio/sm.rs::wait_irq_clears_flag_exactly_once_and_rearms` — flag consumed
  exactly once; re-arms correctly.
- `src/pio/mod.rs::two_sm_wait_irq_handshake_completes_regardless_of_order` —
  block-level 2-SM handshake (programs at distinct instruction-memory offsets),
  exercising the order-dependent case.

All `picoem-common` tests pass (314 baseline + 3 added = 317).

## The bug: `CLKDIV_INT == 0` treated as divide-by-1 instead of divide-by-65536

`clock_tick()` computed the fractional clock-divider threshold with a special
case treating `clkdiv_int == 0` as divisor 1 (`threshold = 256`, the *fastest*
divisor). This is backwards: a zero integer divisor is the *slowest* divisor.

### Root cause

The RP2350 datasheet (§11, `SMx_CLKDIV` register, INT field) states:

> "Value of 0 is interpreted as 65536. If INT is 0, FRAC must also be 0."

So `clkdiv_int == 0` means divisor 65536, i.e. `threshold = 65536 * 256 =
16_777_216` (FRAC guaranteed 0, so no frac term). The emulator instead used
`threshold = 256`, making such an SM run 65536× too fast.

### The fix (`src/pio/sm.rs`, `clock_tick`)

```rust
// before
let threshold = if self.clkdiv_int == 0 {
    256u32
} else { ... };
// after
let threshold = if self.clkdiv_int == 0 {
    65536u32 * 256
} else { ... };
```

`65536 * 256 = 16_777_216` fits in `u32`; `clkdiv_acc` (u32, +256/tick) reaches
it without overflow. The `else` arm and the rest of the accumulator logic are
unchanged.

### Regression tests updated

The two existing tests that baked in the wrong divide-by-256 behavior were
corrected (not deleted) to assert divide-by-65536:

- `src/pio/sm.rs::clock_tick_treats_int_zero_as_65536` (renamed from
  `clock_tick_treats_int_zero_as_256`) — fires exactly once per 65536 cycles.
- `src/pio/mod.rs::clkdiv_int_zero_through_block_divides_by_65536` (renamed from
  `clkdiv_int_zero_through_block_ticks_every_cycle`) — 4 ticks per 4*65536
  cycles through the full `PioBlock` path.

## The bug: opt side-set overlay clobbers same-pin `OUT`/`SET` writes

`PioBlock::merge_pin_outputs` overlaid each state machine's *latched* side-set
value (`sm.sideset_pins` / `sm.sideset_dirs`) onto the side-set pins **every
cycle, unconditionally** — ignoring the per-instruction opt side-set enable bit
(`.side_set N opt`). An instruction whose enable bit is clear performs **no**
side-set that cycle, yet the merge still re-applied the stale latched value.

When side-set and `OUT`/`SET` map to the **same physical pin** and an
instruction opts OUT of side-set, the overlay overwrote what `OUT`/`SET` had
just written. The canonical pico `uart_tx` (side-set drives start/stop framing,
`OUT` drives the data bit, **same pin**, opt-out on the `OUT`) was
mis-emulated: every data bit got stuck at the latched framing level, so the
line transmitted all-zero (or all-framing) data. Putting side-set and `OUT` on
different pins masked the bug.

### Correct semantics (RP2350)

Side-set, when **asserted** (enable=1 in opt mode, or always in non-opt mode),
writes its value into the pin's output register — the *same* register `OUT`/
`SET` write — so it persists until something writes it again. When an
instruction does **not** assert side-set, side-set does nothing that cycle and
the pins **HOLD** their current value (whether last written by side-set or by
`OUT`/`SET`). Side-set is not a separate always-overlaid latch; it is a
conditional write into the shared pin state.

### The fix

- `src/pio/sm.rs`, `apply_sideset`: now takes the shared latches and, when
  side-set is asserted (`decoded.sideset == Some`), writes the value into
  `shared_pin_values` (or `shared_pin_dirs` when SIDE_PINDIR) — the same latch
  `OUT`/`SET`/`MOV` write. It is invoked **after** `execute_insn` (was before),
  so an asserted side-set takes precedence on any shared pin (RP2350 §3.5.1)
  while an opted-out instruction leaves the `OUT`/`SET` write — and the latched
  value — intact. The per-SM `sideset_pins` / `sideset_dirs` fields are retained
  only as a diagnostic mirror.
- `src/pio/mod.rs`, `merge_pin_outputs`: the unconditional per-SM side-set
  overlay loop (and its `any_sideset_programmed` short-circuit) is removed. The
  merge is now a pure copy of the shared latches. `any_sideset_programmed` /
  `recompute_any_sideset` are retained as a maintained cache (no longer
  consulted by the merge).

```rust
// before (merge_pin_outputs, per cycle, for every enabled SM):
out = (out & !positioned_mask) | (sm.sideset_pins & positioned_mask); // value-drive
oe  = (oe  & !positioned_mask) | (sm.sideset_dirs & positioned_mask); // dir-drive
// after: nothing — apply_sideset already wrote shared_pin_values/dirs on assert.
```

### Regression tests added

- `src/pio/mod.rs::opt_sideset_out_same_pin_data_reaches_pin` — UART-like
  repro: side-set framing + `OUT` data on the **same** pin with opt-out; the
  LSB-first data bits of `0x0D` must appear on the pin. **Fails on the pre-fix
  code** (data bit 1 stuck at the framing level), passes after.
- `src/pio/mod.rs::opt_sideset_value_holds_across_opt_out_nop` — HOLD: an
  asserted side-set drives the pin, then opt-out NOPs must hold that value.
- `src/pio/mod.rs::merge_pin_outputs_side_en_and_side_pindir_arms` — reworked
  (no longer pre-seeds the removed latch overlay): an asserted SIDE_PINDIR
  side-set drives `pad_oe` via `shared_pin_dirs`.
- `src/pio/sm.rs::sideset_with_side_pindir_updates_sideset_dirs` — updated to
  assert the shared direction latch (and the mirror) after `apply_sideset`.

The DME harness tests (`pio_harness` `tx_waveform.rs`, `round_trip.rs`) use
`.side_set 1 opt` with side-set on **different** pins than `OUT` and are
unaffected — they continue to pass. All `picoem-common` tests pass (317 → 319).

## Other changes

- `src/pio/decode.rs`: added `#[derive(Debug)]` on `DecodedInsn` and `PioOp`
  (used by the regression tests / debugging). No behavioral change.
