//! End-to-end 10BASE-T1S PIO round-trip:
//!
//!   line codes -> tx_b (encoder) -> tx_a (DME line driver) -> DI pin
//!     -> [wire: DI and RO are the same GPIO] -> rx (sampler/decoder)
//!     -> RX FIFO -> 5-bit symbols
//!
//! TX is PIO block 0 (tx_a sm0 @off0, tx_b sm1 @off4, sharing IRQ 0).
//! RX is PIO block 1 (sm0). The emulator's GPIO merge is the wire.
//!
//! Run: cargo test -p pio_harness --test round_trip -- --nocapture

use pio::{InstructionOperands as Op, MovDestination, MovOperation, MovSource, SetDestination};
use pio_harness::{PinCtrl, Pio, ShiftCtrl, ShiftDir};

const DI: u8 = 2; // TX data line == RX input (the wire)
const RO: u8 = 2; // same physical GPIO as DI
const DE: u8 = 3; // driver enable

fn build_tx(emu_block: usize) -> (Pio, Pio) {
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

    let mut tx_a = Pio::new(emu_block, 0);
    tx_a.load_at(0, &ca, prog_a.program.wrap.target, prog_a.program.wrap.source);
    tx_a.jmp_pin(DE);
    tx_a.pinctrl(PinCtrl { sideset_base: DI, sideset_count: prog_a.program.side_set.bits(), ..Default::default() });
    tx_a.sideset(prog_a.program.side_set.optional(), prog_a.program.side_set.pindirs());
    tx_a.set_output(DI);
    tx_a.set_output(DE);
    tx_a.clkdiv(1, 0);

    let mut tx_b = Pio::from_shared(tx_a.emulator(), emu_block, 1);
    tx_b.load_at(4, &cb, prog_b.program.wrap.target, prog_b.program.wrap.source);
    tx_b.pinctrl(PinCtrl { sideset_base: DE, sideset_count: prog_b.program.side_set.bits(), ..Default::default() });
    tx_b.sideset(prog_b.program.side_set.optional(), prog_b.program.side_set.pindirs());
    tx_b.shiftctrl(ShiftCtrl { autopull: false, out_dir: ShiftDir::Right, pull_threshold: 5, fjoin_tx: true, ..Default::default() });
    tx_b.clkdiv(1, 51); // ~1.2 at 150MHz
    tx_b.exec(Op::SET { destination: SetDestination::Y, data: 0x1F }.encode());
    for _ in 0..4 { tx_b.exec(0x4048); } // in y, 8
    tx_b.exec(Op::MOV { destination: MovDestination::Y, op: MovOperation::None, source: MovSource::ISR }.encode());

    (tx_a, tx_b)
}

fn build_rx(emu: std::rc::Rc<std::cell::RefCell<rp2350_emu::Emulator>>, block: usize) -> Pio {
    // Fast (RP2350, 150MHz) RX variant from dme_pio.rs (32 instructions).
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
    rx.jmp_pin(RO);
    rx.pinctrl(PinCtrl { in_base: RO, ..Default::default() });
    rx.shiftctrl(ShiftCtrl { autopush: true, in_dir: ShiftDir::Left, push_threshold: 5, fjoin_rx: true, ..Default::default() });
    rx.clkdiv(1, 0);
    // init: set x,1F ; mov osr,x
    rx.exec(0xE03F);
    rx.exec(0xA0E1);
    rx
}

#[test]
fn observe_round_trip() {
    let (mut tx_a, mut tx_b) = build_tx(0);
    let mut rx = build_rx(tx_a.emulator(), 1);

    tx_a.enable();
    tx_b.enable();
    rx.enable();

    // A realistic-ish frame: preamble idles, start delimiter, data, end.
    let codes: &[(u8, &str)] = &[
        (0x1F, "Idle"), (0x1F, "Idle"), (0x1F, "Idle"), (0x1F, "Idle"),
        (0x18, "StartJ"), (0x11, "StartK"),
        (0x15, "Data3"), (0x0B, "Data5"), (0x1D, "DataF"),
        (0x0D, "EndT"), (0x1F, "Idle"),
    ];
    for &(c, _) in codes { tx_b.tx_push(c as u32); }

    // Run, capturing DI and any RX pushes.
    let mut di = String::new();
    let mut rx_words: Vec<u32> = Vec::new();
    for _ in 0..2000 {
        tx_a.step();
        di.push(if tx_a.gpio(DI) { '#' } else { '_' });
        if let Some(w) = rx.rx_pop() {
            rx_words.push(w);
        }
    }

    println!("pushed codes: {:?}", codes.iter().map(|c| c.1).collect::<Vec<_>>());
    println!("DI waveform ({} cycles):\n{}", di.len(), di);
    println!("RX pushed {} words: {:?}", rx_words.len(), rx_words.iter().map(|w| format!("{:05b}", w & 0x1F)).collect::<Vec<_>>());
    // Full RX pc_visits, annotated with program regions.
    // Indices: 0=wait_for_low_stall, 1..13=loop scan, 14=found_low(in x),
    //   15=set x 1, 16=jmp pin, 17=wait_for_high(wrap, `wait 1 pin`),
    //   18=mov x~x, 19=in x 1, 20=set x0, 21=jmp pin wait_for_low,
    //   22=low_wait_timeout(mov isr osr), 23=push, 24=irq set,
    //   25=startup_search_high_cont(wait 0), 26..=startup search.
    let v = rx.pc_visits();
    println!("rx.pc_visits[0..32] = {:?}", v);
    println!("  main-loop region (wait_for_high@17..21) visits = {:?}", &v[17..22]);
    println!("  timeout/startup region (22..32) visits = {:?}", &v[22..32]);
    println!("rx stall_cycles={} rx_push_success={}", rx.stall_cycles(), rx.rx_push_success());

    // Observation milestone: RX should be sampling and pushing *something*.
    assert!(rx.rx_push_success() > 0, "RX never pushed a symbol — it is not locking onto the TX waveform");
}

/// Framed round-trip: trigger tx_b's preamble generator by sending the
/// SILENCE word (0x1F1F1F1F, which equals the Y register so the
/// `jmp x!=y` silence branch is taken → idle_wait/pre_loop emits a
/// preamble), then a real start delimiter + data, so RX can phase-lock.
#[test]
fn framed_round_trip() {
    let (mut tx_a, mut tx_b) = build_tx(0);
    let mut rx = build_rx(tx_a.emulator(), 1);
    tx_a.enable(); tx_b.enable(); rx.enable();

    // Silence words trigger the preamble path; then StartJ/StartK lock,
    // then data, then more silence.
    tx_b.tx_push(0x1F1F_1F1F);
    tx_b.tx_push(0x1F1F_1F1F);
    for &c in &[0x18u32, 0x11, 0x15, 0x0B, 0x1D, 0x0D] { tx_b.tx_push(c); } // J K D3 D5 DF T
    tx_b.tx_push(0x1F1F_1F1F);

    let mut rx_words = Vec::new();
    for _ in 0..3000 {
        tx_a.step();
        if let Some(w) = rx.rx_pop() { rx_words.push((w & 0x1F) as u8); }
    }
    let syms: Vec<String> = rx_words.iter().map(|w| format!("{:05b}", w)).collect();
    println!("framed: RX {} syms = {:?}", rx_words.len(), syms);
    let v = rx.pc_visits();
    println!("  main-loop visits={} timeout visits={}", v[15..20].iter().sum::<u64>(), v[20..23].iter().sum::<u64>());
}

/// Build the TX-code -> RX-symbol map empirically: lock RX with a proper
/// preamble + start delimiter, then send a long run of ONE code and take
/// the steady-state (most common) RX symbol. Compare to the firmware's
/// `from_five_bit_reversed` expectation.
fn steady_symbol(code: u8) -> u8 {
    let (mut tx_a, mut tx_b) = build_tx(0);
    let mut rx = build_rx(tx_a.emulator(), 1);
    tx_a.enable(); tx_b.enable(); rx.enable();
    tx_b.tx_push(0x1F1F_1F1F);
    tx_b.tx_push(0x1F1F_1F1F);
    tx_b.tx_push(0x18); // StartJ
    tx_b.tx_push(0x11); // StartK
    for _ in 0..40 { tx_b.tx_push(code as u32); }
    tx_b.tx_push(0x1F1F_1F1F);

    let mut words = Vec::new();
    for _ in 0..5000 {
        tx_a.step();
        if let Some(w) = rx.rx_pop() { words.push((w & 0x1F) as u8); }
    }
    // Steady-state = most frequent symbol among the run (skip first 4 =
    // lock + start-delimiter symbols, and the trailing idle).
    use std::collections::HashMap;
    let mut counts: HashMap<u8, usize> = HashMap::new();
    for &w in words.iter().skip(4) {
        if w != 0x1F { *counts.entry(w).or_default() += 1; } // ignore idle
    }
    counts.into_iter().max_by_key(|&(_, n)| n).map(|(s, _)| s).unwrap_or(0x1F)
}

/// Firmware's RX 5-bit -> LineCode mapping (from line_code.rs
/// `from_five_bit_reversed`). Returns a short label, or "?" if unknown.
fn decode_sym(v: u8) -> &'static str {
    match v & 0x1F {
        0x0F => "0", 0x12 => "1", 0x05 => "2", 0x15 => "3", 0x0A => "4", 0x1A => "5",
        0x0E => "6", 0x1E => "7", 0x09 => "8", 0x19 => "9", 0x0D => "A", 0x1D => "B",
        0x0B => "C", 0x1B => "D", 0x07 => "E", 0x17 => "F",
        0x00 => "Q", 0x1F => "I", 0x04 => "H", 0x03 => "J", 0x11 => "K", 0x16 => "T",
        0x1C => "R", 0x10 => "V", _ => "?",
    }
}

/// Re-frame the recovered bitstream: the RX autopush window can be offset
/// from the true code boundary. Concatenate the recovered symbol bits
/// (MSB-first per symbol = recovery order), then for each of the 5
/// possible bit offsets, regroup into 5-bit codes and pick the offset
/// whose stream contains the `J,K` start delimiter — that's the true
/// framing. Returns the decoded label sequence from the delimiter on.
fn reframe_and_decode(rx_syms: &[u8]) -> Vec<&'static str> {
    let mut bits = Vec::new();
    for &s in rx_syms {
        for i in (0..5).rev() { bits.push((s >> i) & 1); }
    }
    let group = |off: usize| -> Vec<u8> {
        bits[off..].chunks(5).filter(|c| c.len() == 5)
            .map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (b << (4 - i)))).collect()
    };
    // Pick the bit-offset whose regrouping yields a valid frame: locate
    // the StartK delimiter ("K") and score the run of known symbols after
    // it. (StartJ can glitch during lock-settle, so we sync on K.)
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

/// Feed `words` into tx_b's FIFO incrementally (FIFO is only 8 deep, so
/// we can't push them all up front — mimics the firmware's DMA), stepping
/// the emulator and collecting RX symbols.
fn run_feeding(tx_a: &mut Pio, tx_b: &mut Pio, rx: &mut Pio, words: &[u32], cycles: usize) -> Vec<u8> {
    let mut i = 0;
    let mut out = Vec::new();
    for _ in 0..cycles {
        if i < words.len() && !tx_b.tx_full() {
            tx_b.tx_push(words[i]);
            i += 1;
        }
        tx_a.step();
        if let Some(w) = rx.rx_pop() { out.push((w & 0x1F) as u8); }
    }
    out
}

/// THE CLEAN 1:1 ROUND-TRIP: send a framed sequence of distinct line
/// codes, recover them through the full PIO chain, and assert the decoded
/// sequence matches what was sent.
#[test]
fn clean_round_trip() {
    let (mut tx_a, mut tx_b) = build_tx(0);
    let mut rx = build_rx(tx_a.emulator(), 1);
    tx_a.enable(); tx_b.enable(); rx.enable();

    // Frame: silence preamble, StartJ/StartK, a distinctive data payload,
    // EndT, trailing silence. (labels: J K 0 1 2 .. F T)
    let payload: &[(u32, &str)] = &[
        (0x18, "J"), (0x11, "K"),
        (0x1E, "0"), (0x09, "1"), (0x14, "2"), (0x15, "3"),
        (0x0A, "4"), (0x0B, "5"), (0x0E, "6"), (0x0F, "7"),
        (0x12, "8"), (0x13, "9"), (0x16, "A"), (0x17, "B"),
        (0x1A, "C"), (0x1B, "D"), (0x1C, "E"), (0x1D, "F"),
        (0x0D, "T"),
    ];
    let mut words = vec![0x1F1F_1F1Fu32, 0x1F1F_1F1F];
    for &(c, _) in payload { words.push(c); }
    words.push(0x1F1F_1F1F);

    let syms = run_feeding(&mut tx_a, &mut tx_b, &mut rx, &words, 9000);
    let decoded = reframe_and_decode(&syms); // sequence after StartK
    // The 16 data codes follow StartK. (StartJ may glitch during lock,
    // and EndT sits at the trailing-idle boundary — assert the payload.)
    let sent_data: Vec<&str> = payload.iter()
        .map(|&(_, l)| l)
        .filter(|&l| l != "J" && l != "K" && l != "T")
        .collect();
    let got_data: Vec<&str> = decoded.iter().copied().take(sent_data.len()).collect();
    println!("sent data : {:?}", sent_data);
    println!("got  data : {:?}", got_data);
    assert_eq!(got_data, sent_data, "PIO round-trip did not recover the 16 data line codes");
}

#[test]
fn mixed_stream_with_proper_feeding() {
    let (mut tx_a, mut tx_b) = build_tx(0);
    let mut rx = build_rx(tx_a.emulator(), 1);
    tx_a.enable(); tx_b.enable(); rx.enable();

    // silence preamble, StartJ/StartK, then alternating Data5/DataF, fed
    // as FIFO space frees up.
    let mut words = vec![0x1F1F_1F1Fu32, 0x1F1F_1F1F, 0x18, 0x11];
    for _ in 0..20 { words.push(0x0B); words.push(0x1D); }
    words.push(0x1F1F_1F1F);

    let syms = run_feeding(&mut tx_a, &mut tx_b, &mut rx, &words, 6000);
    let s5 = steady_symbol(0x0B);
    let sf = steady_symbol(0x1D);
    println!("steady Data5=0x{s5:02X} DataF=0x{sf:02X}");
    println!("RX {} syms: {:?}", syms.len(), syms.iter().map(|w| format!("0x{:02X}", w)).collect::<Vec<_>>());
    let matched = syms.iter().filter(|&&w| w == s5 || w == sf).count();
    println!("symbols matching Data5/DataF steady value: {}/{}", matched, syms.len());
}

/// Diagnostic: is the DME stream CONTINUOUS across code boundaries?
/// Capture DI run-lengths while alternating two codes. Healthy DME has
/// runs of 6 or 12 cycles only; a run >> 12 means a transition went
/// missing (clock-recovery would drop lock there) = a tx_b pull/wrap
/// boundary discontinuity rather than a transition-density problem.
#[test]
fn dme_continuity_across_code_boundaries() {
    let (mut tx_a, mut tx_b) = build_tx(0);
    tx_a.enable(); tx_b.enable();
    tx_b.tx_push(0x1F1F_1F1F);
    tx_b.tx_push(0x1F1F_1F1F);
    for _ in 0..20 { tx_b.tx_push(0x0B); tx_b.tx_push(0x1D); } // Data5 / DataF alternating

    for _ in 0..200 { tx_a.step(); } // skip startup/preamble
    let mut di = String::new();
    for _ in 0..700 { tx_a.step(); di.push(if tx_a.gpio(DI) { '#' } else { '_' }); }
    let mut runs: Vec<usize> = Vec::new();
    let mut last = ' ';
    for ch in di.chars() {
        if ch == last { *runs.last_mut().unwrap() += 1; } else { runs.push(1); last = ch; }
    }
    let mut hist = std::collections::BTreeMap::new();
    for &n in &runs { *hist.entry(n).or_insert(0usize) += 1; }
    let anomalies: Vec<usize> = runs.iter().copied().filter(|&n| n > 13).collect();
    println!("DI run-length histogram (changing codes): {:?}", hist);
    println!("anomalous runs (>13 cyc = missing transition): {:?}", anomalies);
    println!("DI: {di}");
}

/// Does symbol-boundary alignment hold in a MIXED stream? Lock, then send
/// A,B,A,B,... and check RX alternates between A's and B's steady symbols.
/// If it does, the per-code map decodes arbitrary streams (clean 1:1).
#[test]
fn alignment_holds_in_mixed_stream() {
    let a = 0x0B; // Data5  -> steady 0x0B
    let b = 0x1D; // DataF  -> steady 0x1E
    let sa = steady_symbol(a);
    let sb = steady_symbol(b);
    println!("steady: A(Data5)=0x{sa:02X}  B(DataF)=0x{sb:02X}");

    let (mut tx_a, mut tx_b) = build_tx(0);
    let mut rx = build_rx(tx_a.emulator(), 1);
    tx_a.enable(); tx_b.enable(); rx.enable();
    tx_b.tx_push(0x1F1F_1F1F);
    tx_b.tx_push(0x1F1F_1F1F);
    tx_b.tx_push(0x18); tx_b.tx_push(0x11); // StartJ StartK
    for _ in 0..16 { tx_b.tx_push(a as u32); tx_b.tx_push(b as u32); }
    tx_b.tx_push(0x1F1F_1F1F);

    let mut words = Vec::new();
    for _ in 0..6000 {
        tx_a.step();
        if let Some(w) = rx.rx_pop() { words.push((w & 0x1F) as u8); }
    }
    let syms: Vec<String> = words.iter().map(|w| format!("0x{:02X}", w)).collect();
    println!("RX stream ({}): {:?}", words.len(), syms);
    // Count how many symbols are one of {sa, sb, idle}.
    let aligned = words.iter().filter(|&&w| w == sa || w == sb || w == 0x1F).count();
    println!("aligned-to-map: {}/{}", aligned, words.len());
}

#[test]
fn build_symbol_table() {
    // RP2350 4B5B data codes (LineCode discriminants).
    let codes: &[(u8, &str)] = &[
        (0x1E, "Data0"), (0x09, "Data1"), (0x14, "Data2"), (0x15, "Data3"),
        (0x0A, "Data4"), (0x0B, "Data5"), (0x0E, "Data6"), (0x0F, "Data7"),
        (0x12, "Data8"), (0x13, "Data9"), (0x16, "DataA"), (0x17, "DataB"),
        (0x1A, "DataC"), (0x1B, "DataD"), (0x1C, "DataE"), (0x1D, "DataF"),
    ];
    println!("TXcode  name   sent(5b)  RXsym(5b)  rev(RXsym)");
    for &(c, name) in codes {
        let s = steady_symbol(c);
        let rev = (0..5).fold(0u8, |acc, i| acc | (((s >> i) & 1) << (4 - i)));
        println!("0x{c:02X}    {name}   {:05b}    {:05b}     {:05b} (0x{rev:02X})", c & 0x1F, s, rev);
    }
}

/// Calibration: drive idle preamble then a long run of ONE data code and
/// see what steady-state symbol RX settles on. This separates "timing
/// fidelity" (does RX decode a steady stream correctly?) from "framing"
/// (preamble/lock/bit-order alignment).
#[test]
fn calibrate_single_code() {
    fn run(code: u8, label: &str) {
        let (mut tx_a, mut tx_b) = build_tx(0);
        let mut rx = build_rx(tx_a.emulator(), 1);
        tx_a.enable(); tx_b.enable(); rx.enable();
        // 8 idles to let RX lock, then 24 of the target code.
        for _ in 0..8 { tx_b.tx_push(0x1F); }
        for _ in 0..24 { tx_b.tx_push(code as u32); }
        let mut words = Vec::new();
        for _ in 0..4000 {
            tx_a.step();
            if let Some(w) = rx.rx_pop() { words.push((w & 0x1F) as u8); }
        }
        let syms: Vec<String> = words.iter().map(|w| format!("{:05b}", w)).collect();
        let v = rx.pc_visits();
        let mainloop: u64 = v[15..20].iter().sum();
        let timeout: u64 = v[20..23].iter().sum();
        println!("code {label}: main-loop visits={mainloop} timeout visits={timeout} syms={syms:?}");
    }
    run(0x1F, "Idle");
    run(0x0B, "Data5");
    run(0x1D, "DataF");
    run(0x15, "Data3");
}

/// Measure the TX DME half-bit period: run-length-encode the DI waveform
/// during continuous transmission. This is the ground-truth edge cadence
/// RX must track.
#[test]
fn measure_tx_half_bit_period() {
    let (mut tx_a, mut tx_b) = build_tx(0);
    tx_a.enable();
    tx_b.enable();
    // Continuous data so the line keeps toggling.
    for _ in 0..40 { tx_b.tx_push(0x0B); } // Data5 repeatedly

    // Skip the first ~150 cycles (startup), then capture.
    for _ in 0..150 { tx_a.step(); }
    let mut di = String::new();
    for _ in 0..400 { tx_a.step(); di.push(if tx_a.gpio(DI) { '#' } else { '_' }); }

    // Run-length encode.
    let mut runs: Vec<(char, usize)> = Vec::new();
    for ch in di.chars() {
        match runs.last_mut() {
            Some((c, n)) if *c == ch => *n += 1,
            _ => runs.push((ch, 1)),
        }
    }
    let lens: Vec<usize> = runs.iter().map(|(_, n)| *n).collect();
    println!("DI (mid-stream): {di}");
    println!("run lengths (sys-cycles per level): {:?}", &lens[..lens.len().min(40)]);
    // Histogram of run lengths.
    let mut hist = std::collections::BTreeMap::new();
    for &n in &lens { *hist.entry(n).or_insert(0usize) += 1; }
    println!("run-length histogram (cycles: count): {:?}", hist);
}
