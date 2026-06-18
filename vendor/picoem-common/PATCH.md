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

## Other changes

- `src/pio/decode.rs`: added `#[derive(Debug)]` on `DecodedInsn` and `PioOp`
  (used by the regression tests / debugging). No behavioral change.
