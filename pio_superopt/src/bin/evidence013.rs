//! Ticket 013 evidence checks (2026-07-16), run on the tx_a narrow spec:
//!
//! A. Decode the w02 "CL7 d≡d+24" twins: +24 in the 5-bit shared field is
//!    NOT delay arithmetic under `.side_set 1 opt` — it is side-set
//!    enable(bit12)+side1(bit11). Confirm by encoding.
//! B. The irq schedule has no 24-cycle period (kills the phase-congruence
//!    reading independently).
//! C. Trace differential over sampled completions: `wait 1 irq 0 side 1`
//!    vs plain `wait 1 irq 0` (side 1 re-writes the idle-high DI latch —
//!    expected invisible), with `side 0` as the must-diverge control.
//! D. Self-sync mechanism (013 v2): a downstream stalling WAIT absorbs the
//!    slot-0 delay shift; a downstream visible write does not.
//!
//! Extra mode: `--pair <decided-hex> <v1-hex> <v2-hex>` runs the sampled
//! completion differential on a REAL w02 seed pair (slot-0 decided bits
//! from the two units, shared random fill for undecided bits).
//!
//! Usage: cargo run --release --bin evidence013 [-- --pair f8ff 20c0 38c0]

use pio_superopt::encode::encode_insn;
use pio_superopt::fixtures::{tx_a_narrow_spec, tx_a_narrow_words};
use pio_superopt::ir::{Insn, MovDst, MovOp, MovSrc, Op, WaitSrc};
use pio_superopt::narrow::engine::run_spec_oob;

fn main() {
    let cycles = 460u32;
    let (mut spec, side) = tx_a_narrow_spec(cycles);
    spec.slots = 3;
    spec.cfg.wrap_bottom = 0;
    spec.cfg.wrap_top = 2;
    let base = tx_a_narrow_words(&side); // nop-filled beyond the candidates

    let mut rng: u32 = 0x1357_2468;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        rng
    };
    let run3 = |w0: u16, w1: u16, w2: u16| {
        let mut c = base;
        c[0] = w0;
        c[1] = w1;
        c[2] = w2;
        run_spec_oob(&spec, c)
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--pair") {
        let hex = |s: &String| u16::from_str_radix(s, 16).expect("hex");
        let (decided, v1, v2) = (hex(&args[1]), hex(&args[2]), hex(&args[3]));
        let mut mism = 0;
        let mut total = 0;
        for _ in 0..500 {
            let fill = next() as u16 & !decided;
            let (w1, w2) = (next() as u16, next() as u16);
            let a = run3((v1 & decided) | fill, w1, w2);
            let b = run3((v2 & decided) | fill, w1, w2);
            total += 1;
            if a != b {
                mism += 1;
            }
        }
        println!(
            "pair decided={decided:04x} v1={v1:04x} v2={v2:04x}: trace mismatches {mism}/{total}"
        );
        return;
    }

    // A. encodings of the twin shapes
    let wait1irq0 = Op::Wait { polarity: true, src: WaitSrc::Irq, index: 0 };
    let enc = |sideset| encode_insn(&Insn { op: wait1irq0.clone(), delay: 0, sideset }, &side);
    println!(
        "A. wait 1 irq 0: side1={:#06x} side0={:#06x} plain={:#06x}  (w02 twins: 0x38c0 / 0x20c0; shipped tx_a slot0 = side1)",
        enc(Some(1)),
        enc(Some(0)),
        enc(None)
    );

    // B. irq schedule periodicity
    let gaps: Vec<u32> = spec.irq_sets.windows(2).map(|w| w[1].0 - w[0].0).collect();
    println!(
        "B. irq gaps: {:?}  any-24-period={}",
        gaps,
        gaps.iter().all(|&g| g % 24 == 0)
    );

    // idle premise: nothing drives pin 0 -> captured level?
    let nop = base[10];
    let (t, _) = run3(nop, nop, nop);
    println!(
        "   idle pin0 level (no side-set anywhere): cycle0={} constant={}",
        t[0] & 1,
        t.iter().all(|&s| s & 1 == t[0] & 1)
    );

    // C. side1-vs-plain differential over sampled completions
    let (mut m_side1, mut m_side0, mut total) = (0u32, 0u32, 0u32);
    for _ in 0..400 {
        let (w1, w2) = (next() as u16, next() as u16);
        for d in 0..8u16 {
            let a = run3(0x38C0 | (d << 8), w1, w2); // side 1
            let b = run3(0x20C0 | (d << 8), w1, w2); // plain
            let c = run3(0x30C0 | (d << 8), w1, w2); // side 0 (control)
            total += 1;
            if a != b {
                m_side1 += 1;
            }
            if b != c {
                m_side0 += 1;
            }
        }
    }
    println!(
        "C. side1-vs-plain mismatches: {m_side1}/{total}   side0-vs-plain control: {m_side0}/{total}"
    );

    // D. self-sync: slot-0 delay shift vs downstream stall / visible write
    let stall = enc(None); // wait 1 irq 0, no side
    let vis = encode_insn(
        &Insn {
            op: Op::Mov { dst: MovDst::Y, op: MovOp::None, src: MovSrc::Y },
            delay: 0,
            sideset: Some(0),
        },
        &side,
    ); // nop side 0: drives DI low = visible edge
    for (name, s1, s2) in [("wait-then-vis", stall, vis), ("vis-then-wait", vis, stall)] {
        let t0 = run3(0x20C0, s1, s2);
        let diverging: Vec<u16> =
            (1..8u16).filter(|&d| run3(0x20C0 | (d << 8), s1, s2) != t0).collect();
        println!("D. [{name}] slot0 delays diverging from d=0: {diverging:?}");
    }
}
