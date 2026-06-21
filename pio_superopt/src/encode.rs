//! Encode the symbolic [`Insn`] IR to the 16-bit PIO machine format.
//!
//! Encoding is total: out-of-range `delay`, side-set values, and counts
//! are masked to their field widths. The interesting work is packing
//! `delay` and `sideset` into the shared 5-bit field according to the
//! SM's [`SideCfg`] — the exact inverse of the emulator's decoder.

use crate::ir::*;

/// Encode one instruction to its 16-bit machine word, given the
/// side-set configuration that governs the delay/side-set split.
pub fn encode_insn(insn: &Insn, side: &SideCfg) -> u16 {
    let (opcode, operand) = encode_op(&insn.op);
    let field = pack_delay_sideset(insn.delay, insn.sideset, side);
    ((opcode as u16) << 13) | ((field as u16) << 8) | (operand as u16)
}

/// Returns `(opcode[2:0], operand[7:0])` for an [`Op`].
fn encode_op(op: &Op) -> (u8, u8) {
    match *op {
        Op::Jmp { cond, target } => (0, ((cond as u8) << 5) | (target & 0x1F)),
        Op::Wait { polarity, src, index } => {
            (1, ((polarity as u8) << 7) | ((src as u8) << 5) | (index & 0x1F))
        }
        Op::In { src, count } => (2, ((src as u8) << 5) | enc_count(count)),
        Op::Out { dst, count } => (3, ((dst as u8) << 5) | enc_count(count)),
        Op::Push { if_full, block } => {
            (4, (0 << 7) | ((if_full as u8) << 6) | ((block as u8) << 5))
        }
        Op::Pull { if_empty, block } => {
            (4, (1 << 7) | ((if_empty as u8) << 6) | ((block as u8) << 5))
        }
        Op::Mov { dst, op, src } => {
            (5, ((dst as u8) << 5) | ((op as u8) << 3) | (src as u8))
        }
        Op::Irq { clear, wait, index } => {
            (6, ((clear as u8) << 6) | ((wait as u8) << 5) | (index & 0x1F))
        }
        Op::Set { dst, data } => (7, ((dst as u8) << 5) | (data & 0x1F)),
    }
}

/// A bit count of 1..=32 encodes into a 5-bit field with 32 -> 0.
fn enc_count(count: u8) -> u8 {
    if count == 32 { 0 } else { count & 0x1F }
}

/// Pack `delay` (0..=31) and `sideset` into the 5-bit delay/side-set
/// field per `side`. Total: over-budget values are masked. Coercions
/// (e.g. `None` when side-set is mandatory) follow the hardware's view.
fn pack_delay_sideset(delay: u8, sideset: Option<u8>, side: &SideCfg) -> u8 {
    let count = side.count.min(5);
    if count == 0 {
        // Whole field is delay; any side-set value is dropped.
        return delay & 0x1F;
    }
    let delay_bits = 5 - count;
    let delay_mask = (1u8 << delay_bits) - 1;
    let d = delay & delay_mask;

    let ss_raw: u8 = if side.en {
        match sideset {
            Some(v) => {
                // Top bit = enable(1); the remaining `count-1` bits = value.
                let val_bits = count - 1;
                let val_mask = if val_bits == 0 { 0 } else { (1u8 << val_bits) - 1 };
                (1u8 << (count - 1)) | (v & val_mask)
            }
            None => 0, // enable bit clear, value bits don't matter
        }
    } else {
        // Side-set is mandatory every instruction; `None` -> 0.
        let val_mask = (1u8 << count) - 1;
        sideset.unwrap_or(0) & val_mask
    };

    (ss_raw << delay_bits) | d
}

#[cfg(test)]
mod tests {
    use super::*;
    use picoem_common::pio::decode::{decode, PioOp};

    /// Build a PINCTRL with the given SIDESET_COUNT (bits [31:29]).
    fn pinctrl(sideset_count: u8) -> u32 {
        (sideset_count as u32 & 0x7) << 29
    }
    /// Build an EXECCTRL with SIDE_EN (bit 30) per `en`.
    fn execctrl(en: bool) -> u32 {
        (en as u32) << 30
    }

    // ---- canonical hex spot-checks ------------------------------------

    #[test]
    fn canonical_encodings() {
        let none = SideCfg::NONE;
        // NOP = MOV Y, Y
        assert_eq!(encode_insn(&Insn::nop(), &none), 0xA042);
        // MOV Y, ~X
        let mov = Insn::plain(Op::Mov {
            dst: MovDst::Y,
            op: MovOp::Invert,
            src: MovSrc::X,
        });
        assert_eq!(encode_insn(&mov, &none), 0xA049);
        // OUT PINS, 32
        let out = Insn::plain(Op::Out { dst: OutDst::Pins, count: 32 });
        assert_eq!(encode_insn(&out, &none), 0x6000);
        // IN X, 32
        let in_ = Insn::plain(Op::In { src: InSrc::X, count: 32 });
        assert_eq!(encode_insn(&in_, &none), 0x4020);
        // PULL block
        let pull = Insn::plain(Op::Pull { if_empty: false, block: true });
        assert_eq!(encode_insn(&pull, &none), 0x80A0);
        // SET X, 3  side-set value 1 (count=2, opt) delay 5  ->  0xFD23
        let set = Insn {
            op: Op::Set { dst: SetDst::X, data: 3 },
            delay: 5,
            sideset: Some(1),
        };
        assert_eq!(
            encode_insn(&set, &SideCfg { count: 2, en: true }),
            0xFD23
        );
    }

    // ---- exhaustive round-trip: decode(encode(ir)) == ir --------------

    /// All side-set configs we sweep. `count==0` -> no side-set; with
    /// `en` the field has `count-1` value bits (so count>=1 required).
    fn side_cfgs() -> Vec<SideCfg> {
        let mut v = vec![SideCfg::NONE];
        for count in 1..=5 {
            v.push(SideCfg { count, en: false });
            v.push(SideCfg { count, en: true });
        }
        v
    }

    /// For a given config, the set of side-set intents that are
    /// *faithfully representable* (so decode returns exactly the intent).
    fn sidesets_for(cfg: &SideCfg) -> Vec<Option<u8>> {
        if cfg.count == 0 {
            return vec![None];
        }
        let val_bits = if cfg.en { cfg.count - 1 } else { cfg.count };
        let max = if val_bits == 0 { 0 } else { (1u32 << val_bits) - 1 } as u8;
        let mut out: Vec<Option<u8>> = (0..=max).map(Some).collect();
        if cfg.en {
            out.push(None); // only `opt` mode can represent "no side-set"
        }
        out
    }

    /// Delays representable in this config (0..=2^(5-count)-1).
    fn delays_for(cfg: &SideCfg) -> Vec<u8> {
        let bits = 5 - cfg.count.min(5);
        (0..(1u16 << bits)).map(|d| d as u8).collect()
    }

    /// Assert `decode(encode(insn))` reproduces `insn`'s opcode operands.
    fn check_op(op: Op) {
        for cfg in side_cfgs() {
            for &delay in &delays_for(&cfg) {
                for &ss in &sidesets_for(&cfg) {
                    let insn = Insn { op: op.clone(), delay, sideset: ss };
                    let word = encode_insn(&insn, &cfg);
                    let d = decode(word, pinctrl(cfg.count), execctrl(cfg.en));
                    assert_eq!(d.delay, delay, "delay {insn:?} cfg {cfg:?}");
                    assert_eq!(d.sideset, ss, "sideset {insn:?} cfg {cfg:?}");
                    assert_op(&d.op, &op, &insn, &cfg);
                }
            }
        }
    }

    fn assert_op(got: &PioOp, want: &Op, insn: &Insn, cfg: &SideCfg) {
        let ctx = || format!("{insn:?} cfg {cfg:?}");
        match (got, want) {
            (PioOp::Jmp { condition, address }, Op::Jmp { cond, target }) => {
                assert_eq!(*condition, *cond as u8, "jmp cond {}", ctx());
                assert_eq!(*address, *target, "jmp target {}", ctx());
            }
            (
                PioOp::Wait { polarity, source, index },
                Op::Wait { polarity: p, src, index: i },
            ) => {
                assert_eq!(*polarity, *p, "wait pol {}", ctx());
                assert_eq!(*source, *src as u8, "wait src {}", ctx());
                assert_eq!(*index, *i, "wait index {}", ctx());
            }
            (PioOp::In { source, bit_count }, Op::In { src, count }) => {
                assert_eq!(*source, *src as u8, "in src {}", ctx());
                assert_eq!(*bit_count, *count, "in count {}", ctx());
            }
            (PioOp::Out { destination, bit_count }, Op::Out { dst, count }) => {
                assert_eq!(*destination, *dst as u8, "out dst {}", ctx());
                assert_eq!(*bit_count, *count, "out count {}", ctx());
            }
            (PioOp::Push { if_full, block }, Op::Push { if_full: f, block: b }) => {
                assert_eq!(*if_full, *f, "push iffull {}", ctx());
                assert_eq!(*block, *b, "push block {}", ctx());
            }
            (PioOp::Pull { if_empty, block }, Op::Pull { if_empty: e, block: b }) => {
                assert_eq!(*if_empty, *e, "pull ifempty {}", ctx());
                assert_eq!(*block, *b, "pull block {}", ctx());
            }
            (PioOp::Mov { destination, op, source }, Op::Mov { dst, op: o, src }) => {
                assert_eq!(*destination, *dst as u8, "mov dst {}", ctx());
                assert_eq!(*op, *o as u8, "mov op {}", ctx());
                assert_eq!(*source, *src as u8, "mov src {}", ctx());
            }
            (PioOp::Irq { clear, wait, index }, Op::Irq { clear: c, wait: w, index: i }) => {
                assert_eq!(*clear, *c, "irq clear {}", ctx());
                assert_eq!(*wait, *w, "irq wait {}", ctx());
                assert_eq!(*index, *i, "irq index {}", ctx());
            }
            (PioOp::Set { destination, data }, Op::Set { dst, data: d }) => {
                assert_eq!(*destination, *dst as u8, "set dst {}", ctx());
                assert_eq!(*data, *d, "set data {}", ctx());
            }
            _ => panic!("opcode mismatch: {got:?} vs {want:?} ({})", ctx()),
        }
    }

    #[test]
    fn roundtrip_jmp() {
        for cond in [
            JmpCond::Always, JmpCond::NotX, JmpCond::XPostDec, JmpCond::NotY,
            JmpCond::YPostDec, JmpCond::XneY, JmpCond::Pin, JmpCond::NotOsrEmpty,
        ] {
            for target in [0u8, 1, 7, 16, 31] {
                check_op(Op::Jmp { cond, target });
            }
        }
    }

    #[test]
    fn roundtrip_wait() {
        for src in [WaitSrc::GpioAbs, WaitSrc::PinRel, WaitSrc::Irq, WaitSrc::JmpPin] {
            for polarity in [false, true] {
                for index in [0u8, 5, 31] {
                    check_op(Op::Wait { polarity, src, index });
                }
            }
        }
    }

    #[test]
    fn roundtrip_in_out() {
        for src in [InSrc::Pins, InSrc::X, InSrc::Y, InSrc::Null, InSrc::Isr, InSrc::Osr] {
            for count in [1u8, 8, 31, 32] {
                check_op(Op::In { src, count });
            }
        }
        for dst in [
            OutDst::Pins, OutDst::X, OutDst::Y, OutDst::Null,
            OutDst::PinDirs, OutDst::Pc, OutDst::Isr, OutDst::Exec,
        ] {
            for count in [1u8, 8, 31, 32] {
                check_op(Op::Out { dst, count });
            }
        }
    }

    #[test]
    fn roundtrip_push_pull() {
        for f in [false, true] {
            for b in [false, true] {
                check_op(Op::Push { if_full: f, block: b });
                check_op(Op::Pull { if_empty: f, block: b });
            }
        }
    }

    #[test]
    fn roundtrip_mov() {
        for dst in [
            MovDst::Pins, MovDst::X, MovDst::Y, MovDst::PinDirs, MovDst::Exec,
            MovDst::Pc, MovDst::Isr, MovDst::Osr,
        ] {
            for op in [MovOp::None, MovOp::Invert, MovOp::BitReverse] {
                for src in [
                    MovSrc::Pins, MovSrc::X, MovSrc::Y, MovSrc::Null,
                    MovSrc::Status, MovSrc::Isr, MovSrc::Osr,
                ] {
                    check_op(Op::Mov { dst, op, src });
                }
            }
        }
    }

    #[test]
    fn roundtrip_irq() {
        for clear in [false, true] {
            for wait in [false, true] {
                for index in [0u8, 4, 7, 16, 31] {
                    check_op(Op::Irq { clear, wait, index });
                }
            }
        }
    }

    #[test]
    fn roundtrip_set() {
        for dst in [SetDst::Pins, SetDst::X, SetDst::Y, SetDst::PinDirs] {
            for data in [0u8, 1, 15, 31] {
                check_op(Op::Set { dst, data });
            }
        }
    }
}
