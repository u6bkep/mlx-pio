//! TX stage of the 10BASE-T1S pipeline: `tx_prg_b` (encoder, emits IRQ
//! pulses) + `tx_prg_a` (line driver, turns each IRQ into a DI-pin
//! transition). Both share one PIO block (and thus IRQ 0). We push line
//! codes and observe the differential-Manchester waveform on DI.
//!
//! Run with: cargo test -p pio_harness --test tx_waveform -- --nocapture

use pio::{InstructionOperands as Op, MovDestination, MovOperation, MovSource, SetDestination};
use pio_harness::{PinCtrl, Pio, ShiftCtrl, ShiftDir};

const DI: u8 = 2; // data line (tx_a side-set) -- also the RX input later
const DE: u8 = 3; // driver enable (tx_b side-set, tx_a jmp pin)

const TX_A_SM: usize = 0;
const TX_B_SM: usize = 1;

/// Build both TX state machines on PIO block 0, faithful to dme_pio.rs.
fn build_tx() -> (Pio, Pio) {
    // tx_a: line driver. side-set drives DI; jmp pin is DE.
    let prog_a = pio::pio_asm!(
        "
            .side_set 1 opt
            low:
            .wrap_target
                wait 1 irq 0 side 1
                jmp PIN high
            .wrap
            high:
                wait 1 irq 0 side 0
                jmp low
        "
    );
    // tx_b: encoder. side-set drives DE; emits irq 0 pulses.
    let prog_b = pio::pio_asm!(
        "
            .side_set 1 opt
            .wrap_target
            active_pull:
                pull
                mov x osr
                jmp x!=y bit_start
                irq set 0 [7]
                nop [4]
                nop side 0 [7]
                irq set 0
            idle_wait:
                pull side 0 [2]
                set x 2 side 1
            pre_loop:
                jmp x-- pre_loop [7]
            bit_gap:
                nop [2]
            bit_start:
                irq set 0 side 1
                out x 1
                jmp x-- bit_one [2]
            bit_zero:
                jmp next_bit
            bit_one:
                irq set 0
            next_bit:
                jmp !OSRE bit_gap
            .wrap
        "
    );

    let code_a: Vec<u16> = prog_a.program.code.iter().copied().collect();
    let code_b: Vec<u16> = prog_b.program.code.iter().copied().collect();
    assert_eq!(code_a.len(), 4, "tx_a is 4 instructions");
    assert_eq!(code_b.len(), 17, "tx_b is 17 instructions");

    // tx_a and tx_b SHARE PIO block 0's instruction memory (they share
    // IRQ 0, which is per-block). They must occupy distinct offsets:
    // tx_a at [0,4), tx_b at [4,21).
    const TX_A_OFF: u8 = 0;
    const TX_B_OFF: u8 = 4;

    // tx_a on SM0.
    let mut tx_a = Pio::new(0, TX_A_SM);
    tx_a.load_at(TX_A_OFF, &code_a, prog_a.program.wrap.target, prog_a.program.wrap.source);
    tx_a.jmp_pin(DE);
    tx_a.pinctrl(PinCtrl {
        sideset_base: DI,
        sideset_count: prog_a.program.side_set.bits(),
        ..Default::default()
    });
    tx_a.sideset(prog_a.program.side_set.optional(), prog_a.program.side_set.pindirs());
    tx_a.clkdiv(1, 0);

    // tx_b shares tx_a's emulator (same block 0, shared IRQ 0).
    let mut tx_b = Pio::from_shared(tx_a.emulator(), 0, TX_B_SM);
    tx_b.load_at(TX_B_OFF, &code_b, prog_b.program.wrap.target, prog_b.program.wrap.source);
    tx_b.pinctrl(PinCtrl {
        sideset_base: DE,
        sideset_count: prog_b.program.side_set.bits(),
        ..Default::default()
    });
    tx_b.sideset(prog_b.program.side_set.optional(), prog_b.program.side_set.pindirs());
    tx_b.shiftctrl(ShiftCtrl {
        autopull: false,
        out_dir: ShiftDir::Right,
        pull_threshold: 5,
        fjoin_tx: true,
        ..Default::default()
    });
    // clk_sys/10/baud = (150e6/10)/12.5e6 = 1.2  -> int 1, frac 0.2*256 = 51
    tx_b.clkdiv(1, 51);

    // Init: Y = SILENCE pattern (identical exec sequence to firmware).
    tx_b.exec(Op::SET { destination: SetDestination::Y, data: 0x1F }.encode()); // set y,1F
    for _ in 0..4 {
        tx_b.exec(0x4048); // in y, 8
    }
    tx_b.exec(
        Op::MOV {
            destination: MovDestination::Y,
            op: MovOperation::None,
            source: MovSource::ISR,
        }
        .encode(),
    ); // mov y, isr

    (tx_a, tx_b)
}

#[test]
fn tx_pair_emits_dme_waveform() {
    let (mut tx_a, mut tx_b) = build_tx();

    // DI and DE are PIO outputs (pad OE on, shared across the block).
    tx_a.set_output(DI);
    tx_a.set_output(DE);

    tx_a.enable();
    tx_b.enable();

    // Push a short symbol sequence (start delimiter + a couple of data
    // codes). Codes are transmitted LSB-first, 5 bits each.
    let codes: &[(u8, &str)] = &[
        (0x18, "StartJ"),
        (0x11, "StartK"),
        (0x0B, "Data5"),
        (0x1D, "DataF"),
    ];
    for &(c, _) in codes {
        tx_b.tx_push(c as u32);
    }
    // Trace DI and DE per cycle. tx_b runs at ~1.2 clkdiv, so a 5-bit
    // code takes on the order of ~60-90 system cycles.
    let cycles = 700u64;
    let mut di = String::with_capacity(cycles as usize);
    let mut de = String::with_capacity(cycles as usize);
    for _ in 0..cycles {
        tx_a.step(); // single shared emulator; either handle steps it
        di.push(if tx_a.gpio(DI) { '#' } else { '_' });
        de.push(if tx_a.gpio(DE) { '#' } else { '_' });
    }

    println!("codes pushed: {:?}", codes.iter().map(|c| c.1).collect::<Vec<_>>());
    println!("DI: {di}");
    println!("DE: {de}");
    println!(
        "tx_a pc_visits[0..4]={:?}  tx_b stall={}",
        &tx_a.pc_visits()[..4],
        tx_b.stall_cycles()
    );

    // Sanity: the line actually toggled (DME produces many transitions),
    // and the driver-enable asserted.
    let di_edges = di.as_bytes().windows(2).filter(|w| w[0] != w[1]).count();
    assert!(di_edges > 8, "expected many DI transitions, got {di_edges}");
    assert!(de.contains('#'), "DE (driver enable) should assert during TX");
}
