//! Replay a REAL Saleae capture of the bus through the emulated RX.
//!
//! The fixture (tests/data/stock_ns_125mhz_500msps.csv) is a 500 MS/s
//! capture of a stock two-SM TX Neighbor-Solicitation frame from the
//! R6-1 bench (2026-07-10) — a frame the OTHER board's RX failed to
//! decode (bit-misaligned) while the K2L PHY and our offline decoder
//! read it perfectly.
//!
//! Replaying the true waveform into the emulated slow RX while sweeping
//! the sub-cycle sampling phase (the capture is 2ns-resolution; an RX
//! cycle at 125 MHz is 8ns, so phases 0..8ns in 1ns steps) makes the
//! alignment decision deterministic and fully observable per phase.
//!
//! Polarity note: the analyzer probed one leg of the differential pair
//! referenced to the other, so its idle reads LOW while the board's RO
//! idles HIGH (transceiver failsafe). `invert=true` reconstructs the
//! RO-equivalent signal (idle forced high outside the burst either way).
//!
//! Run: cargo test -p pio_harness --test wave_replay -- --nocapture

use pio_harness::{PinCtrl, Pio, ShiftCtrl, ShiftDir};

const RO: u8 = 2; // the pin the emulated RX samples

/// Slow (125/133 MHz) RX variant from dme_pio.rs, 31 instructions.
fn build_rx_slow() -> Pio {
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

    let mut rx = Pio::new(0, 0);
    rx.load_at(0, &code, prog.program.wrap.target, prog.program.wrap.source);
    rx.jmp_pin(RO);
    rx.pinctrl(PinCtrl { in_base: RO, ..Default::default() });
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

/// Load the Saleae CSV: rows of (time_s, new_value).
fn load_transitions(path: &str) -> Vec<(f64, bool)> {
    let text = std::fs::read_to_string(path).expect("fixture readable");
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let mut it = l.split(',');
            let t: f64 = it.next()?.trim().parse().ok()?;
            let v: u8 = it.next()?.trim().parse().ok()?;
            Some((t, v != 0))
        })
        .collect()
}

/// Signal level at absolute time t (holds last transition's value).
fn level_at(tr: &[(f64, bool)], t: f64) -> bool {
    match tr.binary_search_by(|(tt, _)| tt.partial_cmp(&t).unwrap()) {
        Ok(i) => tr[i].1,
        Err(0) => false,
        Err(i) => tr[i - 1].1,
    }
}

struct ReplayResult {
    syms: Vec<u8>,
    startup_visits: Vec<u64>,
}

/// Replay the capture through a fresh RX. `phase_ns` shifts the RX
/// sample grid within the 8ns cycle; `invert` reconstructs RO polarity.
/// `aperture_ns` models the input synchronizer: if a transition falls
/// within +-aperture of the sample instant, the sampled level resolves
/// RANDOMLY (seeded LCG) — the metastable +-1-cycle uncertainty real
/// silicon has and the ideal emulator lacks. aperture 0 = deterministic.
fn replay(
    tr: &[(f64, bool)],
    burst_start: f64,
    burst_end: f64,
    phase_ns: f64,
    invert: bool,
    aperture_ns: f64,
    seed: u64,
) -> ReplayResult {
    let mut rx = build_rx_slow();
    rx.set_pin(RO, true); // idle-high before we begin
    rx.enable();

    let cycle = 8e-9; // 125 MHz
    let t0 = burst_start - 40e-6; // enough idle for timeout+startup settle
    let t1 = burst_end + 10e-6;
    let n = ((t1 - t0) / cycle) as u64;

    // Nearest-transition distance for aperture checks.
    let nearest = |t: f64| -> f64 {
        let i = match tr.binary_search_by(|(tt, _)| tt.partial_cmp(&t).unwrap()) {
            Ok(i) => i,
            Err(i) => i,
        };
        let mut d = f64::MAX;
        if i < tr.len() {
            d = d.min((tr[i].0 - t).abs());
        }
        if i > 0 {
            d = d.min((t - tr[i - 1].0).abs());
        }
        d
    };
    let mut lcg = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);

    let mut syms = Vec::new();
    for k in 0..n {
        let t = t0 + (k as f64) * cycle + phase_ns * 1e-9;
        let lvl = if t < burst_start - 1e-6 || t > burst_end + 1e-6 {
            true // idle: RO rests high
        } else {
            let raw = if aperture_ns > 0.0 && nearest(t) < aperture_ns * 1e-9 {
                lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (lcg >> 63) != 0
            } else {
                level_at(tr, t)
            };
            if invert { !raw } else { raw }
        };
        rx.set_pin(RO, lvl);
        rx.step();
        if let Some(w) = rx.rx_pop() {
            syms.push((w & 0x1F) as u8);
        }
    }
    ReplayResult {
        syms,
        startup_visits: rx.pc_visits()[22..31].to_vec(),
    }
}

/// Score a symbol stream: fraction of known symbols at each of the 5 bit
/// offsets; returns (best_offset, best_fraction, fraction_at_offset0).
fn score(syms: &[u8]) -> (usize, f64, f64) {
    let mut bits = Vec::new();
    for &s in syms {
        for i in (0..5).rev() {
            bits.push((s >> i) & 1);
        }
    }
    let frac = |off: usize| -> f64 {
        let codes: Vec<u8> = bits[off..]
            .chunks(5)
            .filter(|c| c.len() == 5)
            .map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (b << (4 - i))))
            .collect();
        if codes.is_empty() {
            return 0.0;
        }
        let known = codes.iter().filter(|&&c| decode_sym(c) != "?").count();
        known as f64 / codes.len() as f64
    };
    let mut best = (0usize, 0.0f64);
    for off in 0..5 {
        let f = frac(off);
        if f > best.1 {
            best = (off, f);
        }
    }
    (best.0, best.1, frac(0))
}

#[test]
fn replay_real_capture_phase_sweep() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/stock_ns_125mhz_500msps.csv");
    let tr = load_transitions(path);
    assert!(tr.len() > 1000, "fixture loaded ({} transitions)", tr.len());

    // Locate the burst: transitions spaced <1us belong to it.
    let mut burst_start = None;
    let mut burst_end = 0.0f64;
    for w in tr.windows(2) {
        if (w[1].0 - w[0].0) < 1e-6 {
            if burst_start.is_none() {
                burst_start = Some(w[0].0);
            }
            burst_end = w[1].0;
        }
    }
    let burst_start = burst_start.expect("burst found");
    println!(
        "burst: {:.6}s .. {:.6}s ({:.0}us)",
        burst_start,
        burst_end,
        (burst_end - burst_start) * 1e6
    );

    for invert in [true, false] {
        println!(
            "== polarity: {} ==",
            if invert { "inverted (RO-equivalent, idle-high)" } else { "raw analyzer" }
        );
        for phase in 0..8 {
            let r = replay(&tr, burst_start, burst_end, phase as f64, invert, 0.0, 0);
            let (best_off, best_frac, frac0) = score(&r.syms);
            let fw = if best_off == 0 && best_frac > 0.95 { "FW-OK" } else { "FW-GARBAGE" };
            let head: Vec<&str> = {
                // decode at the best offset for a peek
                let mut bits = Vec::new();
                for &s in &r.syms {
                    for i in (0..5).rev() {
                        bits.push((s >> i) & 1);
                    }
                }
                bits[best_off..]
                    .chunks(5)
                    .filter(|c| c.len() == 5)
                    .take(6)
                    .map(|c| {
                        decode_sym(c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (b << (4 - i))))
                    })
                    .collect()
            };
            println!(
                "  phase {phase}ns: {} syms, best_off={best_off} ({:.0}% valid; {:.0}% at off0) [{}] startup={:?} head={:?}",
                r.syms.len(),
                best_frac * 100.0,
                frac0 * 100.0,
                fw,
                r.startup_visits,
                head
            );
        }
    }
}

/// Model the input synchronizer's metastable aperture: edges landing
/// within +-aperture of a sample instant resolve randomly. With ~1ppm
/// relative crystals the bench boards PARK at a fixed sub-cycle phase
/// for seconds — if that phase puts frame edges in the aperture, every
/// frame decodes under randomized sampling. Sweep phase finely and
/// Monte-Carlo the aperture; count misalignments and corrupted decodes.
#[test]
fn replay_with_sync_aperture_jitter() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/stock_ns_125mhz_500msps.csv");
    let tr = load_transitions(path);
    let mut burst_start = None;
    let mut burst_end = 0.0f64;
    for w in tr.windows(2) {
        if (w[1].0 - w[0].0) < 1e-6 {
            if burst_start.is_none() {
                burst_start = Some(w[0].0);
            }
            burst_end = w[1].0;
        }
    }
    let burst_start = burst_start.unwrap();

    let aperture = 1.5; // ns — order of a sync FF aperture + threshold noise
    println!("aperture = {aperture}ns, 4 seeds per phase, RO polarity");
    let mut bad_phases = 0;
    let mut total_phases = 0;
    for phase10 in (0..80).step_by(5) {
        let phase = phase10 as f64 / 10.0;
        let mut misaligned = 0;
        let mut corrupted = 0;
        let mut clean = 0;
        for seed in 1..=4u64 {
            let r = replay(&tr, burst_start, burst_end, phase, true, aperture, seed);
            let (best_off, best_frac, frac0) = score(&r.syms);
            if best_off != 0 {
                misaligned += 1;
            } else if frac0 < 0.99 {
                corrupted += 1;
            } else {
                clean += 1;
            }
            let _ = best_frac;
        }
        total_phases += 1;
        if misaligned + corrupted > 0 {
            bad_phases += 1;
        }
        println!(
            "  phase {phase:>4}ns: clean={clean} corrupted={corrupted} misaligned={misaligned} (of 4 seeds)"
        );
    }
    println!("phases with any failure: {bad_phases}/{total_phases}");
}
