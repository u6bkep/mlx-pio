//! The search subject: a fixed 32-slot PIO program plus the SM config
//! that co-determines behavior. This is the MCMC genome.
//!
//! Slots never move, so jump/wrap targets are absolute slot indices that
//! stay valid under mutation (slot index == hardware instruction address).
//! An empty slot (`None`) encodes to a NOP.

use crate::decode::{decode_insn, DecodeError};
use crate::encode::encode_insn;
use crate::ir::{Insn, SideCfg};

/// ISR/OSR shift direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftDir {
    Left,
    Right,
}

/// SHIFTCTRL genes: autopush/pull, their thresholds (1..=32), shift
/// directions, and FIFO join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShiftCfg {
    pub autopush: bool,
    pub autopull: bool,
    pub push_threshold: u8, // 1..=32
    pub pull_threshold: u8, // 1..=32
    pub in_dir: ShiftDir,
    pub out_dir: ShiftDir,
    pub fjoin_rx: bool,
    pub fjoin_tx: bool,
}

impl Default for ShiftCfg {
    fn default() -> Self {
        ShiftCfg {
            autopush: false,
            autopull: false,
            push_threshold: 32,
            pull_threshold: 32,
            in_dir: ShiftDir::Right,
            out_dir: ShiftDir::Right,
            fjoin_rx: false,
            fjoin_tx: false,
        }
    }
}

/// PINCTRL pin bases/counts. Treated as part of the fixed per-target
/// contract (board wiring), not search genes, but carried here so a
/// `Program` is self-describing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PinMap {
    pub out_base: u8,
    pub out_count: u8,
    pub set_base: u8,
    pub set_count: u8,
    pub in_base: u8,
    pub sideset_base: u8,
}

/// The full SM configuration. Which fields are mutated (genes) vs held
/// fixed (contract) is the mutation operator's concern; the type carries
/// all of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub side: SideCfg,
    pub side_pindir: bool,
    /// Clock divider integer part. Must be >= 1 (the emulator interprets 0
    /// as 65536; the search has no reason to go there).
    pub clkdiv_int: u16,
    pub clkdiv_frac: u8,
    pub shift: ShiftCfg,
    pub pins: PinMap,
    pub jmp_pin: u8,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            side: SideCfg::NONE,
            side_pindir: false,
            clkdiv_int: 1,
            clkdiv_frac: 0,
            shift: ShiftCfg::default(),
            pins: PinMap::default(),
            jmp_pin: 0,
        }
    }
}

/// A 32-slot program with wrap bounds and config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub slots: [Option<Insn>; 32],
    /// Wrap target/source as slot indices (EXECCTRL WRAP_BOTTOM/TOP).
    pub wrap_bottom: u8,
    pub wrap_top: u8,
    pub config: Config,
}

impl Program {
    /// Empty program (all NOP) with the given config.
    pub fn empty(config: Config) -> Self {
        Program { slots: [const { None }; 32], wrap_bottom: 0, wrap_top: 31, config }
    }

    /// `(min, max)` slot index in use, or `None` if the program is empty.
    pub fn span(&self) -> Option<(u8, u8)> {
        let used: Vec<u8> = (0..32u8).filter(|&i| self.slots[i as usize].is_some()).collect();
        match (used.first(), used.last()) {
            (Some(&lo), Some(&hi)) => Some((lo, hi)),
            _ => None,
        }
    }

    /// Adopted size metric ①: occupied **span** in instruction-memory
    /// words = `max_used - min_used + 1` (interior NOPs are real flash
    /// words and count). 0 for an empty program.
    pub fn size(&self) -> u8 {
        match self.span() {
            Some((lo, hi)) => hi - lo + 1,
            None => 0,
        }
    }

    /// Encode all 32 slots to machine words (empty slots -> NOP). Targets
    /// are absolute slot indices, loaded as-is at instruction address ==
    /// slot index (no relocation).
    pub fn assemble(&self) -> [u16; 32] {
        let mut out = [0u16; 32];
        let nop = Insn::nop_for(&self.config.side);
        for (i, slot) in self.slots.iter().enumerate() {
            let insn = slot.as_ref().unwrap_or(&nop);
            out[i] = encode_insn(insn, &self.config.side);
        }
        out
    }

    /// Validate the whole genome (decision ②: illegal states are explicit,
    /// not silently legalized). Checks config gene ranges, wrap/target
    /// bounds, and every slot against the side-set budget.
    pub fn validate(&self) -> Result<(), String> {
        let c = &self.config;
        if c.clkdiv_int == 0 {
            return Err("clkdiv_int must be >= 1".into());
        }
        if !(1..=32).contains(&c.shift.push_threshold) || !(1..=32).contains(&c.shift.pull_threshold) {
            return Err("shift thresholds must be 1..=32".into());
        }
        if c.side.count > 5 {
            return Err("sideset_count must be <= 5".into());
        }
        if self.wrap_bottom > 31 || self.wrap_top > 31 {
            return Err("wrap bounds must be slot indices 0..=31".into());
        }
        for (i, slot) in self.slots.iter().enumerate() {
            if let Some(insn) = slot {
                insn.validate(&c.side).map_err(|e| format!("slot {i}: {e}"))?;
                // A jump target must reference a real slot index.
                if let crate::ir::Op::Jmp { target, .. } = insn.op {
                    if target > 31 {
                        return Err(format!("slot {i}: jmp target {target} > 31"));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Import a reference program (contiguous from address 0) into the IR for
/// optimization-mode seeding. `words` occupy slots `0..words.len()`; the
/// rest are empty. Fails if any word is not legal IR (e.g. reserved code).
pub fn import_program(
    words: &[u16],
    wrap_bottom: u8,
    wrap_top: u8,
    config: Config,
) -> Result<Program, DecodeError> {
    assert!(words.len() <= 32, "program exceeds 32 instruction slots");
    let mut slots: [Option<Insn>; 32] = [const { None }; 32];
    for (i, &w) in words.iter().enumerate() {
        slots[i] = Some(decode_insn(w, &config.side)?);
    }
    Ok(Program { slots, wrap_bottom, wrap_top, config })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;

    fn set(slot: &mut [Option<Insn>; 32], i: usize, op: Op) {
        slot[i] = Some(Insn::plain(op));
    }

    #[test]
    fn span_counts_interior_gaps() {
        let mut p = Program::empty(Config::default());
        set(&mut p.slots, 0, Op::Set { dst: SetDst::X, data: 1 });
        set(&mut p.slots, 17, Op::Set { dst: SetDst::Y, data: 2 });
        // Two real instructions, but the deployable footprint is 18 words.
        assert_eq!(p.span(), Some((0, 17)));
        assert_eq!(p.size(), 18);
    }

    #[test]
    fn empty_program_has_zero_size() {
        let p = Program::empty(Config::default());
        assert_eq!(p.span(), None);
        assert_eq!(p.size(), 0);
    }

    #[test]
    fn import_then_assemble_round_trips() {
        // A tiny program: PULL block; OUT PINS, 1; JMP 0.
        let prog = [0x80A0u16, 0x6001, 0x0000];
        let p = import_program(&prog, 0, 2, Config::default()).unwrap();
        let asm = p.assemble();
        assert_eq!(&asm[0..3], &prog, "imported words re-assemble identically");
        // Slots beyond the program are NOP.
        assert_eq!(asm[3], encode_insn(&Insn::nop(), &SideCfg::NONE));
        assert_eq!(p.size(), 3);
    }

    #[test]
    fn validate_rejects_clkdiv_zero_and_overbudget() {
        let p = Program::empty(Config { clkdiv_int: 0, ..Config::default() });
        assert!(p.validate().is_err());

        // Over-budget delay for sideset_count=5 (delay_bits=0, max delay 0).
        let mut cfg = Config::default();
        cfg.side = SideCfg { count: 5, en: false };
        let mut p2 = Program::empty(cfg);
        p2.slots[0] = Some(Insn { op: Op::Set { dst: SetDst::X, data: 0 }, delay: 1, sideset: Some(0) });
        assert!(p2.validate().is_err(), "delay 1 must not fit when count=5");

        // The same instruction with delay 0 and a valid side-set is fine.
        p2.slots[0] = Some(Insn { op: Op::Set { dst: SetDst::X, data: 0 }, delay: 0, sideset: Some(0) });
        assert!(p2.validate().is_ok());
    }

    #[test]
    fn import_rejects_reserved() {
        // MOV op = reserved 0b11.
        let bad = [0b101_00000_000_11_000u16];
        assert!(import_program(&bad, 0, 0, Config::default()).is_err());
    }
}
