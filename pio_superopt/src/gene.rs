//! Structured genome: the first-class building-block IR.
//!
//! The slot-based [`Program`] is a flat array of instructions — great as the
//! *evaluation* form (slot index == hardware address), but fragile as the
//! *search* form: a building block like a counted loop is just a lucky
//! arrangement of primitives, and any point move can amputate one limb of it.
//! The diagnostics showed exactly this — injected loops survived but got
//! mangled and never integrated into a correct program.
//!
//! A `Gene` instead is a sequence of **nodes**, where a loop is a structured,
//! atomic `Loop` node — a "loop function" that *contains* its body and owns its
//! back-jump. Mutations operate on nodes, so a loop is tuned or removed as a
//! whole, never partially dismantled. The `Gene` is **lowered** to a `Program`
//! only at evaluation, reusing the entire encode/run/cost path unchanged.
//!
//! No labels. Plain-`JMP` targets are *literal* (only the condition is
//! runtime), so a loop-as-structure loses no runtime novelty vs a `jmp x--`
//! loop — and structural control flow needs no symbolic addresses. The two real
//! runtime-control novelties are preserved as node parameters: a **data-driven
//! exit** (`UntilOsrEmpty`) and a **data-driven count** (`counter_init: None`,
//! i.e. the counter register is established by data, not a literal). Computed
//! `MOV/OUT PC` jumps (register-targeted, also label-free) and general labels
//! for irreducible control flow are a documented v2.

use crate::ir::{Insn, JmpCond, Op, SetDst, SideCfg};
use crate::program::{Config, Program};

/// A loop's continue-condition — the back-jump taken while the loop should keep
/// running; the loop falls through (exits) when it fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopCond {
    /// `jmp x--` — decrement X, continue while it was non-zero (counted).
    CountX,
    /// `jmp y--` — decrement Y, continue while it was non-zero (counted).
    CountY,
    /// `jmp !OSRE` — continue while the OSR still holds bits. The iteration
    /// count is set by how much data was shifted in: a **data-driven** bound.
    UntilOsrEmpty,
}

/// A structured conditional's test — the JMP condition that selects the *taken*
/// branch. Mirrors [`LoopCond`]'s label-free philosophy: the branch targets are
/// structural (resolved at lowering), only the condition is runtime.
///
/// Includes the **post-decrement** forms (`x--`/`y--`), which are *destructive*
/// — they consume the tested register. DME relies on exactly this to branch on
/// a data bit freshly shifted into a scratch register (`out x,1` / `jmp x--`).
/// `Always` is excluded: a constant condition is a degenerate node (the search
/// expresses unconditional sequencing with plain nodes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondKind {
    /// `jmp !x` — taken when X == 0 (non-destructive).
    NotX,
    /// `jmp x--` — taken when X != 0, then X decrements (destructive).
    XPostDec,
    /// `jmp !y` — taken when Y == 0 (non-destructive).
    NotY,
    /// `jmp y--` — taken when Y != 0, then Y decrements (destructive).
    YPostDec,
    /// `jmp x!=y` — taken when X != Y (non-destructive).
    XneY,
    /// `jmp pin` — taken when the configured JMP pin is high (non-destructive).
    Pin,
    /// `jmp !osre` — taken while the OSR still holds bits (non-destructive).
    NotOsrEmpty,
}

impl CondKind {
    fn jmp_cond(self) -> JmpCond {
        match self {
            CondKind::NotX => JmpCond::NotX,
            CondKind::XPostDec => JmpCond::XPostDec,
            CondKind::NotY => JmpCond::NotY,
            CondKind::YPostDec => JmpCond::YPostDec,
            CondKind::XneY => JmpCond::XneY,
            CondKind::Pin => JmpCond::Pin,
            CondKind::NotOsrEmpty => JmpCond::NotOsrEmpty,
        }
    }
}

impl LoopCond {
    fn jmp_cond(self) -> JmpCond {
        match self {
            LoopCond::CountX => JmpCond::XPostDec,
            LoopCond::CountY => JmpCond::YPostDec,
            LoopCond::UntilOsrEmpty => JmpCond::NotOsrEmpty,
        }
    }
    /// The counter register this condition decrements, if any.
    fn counter_dst(self) -> Option<SetDst> {
        match self {
            LoopCond::CountX => Some(SetDst::X),
            LoopCond::CountY => Some(SetDst::Y),
            LoopCond::UntilOsrEmpty => None,
        }
    }
    pub fn is_counted(self) -> bool {
        self.counter_dst().is_some()
    }
}

/// A node in the structured genome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// A single primitive instruction. Never a `JMP` (control flow is
    /// structural in v1); the mutation operators and `validate` enforce this.
    Prim(Insn),
    /// A loop function: lowers to an optional counter init, the body, then the
    /// closing back-jump — `[set reg,N] / body… / jmp <cond> body_top`. The
    /// body runs `N+1` times (counted) or until the data-driven condition
    /// fails. The back-jump is owned by the node, so point moves cannot
    /// separate it from the body.
    Loop {
        /// Lowered contiguously; may nest further nodes.
        body: Vec<Node>,
        cond: LoopCond,
        /// `Some(n)` emits `set reg, n` before the body (literal bound);
        /// `None` emits no init — the counter register is established by data
        /// (a pulled/computed count), or the condition needs none
        /// (`UntilOsrEmpty`).
        counter_init: Option<u8>,
        /// Delay carried on the closing back-jump.
        jmp_delay: u8,
    },
    /// A structured conditional: structured *selection*, the dual of `Loop`'s
    /// structured *iteration*. Lowers (label-free, targets resolved at lowering)
    /// to:
    /// ```text
    ///         jmp <cond> L_then [dispatch_delay]   ; cond true  -> then
    ///         <els…>                               ; cond false -> fall through
    ///         jmp        L_end  [skip_delay]        ; hop over then
    ///   L_then: <then…>
    ///   L_end:
    /// ```
    /// An empty `els` yields the DME shape (`jmp x-- mid / jmp end / mid: …`).
    /// Both jumps are owned by the node, so point moves can never separate a
    /// branch from its dispatch.
    Cond {
        cond: CondKind,
        /// Taken branch (cond true). Must be non-empty.
        then: Vec<Node>,
        /// Fall-through branch (cond false). May be empty.
        els: Vec<Node>,
        /// Delay on the dispatch jump (`jmp <cond> L_then`).
        dispatch_delay: u8,
        /// Delay on the skip jump (`jmp L_end`) that hops over `then`.
        skip_delay: u8,
    },
}

/// The search genome: a sequence of nodes plus the (fixed, in v1) SM config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gene {
    pub nodes: Vec<Node>,
    pub config: Config,
}

fn nodes_len(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| match n {
            Node::Prim(_) => 1,
            Node::Loop { body, counter_init, .. } => {
                counter_init.is_some() as usize + nodes_len(body) + 1
            }
            // dispatch jmp + els + skip jmp + then
            Node::Cond { then, els, .. } => 1 + nodes_len(els) + 1 + nodes_len(then),
        })
        .sum()
}

fn structural_sideset(side: &SideCfg) -> Option<u8> {
    if side.count > 0 && !side.en {
        Some(0)
    } else {
        None
    }
}

fn lower_nodes(nodes: &[Node], struct_ss: Option<u8>, out: &mut Vec<Insn>) {
    for node in nodes {
        match node {
            Node::Prim(i) => out.push(i.clone()),
            Node::Loop { body, cond, counter_init, jmp_delay } => {
                if let (Some(n), Some(dst)) = (counter_init, cond.counter_dst()) {
                    out.push(Insn { op: Op::Set { dst, data: *n }, delay: 0, sideset: struct_ss });
                }
                let body_top = out.len().min(31) as u8;
                lower_nodes(body, struct_ss, out);
                out.push(Insn {
                    op: Op::Jmp { cond: cond.jmp_cond(), target: body_top },
                    delay: *jmp_delay,
                    sideset: struct_ss,
                });
            }
            Node::Cond { cond, then, els, dispatch_delay, skip_delay } => {
                // jmp <cond> L_then  — target fixed up after `then` is placed.
                let dispatch = out.len();
                out.push(Insn {
                    op: Op::Jmp { cond: cond.jmp_cond(), target: 0 },
                    delay: *dispatch_delay,
                    sideset: struct_ss,
                });
                lower_nodes(els, struct_ss, out);
                // jmp L_end — hops over `then`; target fixed up below.
                let skip = out.len();
                out.push(Insn {
                    op: Op::Jmp { cond: JmpCond::Always, target: 0 },
                    delay: *skip_delay,
                    sideset: struct_ss,
                });
                let then_top = out.len().min(31) as u8;
                lower_nodes(then, struct_ss, out);
                let end = out.len().min(31) as u8;
                if let Op::Jmp { target, .. } = &mut out[dispatch].op {
                    *target = then_top;
                }
                if let Op::Jmp { target, .. } = &mut out[skip].op {
                    *target = end;
                }
            }
        }
    }
}

fn validate_nodes(nodes: &[Node]) -> Result<(), String> {
    for (i, node) in nodes.iter().enumerate() {
        match node {
            Node::Prim(insn) => {
                if matches!(insn.op, Op::Jmp { .. }) {
                    return Err(format!("node {i}: standalone JMP not allowed (control flow is structural)"));
                }
            }
            Node::Loop { body, cond, counter_init, .. } => {
                if body.is_empty() {
                    return Err(format!("node {i}: loop body must be non-empty"));
                }
                if let Some(n) = counter_init {
                    if !cond.is_counted() {
                        return Err(format!("node {i}: counter_init set on a non-counted loop"));
                    }
                    if *n > 31 {
                        return Err(format!("node {i}: loop count {n} > 31"));
                    }
                }
                validate_nodes(body)?;
            }
            Node::Cond { then, els, .. } => {
                if then.is_empty() {
                    return Err(format!("node {i}: conditional `then` branch must be non-empty"));
                }
                validate_nodes(then)?;
                validate_nodes(els)?;
            }
        }
    }
    Ok(())
}

impl Gene {
    /// An empty genome with the given config.
    pub fn empty(config: Config) -> Self {
        Gene { nodes: Vec::new(), config }
    }

    /// Total lowered instruction count — the honest flash footprint and the
    /// size objective. (The lowered program is contiguous from address 0, so
    /// this equals the lowered `Program::size`.)
    pub fn lowered_len(&self) -> usize {
        nodes_len(&self.nodes)
    }

    /// Lower to the flat [`Program`] evaluation form: walk the nodes, emit each
    /// one's instructions contiguously, wire each loop's back-jump to its body
    /// top, and wrap the whole program. Truncates at 32 slots (an over-length
    /// gene is rejected by [`validate`](Gene::validate) before it ever runs).
    pub fn lower(&self) -> Program {
        let struct_ss = structural_sideset(&self.config.side);
        let mut insns: Vec<Insn> = Vec::new();
        lower_nodes(&self.nodes, struct_ss, &mut insns);
        let n = insns.len().min(32);
        // A `Cond` whose skip jump hops over a `then` block that ends the program
        // targets one-past-the-end. That address means "fall through / wrap"; the
        // SM's wrap takes it to `wrap_bottom` (0), so retarget it there explicitly
        // (the only structural target that can equal `n`).
        for ins in insns.iter_mut() {
            if let Op::Jmp { target, .. } = &mut ins.op {
                if *target as usize == n {
                    *target = 0;
                }
            }
        }
        let mut p = Program::empty(self.config);
        for (i, ins) in insns.into_iter().take(32).enumerate() {
            p.slots[i] = Some(ins);
        }
        p.wrap_bottom = 0;
        p.wrap_top = if n == 0 { 0 } else { (n - 1) as u8 };
        p
    }

    /// Validate the structure, then the lowered program. Cheap structural
    /// checks (length fits, loops well-formed, no stray jumps) gate before the
    /// per-slot legality check on the lowering.
    pub fn validate(&self) -> Result<(), String> {
        if self.lowered_len() > 32 {
            return Err(format!("lowered length {} exceeds 32 slots", self.lowered_len()));
        }
        validate_nodes(&self.nodes)?;
        self.lower().validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;
    use crate::program::*;

    fn serializer_cfg() -> Config {
        Config {
            shift: ShiftCfg { pull_threshold: 8, out_dir: ShiftDir::Right, ..ShiftCfg::default() },
            pins: PinMap { out_base: 0, out_count: 1, ..PinMap::default() },
            ..Config::default()
        }
    }

    /// A counted-loop gene lowers to the canonical serializer program.
    #[test]
    fn counted_loop_lowers_to_serializer() {
        let gene = Gene {
            config: serializer_cfg(),
            nodes: vec![
                Node::Prim(Insn::plain(Op::Pull { if_empty: false, block: true })),
                Node::Loop {
                    cond: LoopCond::CountX,
                    counter_init: Some(7),
                    body: vec![Node::Prim(Insn {
                        op: Op::Out { dst: OutDst::Pins, count: 1 },
                        delay: 6,
                        sideset: None,
                    })],
                    jmp_delay: 0,
                },
            ],
        };
        assert!(gene.validate().is_ok(), "{:?}", gene.validate());
        assert_eq!(gene.lowered_len(), 4); // pull + set + out + jmp
        let p = gene.lower();
        assert_eq!(p.size(), 4);
        assert_eq!((p.wrap_bottom, p.wrap_top), (0, 3));
        assert!(matches!(p.slots[0].as_ref().unwrap().op, Op::Pull { .. }));
        assert!(matches!(p.slots[1].as_ref().unwrap().op, Op::Set { dst: SetDst::X, data: 7 }));
        assert!(matches!(p.slots[2].as_ref().unwrap().op, Op::Out { count: 1, .. }));
        match p.slots[3].as_ref().unwrap().op {
            Op::Jmp { cond: JmpCond::XPostDec, target } => assert_eq!(target, 2, "back-jump to body top"),
            ref o => panic!("expected jmp x--, got {o:?}"),
        }
    }

    /// A data-driven loop (`until OSR empty`, no counter init) lowers without a
    /// `set`, closing with `jmp !OSRE`.
    #[test]
    fn data_driven_loop_has_no_counter() {
        let gene = Gene {
            config: serializer_cfg(),
            nodes: vec![Node::Loop {
                cond: LoopCond::UntilOsrEmpty,
                counter_init: None,
                body: vec![Node::Prim(Insn {
                    op: Op::Out { dst: OutDst::Pins, count: 1 },
                    delay: 6,
                    sideset: None,
                })],
                jmp_delay: 0,
            }],
        };
        assert!(gene.validate().is_ok(), "{:?}", gene.validate());
        assert_eq!(gene.lowered_len(), 2, "out + jmp, no set");
        let p = gene.lower();
        assert!(matches!(p.slots[0].as_ref().unwrap().op, Op::Out { .. }));
        assert!(matches!(
            p.slots[1].as_ref().unwrap().op,
            Op::Jmp { cond: JmpCond::NotOsrEmpty, target: 0 }
        ));
    }

    /// A conditional with an empty `els` lowers to the DME shape:
    /// `jmp <cond> L_then / jmp L_end / L_then: <then>`. The trailing skip
    /// target is one-past-the-end, retargeted to wrap (0).
    #[test]
    fn cond_empty_els_lowers_to_dme_shape() {
        let gene = Gene {
            config: serializer_cfg(),
            nodes: vec![Node::Cond {
                cond: CondKind::XPostDec,
                then: vec![Node::Prim(Insn::nop())],
                els: vec![],
                dispatch_delay: 2,
                skip_delay: 0,
            }],
        };
        assert!(gene.validate().is_ok(), "{:?}", gene.validate());
        assert_eq!(gene.lowered_len(), 3, "dispatch jmp + skip jmp + then");
        let p = gene.lower();
        // 0: jmp x-- -> then_top (2), carrying the dispatch delay.
        match p.slots[0].as_ref().unwrap() {
            Insn { op: Op::Jmp { cond: JmpCond::XPostDec, target }, delay, .. } => {
                assert_eq!(*target, 2, "dispatch targets then-block top");
                assert_eq!(*delay, 2, "dispatch carries dispatch_delay");
            }
            o => panic!("slot0 expected jmp x--, got {o:?}"),
        }
        // 1: jmp Always -> end == 3 == n, retargeted to wrap (0).
        match p.slots[1].as_ref().unwrap().op {
            Op::Jmp { cond: JmpCond::Always, target } => assert_eq!(target, 0, "skip past-end -> wrap"),
            ref o => panic!("slot1 expected jmp always, got {o:?}"),
        }
        // 2: the then-block.
        assert!(matches!(p.slots[2].as_ref().unwrap().op, Op::Mov { .. }), "slot2 is the then nop");
        assert_eq!((p.wrap_bottom, p.wrap_top), (0, 2));
    }

    /// A conditional with a non-empty `els` places els before the skip jump and
    /// then after it: `jmp <cond> L_then / <els> / jmp L_end / L_then: <then>`.
    #[test]
    fn cond_with_else_lowers_in_order() {
        let gene = Gene {
            config: serializer_cfg(),
            nodes: vec![Node::Cond {
                cond: CondKind::Pin,
                then: vec![Node::Prim(Insn { op: Op::Set { dst: SetDst::Y, data: 1 }, delay: 0, sideset: None })],
                els: vec![Node::Prim(Insn { op: Op::Set { dst: SetDst::X, data: 1 }, delay: 0, sideset: None })],
                dispatch_delay: 0,
                skip_delay: 1,
            }],
        };
        assert!(gene.validate().is_ok(), "{:?}", gene.validate());
        assert_eq!(gene.lowered_len(), 4, "dispatch + els + skip + then");
        let p = gene.lower();
        match p.slots[0].as_ref().unwrap().op {
            Op::Jmp { cond: JmpCond::Pin, target } => assert_eq!(target, 3, "dispatch -> then top (past els+skip)"),
            ref o => panic!("slot0 expected jmp pin, got {o:?}"),
        }
        assert!(matches!(p.slots[1].as_ref().unwrap().op, Op::Set { dst: SetDst::X, .. }), "slot1 is els");
        match p.slots[2].as_ref().unwrap() {
            Insn { op: Op::Jmp { cond: JmpCond::Always, target }, delay, .. } => {
                assert_eq!(*target, 0, "skip past-end -> wrap");
                assert_eq!(*delay, 1, "skip carries skip_delay");
            }
            o => panic!("slot2 expected jmp always, got {o:?}"),
        }
        assert!(matches!(p.slots[3].as_ref().unwrap().op, Op::Set { dst: SetDst::Y, .. }), "slot3 is then");
    }

    #[test]
    fn rejects_empty_then_branch() {
        let mut g = Gene::empty(Config::default());
        g.nodes.push(Node::Cond {
            cond: CondKind::NotX,
            then: vec![],
            els: vec![Node::Prim(Insn::nop())],
            dispatch_delay: 0,
            skip_delay: 0,
        });
        assert!(g.validate().is_err(), "empty `then` branch rejected");
    }

    #[test]
    fn rejects_stray_jmp_empty_body_and_bad_counter() {
        let mut g = Gene::empty(Config::default());
        g.nodes.push(Node::Prim(Insn::plain(Op::Jmp { cond: JmpCond::Always, target: 0 })));
        assert!(g.validate().is_err(), "standalone JMP rejected");

        let mut g2 = Gene::empty(Config::default());
        g2.nodes.push(Node::Loop { cond: LoopCond::CountX, counter_init: Some(3), body: vec![], jmp_delay: 0 });
        assert!(g2.validate().is_err(), "empty loop body rejected");

        let mut g3 = Gene::empty(Config::default());
        g3.nodes.push(Node::Loop {
            cond: LoopCond::UntilOsrEmpty,
            counter_init: Some(3), // counter on a non-counted loop
            body: vec![Node::Prim(Insn::nop())],
            jmp_delay: 0,
        });
        assert!(g3.validate().is_err(), "counter_init on data-driven loop rejected");
    }
}
