//! Scoring a candidate program against a reference waveform.
//!
//! Phase-1 metric (agreed): **strict cycle-aligned Hamming distance** on
//! the captured pins. The golden waveform comes from running a
//! known-correct reference *in the same emulator* (the reference-oracle
//! approach), so there is no sim/HW gap during search. Edge-distance and
//! timing tolerance come later.

use crate::program::Program;
use crate::run::{run, RunSpec};

/// Strict cycle-aligned Hamming distance between two captured waveforms.
/// Each sample already packs only the meaningful bits (pin levels and
/// output-enables), so every differing bit counts. Differing lengths are
/// compared against an implicit 0 (penalising a candidate that halts early
/// or runs long).
pub fn hamming(golden: &[u32], candidate: &[u32]) -> u32 {
    let n = golden.len().max(candidate.len());
    (0..n)
        .map(|i| {
            let g = golden.get(i).copied().unwrap_or(0);
            let c = candidate.get(i).copied().unwrap_or(0);
            (g ^ c).count_ones()
        })
        .sum()
}

/// The decomposed score of a candidate. The MH loop combines these into a
/// scalar (correctness gated ahead of size); kept separate here so the
/// weighting policy lives with the search, not the metric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Score {
    /// `false` if the genome is illegal (decision ②: invalid != clamped).
    pub valid: bool,
    /// Strict Hamming vs golden; 0 means the waveform matches exactly.
    pub correctness: u32,
    /// Occupied span (instruction-memory footprint).
    pub size: u8,
}

/// Score `program` against a `golden` waveform produced under `spec`.
pub fn score(program: &Program, golden: &[u32], spec: &RunSpec) -> Score {
    if program.validate().is_err() {
        return Score { valid: false, correctness: u32::MAX, size: program.size() };
    }
    let wave = run(program, spec);
    Score {
        valid: true,
        correctness: hamming(golden, &wave),
        size: program.size(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;
    use crate::program::*;

    const DATA: u8 = 0;
    const CLK: u8 = 1;

    /// Canonical SPI-TX (mode 0, MSB-first, 8-bit): clock via mandatory
    /// 1-bit side-set, data via OUT PINS. The known ~2-instruction optimum.
    ///
    ///   out pins, 1  side 0   ; present bit, clock low
    ///   nop          side 1   ; clock high (sample edge)
    fn spi_reference() -> Program {
        let mut p = Program::empty(Config {
            side: SideCfg { count: 1, en: false }, // clock = mandatory side-set
            clkdiv_int: 1,
            clkdiv_frac: 0,
            shift: ShiftCfg {
                autopull: true,
                pull_threshold: 8,
                out_dir: ShiftDir::Left, // MSB first
                ..ShiftCfg::default()
            },
            pins: PinMap { out_base: DATA, out_count: 1, sideset_base: CLK, ..PinMap::default() },
            ..Config::default()
        });
        p.slots[0] = Some(Insn {
            op: Op::Out { dst: OutDst::Pins, count: 1 },
            delay: 0,
            sideset: Some(0),
        });
        p.slots[1] = Some(Insn {
            op: Op::Mov { dst: MovDst::Y, op: MovOp::None, src: MovSrc::Y }, // nop
            delay: 0,
            sideset: Some(1),
        });
        p.wrap_bottom = 0;
        p.wrap_top = 1;
        p
    }

    fn spi_spec() -> RunSpec {
        RunSpec {
            block: 0,
            sm: 0,
            // MSB-first (Left shift) outputs the high bits first, so the
            // byte is left-justified in the 32-bit FIFO word (0xA5 << 24).
            inputs: vec![0xA5 << 24],
            output_pins: vec![DATA, CLK],
            capture_pins: vec![DATA, CLK], // bit0 = data, bit1 = clock
            cycles: 24,
        }
    }

    #[test]
    fn reference_is_legal_and_minimal() {
        let p = spi_reference();
        assert!(p.validate().is_ok(), "{:?}", p.validate());
        assert_eq!(p.size(), 2, "the SPI optimum is 2 instruction slots");
    }

    #[test]
    fn reference_produces_a_real_clock() {
        let golden = run(&spi_reference(), &spi_spec());
        // Clock is bit 1. It must actually toggle.
        let clk_edges = golden
            .windows(2)
            .filter(|w| (w[0] >> 1) & 1 != (w[1] >> 1) & 1)
            .count();
        assert!(clk_edges >= 4, "expected a toggling clock, got {clk_edges} edges");
        // Data line (bit 0) must show activity for 0xA5 (not stuck).
        assert!(golden.iter().any(|s| s & 1 != 0), "data line never went high");
    }

    #[test]
    fn reference_scores_zero_against_itself() {
        let spec = spi_spec();
        let golden = run(&spi_reference(), &spec);
        let s = score(&spi_reference(), &golden, &spec);
        assert_eq!(s, Score { valid: true, correctness: 0, size: 2 });
    }

    #[test]
    fn broken_clock_scores_nonzero() {
        let spec = spi_spec();
        let golden = run(&spi_reference(), &spec);
        // Perturb: clock-high instruction now also side-sets 0, so the
        // clock never rises — a wrong waveform.
        let mut bad = spi_reference();
        if let Some(insn) = &mut bad.slots[1] {
            insn.sideset = Some(0);
        }
        let s = score(&bad, &golden, &spec);
        assert!(s.valid, "still a legal program");
        assert!(s.correctness > 0, "broken clock must diverge from golden");
    }

    #[test]
    fn illegal_program_is_invalid_not_scored() {
        let spec = spi_spec();
        let golden = run(&spi_reference(), &spec);
        let mut bad = spi_reference();
        bad.config.clkdiv_int = 0; // invalid
        let s = score(&bad, &golden, &spec);
        assert!(!s.valid);
        assert_eq!(s.correctness, u32::MAX);
    }
}
