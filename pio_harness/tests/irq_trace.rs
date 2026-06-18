use pio_harness::Pio;

// waiter and setter, with correct distinct offsets. `waiter_sm` chooses
// which SM index the waiter occupies (lower index executes first).
fn completions(waiter_sm: usize) -> u64 {
    let setter_sm = 1 - waiter_sm;
    let wp = pio::pio_asm!(".wrap_target\n wait 1 irq 0\n.wrap\n");
    let sp = pio::pio_asm!(".wrap_target\n irq set 0\n nop\n.wrap\n");
    let wc: Vec<u16> = wp.program.code.iter().copied().collect();
    let sc: Vec<u16> = sp.program.code.iter().copied().collect();

    let mut w = Pio::new(0, waiter_sm);
    w.load_at(0, &wc, wp.program.wrap.target, wp.program.wrap.source);
    let mut s = Pio::from_shared(w.emulator(), 0, setter_sm);
    s.load_at(2, &sc, sp.program.wrap.target, sp.program.wrap.source);

    // Enable lower index first is irrelevant; emulator steps sm0..sm3 each cycle.
    w.enable(); s.enable();
    w.steps(60);
    w.pc_visits()[0]
}

/// Regression for the `WAIT IRQ` fix: a cross-SM handshake must resolve
/// regardless of which SM index (execution order) the waiter occupies.
/// Pre-fix, the waiter-runs-first case deadlocked (0 completions).
#[test]
fn wait_irq_handshake_order_independent() {
    let before = completions(0); // waiter on sm0 (runs before setter)
    let after = completions(1); // waiter on sm1 (runs after setter)
    println!("waiter-first completions={before}  setter-first completions={after}");
    assert!(before > 10, "waiter-before-setter deadlocked ({before}) — WAIT IRQ bug regressed");
    assert!(after > 10, "waiter-after-setter failed ({after})");
}
