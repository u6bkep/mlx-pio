//! Golden test: port of the 10BASE-T1S hardware-timestamp PIO program
//! from `rs485-eth/src/dme_pio.rs` (`TimestampCapture`).
//!
//! The program keeps X as a free-running 32-bit down-counter ("the
//! clock"). It waits for the line to be idle-high for ~31 samples, arms,
//! then captures X at the next falling edge and autopushes it to the RX
//! FIFO. We drive a synthetic line waveform and assert on the captured
//! timestamps.
//!
//! Run with:  cargo test -p pio_harness -- --nocapture

use pio::{InstructionOperands, MovDestination, MovOperation, MovSource, SetDestination};
use pio_harness::{Pio, PinCtrl, ShiftCtrl, ShiftDir};

const RO_PIN: u8 = 10; // the receive line the SM watches

fn build() -> (Pio, usize) {
    // Exact program from dme_pio.rs (the idle-detection variant).
    let prog = pio::pio_asm!(
        "
         .wrap_target
         tick:
             jmp x-- tick1
         tick1:
             jmp pin high_sample
             mov y osr
             jmp tick
         high_sample:
             jmp y-- tick [1]
         wait_fall:
             jmp x-- wf1
         wf_high:
             jmp wait_fall [1]
         wf1:
             jmp pin wf_high
             in x 32
             mov y osr
         .wrap
        "
    );
    let code: Vec<u16> = prog.program.code.iter().copied().collect();
    let len = code.len();

    let mut pio = Pio::new(0, 0);
    pio.load(&code);
    let w = prog.program.wrap;
    pio.wrap(w.target, w.source);
    pio.jmp_pin(RO_PIN);
    pio.pinctrl(PinCtrl {
        in_base: RO_PIN,
        ..Default::default()
    });
    pio.shiftctrl(ShiftCtrl {
        autopush: true,
        in_dir: ShiftDir::Right,
        push_threshold: 32,
        fjoin_rx: true,
        ..Default::default()
    });
    pio.clkdiv(1, 0);

    // Init sequence — identical to the firmware's exec_instr calls:
    //   set x,31 ; mov x,~null (x=0xFFFFFFFF) ; set y,31 ;
    //   mov osr,y (reload=31) ; set y,31
    pio.exec(
        InstructionOperands::SET { destination: SetDestination::X, data: 31 }.encode(),
    );
    pio.exec(
        InstructionOperands::MOV {
            destination: MovDestination::X,
            op: MovOperation::Invert,
            source: MovSource::NULL,
        }
        .encode(),
    );
    pio.exec(
        InstructionOperands::SET { destination: SetDestination::Y, data: 31 }.encode(),
    );
    pio.exec(
        InstructionOperands::MOV {
            destination: MovDestination::OSR,
            op: MovOperation::None,
            source: MovSource::Y,
        }
        .encode(),
    );
    pio.exec(
        InstructionOperands::SET { destination: SetDestination::Y, data: 31 }.encode(),
    );

    pio.set_pin(RO_PIN, false); // line starts low
    pio.enable();
    (pio, len)
}

/// Drive: idle-high for `idle` cycles (to arm), then low to create a
/// falling edge; step until a capture lands. Returns the captured word.
fn capture_one_frame(pio: &mut Pio, idle: u64) -> u32 {
    let before = pio.rx_push_success();
    pio.set_pin(RO_PIN, true);
    pio.steps(idle); // hold idle-high long enough to arm + loop in wait_fall
    pio.set_pin(RO_PIN, false); // falling edge
    let armed = pio
        .step_until(64, |p| p.rx_push_success() > before)
        .expect("expected a capture within 64 cycles of the falling edge");
    let _ = armed;
    pio.rx_pop().expect("a word should be in the RX FIFO")
}

#[test]
fn timestamp_program_assembles_to_10_instructions() {
    let (_pio, len) = build();
    // The firmware comment promises 10 instructions; this is the
    // instruction-memory-budget signal a closed loop would track.
    assert_eq!(len, 10, "timestamp program must fit in 10 instructions");
}

#[test]
fn captures_one_timestamp_per_falling_edge_and_counts_down() {
    let (mut pio, _len) = build();

    // No capture should occur while the line never goes idle-high.
    pio.set_pin(RO_PIN, false);
    pio.steps(200);
    assert_eq!(
        pio.rx_push_success(),
        0,
        "no capture without an idle-high period + falling edge"
    );

    // Frame 1.
    let t1 = capture_one_frame(&mut pio, 200);
    // Frame 2 (re-arm: idle-high again, then falling edge).
    let t2 = capture_one_frame(&mut pio, 200);
    // Frame 3.
    let t3 = capture_one_frame(&mut pio, 200);

    println!("captured timestamps: t1=0x{t1:08X} t2=0x{t2:08X} t3=0x{t3:08X}");
    println!(
        "diagnostics: pushes={} drops={} stalls={}",
        pio.rx_push_success(),
        pio.rx_fifo_drops(),
        pio.stall_cycles()
    );

    // Exactly three captures total.
    assert_eq!(pio.rx_push_success(), 3, "one capture per falling edge");
    assert_eq!(pio.rx_fifo_drops(), 0, "no dropped captures");

    // X counts DOWN from 0xFFFFFFFF; with only a few hundred ticks each
    // frame, the high bits stay set and successive frames strictly
    // decrease (a real, monotonic free-running clock).
    assert!(t1 > 0xFFFF_0000, "t1 near top of counter: 0x{t1:08X}");
    assert!(t2 < t1, "clock must advance (count down): t2 < t1");
    assert!(t3 < t2, "clock must advance (count down): t3 < t2");

    // The program should never stall (autopush to a non-full RX FIFO).
    assert_eq!(pio.stall_cycles(), 0, "timestamp SM should not stall");
}

#[test]
fn no_capture_if_idle_period_too_short_to_arm() {
    let (mut pio, _len) = build();

    // Pulse high for only a handful of cycles — not enough high_sample
    // iterations to decrement Y to zero and arm — then drop low.
    for _ in 0..5 {
        pio.set_pin(RO_PIN, true);
        pio.steps(4);
        pio.set_pin(RO_PIN, false);
        pio.steps(4);
    }
    assert_eq!(
        pio.rx_push_success(),
        0,
        "short high pulses must not arm a capture"
    );
}
