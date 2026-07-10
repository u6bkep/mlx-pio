//! Diagnostic: slow-RX decode margin vs TX/RX relative phase and rate.
//!
//! Bench observation (2026-07-10): at 125 MHz both boards' RX decode their
//! OWN echo (TX and RX share a crystal -> edges phase-locked to the RX
//! sample grid) but fail to decode the OTHER board's frames (independent
//! crystals -> edges sweep through all phases). The Saleae shows the wire
//! waveform is perfect. Hypothesis: the slow RX variant's sampling
//! points sit close enough to the mid-bit transition that some phases
//! misdecode.
//!
//! The emulator quantizes phase to whole sys cycles, so we probe by
//! (a) delaying RX enable by 0..10 cycles at TX clkdiv exactly 1.0
//!     (each offset = one 8ns-equivalent phase step of the 80ns bit), and
//! (b) giving TX a tiny fractional clkdiv so edges precess through the
//!     RX grid within one frame (drift compressed in time).
//!
//! Run: cargo test -p pio_harness --test rx_diag -- --nocapture

use pio::{InstructionOperands as Op, MovDestination, MovOperation, MovSource, SetDestination};
use pio_harness::{PinCtrl, Pio, ShiftCtrl, ShiftDir};

const DI: u8 = 2;
const DE: u8 = 3;

fn init_y_silence(sm: &mut Pio) {
    sm.exec(Op::SET { destination: SetDestination::Y, data: 0x1F }.encode());
    for _ in 0..4 {
        sm.exec(0x4048); // in y, 8
    }
    sm.exec(
        Op::MOV {
            destination: MovDestination::Y,
            op: MovOperation::None,
            source: MovSource::ISR,
        }
        .encode(),
    );
}

/// The shipped two-SM pair (as in dme_pio.rs / tx_single_sm.rs).
fn build_pair(clkdiv_int: u16, clkdiv_frac: u8) -> (Pio, Pio) {
    let prog_a = pio::pio_asm!(
        ".side_set 1 opt
         low:
         .wrap_target
             wait 1 irq 0 side 1
             jmp PIN high
         .wrap
         high:
             wait 1 irq 0 side 0
             jmp low"
    );
    let prog_b = pio::pio_asm!(
        ".side_set 1 opt
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
         .wrap"
    );
    let ca: Vec<u16> = prog_a.program.code.iter().copied().collect();
    let cb: Vec<u16> = prog_b.program.code.iter().copied().collect();

    let mut tx_a = Pio::new(0, 0);
    tx_a.load_at(0, &ca, prog_a.program.wrap.target, prog_a.program.wrap.source);
    tx_a.jmp_pin(DE);
    tx_a.pinctrl(PinCtrl {
        sideset_base: DI,
        sideset_count: prog_a.program.side_set.bits(),
        ..Default::default()
    });
    tx_a.sideset(prog_a.program.side_set.optional(), prog_a.program.side_set.pindirs());
    tx_a.set_output(DI);
    tx_a.set_output(DE);
    tx_a.clkdiv(1, 0);

    let mut tx_b = Pio::from_shared(tx_a.emulator(), 0, 1);
    tx_b.load_at(4, &cb, prog_b.program.wrap.target, prog_b.program.wrap.source);
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
    tx_b.clkdiv(clkdiv_int, clkdiv_frac);
    init_y_silence(&mut tx_b);

    (tx_a, tx_b)
}

/// Slow RX variant (as certified in tx_single_sm.rs), 31 instructions.
fn build_rx_slow(emu: std::rc::Rc<std::cell::RefCell<rp2350_emu::Emulator>>, block: usize) -> Pio {
    let prog = pio::pio_asm!(
        "wait_for_low_stall:
             nop
         wait_for_low:
         loop:
             jmp pin loop1
             jmp found_low
         loop1:
             jmp pin loop2
             jmp found_low
         loop2:
             jmp pin loop3
             jmp found_low
         loop3:
             jmp pin loop4
             jmp found_low
         loop4:
             jmp pin loop5
             jmp found_low
         loop5:
             jmp pin low_wait_timeout [1]
         found_low:
             in x 1
             set x 1 [1]
             jmp pin wait_for_low_stall
         .wrap_target
         wait_for_high:
             wait 1 pin 0
             mov x ~ x
             in x 1
             set x 0 [2]
             jmp pin wait_for_low
         .wrap
         low_wait_timeout:
             mov isr osr
             push
             irq set 0
         startup_search_high_cont:
             wait 0 pin 0 [5]
         startup_search_low:
             jmp pin startup_foundit_next_low [2]
             wait 1 pin 0 [5]
         startup_search_high:
             jmp pin startup_search_high_cont [2]
         startup_foundit_next_high:
             set X 0 [7]
             jmp wait_for_low_stall [7]
         startup_foundit_next_low:
             set X 1 [3]
             jmp wait_for_high [7]"
    );
    let code: Vec<u16> = prog.program.code.iter().copied().collect();
    assert_eq!(code.len(), 31);

    let mut rx = Pio::from_shared(emu, block, 0);
    rx.load_at(0, &code, prog.program.wrap.target, prog.program.wrap.source);
    rx.jmp_pin(DI);
    rx.pinctrl(PinCtrl { in_base: DI, ..Default::default() });
    rx.shiftctrl(ShiftCtrl {
        autopush: true,
        in_dir: ShiftDir::Left,
        push_threshold: 5,
        fjoin_rx: true,
        ..Default::default()
    });
    rx.clkdiv(1, 0);
    rx.exec(0xE03F); // set x, 1F
    rx.exec(0xA0E1); // mov osr, x
    rx
}

fn decode_sym(v: u8) -> &'static str {
    match v & 0x1F {
        0x0F => "0", 0x12 => "1", 0x05 => "2", 0x15 => "3", 0x0A => "4", 0x1A => "5",
        0x0E => "6", 0x1E => "7", 0x09 => "8", 0x19 => "9", 0x0D => "A", 0x1D => "B",
        0x0B => "C", 0x1B => "D", 0x07 => "E", 0x17 => "F",
        0x00 => "Q", 0x1F => "I", 0x04 => "H", 0x03 => "J", 0x11 => "K", 0x16 => "T",
        0x1C => "R", 0x10 => "V", _ => "?",
    }
}

fn reframe_and_decode(rx_syms: &[u8]) -> Vec<&'static str> {
    let mut bits = Vec::new();
    for &s in rx_syms {
        for i in (0..5).rev() {
            bits.push((s >> i) & 1);
        }
    }
    let group = |off: usize| -> Vec<u8> {
        bits[off..]
            .chunks(5)
            .filter(|c| c.len() == 5)
            .map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (b << (4 - i))))
            .collect()
    };
    let mut best: (usize, Vec<&str>) = (0, Vec::new());
    for off in 0..5 {
        let labels: Vec<&str> = group(off).iter().map(|&c| decode_sym(c)).collect();
        if let Some(p) = labels.iter().position(|&l| l == "K") {
            let after = &labels[p + 1..];
            let known = after.iter().take_while(|&&l| l != "?").count();
            if known > best.0 {
                best = (known, after.to_vec());
            }
        }
    }
    best.1
}

/// One round trip; TX runs at (tx_int, tx_frac)·(base), the tx_a toggler
/// and RX at (rx_int, rx_frac). Returns (matched, total) data codes.
/// Scaling all divisors up by ~10x lets us set relative TX/RX rate
/// offsets down to ~hundreds of ppm (real crystal territory) while the
/// programs see identical per-cycle behavior.
fn trial2(
    tx_int: u16,
    tx_frac: u8,
    toggler_int: u16,
    rx_int: u16,
    rx_frac: u8,
    rx_delay: u64,
) -> (usize, usize) {
    let (mut tx_a, mut tx_b) = build_pair(1, 0);
    // Override divisors: tx_b encoder base is 1.0 at "125MHz"; scale.
    tx_b.clkdiv(tx_int, tx_frac);
    tx_a.clkdiv(toggler_int, 0);
    let mut rx = build_rx_slow(tx_a.emulator(), 1);
    rx.clkdiv(rx_int, rx_frac);
    tx_a.enable();
    tx_b.enable();
    // Phase-shift RX relative to TX by starting it later.
    for _ in 0..rx_delay {
        tx_a.step();
    }
    rx.enable();

    let payload: &[u32] = &[
        0x18, 0x11, 0x1E, 0x09, 0x14, 0x15, 0x0A, 0x0B, 0x0E, 0x0F,
        0x12, 0x13, 0x16, 0x17, 0x1A, 0x1B, 0x1C, 0x1D, 0x0D,
    ];
    let sent: Vec<&str> = vec![
        "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "A", "B", "C", "D", "E", "F",
    ];
    let mut words = vec![0x1F1F_1F1Fu32, 0x1F1F_1F1F];
    words.extend_from_slice(payload);
    words.push(0x1F1F_1F1F);

    let mut i = 0;
    let mut syms: Vec<u8> = Vec::new();
    let cycles = 12000 * (tx_int as usize);
    for _ in 0..cycles {
        if i < words.len() && !tx_b.tx_full() {
            tx_b.tx_push(words[i]);
            i += 1;
        }
        tx_a.step();
        if let Some(w) = rx.rx_pop() {
            syms.push((w & 0x1F) as u8);
        }
    }
    let decoded = reframe_and_decode(&syms);
    let matched = decoded
        .iter()
        .zip(&sent)
        .filter(|(a, b)| a == b)
        .count();
    (matched, sent.len())
}

#[test]
fn rx_phase_margin_sweep() {
    println!("== RX enable-delay sweep, exact rate lock (TX=RX=10.0) ==");
    for d in 0..10u64 {
        let (m, t) = trial2(10, 0, 10, 10, 0, d * 10);
        println!("  rx_delay={d}: {m}/{t} data codes");
    }
    println!("== relative rate sweep: RX clkdiv 10+f/256 vs TX 10.0 ==");
    println!("   (f=1 -> 390ppm, f=2 -> 780ppm, f=4 -> 1560ppm, f=8 -> 3125ppm)");
    for f in [1u8, 2, 4, 8] {
        let (m, t) = trial2(10, 0, 10, 10, f, 0);
        println!("  rx slower by {f}/2560: {m}/{t} data codes");
    }
    println!("== reverse: TX 10+f/256, RX 10.0 ==");
    for f in [1u8, 2, 4, 8] {
        let (m, t) = trial2(10, f, 10, 10, 0, 0);
        println!("  tx slower by {f}/2560: {m}/{t} data codes");
    }
}
