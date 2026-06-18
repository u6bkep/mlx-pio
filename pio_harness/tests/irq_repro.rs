//! Does a cross-SM `wait irq` / `irq set` handshake resolve in the
//! emulator? Progress is measured by PC advancement (NOT pin value:
//! `shared_pin_values` defaults to all-ones, so an output pin reads high
//! before `set pins` runs).

use pio_harness::{PinCtrl, Pio};

/// SM0 waits on irq0 then runs `set pins,1` (pc1) and spins (pc2).
/// `pc_visits[0] > 0` means the wait actually completed.
fn waiter_resolves(setter_src: &str) -> (bool, u8, u64) {
    let waiter = pio::pio_asm!(
        "
            wait 1 irq 0
            set pins, 1
        spin:
            jmp spin
        "
    );
    let wcode: Vec<u16> = waiter.program.code.iter().copied().collect();

    let mut sm0 = Pio::new(0, 0);
    sm0.load(&wcode);
    sm0.pinctrl(PinCtrl { set_base: 5, set_count: 1, ..Default::default() });

    let mut sm1 = Pio::from_shared(sm0.emulator(), 0, 1);
    // setter_src is assembled by the caller and passed as code.
    let scode = assemble(setter_src);
    sm1.load(&scode);

    sm0.enable();
    sm1.enable();

    sm0.step_until(80, |p| p.pc_visits()[0] > 0);
    (sm0.pc_visits()[0] > 0, sm0.pc(), sm0.pc_visits()[0])
}

fn assemble(which: &str) -> Vec<u16> {
    match which {
        "immediate" => pio::pio_asm!(
            "
                irq set 0
            spin:
                jmp spin
            "
        ).program.code.iter().copied().collect(),
        "delayed" => pio::pio_asm!(
            "
                nop [15]
                irq set 0
            spin:
                jmp spin
            "
        ).program.code.iter().copied().collect(),
        _ => unreachable!(),
    }
}

#[test]
fn wait_irq_resolves_flag_set_immediately() {
    let (ok, pc, visits) = waiter_resolves("immediate");
    println!("immediate: resolved={ok} sm0.pc={pc} wait_completions={visits}");
    assert!(ok, "wait irq never completed (sm0 stuck at pc={pc})");
}

#[test]
fn wait_irq_resolves_flag_set_after_stall() {
    let (ok, pc, visits) = waiter_resolves("delayed");
    println!("delayed: resolved={ok} sm0.pc={pc} wait_completions={visits}");
    assert!(ok, "wait irq never completed when flag set after stall (sm0 stuck at pc={pc})");
}
