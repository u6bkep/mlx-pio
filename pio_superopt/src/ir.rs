//! Symbolic IR for RP2350 PIO instructions.
//!
//! Every operand enum carries the hardware field code as its explicit
//! discriminant, so encoding is `variant as u8` and only legal codes are
//! representable. Reserved codes (e.g. `MOV` op `0b11`) are simply absent.

/// One PIO instruction: an opcode-with-operands plus the per-instruction
/// `delay` and `sideset`, stored independently of the side-set config.
///
/// `delay` is the semantic 0..=31 value; `sideset` is `None` (this
/// instruction performs no side-set) or `Some(value)`. How these pack
/// into the shared 5-bit field is decided at encode time — see
/// [`crate::encode::encode_insn`] and [`SideCfg`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Insn {
    pub op: Op,
    pub delay: u8,
    pub sideset: Option<u8>,
}

impl Insn {
    /// An instruction with no delay and no side-set (the common case).
    pub fn plain(op: Op) -> Self {
        Insn { op, delay: 0, sideset: None }
    }

    /// The canonical NOP: `MOV Y, Y` (delay 0, no side-set).
    pub fn nop() -> Self {
        Insn::plain(Op::Mov { dst: MovDst::Y, op: MovOp::None, src: MovSrc::Y })
    }

    /// Compact one-line rendering of this instruction for experiment logs and
    /// traces (e.g. `out Pins,24[6]`, `jmp Pin->1`, `mov Pins,InvertOsr`).
    /// Delay is shown as `[n]`, an explicit side-set as `.sN`.
    pub fn brief(&self) -> String {
        let d = if self.delay > 0 { format!("[{}]", self.delay) } else { String::new() };
        let ss = match self.sideset {
            Some(v) => format!(".s{v}"),
            None => String::new(),
        };
        let op = match &self.op {
            Op::Jmp { cond, target } => format!("jmp {cond:?}->{target}"),
            Op::Wait { polarity, src, index } => format!("wait{} {src:?}{index}", *polarity as u8),
            Op::In { src, count } => format!("in {src:?},{count}"),
            Op::Out { dst, count } => format!("out {dst:?},{count}"),
            Op::Push { .. } => "push".into(),
            Op::Pull { .. } => "pull".into(),
            Op::Mov { dst, op, src } => format!("mov {dst:?},{op:?}{src:?}"),
            Op::Irq { index, .. } => format!("irq {index}"),
            Op::Set { dst, data } => format!("set {dst:?},{data}"),
        };
        format!("{op}{ss}{d}")
    }

    /// A NOP that is *legal under `side`*: when side-set is mandatory
    /// (`count > 0`, no enable bit) every instruction must drive it, so the
    /// fill NOP carries `Some(0)`; otherwise it opts out. Used to encode
    /// empty program slots. (Empty slots sit outside the executed wrap
    /// region, so the driven 0 is never observed — but it must encode.)
    pub fn nop_for(side: &SideCfg) -> Self {
        let sideset = if side.count > 0 && !side.en { Some(0) } else { None };
        Insn { op: Op::Mov { dst: MovDst::Y, op: MovOp::None, src: MovSrc::Y }, delay: 0, sideset }
    }
}

/// PIO opcode with its operands. Variants mirror the 8 opcode groups
/// (PUSH and PULL share opcode `0b100`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Op {
    /// `JMP <cond> <target>` — `target` is a slot index (0..=31).
    Jmp { cond: JmpCond, target: u8 },
    /// `WAIT <polarity> <src> <index>`. `index` is the raw 5-bit field
    /// (pin/IRQ number, with the source's rel bit folded in).
    Wait { polarity: bool, src: WaitSrc, index: u8 },
    /// `IN <src>, <count>` — `count` is 1..=32.
    In { src: InSrc, count: u8 },
    /// `OUT <dst>, <count>` — `count` is 1..=32.
    Out { dst: OutDst, count: u8 },
    /// `PUSH [iffull] [block]`.
    Push { if_full: bool, block: bool },
    /// `PULL [ifempty] [block]`.
    Pull { if_empty: bool, block: bool },
    /// `MOV <dst>, <op> <src>`.
    Mov { dst: MovDst, op: MovOp, src: MovSrc },
    /// `IRQ [clear] [wait] <index>` — raw 5-bit `index` (irq + rel).
    Irq { clear: bool, wait: bool, index: u8 },
    /// `SET <dst>, <data>` — `data` is a 0..=31 immediate.
    Set { dst: SetDst, data: u8 },
}

/// JMP condition (operand bits [7:5]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum JmpCond {
    Always = 0,
    NotX = 1,        // !X (X == 0)
    XPostDec = 2,    // X-- (X != 0, post-decrement)
    NotY = 3,        // !Y
    YPostDec = 4,    // Y--
    XneY = 5,        // X != Y
    Pin = 6,         // input pin (EXECCTRL_JMP_PIN)
    NotOsrEmpty = 7, // !OSRE
}

/// WAIT source (operand bits [6:5]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum WaitSrc {
    GpioAbs = 0, // absolute GPIO index
    PinRel = 1,  // relative to IN_BASE
    Irq = 2,
    /// RP2350 only: wait on PINCTRL_JMP_PIN + Index (Index must be 0..=3;
    /// other Index values are reserved — not yet enforced, like the other
    /// raw `index` fields).
    JmpPin = 3,
}

/// IN source (operand bits [7:5]); codes 4,5 reserved and omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum InSrc {
    Pins = 0,
    X = 1,
    Y = 2,
    Null = 3,
    Isr = 6,
    Osr = 7,
}

/// OUT destination (operand bits [7:5]); all 8 codes legal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum OutDst {
    Pins = 0,
    X = 1,
    Y = 2,
    Null = 3,
    PinDirs = 4,
    Pc = 5, // computed jump
    Isr = 6,
    Exec = 7, // execute OSR contents as an instruction
}

/// MOV destination (operand bits [7:5]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum MovDst {
    Pins = 0,
    X = 1,
    Y = 2,
    PinDirs = 3, // RP2350 only (PIO v1); same pin mapping as OUT
    Exec = 4,
    Pc = 5, // computed jump
    Isr = 6,
    Osr = 7,
}

/// MOV operation (operand bits [4:3]); code 3 reserved and omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum MovOp {
    None = 0,
    Invert = 1,     // bitwise NOT
    BitReverse = 2,
}

/// MOV source (operand bits [2:0]); code 4 reserved and omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum MovSrc {
    Pins = 0,
    X = 1,
    Y = 2,
    Null = 3,
    Status = 5,
    Isr = 6,
    Osr = 7,
}

/// SET destination (operand bits [7:5]); codes 3,5,6,7 reserved/omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum SetDst {
    Pins = 0,
    X = 1,
    Y = 2,
    PinDirs = 4,
}

/// The SM-global side-set configuration that determines how the 5-bit
/// delay/side-set field is split. Mirrors PINCTRL_SIDESET_COUNT and
/// EXECCTRL_SIDE_EN.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SideCfg {
    /// Number of MSBs of the 5-bit field used for side-set, **including
    /// the enable bit** when `en` is set. 0..=5.
    pub count: u8,
    /// When true the top side-set bit is a per-instruction enable
    /// (`.side_set N opt`); side-set then has `count - 1` value bits.
    pub en: bool,
}

impl SideCfg {
    /// No side-set: the whole 5-bit field is delay.
    pub const NONE: SideCfg = SideCfg { count: 0, en: false };

    /// Bits of the 5-bit field available for `delay` (0..=5).
    pub fn delay_bits(self) -> u8 {
        5 - self.count.min(5)
    }
    /// Inclusive max `delay` representable in this config.
    pub fn max_delay(self) -> u8 {
        (1u16 << self.delay_bits()) as u8 - 1
    }
    /// Number of side-set *value* bits (excluding the enable bit). `None`
    /// if this config has no side-set at all (`count == 0`).
    pub fn sideset_value_bits(self) -> Option<u8> {
        match self.count.min(5) {
            0 => None,
            c if self.en => Some(c - 1),
            c => Some(c),
        }
    }
    /// Inclusive max side-set *value*, or `None` if `count == 0`.
    pub fn max_sideset(self) -> Option<u8> {
        self.sideset_value_bits().map(|b| (1u16 << b) as u8 - 1)
    }
}

// ---- legal-code decoding: `u8` field value -> enum, `None` if reserved ----
//
// `as u8` is the encoder; these are the decoder. Each returns `None` for a
// reserved/undefined field code, which is what makes the legal-code set
// testable and keeps reserved encodings out of the IR.

impl JmpCond {
    pub fn from_code(c: u8) -> Option<Self> {
        use JmpCond::*;
        Some(match c {
            0 => Always, 1 => NotX, 2 => XPostDec, 3 => NotY,
            4 => YPostDec, 5 => XneY, 6 => Pin, 7 => NotOsrEmpty,
            _ => return None,
        })
    }
}
impl WaitSrc {
    pub fn from_code(c: u8) -> Option<Self> {
        use WaitSrc::*;
        Some(match c { 0 => GpioAbs, 1 => PinRel, 2 => Irq, 3 => JmpPin, _ => return None })
    }
}
impl InSrc {
    pub fn from_code(c: u8) -> Option<Self> {
        use InSrc::*;
        Some(match c { 0 => Pins, 1 => X, 2 => Y, 3 => Null, 6 => Isr, 7 => Osr, _ => return None })
    }
}
impl OutDst {
    pub fn from_code(c: u8) -> Option<Self> {
        use OutDst::*;
        Some(match c {
            0 => Pins, 1 => X, 2 => Y, 3 => Null,
            4 => PinDirs, 5 => Pc, 6 => Isr, 7 => Exec, _ => return None,
        })
    }
}
impl MovDst {
    pub fn from_code(c: u8) -> Option<Self> {
        use MovDst::*;
        Some(match c {
            0 => Pins, 1 => X, 2 => Y, 3 => PinDirs,
            4 => Exec, 5 => Pc, 6 => Isr, 7 => Osr, _ => return None,
        })
    }
}
impl MovOp {
    pub fn from_code(c: u8) -> Option<Self> {
        use MovOp::*;
        Some(match c { 0 => None, 1 => Invert, 2 => BitReverse, _ => return Option::None })
    }
}
impl MovSrc {
    pub fn from_code(c: u8) -> Option<Self> {
        use MovSrc::*;
        Some(match c { 0 => Pins, 1 => X, 2 => Y, 3 => Null, 5 => Status, 6 => Isr, 7 => Osr, _ => return None })
    }
}
impl SetDst {
    pub fn from_code(c: u8) -> Option<Self> {
        use SetDst::*;
        Some(match c { 0 => Pins, 1 => X, 2 => Y, 4 => PinDirs, _ => return None })
    }
}

// ---- range validation: the IR must hold only in-range operands ----------
//
// Adopted decision ②: nothing is silently masked. Mutation operators must
// produce in-range values; `encode` asserts these; `validate` reports them.

impl Op {
    /// Check that every operand sits in its field width. Bit counts are
    /// 1..=32; targets/indices/immediates are 0..=31.
    pub fn validate_ranges(&self) -> Result<(), &'static str> {
        let in5 = |v: u8| v <= 31;
        let cnt = |v: u8| (1..=32).contains(&v);
        match *self {
            Op::Jmp { target, .. } => in5(target).then_some(()).ok_or("jmp target > 31"),
            Op::Wait { index, .. } => in5(index).then_some(()).ok_or("wait index > 31"),
            Op::In { count, .. } => cnt(count).then_some(()).ok_or("in count out of 1..=32"),
            Op::Out { count, .. } => cnt(count).then_some(()).ok_or("out count out of 1..=32"),
            Op::Irq { index, .. } => in5(index).then_some(()).ok_or("irq index > 31"),
            Op::Set { data, .. } => in5(data).then_some(()).ok_or("set data > 31"),
            Op::Push { .. } | Op::Pull { .. } | Op::Mov { .. } => Ok(()),
        }
    }
}

impl Insn {
    /// Validate this instruction against a side-set config: operand ranges,
    /// plus that `delay` and `sideset` fit the shared 5-bit budget. Returns
    /// the first problem found.
    pub fn validate(&self, side: &SideCfg) -> Result<(), &'static str> {
        self.op.validate_ranges()?;
        if self.delay > side.max_delay() {
            return Err("delay exceeds field width for this sideset_count");
        }
        match (side.sideset_value_bits(), self.sideset) {
            // No side-set configured: must not request one.
            (None, Some(_)) => Err("sideset value set but sideset_count == 0"),
            (None, None) => Ok(()),
            // Mandatory side-set (no enable bit): every insn must drive it.
            (Some(_), None) if !side.en => Err("sideset is mandatory (side_en off) but None"),
            (Some(bits), Some(v)) => {
                let max = (1u16 << bits) as u8 - 1;
                if v > max { Err("sideset value exceeds its field width") } else { Ok(()) }
            }
            (Some(_), None) => Ok(()), // opt mode: opting out is fine
        }
    }
}
