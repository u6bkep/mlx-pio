//! Decode a 16-bit PIO machine word back into the symbolic [`Insn`] IR,
//! and import a whole reference program into a [`crate::program::Program`].
//!
//! This is the inverse of [`crate::encode`]. It exists for *optimization
//! mode*: seed the search from a known-correct reference program and shrink
//! it. Reserved field codes are a hard error — a real reference never
//! contains them, and silently coercing would corrupt the seed.

use crate::ir::*;

/// Why a word could not be decoded into legal IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    pub word: u16,
    pub reason: &'static str,
}

fn err(word: u16, reason: &'static str) -> DecodeError {
    DecodeError { word, reason }
}

/// Split the 5-bit delay/side-set field into semantic `(delay, sideset)`.
/// Exact inverse of `encode::pack_delay_sideset` and a mirror of the
/// emulator's decoder.
fn unpack_delay_sideset(field: u8, side: &SideCfg) -> (u8, Option<u8>) {
    let count = side.count.min(5);
    if count == 0 {
        return (field & 0x1F, None);
    }
    let delay_bits = 5 - count;
    let delay = field & ((1u8 << delay_bits) - 1);
    let ss_raw = field >> delay_bits; // exactly `count` bits remain

    let sideset = if side.en {
        let enable = (ss_raw >> (count - 1)) & 1 != 0;
        if enable {
            let val_bits = count - 1;
            let mask = if val_bits == 0 { 0 } else { (1u8 << val_bits) - 1 };
            Some(ss_raw & mask)
        } else {
            None
        }
    } else {
        Some(ss_raw & ((1u8 << count) - 1))
    };
    (delay, sideset)
}

/// Decode one machine word given the SM's side-set config.
pub fn decode_insn(word: u16, side: &SideCfg) -> Result<Insn, DecodeError> {
    let opcode = (word >> 13) & 0x7;
    let field = ((word >> 8) & 0x1F) as u8;
    let operand = (word & 0xFF) as u8;
    let (delay, sideset) = unpack_delay_sideset(field, side);

    let bitcount = |raw: u8| if raw == 0 { 32 } else { raw };

    let op = match opcode {
        0 => Op::Jmp {
            cond: JmpCond::from_code((operand >> 5) & 0x7).ok_or(err(word, "jmp cond"))?,
            target: operand & 0x1F,
        },
        1 => Op::Wait {
            polarity: (operand >> 7) & 1 != 0,
            src: WaitSrc::from_code((operand >> 5) & 0x3).ok_or(err(word, "wait src reserved"))?,
            index: operand & 0x1F,
        },
        2 => Op::In {
            src: InSrc::from_code((operand >> 5) & 0x7).ok_or(err(word, "in src reserved"))?,
            count: bitcount(operand & 0x1F),
        },
        3 => Op::Out {
            dst: OutDst::from_code((operand >> 5) & 0x7).ok_or(err(word, "out dst reserved"))?,
            count: bitcount(operand & 0x1F),
        },
        4 => {
            // PUSH/PULL must have operand bits [4:0] == 0. A nonzero low
            // field is a different RP2350 instruction (FIFO PUT/GET) the
            // IR does not model — refuse rather than mis-decode.
            if operand & 0x1F != 0 {
                return Err(err(word, "opcode 0b100 with nonzero low bits (PUT/GET unsupported)"));
            }
            let block = (operand >> 5) & 1 != 0;
            let if_x = (operand >> 6) & 1 != 0;
            if (operand >> 7) & 1 != 0 {
                Op::Pull { if_empty: if_x, block }
            } else {
                Op::Push { if_full: if_x, block }
            }
        }
        5 => Op::Mov {
            dst: MovDst::from_code((operand >> 5) & 0x7).ok_or(err(word, "mov dst reserved"))?,
            op: MovOp::from_code((operand >> 3) & 0x3).ok_or(err(word, "mov op reserved"))?,
            src: MovSrc::from_code(operand & 0x7).ok_or(err(word, "mov src reserved"))?,
        },
        6 => {
            if (operand >> 7) & 1 != 0 {
                return Err(err(word, "irq with reserved bit 7 set"));
            }
            Op::Irq {
                clear: (operand >> 6) & 1 != 0,
                wait: (operand >> 5) & 1 != 0,
                index: operand & 0x1F,
            }
        }
        _ => Op::Set {
            dst: SetDst::from_code((operand >> 5) & 0x7).ok_or(err(word, "set dst reserved"))?,
            data: operand & 0x1F,
        },
    };

    Ok(Insn { op, delay, sideset })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All legal-code sets, asserted independently of round-trip so a
    /// missing/extra variant is caught directly (the gap the review found).
    #[test]
    fn legal_code_sets_match_datasheet() {
        let ok = |present: &'static [u8]| move |c: u8| present.contains(&c);

        for c in 0..8 {
            assert_eq!(JmpCond::from_code(c).is_some(), ok(&[0, 1, 2, 3, 4, 5, 6, 7])(c), "jmp {c}");
            assert_eq!(InSrc::from_code(c).is_some(), ok(&[0, 1, 2, 3, 6, 7])(c), "in {c}");
            assert_eq!(OutDst::from_code(c).is_some(), ok(&[0, 1, 2, 3, 4, 5, 6, 7])(c), "out {c}");
            assert_eq!(MovDst::from_code(c).is_some(), ok(&[0, 1, 2, 3, 4, 5, 6, 7])(c), "movdst {c}");
            assert_eq!(MovSrc::from_code(c).is_some(), ok(&[0, 1, 2, 3, 5, 6, 7])(c), "movsrc {c}");
            assert_eq!(SetDst::from_code(c).is_some(), ok(&[0, 1, 2, 4])(c), "set {c}");
        }
        for c in 0..4 {
            assert_eq!(WaitSrc::from_code(c).is_some(), ok(&[0, 1, 2, 3])(c), "wait {c}");
            assert_eq!(MovOp::from_code(c).is_some(), ok(&[0, 1, 2])(c), "movop {c}");
        }
    }

    #[test]
    fn reserved_words_rejected() {
        let none = SideCfg::NONE;
        // MOV with op = 0b11 (reserved): opcode 101, op bits [4:3] = 11.
        assert!(decode_insn(0b101_00000_000_11_000, &none).is_err());
        // PUSH-opcode with nonzero low bits (PUT/GET): opcode 100, low=1.
        assert!(decode_insn(0b100_00000_000_00001, &none).is_err());
    }
}
