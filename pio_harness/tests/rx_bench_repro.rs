//! Reproduce the bench frame-MISS signature in the emulator.
//!
//! Bench state (2026-07-10, raven ca183707): board -1 runs the FLASHED
//! RX (data-loop margin patches idx13 [1]->[2], idx18 [2]->[3]; startup
//! at shipped [7][7]) and fully decodes the K2L's 926-byte frames with
//! valid CRC, but captures ONLY the trailing R symbol of the peer
//! pneumatics board's NS frames (S raw: [07, 1f, ...]) — the RX sleeps
//! through the whole frame. Wire verified perfect (Saleae + offline
//! decoder). Every 3s frame shows the same signature, so the mechanism
//! is (quasi-)phase-independent or the parked phase is adversarial and
//! stable.
//!
//! Prior replays (wave_replay.rs, rx_fix.rs) were fresh-boot, shipped or
//! candidate-C code, and modeled only a metastable APERTURE — not the
//! 2-FF input-synchronizer DELAY, and never carried RX state across
//! back-to-back frames. The [9][9] startup experiment proved the delay
//! is load-bearing: emulator-certified retiming missed whole frames on
//! hardware. This harness adds all three missing pieces:
//!   1. the FLASHED code exactly (data-loop patches only),
//!   2. a 2-cycle synchronizer pipeline (sample at the FF grid with the
//!      aperture model, deliver to the PIO 2 cycles later),
//!   3. N repeated bursts through ONE RX instance with idle gaps,
//! and classifies each frame occurrence: DECODED / MISALIGNED /
//! GARBAGE / MISS (bench signature: <5 symbols, arriving near frame end).
//!
//! Run: cargo test -p pio_harness --test rx_bench_repro -- --nocapture

use pio_harness::{PinCtrl, Pio, ShiftCtrl, ShiftDir};

const RO: u8 = 2;
const CYCLE: f64 = 8e-9; // RX cycle at 125 MHz

fn slow_rx_code() -> (Vec<u16>, u8, u8) {
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
    (code, prog.program.wrap.target, prog.program.wrap.source)
}

fn patch_delay(code: &mut [u16], idx: usize, delay: u16) {
    assert!(delay < 32);
    code[idx] = (code[idx] & !(0x1F << 8)) | (delay << 8);
}

/// The code actually in flash on board -1 (raven ca183707): data-loop
/// margin patches only, startup untouched.
fn flashed_code() -> (Vec<u16>, u8, u8) {
    let (mut code, wt, ws) = slow_rx_code();
    // Verify indices before patching (hand counts were wrong once).
    assert_eq!(code[13] & 0xE0FF, 0xE021, "idx13 is `set x 1`");
    assert_eq!((code[13] >> 8) & 0x1F, 1, "idx13 shipped delay [1]");
    assert_eq!(code[18] & 0xE0FF, 0xE020, "idx18 is `set x 0`");
    assert_eq!((code[18] >> 8) & 0x1F, 2, "idx18 shipped delay [2]");
    patch_delay(&mut code, 13, 2);
    patch_delay(&mut code, 18, 3);
    (code, wt, ws)
}

/// Candidate C as it was ACTUALLY on board -1 during the whole "misses
/// NS frames" bench phase (flash timeline: last -1 flash 18:21, startup
/// revert edited 18:23 — the revert never reached hardware).
fn candidate_c_code() -> (Vec<u16>, u8, u8) {
    let (mut code, wt, ws) = flashed_code();
    assert_eq!((code[27] >> 8) & 0x1F, 7, "idx27 is set X 0 [7]");
    assert_eq!((code[28] >> 8) & 0x1F, 7, "idx28 is jmp [7]");
    patch_delay(&mut code, 27, 9);
    patch_delay(&mut code, 28, 9);
    (code, wt, ws)
}

fn build_rx(code: &[u16], wrap_t: u8, wrap_s: u8) -> Pio {
    let mut rx = Pio::new(0, 0);
    rx.load_at(0, code, wrap_t, wrap_s);
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

/// Extract the burst as transitions relative to its first edge, in RO
/// polarity (capture is inverted), plus its duration.
fn burst_edges(tr: &[(f64, bool)]) -> (Vec<(f64, bool)>, f64) {
    let mut start = None;
    let mut end = 0.0f64;
    for w in tr.windows(2) {
        if (w[1].0 - w[0].0) < 1e-6 {
            if start.is_none() {
                start = Some(w[0].0);
            }
            end = w[1].0;
        }
    }
    let start = start.expect("burst found");
    let edges: Vec<(f64, bool)> = tr
        .iter()
        .filter(|(t, _)| *t >= start - 1e-9 && *t <= end + 1e-9)
        .map(|(t, v)| (t - start, !v)) // RO polarity = inverted capture
        .collect();
    (edges, end - start)
}

fn level_at(edges: &[(f64, bool)], t: f64) -> bool {
    match edges.binary_search_by(|(tt, _)| tt.partial_cmp(&t).unwrap()) {
        Ok(i) => edges[i].1,
        Err(0) => true, // before first edge: idle high (RO polarity)
        Err(i) => edges[i - 1].1,
    }
}

fn nearest_edge(edges: &[(f64, bool)], t: f64) -> f64 {
    let i = match edges.binary_search_by(|(tt, _)| tt.partial_cmp(&t).unwrap()) {
        Ok(i) => i,
        Err(i) => i,
    };
    let mut d = f64::MAX;
    if i < edges.len() {
        d = d.min((edges[i].0 - t).abs());
    }
    if i > 0 {
        d = d.min((t - edges[i - 1].0).abs());
    }
    d
}

struct FrameResult {
    /// symbols captured while this burst (plus 2us tail) was on the wire
    syms: Vec<u8>,
    /// time of first captured symbol relative to burst start (us)
    first_sym_us: Option<f64>,
    burst_len_us: f64,
}

#[derive(PartialEq, Debug, Clone, Copy)]
enum Verdict {
    Decoded,
    Misaligned,
    Garbage,
    Miss,
}

impl FrameResult {
    /// Fraction of valid symbols at bit offset `off`.
    fn frac(&self, off: usize) -> f64 {
        let mut bits = Vec::new();
        for &s in &self.syms {
            for i in (0..5).rev() {
                bits.push((s >> i) & 1);
            }
        }
        let codes: Vec<u8> = bits
            .get(off..)
            .unwrap_or(&[])
            .chunks(5)
            .filter(|c| c.len() == 5)
            .map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (b << (4 - i))))
            .collect();
        if codes.is_empty() {
            return 0.0;
        }
        codes.iter().filter(|&&c| decode_sym(c) != "?").count() as f64 / codes.len() as f64
    }

    fn verdict(&self) -> Verdict {
        // ~185 symbols in the NS frame; bench MISS = a couple of tail
        // symbols only.
        if self.syms.len() < 20 {
            return Verdict::Miss;
        }
        let f0 = self.frac(0);
        if f0 > 0.95 {
            return Verdict::Decoded;
        }
        let best = (1..5).map(|o| self.frac(o)).fold(0.0f64, f64::max);
        if best > 0.95 {
            Verdict::Misaligned
        } else {
            Verdict::Garbage
        }
    }
}

/// Replay `repeats` back-to-back bursts through ONE RX instance.
///
/// * `phase_ns` — sub-cycle phase of the burst within the RX sample grid
///   (constant across a burst: bench boards are ~ppm-matched).
/// * `sync_delay` — cycles of input-synchronizer latency: the level is
///   sampled at the FF grid (where the aperture model applies) and the
///   PIO observes it `sync_delay` cycles later. Hardware default = 2.
/// * `aperture_ns` — metastable window around the FF sampling instant;
///   levels inside it resolve randomly (seeded LCG).
/// * `gap_us` — idle between bursts (bench is 3s; state converges long
///   before that, the gap just has to exhaust timeout+startup settle).
/// * `phase_step_ns` — phase drift added per repeat (0 = parked bench
///   pair; nonzero models slow thermal walk).
fn replay_frames(
    code: &(Vec<u16>, u8, u8),
    edges: &[(f64, bool)],
    burst_len: f64,
    repeats: usize,
    phase_ns: f64,
    sync_delay: usize,
    aperture_ns: f64,
    seed: u64,
    gap_us: f64,
    phase_step_ns: f64,
) -> Vec<FrameResult> {
    let (code, wt, ws) = (code.0.clone(), code.1, code.2);
    let mut rx = build_rx(&code, wt, ws);
    rx.set_pin(RO, true);
    rx.enable();

    let mut lcg = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let mut sample = |edges: &[(f64, bool)], t: f64| -> bool {
        if aperture_ns > 0.0 && nearest_edge(edges, t) < aperture_ns * 1e-9 {
            lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (lcg >> 63) != 0
        } else {
            level_at(edges, t)
        }
    };

    // Synchronizer pipeline, preloaded with idle.
    let mut pipe = vec![true; sync_delay + 1];

    let lead_cycles = (40e-6 / CYCLE) as u64; // settle: timeout + startup park
    let gap_cycles = (gap_us * 1e-6 / CYCLE) as u64;
    let burst_cycles = ((burst_len + 2e-6) / CYCLE) as u64;

    let mut run_idle = |rx: &mut Pio, pipe: &mut Vec<bool>, n: u64| {
        for _ in 0..n {
            pipe.push(true);
            let lvl = pipe.remove(0);
            rx.set_pin(RO, lvl);
            rx.step();
            rx.rx_pop(); // drain timeout markers
        }
    };

    run_idle(&mut rx, &mut pipe, lead_cycles);

    let mut frames = Vec::new();
    for rep in 0..repeats {
        let phase = phase_ns + phase_step_ns * rep as f64;
        let mut syms = Vec::new();
        let mut first_sym_us = None;
        for k in 0..burst_cycles {
            // Burst starts at FF-grid cycle 0 + phase: the waveform time
            // seen at grid instant k is k*CYCLE - phase (phase>0 delays
            // the burst within the grid).
            let t = (k as f64) * CYCLE - phase * 1e-9;
            let raw = if t < 0.0 { true } else { sample(edges, t) };
            pipe.push(raw);
            let lvl = pipe.remove(0);
            rx.set_pin(RO, lvl);
            rx.step();
            if let Some(w) = rx.rx_pop() {
                syms.push((w & 0x1F) as u8);
                if first_sym_us.is_none() {
                    first_sym_us = Some(t * 1e6);
                }
            }
        }
        frames.push(FrameResult { syms, first_sym_us, burst_len_us: burst_len * 1e6 });
        run_idle(&mut rx, &mut pipe, gap_cycles);
    }
    frames
}

fn fixture() -> (Vec<(f64, bool)>, f64) {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/stock_ns_125mhz_500msps.csv");
    let tr = load_transitions(path);
    assert!(tr.len() > 1000);
    burst_edges(&tr)
}

/// The capture taken at the FINAL bench config (2026-07-10 18:22,
/// cap_ns2): an NS frame from board -0's stock TX exactly as flashed —
/// the waveform board -1 is missing. Edge-list export (not sampled).
fn fixture_current() -> (Vec<(f64, bool)>, f64) {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/current_ns_125mhz_edges.csv");
    let tr = load_transitions(path);
    assert!(tr.len() > 500);
    burst_edges(&tr)
}

/// Same full-model sweep against the CURRENT capture.
#[test]
fn flashed_rx_current_capture_sweep() {
    let (edges, burst_len) = fixture_current();
    println!("current burst: {:.1}us, {} edges", burst_len * 1e6, edges.len());
    let mut any_bad = Vec::new();
    for phase4 in 0..32 {
        let phase = phase4 as f64 * 0.25;
        let mut verdicts = Vec::new();
        for seed in [1u64, 2, 3] {
            let frames =
                replay_frames(&flashed_code(), &edges, burst_len, 3, phase, 2, 1.5, seed, 200.0, 0.0);
            verdicts.extend(frames.iter().map(|f| f.verdict()));
        }
        let miss = verdicts.iter().filter(|&&v| v == Verdict::Miss).count();
        let bad = verdicts.iter().filter(|&&v| v != Verdict::Decoded).count();
        if bad > 0 {
            any_bad.push((phase, verdicts.clone()));
        }
        println!("  phase {phase:>5}ns: bad={bad} MISS={miss} (of {})", verdicts.len());
    }
    println!("phases with any non-decode: {}", any_bad.len());
}

/// The code that was ACTUALLY on board -1 (candidate C, startup [9][9])
/// against the capture of the frames it was missing. If the model shows
/// misses here, the bench mystery is fully closed: hardware behavior was
/// a property of the [9][9] build, and the sync-delay model captures it.
#[test]
fn candidate_c_reproduces_bench_miss() {
    let (edges, burst_len) = fixture_current();
    let mut summary = Vec::new();
    for sync_delay in [0usize, 2] {
        let mut miss = 0;
        let mut bad = 0;
        let mut total = 0;
        for phase4 in 0..32 {
            let phase = phase4 as f64 * 0.25;
            for seed in [1u64, 2, 3] {
                let frames = replay_frames(
                    &candidate_c_code(), &edges, burst_len, 3, phase, sync_delay, 1.5, seed, 200.0, 0.0,
                );
                for f in &frames {
                    total += 1;
                    match f.verdict() {
                        Verdict::Miss => {
                            miss += 1;
                            bad += 1;
                        }
                        Verdict::Decoded => {}
                        _ => bad += 1,
                    }
                }
            }
        }
        println!("candidate C sync_delay={sync_delay}: MISS={miss} bad={bad} of {total}");
        summary.push((sync_delay, miss, bad, total));
    }
    // Also detail one phase with the delay model for a look at the signature.
    let frames = replay_frames(&candidate_c_code(), &edges, burst_len, 3, 4.0, 2, 1.5, 7, 200.0, 0.0);
    for (i, f) in frames.iter().enumerate() {
        println!(
            "  frame {i}: {} syms, first@{:?}us, verdict {:?}, head {:02x?}",
            f.syms.len(),
            f.first_sym_us.map(|t| (t * 10.0).round() / 10.0),
            f.verdict(),
            &f.syms[..f.syms.len().min(6)]
        );
    }
}

/// Sweep phase finely with the full hardware model (2-cycle sync delay +
/// aperture), flashed code, 3 repeated frames. Hunt for the bench MISS.
#[test]
fn flashed_rx_bench_model_sweep() {
    let (edges, burst_len) = fixture();
    println!(
        "burst: {:.1}us, {} edges; flashed code (idx13 [2], idx18 [3], startup [7][7])",
        burst_len * 1e6,
        edges.len()
    );

    for (sync_delay, aperture) in [(0usize, 0.0f64), (2, 0.0), (2, 1.5)] {
        println!("== sync_delay={sync_delay} aperture={aperture}ns ==");
        let mut miss_phases = Vec::new();
        for phase4 in 0..32 {
            let phase = phase4 as f64 * 0.25;
            let seeds: &[u64] = if aperture > 0.0 { &[1, 2, 3] } else { &[1] };
            let mut verdicts = Vec::new();
            let mut detail = String::new();
            for &seed in seeds {
                let frames = replay_frames(
                    &flashed_code(), &edges, burst_len, 3, phase, sync_delay, aperture, seed, 200.0, 0.0,
                );
                for (i, f) in frames.iter().enumerate() {
                    let v = f.verdict();
                    if seed == seeds[0] && i == 0 {
                        detail = format!(
                            "{} syms, first@{:?}us of {:.0}us",
                            f.syms.len(),
                            f.first_sym_us.map(|t| (t * 10.0).round() / 10.0),
                            f.burst_len_us
                        );
                    }
                    verdicts.push(v);
                }
            }
            let miss = verdicts.iter().filter(|&&v| v == Verdict::Miss).count();
            let misal = verdicts.iter().filter(|&&v| v == Verdict::Misaligned).count();
            let garb = verdicts.iter().filter(|&&v| v == Verdict::Garbage).count();
            let dec = verdicts.iter().filter(|&&v| v == Verdict::Decoded).count();
            if miss > 0 {
                miss_phases.push(phase);
            }
            println!(
                "  phase {phase:>5}ns: dec={dec} misal={misal} garb={garb} MISS={miss} (of {}) [{detail}]",
                verdicts.len()
            );
        }
        println!("  -> phases with any MISS: {:?}", miss_phases);
    }
}

// ---------------------------------------------------------------------
// Post-transceiver sampled captures (ro_sampler.rs firmware, 2026-07-10
// night): PIO1 on board -1 raw-sampled its own RO pin at 125 Msps. The
// signal is duty-distorted ~+16..24ns on LOW runs / -16..24ns on HIGH
// runs (rising edges late; wire is pristine 40/40 per Saleae). Feeding
// these bitstreams into the emulated RX 1:1 needs NO phase/aperture
// modeling — this IS what the SM saw.
// ---------------------------------------------------------------------

/// Load an RLE fixture -> 8ns sample bitstream (starts at the trigger
/// falling edge; first run is LOW).
fn load_sampled(path: &str) -> Vec<bool> {
    let text = std::fs::read_to_string(path).expect("fixture readable");
    let line = text.lines().find(|l| !l.starts_with('#')).unwrap();
    let mut samples = Vec::new();
    let mut level = false;
    for run in line.split_whitespace() {
        let n: usize = run.parse().unwrap();
        samples.extend(std::iter::repeat(level).take(n));
        level = !level;
    }
    samples
}

/// Replay a sampled bitstream through a fresh RX instance directly.
fn replay_sampled(code: &(Vec<u16>, u8, u8), samples: &[bool]) -> Vec<u8> {
    let (code, wt, ws) = (code.0.clone(), code.1, code.2);
    let mut rx = build_rx(&code, wt, ws);
    rx.set_pin(RO, true);
    rx.enable();
    let mut syms = Vec::new();
    // idle lead-in so the RX reaches its parked startup wait
    for _ in 0..(40e-6 / CYCLE) as u64 {
        rx.step();
        rx.rx_pop();
    }
    for &s in samples {
        rx.set_pin(RO, s);
        rx.step();
        if let Some(w) = rx.rx_pop() {
            syms.push((w & 0x1F) as u8);
        }
    }
    // trailing idle to flush
    rx.set_pin(RO, true);
    for _ in 0..2000 {
        rx.step();
        if let Some(w) = rx.rx_pop() {
            syms.push((w & 0x1F) as u8);
        }
    }
    syms
}

/// Build a candidate RX with the four timing knobs:
/// d13 = found_low mid-bit delay (sample at fall + 4+d13 (+1 poll)),
/// d18 = wait_for_high mid-bit delay (sample at rise + 4+d18),
/// d23 = startup wait-low delay (scan low test at fall + d23+1),
/// d25 = startup wait-high delay (scan high test at rise + d25+1).
fn candidate(d13: u16, d18: u16, d23: u16, d25: u16) -> (Vec<u16>, u8, u8) {
    let (mut code, wt, ws) = slow_rx_code();
    patch_delay(&mut code, 13, d13);
    patch_delay(&mut code, 18, d18);
    patch_delay(&mut code, 23, d23);
    patch_delay(&mut code, 25, d25);
    (code, wt, ws)
}

/// Apply duty distortion to an RO edge list: delay rising edges by
/// `delta` seconds (measured hardware: rising late, falling on-time).
/// High pulses squeezed below ~1 sample vanish (observed on hardware).
fn distort(edges: &[(f64, bool)], delta: f64) -> Vec<(f64, bool)> {
    let mut out: Vec<(f64, bool)> = Vec::with_capacity(edges.len());
    for &(t, v) in edges {
        let t = if v { t + delta } else { t };
        // drop inverted pairs: a delayed rise that lands at/after the
        // following fall annihilates both (pulse vanished)
        if let Some(&(tp, vp)) = out.last() {
            if t <= tp + 4e-9 && v != vp {
                out.pop();
                continue;
            }
        }
        out.push((t, v));
    }
    out
}

/// THE closed-loop test: the emulated RX fed the actual post-pad signal
/// must behave like the hardware — miss the NS frames, decode cap89
/// (the 30s-cadence frame hardware accepts).
#[test]
fn sampled_captures_reproduce_hardware() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let cases = [
        ("ro_sampled_cap19_1145edges.txt", "NS-class"),
        ("ro_sampled_cap59_1133edges.txt", "NS-class"),
        ("ro_sampled_cap78_1121edges.txt", "NS-class"),
        ("ro_sampled_cap90_1143edges.txt", "NS-class"),
        ("ro_sampled_cap1_2337edges.txt", "host-132B"),
        ("ro_sampled_cap2_5109edges.txt", "host-304B"),
        ("ro_sampled_cap89_15843edges.txt", "giant-926B (hw DECODES this)"),
    ];
    for (file, label) in cases {
        let samples = load_sampled(&format!("{dir}{file}"));
        let syms = replay_sampled(&flashed_code(), &samples);
        let fr = FrameResult { syms: syms.clone(), first_sym_us: None, burst_len_us: 0.0 };
        // expected symbol count if fully decoded: edges/2/... ~ bits/5
        println!(
            "{label:32} {file}: {} samples -> {} syms, verdict {:?}, frac0 {:.2}, head {:02x?}",
            samples.len(),
            syms.len(),
            fr.verdict(),
            fr.frac(0),
            &syms[..syms.len().min(8)]
        );
    }
}

/// Score a candidate: fraction of trials fully decoded (frac0 > 0.97 at
/// offset 0 OR any fixed offset — the firmware BitRealigner absorbs
/// constant offsets, so alignment-only misses count as decoded).
fn decoded(syms: &[u8]) -> bool {
    let fr = FrameResult { syms: syms.to_vec(), first_sym_us: None, burst_len_us: 0.0 };
    (0..5).any(|off| fr.frac(off) > 0.97) && syms.len() > 20
}

/// Grid-search the four timing knobs against BOTH worlds:
/// - the 7 real sampled (distorted) captures, fed 1:1
/// - the clean wire capture, phase-swept, at duty distortion 0/8/16/24ns
#[test]
#[ignore] // search harness; run explicitly
fn duty_robust_candidate_search() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let sampled: Vec<Vec<bool>> = [
        "ro_sampled_cap19_1145edges.txt",
        "ro_sampled_cap59_1133edges.txt",
        "ro_sampled_cap78_1121edges.txt",
        "ro_sampled_cap90_1143edges.txt",
        "ro_sampled_cap1_2337edges.txt",
        "ro_sampled_cap2_5109edges.txt",
        "ro_sampled_cap89_15843edges.txt",
    ]
    .iter()
    .map(|f| load_sampled(&format!("{dir}{f}")))
    .collect();
    let (wire, burst_len) = fixture_current();

    let mut results: Vec<(u16, u16, u16, u16, usize, usize, usize)> = Vec::new();
    for d13 in [4u16, 5, 6] {
        for d18 in [0u16, 1, 2] {
            for d23 in [8u16, 9, 10] {
                for d25 in [3u16, 4, 5] {
                    let code = candidate(d13, d18, d23, d25);
                    // real distorted captures
                    let s_ok = sampled
                        .iter()
                        .filter(|s| decoded(&replay_sampled(&code, s)))
                        .count();
                    // wire, clean + synthetic duty, phase swept
                    let mut w_ok = 0;
                    let mut w_tot = 0;
                    for duty_ns in [0.0f64, 8.0, 16.0, 20.0, 24.0] {
                        let e = distort(&wire, duty_ns * 1e-9);
                        for phase4 in (0..32).step_by(4) {
                            let phase = phase4 as f64 * 0.25;
                            let frames =
                                replay_frames(&code, &e, burst_len, 1, phase, 0, 0.0, 1, 200.0, 0.0);
                            w_tot += 1;
                            if decoded(&frames[0].syms) {
                                w_ok += 1;
                            }
                        }
                    }
                    results.push((d13, d18, d23, d25, s_ok, w_ok, w_tot));
                }
            }
        }
    }
    results.sort_by_key(|r| std::cmp::Reverse((r.4, r.5)));
    println!("top candidates (d13 d18 d23 d25 : sampled/7  wire):");
    for r in results.iter().take(15) {
        println!(
            "  [{}] [{}] [{}] [{}] : {}/7  {}/{}",
            r.0, r.1, r.2, r.3, r.4, r.5, r.6
        );
    }
    // shipped + flashed baselines
    for (name, code) in [
        ("shipped", slow_rx_code()),
        ("flashed", flashed_code()),
    ] {
        let s_ok = sampled.iter().filter(|s| decoded(&replay_sampled(&code, s))).count();
        println!("baseline {name}: sampled {s_ok}/7");
    }
}

/// Post-ground-fix signal (ro_sampled2_*): the ±20ns skew was the Saleae
/// ground clip on one differential leg. Residual: lows 4-5/9-10, highs
/// 5-6/10-11 (~±4-8ns, opposite sign). Find timing that decodes the NEW
/// signal bit-perfect, and report robustness on the old distorted set.
#[test]
#[ignore]
fn ground_fixed_candidate_search() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let new_files: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().into_string().unwrap();
            n.starts_with("ro_sampled2_").then_some(n)
        })
        .collect();
    let old_files: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().into_string().unwrap();
            (n.starts_with("ro_sampled_cap")).then_some(n)
        })
        .collect();
    let load_all = |files: &[String]| -> Vec<Vec<bool>> {
        files.iter().map(|f| load_sampled(&format!("{dir}{f}"))).collect()
    };
    let new_s = load_all(&new_files);
    let old_s = load_all(&old_files);
    let perfect = |code: &(Vec<u16>, u8, u8), set: &[Vec<bool>]| -> (usize, f64) {
        let mut p = 0;
        let mut tot = 0.0;
        for s in set {
            let syms = replay_sampled(code, s);
            let fr = FrameResult { syms, first_sym_us: None, burst_len_us: 0.0 };
            let best = (0..5).map(|o| fr.frac(o)).fold(0.0f64, f64::max);
            tot += best;
            if best >= 0.999 {
                p += 1;
            }
        }
        (p, tot / set.len() as f64)
    };
    println!("baselines on NEW signal ({} fixtures):", new_s.len());
    for (name, code) in [
        ("shipped [1][2][5][5]", slow_rx_code()),
        ("aperture [2][3][5][5]", flashed_code()),
        ("duty-old [4][1][9][4]", candidate(4, 1, 9, 4)),
    ] {
        let (p, m) = perfect(&code, &new_s);
        println!("  {name}: perfect {p}/{}, mean {m:.4}", new_s.len());
    }
    let mut scored = Vec::new();
    for d13 in 1u16..=5 {
        for d18 in 0u16..=4 {
            for d23 in 4u16..=8 {
                for d25 in 3u16..=7 {
                    let code = candidate(d13, d18, d23, d25);
                    let (p_new, m_new) = perfect(&code, &new_s);
                    if p_new == new_s.len() {
                        let (p_old, m_old) = perfect(&code, &old_s);
                        scored.push((p_old, m_old, m_new, (d13, d18, d23, d25)));
                    } else if p_new + 1 >= new_s.len() {
                        scored.push((0, 0.0, m_new, (d13, d18, d23, d25)));
                    }
                }
            }
        }
    }
    scored.sort_by(|a, b| (b.0, b.1).partial_cmp(&(a.0, a.1)).unwrap());
    println!("candidates perfect on NEW (ranked by old-signal robustness):");
    for (p_old, m_old, m_new, (a, b, c, d)) in scored.iter().take(12) {
        println!(
            "  [{a}][{b}][{c}][{d}]: new mean {m_new:.4}, old perfect {p_old}/7 mean {m_old:.4}"
        );
    }
}

/// Tie-break finalists on the wire capture: phase sweep x duty sweep
/// including NEGATIVE duty (rising early — the post-ground-fix sign).
#[test]
#[ignore]
fn finalist_wire_battery() {
    let (wire, burst_len) = fixture_current();
    for (d13, d18, d23, d25) in
        [(1u16, 4u16, 4u16, 7u16), (2, 4, 4, 7), (3, 4, 4, 4), (2, 4, 8, 6)]
    {
        let code = candidate(d13, d18, d23, d25);
        let mut ok = 0;
        let mut tot = 0;
        let mut fail_at: Vec<(f64, f64)> = Vec::new();
        for duty_ns in [-12.0f64, -8.0, -4.0, 0.0, 4.0, 8.0, 12.0, 16.0, 20.0, 24.0] {
            let e = distort(&wire, duty_ns * 1e-9);
            for phase4 in (0..32).step_by(2) {
                let phase = phase4 as f64 * 0.25;
                let frames = replay_frames(&code, &e, burst_len, 1, phase, 0, 0.0, 1, 200.0, 0.0);
                tot += 1;
                if decoded(&frames[0].syms) {
                    ok += 1;
                } else if fail_at.len() < 6 {
                    fail_at.push((duty_ns, phase));
                }
            }
        }
        println!("[{d13}][{d18}][{d23}][{d25}]: {ok}/{tot} wire trials; first fails {fail_at:?}");
    }
}

/// Per-fixture detail for a specific candidate.
#[test]
#[ignore]
fn duty_robust_candidate_detail() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let files = [
        "ro_sampled_cap19_1145edges.txt",
        "ro_sampled_cap59_1133edges.txt",
        "ro_sampled_cap78_1121edges.txt",
        "ro_sampled_cap90_1143edges.txt",
        "ro_sampled_cap1_2337edges.txt",
        "ro_sampled_cap2_5109edges.txt",
        "ro_sampled_cap89_15843edges.txt",
    ];
    // fine grid around the winner; score = mean best-offset frac + perfect count
    let mut scored: Vec<(f64, usize, (u16, u16, u16, u16))> = Vec::new();
    for d13 in [3u16, 4] {
        for d18 in [0u16, 1, 2] {
            for d23 in [7u16, 8, 9] {
                for d25 in [2u16, 3, 4] {
                    let code = candidate(d13, d18, d23, d25);
                    let mut tot = 0.0;
                    let mut perfect = 0;
                    for f in files {
                        let samples = load_sampled(&format!("{dir}{f}"));
                        let syms = replay_sampled(&code, &samples);
                        let fr = FrameResult { syms, first_sym_us: None, burst_len_us: 0.0 };
                        let best = (0..5).map(|o| fr.frac(o)).fold(0.0f64, f64::max);
                        tot += best;
                        if best >= 0.999 {
                            perfect += 1;
                        }
                    }
                    scored.push((tot, perfect, (d13, d18, d23, d25)));
                }
            }
        }
    }
    scored.sort_by(|a, b| (b.1, b.0).partial_cmp(&(a.1, a.0)).unwrap());
    println!("fine grid (perfect-count, mean-frac):");
    for (tot, perfect, (a, b, c, d)) in scored.iter().take(10) {
        println!("  [{a}][{b}][{c}][{d}]: perfect {perfect}/7, mean {:.4}", tot / 7.0);
    }

    for (d13, d18, d23, d25) in [(4u16, 1u16, 8u16, 3u16), (5, 1, 9, 4), (6, 1, 9, 4)] {
        println!("== candidate [{d13}][{d18}][{d23}][{d25}] ==");
        let code = candidate(d13, d18, d23, d25);
        for f in files {
            let samples = load_sampled(&format!("{dir}{f}"));
            let syms = replay_sampled(&code, &samples);
            let fr = FrameResult { syms: syms.clone(), first_sym_us: None, burst_len_us: 0.0 };
            let best = (0..5)
                .map(|o| (o, fr.frac(o)))
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .unwrap();
            println!(
                "  {f}: {} syms, best off {} frac {:.3}, head {:02x?}",
                syms.len(),
                best.0,
                best.1,
                &syms[..syms.len().min(8)]
            );
        }
    }
}

/// Does carried state across frames change anything vs fresh boot?
/// Frame 1 vs frames 2..6 at a few phases, deterministic model.
#[test]
fn carried_state_vs_fresh() {
    let (edges, burst_len) = fixture();
    for phase in [0.0f64, 2.0, 4.0, 6.0] {
        let frames = replay_frames(&flashed_code(), &edges, burst_len, 6, phase, 2, 0.0, 1, 200.0, 0.0);
        let vs: Vec<Verdict> = frames.iter().map(|f| f.verdict()).collect();
        let counts: Vec<usize> = frames.iter().map(|f| f.syms.len()).collect();
        println!("phase {phase}ns: verdicts {vs:?} sym-counts {counts:?}");
        assert!(
            vs.windows(2).all(|w| w[0] == w[1]),
            "frame verdict varies with carried state at phase {phase}: {vs:?}"
        );
    }
}
