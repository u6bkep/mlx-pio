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
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// The canonical NOP: `MOV Y, Y` (delay 0, no side-set). Empty
    /// program slots encode to this.
    pub fn nop() -> Self {
        Insn::plain(Op::Mov { dst: MovDst::Y, op: MovOp::None, src: MovSrc::Y })
    }
}

/// PIO opcode with its operands. Variants mirror the 8 opcode groups
/// (PUSH and PULL share opcode `0b100`).
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MovOp {
    None = 0,
    Invert = 1,     // bitwise NOT
    BitReverse = 2,
}

/// MOV source (operand bits [2:0]); code 4 reserved and omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}
