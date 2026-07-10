//! Single-SM TX candidate vs the shipped two-SM TX pair.
//!
//! The shipped 10BASE-T1S TX uses two state machines: tx_b (encoder)
//! fires IRQ 0 at every required DME transition and tx_a (4-instr line
//! driver) turns each IRQ into a DI toggle, holding line polarity in its
//! program counter. tx_a's `jmp PIN` on DE also makes IRQs *while DE is
//! low* idempotent force-high writes, which absorbs the per-frame toggle
//! parity so DI always parks high between frames.
//!
//! The candidate collapses the pair into ONE state machine:
//!   - every mid-frame `irq set 0` becomes `mov pins ~pins` (DI mapped as
//!     both OUT and IN pin: pin-as-state toggle, no polarity bookkeeping),
//!   - the final parking `irq set 0` (fired after DE drops) becomes
//!     `mov pins ~null` (absolute DI-high write = the parity absorber),
//!   - side-set stays on DE, control flow and cycle schedule unchanged.
//!
//! Equivalence claim tested here, at both shipped clock configs
//! (150 MHz/RP2350: clkdiv 1+51/256; 133 MHz/RP2040: clkdiv 1+16/256):
//!   - DE traces are cycle-exact identical,
//!   - DI traces have identical edge sequences (same count, same
//!     directions, same spacing) up to one constant lag: the pair's DI
//!     edges trail the candidate's by tx_a's fixed IRQ->side-set latency.
//!
//! Run: cargo test -p pio_harness --test tx_single_sm -- --nocapture

use pio::{InstructionOperands as Op, MovDestination, MovOperation, MovSource, SetDestination};
use pio_harness::{PinCtrl, Pio, ShiftCtrl, ShiftDir};

const DI: u8 = 2; // data line
const DE: u8 = 3; // driver enable

/// Y := 0x1F1F1F1F (the SILENCE pattern as byte-lane-replicated by DMA),
/// identical exec sequence to the firmware.
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

/// The shipped two-SM pair, faithful to dme_pio.rs (as in round_trip.rs).
fn build_pair(clkdiv_frac: u8) -> (Pio, Pio) {
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
    tx_b.clkdiv(1, clkdiv_frac);
    init_y_silence(&mut tx_b);

    (tx_a, tx_b)
}

/// The single-SM candidate: tx_b's program with the IRQ toggles replaced
/// by direct pin writes. 17 instructions, one SM, no IRQ.
fn build_single(clkdiv_frac: u8) -> Pio {
    let prog = pio::pio_asm!(
        ".side_set 1 opt
         .wrap_target
         active_pull:
             pull
             mov x osr
             jmp x!=y bit_start
             mov pins ~pins [7]
             nop [4]
             nop side 0 [7]
             mov pins ~null
         idle_wait:
             pull side 0 [2]
             set x 2 side 1
         pre_loop:
             jmp x-- pre_loop [7]
         bit_gap:
             nop [2]
         bit_start:
             mov pins ~pins side 1
             out x 1
             jmp x-- bit_one [2]
         bit_zero:
             jmp next_bit
         bit_one:
             mov pins ~pins
         next_bit:
             jmp !OSRE bit_gap
         .wrap"
    );
    let code: Vec<u16> = prog.program.code.iter().copied().collect();
    assert_eq!(code.len(), 17, "single-SM TX must stay 17 instructions");

    let mut tx = Pio::new(0, 0);
    tx.load_at(0, &code, prog.program.wrap.target, prog.program.wrap.source);
    tx.pinctrl(PinCtrl {
        out_base: DI,
        out_count: 1,
        in_base: DI, // pin-as-state: mov reads DI back through the IN mapping
        sideset_base: DE,
        sideset_count: prog.program.side_set.bits(),
        ..Default::default()
    });
    tx.sideset(prog.program.side_set.optional(), prog.program.side_set.pindirs());
    tx.shiftctrl(ShiftCtrl {
        autopull: false,
        out_dir: ShiftDir::Right,
        pull_threshold: 5,
        fjoin_tx: true,
        ..Default::default()
    });
    tx.clkdiv(1, clkdiv_frac);
    tx.set_output(DI);
    tx.set_output(DE);
    // The pair parks DI high (tx_a's wait side 1 issues at enable); the
    // candidate must start from the same level for the toggle stream to
    // produce the same waveform. Preset the output latch through the SM
    // (the firmware equivalent is `sm.set_pins(Level::High)`) — NOT via
    // set_pin(), which is external stimulus and would override the SM's
    // own writes in the GPIO merge.
    tx.exec(
        Op::MOV {
            destination: MovDestination::PINS,
            op: MovOperation::Invert,
            source: MovSource::NULL,
        }
        .encode(),
    );
    init_y_silence(&mut tx);
    tx
}

/// A multi-frame code stream: silence gaps, two frames of differing
/// '1'-bit parity (so both frame-end parking parities are exercised),
/// mid-stream restart from the parked idle state.
fn test_stream() -> Vec<u32> {
    let sil = 0x1F1F_1F1Fu32;
    let mut w = vec![sil, sil];
    // Frame 1: J K 0 1 2 3 4 5 6 7 T
    for &c in &[0x18u32, 0x11, 0x1E, 0x09, 0x14, 0x15, 0x0A, 0x0B, 0x0E, 0x0F, 0x0D] {
        w.push(c);
    }
    w.push(sil);
    w.push(sil);
    // Frame 2 (shorter, different parity): J K 8 9 A T
    for &c in &[0x18u32, 0x11, 0x12, 0x13, 0x16, 0x0D] {
        w.push(c);
    }
    w.push(sil);
    w.push(sil);
    w
}

/// Step `cycles` system cycles, feeding `words` into the encoder FIFO as
/// space frees up (mimics DMA), tracing DI and DE each cycle.
fn run_traced(step_h: &mut Pio, fifo_h: &mut Pio, words: &[u32], cycles: usize) -> (String, String) {
    let mut i = 0;
    let mut di = String::with_capacity(cycles);
    let mut de = String::with_capacity(cycles);
    for _ in 0..cycles {
        if i < words.len() && !fifo_h.tx_full() {
            fifo_h.tx_push(words[i]);
            i += 1;
        }
        step_h.step();
        di.push(if step_h.gpio(DI) { '#' } else { '_' });
        de.push(if step_h.gpio(DE) { '#' } else { '_' });
    }
    assert_eq!(i, words.len(), "feed did not drain: {}/{} words pushed", i, words.len());
    (di, de)
}

/// Rising/falling edge list: (cycle index, '+' or '-').
fn edges(trace: &str) -> Vec<(usize, char)> {
    trace
        .as_bytes()
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] != w[1])
        .map(|(t, w)| (t + 1, if w[1] == b'#' { '+' } else { '-' }))
        .collect()
}

fn compare_at(clkdiv_frac: u8, label: &str) {
    let words = test_stream();
    let cycles = 8000;

    let (mut tx_a, mut tx_b) = build_pair(clkdiv_frac);
    tx_a.enable();
    tx_b.enable();
    let (di_p, de_p) = run_traced(&mut tx_a, &mut tx_b, &words, cycles);

    let mut tx = build_single(clkdiv_frac);
    tx.enable();
    // Single handle plays both roles (stepping and FIFO feeding).
    let (di_s, de_s) = {
        let mut i = 0;
        let mut di = String::with_capacity(cycles);
        let mut de = String::with_capacity(cycles);
        for _ in 0..cycles {
            if i < words.len() && !tx.tx_full() {
                tx.tx_push(words[i]);
                i += 1;
            }
            tx.step();
            di.push(if tx.gpio(DI) { '#' } else { '_' });
            de.push(if tx.gpio(DE) { '#' } else { '_' });
        }
        assert_eq!(i, words.len(), "single-SM feed did not drain");
        (di, de)
    };

    // DE: the encoder's control flow and cycle schedule are unchanged, so
    // driver-enable must match cycle-for-cycle.
    if de_p != de_s {
        let first = de_p.bytes().zip(de_s.bytes()).position(|(a, b)| a != b).unwrap();
        println!("DE pair  : ...{}", &de_p[first.saturating_sub(40)..(first + 40).min(cycles)]);
        println!("DE single: ...{}", &de_s[first.saturating_sub(40)..(first + 40).min(cycles)]);
        panic!("[{label}] DE traces diverge at cycle {first}");
    }

    // DI: same edge sequence, constant lag (pair trails by tx_a's fixed
    // IRQ->side-set latency).
    let ep = edges(&di_p);
    let es = edges(&di_s);
    println!(
        "[{label}] DI edges: pair={} single={}  DE asserts: {}",
        ep.len(),
        es.len(),
        de_p.matches('#').count()
    );
    assert_eq!(ep.len(), es.len(), "[{label}] DI edge count differs");
    assert!(!ep.is_empty(), "[{label}] no DI activity — test stream produced nothing");

    let lags: Vec<i64> = ep.iter().zip(&es).map(|(p, s)| p.0 as i64 - s.0 as i64).collect();
    let lag0 = lags[0];
    for (k, ((p, s), &lag)) in ep.iter().zip(&es).zip(&lags).enumerate() {
        assert_eq!(
            p.1, s.1,
            "[{label}] edge {k} direction differs: pair {:?} vs single {:?}",
            p, s
        );
        assert_eq!(
            lag, lag0,
            "[{label}] edge {k} lag {lag} != constant lag {lag0} (pair {:?} vs single {:?})",
            p, s
        );
    }
    assert!(
        (0..=6).contains(&lag0),
        "[{label}] pair-behind-single lag {lag0} outside expected tx_a latency range"
    );
    println!("[{label}] OK: {} edges, identical directions/spacing, constant lag {lag0} sys-cycles", ep.len());

    // Both must park DI high after the trailing silence.
    assert_eq!(di_p.as_bytes()[cycles - 1], b'#', "[{label}] pair DI not parked high");
    assert_eq!(di_s.as_bytes()[cycles - 1], b'#', "[{label}] single DI not parked high");
}

#[test]
fn single_sm_matches_pair_150mhz() {
    // (150e6/10)/12.5e6 = 1.2 -> int 1, frac 0.2*256 = 51
    compare_at(51, "150MHz clkdiv 1.199");
}

#[test]
fn single_sm_matches_pair_133mhz() {
    // (133e6/10)/12.5e6 = 1.064 -> int 1, frac 0.064*256 = 16
    compare_at(16, "133MHz clkdiv 1.0625");
}

// ---------------------------------------------------------------------
// Round-trip: single-SM TX -> shipped RX decoder (fast/RP2350 variant),
// same emulator, DI is the wire. Mirrors round_trip.rs::clean_round_trip
// with the two-SM pair swapped out for the candidate.
// ---------------------------------------------------------------------

/// Fast (RP2350, 150MHz) RX variant from dme_pio.rs, as in round_trip.rs.
fn build_rx(emu: std::rc::Rc<std::cell::RefCell<rp2350_emu::Emulator>>, block: usize) -> Pio {
    let prog = pio::pio_asm!(
        "wait_for_low_stall:
             nop [1]
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
             set x 1 [3]
             jmp pin wait_for_low_stall
         .wrap_target
         wait_for_high:
             wait 1 pin 0
             mov x ~ x
             in x 1
             set x 0 [4]
             jmp pin wait_for_low
         .wrap
         low_wait_timeout:
             mov isr osr
             push
             irq set 0
         startup_search_high_cont:
             wait 0 pin 0 [5]
         startup_search_low:
             jmp pin startup_foundit_next_low [5]
             wait 1 pin 0 [5]
         startup_search_high:
             jmp pin startup_search_high_cont
         startup_foundit_next_high:
             nop [7]
             set X 0 [7]
             jmp wait_for_low [7]
         startup_foundit_next_low:
             set X 1 [7]
             jmp wait_for_high [7]"
    );
    let code: Vec<u16> = prog.program.code.iter().copied().collect();
    assert_eq!(code.len(), 32, "fast RX program is 32 instructions");

    let mut rx = Pio::from_shared(emu, block, 0);
    rx.load_at(0, &code, prog.program.wrap.target, prog.program.wrap.source);
    rx.jmp_pin(DI); // the wire: RX reads the TX data pin
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

/// Firmware's RX 5-bit -> LineCode mapping (line_code.rs
/// `from_five_bit_reversed`). "?" = unknown symbol.
fn decode_sym(v: u8) -> &'static str {
    match v & 0x1F {
        0x0F => "0", 0x12 => "1", 0x05 => "2", 0x15 => "3", 0x0A => "4", 0x1A => "5",
        0x0E => "6", 0x1E => "7", 0x09 => "8", 0x19 => "9", 0x0D => "A", 0x1D => "B",
        0x0B => "C", 0x1B => "D", 0x07 => "E", 0x17 => "F",
        0x00 => "Q", 0x1F => "I", 0x04 => "H", 0x03 => "J", 0x11 => "K", 0x16 => "T",
        0x1C => "R", 0x10 => "V", _ => "?",
    }
}

/// Re-frame the recovered bitstream (RX autopush window can be offset from
/// the code boundary): try all 5 bit offsets, sync on the StartK symbol,
/// keep the offset with the longest run of known symbols after it.
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

#[test]
fn single_sm_round_trip_through_shipped_rx() {
    let mut tx = build_single(51); // 150MHz config to match the fast RX
    let mut rx = build_rx(tx.emulator(), 1);
    tx.enable();
    rx.enable();

    // Same framed payload as round_trip.rs::clean_round_trip: silence
    // preamble, StartJ/StartK, 16 distinct data codes, EndT, silence.
    let payload: &[(u32, &str)] = &[
        (0x18, "J"), (0x11, "K"),
        (0x1E, "0"), (0x09, "1"), (0x14, "2"), (0x15, "3"),
        (0x0A, "4"), (0x0B, "5"), (0x0E, "6"), (0x0F, "7"),
        (0x12, "8"), (0x13, "9"), (0x16, "A"), (0x17, "B"),
        (0x1A, "C"), (0x1B, "D"), (0x1C, "E"), (0x1D, "F"),
        (0x0D, "T"),
    ];
    let mut words = vec![0x1F1F_1F1Fu32, 0x1F1F_1F1F];
    for &(c, _) in payload {
        words.push(c);
    }
    words.push(0x1F1F_1F1F);

    let mut i = 0;
    let mut syms: Vec<u8> = Vec::new();
    for _ in 0..9000 {
        if i < words.len() && !tx.tx_full() {
            tx.tx_push(words[i]);
            i += 1;
        }
        tx.step();
        if let Some(w) = rx.rx_pop() {
            syms.push((w & 0x1F) as u8);
        }
    }
    assert_eq!(i, words.len(), "feed did not drain");

    let decoded = reframe_and_decode(&syms);
    let sent_data: Vec<&str> = payload
        .iter()
        .map(|&(_, l)| l)
        .filter(|&l| l != "J" && l != "K" && l != "T")
        .collect();
    let got_data: Vec<&str> = decoded.iter().copied().take(sent_data.len()).collect();
    println!("sent data : {:?}", sent_data);
    println!("got  data : {:?}", got_data);
    assert_eq!(
        got_data, sent_data,
        "shipped RX did not recover the 16 data codes from the single-SM TX"
    );
}
