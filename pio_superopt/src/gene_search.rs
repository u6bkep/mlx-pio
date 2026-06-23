//! Simulated annealing over the structured [`Gene`] genome, with a two-phase
//! size schedule.
//!
//! The flat-array search assembled building blocks from coordinated point
//! moves and couldn't keep them intact. Here a loop is a first-class [`Node`],
//! so "insert a counted loop" is a single structural move and the block is
//! mutated/removed as a unit — never partially dismantled.
//!
//! **Two-phase (STOKE-style).** Phase 1 synthesizes with **no size pressure**
//! (`size_weight = 0`): structure may accrete freely without paying flash rent,
//! which is exactly what an injected block needs to survive before it is fully
//! correct. Phase 2 reintroduces size on the phase-1 champion and greedily
//! shrinks while `W·correctness` holds correctness fixed.

use crate::cost::{edge_cost, hamming_tolerant, score_masked, Metric, Score};
use crate::gene::{CondKind, Gene, LoopCond, Node};
use crate::ir::{Insn, Op, OutDst, SetDst, SideCfg};
use crate::program::Config;
use crate::rng::Rng;
use crate::run::{run, RunSpec};
use crate::search::{
    random_delay, random_sideset, MigrateCfg, Params, IN_SRCS, MOV_DSTS, MOV_OPS, MOV_SRCS,
    OUT_DSTS, SET_DSTS, WAIT_SRCS,
};
use std::sync::Mutex;

// ---- random primitives (never a JMP — control flow is structural) ----

fn random_prim_op(rng: &mut Rng) -> Op {
    match rng.below(8) {
        0 => Op::Wait { polarity: rng.boolean(), src: *rng.pick(&WAIT_SRCS), index: rng.below(32) as u8 },
        1 => Op::In { src: *rng.pick(&IN_SRCS), count: 1 + rng.below(32) as u8 },
        2 => Op::Out { dst: *rng.pick(&OUT_DSTS), count: 1 + rng.below(32) as u8 },
        3 => Op::Push { if_full: rng.boolean(), block: rng.boolean() },
        4 => Op::Pull { if_empty: rng.boolean(), block: rng.boolean() },
        5 => Op::Mov { dst: *rng.pick(&MOV_DSTS), op: *rng.pick(&MOV_OPS), src: *rng.pick(&MOV_SRCS) },
        6 => Op::Irq { clear: rng.boolean(), wait: rng.boolean(), index: rng.below(32) as u8 },
        _ => Op::Set { dst: *rng.pick(&SET_DSTS), data: rng.below(32) as u8 },
    }
}

fn random_prim_insn(rng: &mut Rng, side: &SideCfg) -> Insn {
    Insn { op: random_prim_op(rng), delay: random_delay(rng, side), sideset: random_sideset(rng, side) }
}

/// Re-roll an op's immediate without changing its kind (count dialing). No JMP
/// arm: prims are never jumps.
fn mutate_prim_immediate(op: &mut Op, rng: &mut Rng) {
    let new = match &*op {
        Op::Set { dst, .. } => Op::Set { dst: *dst, data: rng.below(32) as u8 },
        Op::Out { dst, .. } => Op::Out { dst: *dst, count: 1 + rng.below(32) as u8 },
        Op::In { src, .. } => Op::In { src: *src, count: 1 + rng.below(32) as u8 },
        Op::Wait { polarity, src, .. } => Op::Wait { polarity: *polarity, src: *src, index: rng.below(32) as u8 },
        Op::Irq { clear, wait, .. } => Op::Irq { clear: *clear, wait: *wait, index: rng.below(32) as u8 },
        _ => return,
    };
    *op = new;
}

fn mutate_insn(insn: &mut Insn, rng: &mut Rng, side: &SideCfg) {
    match rng.below(4) {
        0 => insn.op = random_prim_op(rng),
        1 => insn.delay = random_delay(rng, side),
        2 => insn.sideset = random_sideset(rng, side),
        _ => mutate_prim_immediate(&mut insn.op, rng),
    }
}

fn random_loop_cond(rng: &mut Rng) -> LoopCond {
    match rng.below(3) {
        0 => LoopCond::CountX,
        1 => LoopCond::CountY,
        _ => LoopCond::UntilOsrEmpty,
    }
}

/// All structured-conditional tests (the polish sweep set).
const COND_KINDS: [CondKind; 7] = [
    CondKind::NotX,
    CondKind::XPostDec,
    CondKind::NotY,
    CondKind::YPostDec,
    CondKind::XneY,
    CondKind::Pin,
    CondKind::NotOsrEmpty,
];

fn random_cond_kind(rng: &mut Rng) -> CondKind {
    *rng.pick(&COND_KINDS)
}

/// A counted serialization loop: `set reg,N` / `out pins,1 [d]` / `jmp reg--`.
fn random_loop(rng: &mut Rng, side: &SideCfg) -> Node {
    Node::Loop {
        cond: if rng.boolean() { LoopCond::CountX } else { LoopCond::CountY },
        counter_init: Some(rng.below(32) as u8),
        body: vec![Node::Prim(Insn {
            op: Op::Out { dst: OutDst::Pins, count: 1 },
            delay: random_delay(rng, side),
            sideset: random_sideset(rng, side),
        })],
        jmp_delay: 0,
    }
}

/// The building-block move: insert a **self-sufficient serializer** — a `pull`
/// feeding a counted loop — so the block emits real data (not OSR zeros) the
/// moment it lands and has fitness to survive. (The pull is a separate node,
/// removable on its own; the loop stays atomic.)
fn insert_serializer(nodes: &mut Vec<Node>, pos: usize, rng: &mut Rng, side: &SideCfg) {
    nodes.insert(pos, Node::Prim(Insn::plain(Op::Pull { if_empty: false, block: true })));
    nodes.insert(pos + 1, random_loop(rng, side));
}

/// The **thin** structural move for selection: insert a minimal conditional —
/// `if(<cond>) { <prim> }`, empty else, zero delays. Deliberately tiny: the
/// search grows and wires the branch (the toggle idiom, the data-bit dispatch,
/// the timing balance) from here via the node-level mutations and polish. We
/// hand it the *capability* (a forward conditional) but not the *answer* (a
/// pre-assembled DME cell), so the coordination — the thing PT is meant to help
/// with — stays in the problem.
fn insert_cond(nodes: &mut Vec<Node>, pos: usize, rng: &mut Rng, side: &SideCfg) {
    nodes.insert(
        pos,
        Node::Cond {
            cond: random_cond_kind(rng),
            then: vec![Node::Prim(random_prim_insn(rng, side))],
            els: vec![],
            dispatch_delay: 0,
            skip_delay: 0,
        },
    );
}

fn keep_counter_consistent(node: &mut Node) {
    if let Node::Loop { cond, counter_init, .. } = node {
        if !cond.is_counted() {
            *counter_init = None;
        }
    }
}

fn mutate_body(body: &mut Vec<Node>, rng: &mut Rng, side: &SideCfg) {
    match rng.below(3) {
        0 => {
            let pos = rng.below(body.len() as u32 + 1) as usize;
            body.insert(pos, Node::Prim(random_prim_insn(rng, side)));
        }
        1 if body.len() > 1 => {
            body.remove(rng.below(body.len() as u32) as usize);
        }
        _ => {
            let i = rng.below(body.len() as u32) as usize;
            if let Node::Prim(insn) = &mut body[i] {
                mutate_insn(insn, rng, side);
            } else {
                body[i] = Node::Prim(random_prim_insn(rng, side));
            }
        }
    }
}

fn mutate_node(node: &mut Node, rng: &mut Rng, side: &SideCfg) {
    match node {
        Node::Prim(insn) => mutate_insn(insn, rng, side),
        Node::Loop { body, cond, counter_init, jmp_delay } => {
            match rng.below(5) {
                0 => *cond = random_loop_cond(rng),
                1 => {
                    // Toggle between a literal count and a data-driven one.
                    *counter_init = if rng.below(4) == 0 { None } else { Some(rng.below(32) as u8) };
                }
                2 => *jmp_delay = random_delay(rng, side),
                _ => mutate_body(body, rng, side),
            }
            keep_counter_consistent(node);
        }
        Node::Cond { cond, then, els, dispatch_delay, skip_delay } => match rng.below(5) {
            0 => *cond = random_cond_kind(rng),
            1 => *dispatch_delay = random_delay(rng, side),
            2 => *skip_delay = random_delay(rng, side),
            3 => mutate_body(then, rng, side), // never empties `then` (mutate_body keeps len>=1)
            _ => {
                if els.is_empty() {
                    els.push(Node::Prim(random_prim_insn(rng, side)));
                } else {
                    mutate_body(els, rng, side);
                }
            }
        },
    }
}

// ---- timing-aware (duration-preserving) moves ----
//
// Strict cycle-aligned Hamming is hypersensitive to total duration: a single
// insert/remove/delay edit shifts the whole suffix out of phase and is rejected
// regardless of merit. These moves change *structure* while holding *duration*
// constant, so the search can travel the iso-duration neutral network (e.g.
// convert a padding delay into framing) without falling off the phase cliff.
//
// They operate only on **top-level primitive delays**: a top-level prim runs
// once per program pass, so adjusting its delay changes duration exactly 1:1.
// A delay inside a loop body would scale by the iteration count — excluded to
// keep the compensation arithmetic exact.

fn top_prim_indices(nodes: &[Node]) -> Vec<usize> {
    (0..nodes.len()).filter(|&i| matches!(nodes[i], Node::Prim(_))).collect()
}

fn prim_delay_of(node: &Node) -> Option<u8> {
    if let Node::Prim(i) = node {
        Some(i.delay)
    } else {
        None
    }
}

/// Insert a 1+`d`-cycle prim and steal those cycles from a top-level prim's
/// delay — net duration unchanged.
fn insert_compensated(nodes: &mut Vec<Node>, rng: &mut Rng, side: &SideCfg) {
    let donors: Vec<usize> =
        top_prim_indices(nodes).into_iter().filter(|&i| prim_delay_of(&nodes[i]).unwrap() >= 1).collect();
    if donors.is_empty() {
        return;
    }
    let donor = *rng.pick(&donors);
    let avail = prim_delay_of(&nodes[donor]).unwrap();
    let d = rng.below(avail as u32) as u8; // 0..=avail-1, so 1+d <= avail
    if let Node::Prim(i) = &mut nodes[donor] {
        i.delay -= 1 + d;
    }
    let node = Node::Prim(Insn { op: random_prim_op(rng), delay: d, sideset: random_sideset(rng, side) });
    let pos = rng.below(nodes.len() as u32 + 1) as usize;
    nodes.insert(pos, node);
}

/// Remove a top-level prim and dump its cycles into another top-level prim's
/// delay — net duration unchanged.
fn remove_compensated(nodes: &mut Vec<Node>, rng: &mut Rng) {
    let prims = top_prim_indices(nodes);
    if prims.len() < 2 {
        return;
    }
    let victim = *rng.pick(&prims);
    let c = 1 + prim_delay_of(&nodes[victim]).unwrap() as u16;
    let donors: Vec<usize> = prims
        .iter()
        .copied()
        .filter(|&i| i != victim && prim_delay_of(&nodes[i]).unwrap() as u16 + c <= 31)
        .collect();
    if donors.is_empty() {
        return;
    }
    let donor = *rng.pick(&donors);
    if let Node::Prim(i) = &mut nodes[donor] {
        i.delay += c as u8;
    }
    nodes.remove(victim);
}

/// Move `k` cycles between two top-level prim delays — pure duration-neutral
/// timing reshape (dials per-region timing without touching the total).
fn shift_cycles(nodes: &mut Vec<Node>, rng: &mut Rng) {
    let prims = top_prim_indices(nodes);
    if prims.len() < 2 {
        return;
    }
    let a = *rng.pick(&prims);
    let b = *rng.pick(&prims);
    if a == b {
        return;
    }
    let ad = prim_delay_of(&nodes[a]).unwrap();
    let bd = prim_delay_of(&nodes[b]).unwrap();
    if ad < 1 || bd >= 31 {
        return;
    }
    let k = 1 + rng.below(ad.min(31 - bd) as u32) as u8;
    if let Node::Prim(i) = &mut nodes[a] {
        i.delay -= k;
    }
    if let Node::Prim(i) = &mut nodes[b] {
        i.delay += k;
    }
}

/// Candidate gene for a compensated insert (polish form): insert `Prim(op, d)`
/// at `pos`, funded by reducing top-level prim `donor`'s delay by `1+d`.
/// `None` if the donor can't fund it.
fn compensated_insert_candidate(g: &Gene, donor: usize, pos: usize, op: Op, d: u8) -> Option<Gene> {
    let dd = prim_delay_of(&g.nodes[donor])?;
    if dd < 1 + d {
        return None;
    }
    let mut m = g.clone();
    if let Node::Prim(i) = &mut m.nodes[donor] {
        i.delay -= 1 + d;
    }
    m.nodes.insert(pos.min(m.nodes.len()), Node::Prim(Insn { op, delay: d, sideset: None }));
    Some(m)
}

/// One structural mutation. Always yields a structurally-formed gene (legality
/// of the lowering is checked by cost).
fn mutate_gene(g: &Gene, rng: &mut Rng) -> Gene {
    let mut m = g.clone();
    let n = m.nodes.len();
    if n == 0 {
        if rng.boolean() {
            insert_serializer(&mut m.nodes, 0, rng, &m.config.side);
        } else {
            m.nodes.push(Node::Prim(random_prim_insn(rng, &m.config.side)));
        }
        return m;
    }
    match rng.below(10) {
        0 => {
            let pos = rng.below(n as u32 + 1) as usize;
            m.nodes.insert(pos, Node::Prim(random_prim_insn(rng, &m.config.side)));
        }
        1 => {
            let pos = rng.below(n as u32 + 1) as usize;
            insert_serializer(&mut m.nodes, pos, rng, &m.config.side);
        }
        2 => {
            let pos = rng.below(n as u32 + 1) as usize;
            insert_cond(&mut m.nodes, pos, rng, &m.config.side);
        }
        3 => {
            m.nodes.remove(rng.below(n as u32) as usize);
        }
        4 => {
            let i = rng.below(n as u32) as usize;
            m.nodes[i] = if rng.boolean() {
                Node::Prim(random_prim_insn(rng, &m.config.side))
            } else {
                random_loop(rng, &m.config.side)
            };
        }
        // Timing-aware moves: restructure at constant total duration.
        5 => insert_compensated(&mut m.nodes, rng, &m.config.side),
        6 => remove_compensated(&mut m.nodes, rng),
        7 => shift_cycles(&mut m.nodes, rng),
        _ => {
            let i = rng.below(n as u32) as usize;
            mutate_node(&mut m.nodes[i], rng, &m.config.side);
        }
    }
    m
}

fn random_gene(config: Config, rng: &mut Rng) -> Gene {
    let mut g = Gene::empty(config);
    let k = 1 + rng.below(4);
    for _ in 0..k {
        if rng.below(3) == 0 {
            let pos = g.nodes.len();
            insert_serializer(&mut g.nodes, pos, rng, &config.side);
        } else {
            g.nodes.push(Node::Prim(random_prim_insn(rng, &config.side)));
        }
    }
    g
}

/// The objective configuration for a search phase.
///
/// - `w` weights correctness over size (the STOKE two-phase split).
/// - `size_weight` is the parsimony gradient (reduced during synthesis, 1 at
///   the end). `max_len` is the hard length bound that prevents bloat — the
///   gene analogue of the slot search's fixed window.
/// - `k` is the tolerance band (see [`hamming_tolerant`]) / edge-matching window
///   (see [`edge_cost`]): large early to smooth the landscape, annealed to 0
///   (strict) to certify.
/// - `metric` selects the smooth gradient (level-tolerant vs transition-event).
#[derive(Debug, Clone, Copy)]
pub struct Obj {
    pub w: f64,
    pub size_weight: f64,
    pub max_len: usize,
    pub k: usize,
    pub metric: Metric,
}

/// Scalar cost: `∞` if the gene exceeds `max_len` or lowers to an illegal
/// program, else `w·correctness + size_weight·lowered_len`, where correctness is
/// the chosen smooth metric at radius/window `obj.k`.
fn cost_gene(g: &Gene, golden: &[u32], mask: &[u32], spec: &RunSpec, obj: &Obj) -> f64 {
    if g.lowered_len() > obj.max_len || g.validate().is_err() {
        return f64::INFINITY;
    }
    let wave = run(&g.lower(), spec);
    let corr = match obj.metric {
        Metric::LevelTolerant => hamming_tolerant(golden, &wave, mask, obj.k),
        Metric::Edge => edge_cost(golden, &wave, mask, obj.k),
    };
    obj.w * corr + obj.size_weight * g.lowered_len() as f64
}

// ---- deterministic gene polish ----

/// Curated non-jump op candidates for the polish, with representative
/// immediates (mirrors the slot `op_neighbors`, minus JMP).
fn prim_op_neighbors() -> Vec<Op> {
    let counts = [1u8, 2, 8, 32];
    let datas = [0u8, 1, 31];
    let indices = [0u8, 1];
    let mut v = Vec::new();
    for &polarity in &[false, true] {
        for &src in &WAIT_SRCS {
            for &index in &indices {
                v.push(Op::Wait { polarity, src, index });
            }
        }
    }
    for &src in &IN_SRCS {
        for &count in &counts {
            v.push(Op::In { src, count });
        }
    }
    for &dst in &OUT_DSTS {
        for &count in &counts {
            v.push(Op::Out { dst, count });
        }
    }
    for &a in &[false, true] {
        for &block in &[false, true] {
            v.push(Op::Push { if_full: a, block });
            v.push(Op::Pull { if_empty: a, block });
        }
    }
    for &dst in &MOV_DSTS {
        for &op in &MOV_OPS {
            for &src in &MOV_SRCS {
                v.push(Op::Mov { dst, op, src });
            }
        }
    }
    for &clear in &[false, true] {
        for &wait in &[false, true] {
            for &index in &indices {
                v.push(Op::Irq { clear, wait, index });
            }
        }
    }
    for &dst in &SET_DSTS {
        for &data in &datas {
            v.push(Op::Set { dst, data });
        }
    }
    v
}

fn legal_sidesets(side: &SideCfg) -> Vec<Option<u8>> {
    match side.sideset_value_bits() {
        None => vec![None],
        Some(bits) => {
            let max = if bits == 0 { 0 } else { (1u16 << bits) - 1 } as u8;
            let mut v: Vec<Option<u8>> = (0..=max).map(Some).collect();
            if side.en {
                v.push(None);
            }
            v
        }
    }
}

/// Deterministic best-improvement local search over a gene: per top-level node,
/// sweep op/delay/side-set neighbors, loop count/cond/jmp-delay, body-instr
/// neighbors, and node removal; apply the single best strict improvement, then
/// repeat. This is the analogue of the slot polish — it grinds out exact-value
/// residuals (delays, counts) annealing leaves on the table and deletes junk
/// nodes that the size gradient alone is too weak to remove.
fn polish_gene(start: &Gene, golden: &[u32], mask: &[u32], spec: &RunSpec, obj: &Obj) -> Gene {
    let side = start.config.side;
    let maxd = side.max_delay();
    let ops = prim_op_neighbors();
    let sidesets = legal_sidesets(&side);

    let mut cur = start.clone();
    let mut cur_cost = cost_gene(&cur, golden, mask, spec, obj);

    loop {
        let mut best: Option<(Gene, f64)> = None;
        let consider = |cand: Gene, best: &mut Option<(Gene, f64)>| {
            let c = cost_gene(&cand, golden, mask, spec, obj);
            if c < cur_cost && best.as_ref().map_or(true, |(_, bc)| c < *bc) {
                *best = Some((cand, c));
            }
        };

        for i in 0..cur.nodes.len() {
            // Drop this node (junk removal).
            {
                let mut g = cur.clone();
                g.nodes.remove(i);
                consider(g, &mut best);
            }
            match cur.nodes[i].clone() {
                Node::Prim(_) => {
                    for op in &ops {
                        let mut g = cur.clone();
                        if let Node::Prim(x) = &mut g.nodes[i] {
                            x.op = op.clone();
                        }
                        consider(g, &mut best);
                    }
                    for d in 0..=maxd {
                        let mut g = cur.clone();
                        if let Node::Prim(x) = &mut g.nodes[i] {
                            x.delay = d;
                        }
                        consider(g, &mut best);
                    }
                    for &ss in &sidesets {
                        let mut g = cur.clone();
                        if let Node::Prim(x) = &mut g.nodes[i] {
                            x.sideset = ss;
                        }
                        consider(g, &mut best);
                    }
                }
                Node::Loop { body, .. } => {
                    for cnt in 0..=31u8 {
                        let mut g = cur.clone();
                        if let Node::Loop { cond, counter_init, .. } = &mut g.nodes[i] {
                            if cond.is_counted() {
                                *counter_init = Some(cnt);
                            }
                        }
                        consider(g, &mut best);
                    }
                    // Data-driven count (drop the literal init).
                    {
                        let mut g = cur.clone();
                        if let Node::Loop { counter_init, .. } = &mut g.nodes[i] {
                            *counter_init = None;
                        }
                        consider(g, &mut best);
                    }
                    for d in 0..=maxd {
                        let mut g = cur.clone();
                        if let Node::Loop { jmp_delay, .. } = &mut g.nodes[i] {
                            *jmp_delay = d;
                        }
                        consider(g, &mut best);
                    }
                    for cond in [LoopCond::CountX, LoopCond::CountY, LoopCond::UntilOsrEmpty] {
                        let mut g = cur.clone();
                        if let Node::Loop { cond: c, .. } = &mut g.nodes[i] {
                            *c = cond;
                        }
                        keep_counter_consistent(&mut g.nodes[i]);
                        consider(g, &mut best);
                    }
                    // Body instruction neighbors (bodies hold Prims).
                    for j in 0..body.len() {
                        for op in &ops {
                            let mut g = cur.clone();
                            if let Node::Loop { body, .. } = &mut g.nodes[i] {
                                if let Node::Prim(x) = &mut body[j] {
                                    x.op = op.clone();
                                }
                            }
                            consider(g, &mut best);
                        }
                        for d in 0..=maxd {
                            let mut g = cur.clone();
                            if let Node::Loop { body, .. } = &mut g.nodes[i] {
                                if let Node::Prim(x) = &mut body[j] {
                                    x.delay = d;
                                }
                            }
                            consider(g, &mut best);
                        }
                        if body.len() > 1 {
                            let mut g = cur.clone();
                            if let Node::Loop { body, .. } = &mut g.nodes[i] {
                                body.remove(j);
                            }
                            consider(g, &mut best);
                        }
                    }
                }
                Node::Cond { .. } => {
                    // Tune the dispatch/skip delays (timing balance) and the test.
                    // Branch-internal polish is deferred to a later increment.
                    for d in 0..=maxd {
                        let mut g = cur.clone();
                        if let Node::Cond { dispatch_delay, .. } = &mut g.nodes[i] {
                            *dispatch_delay = d;
                        }
                        consider(g, &mut best);
                        let mut g = cur.clone();
                        if let Node::Cond { skip_delay, .. } = &mut g.nodes[i] {
                            *skip_delay = d;
                        }
                        consider(g, &mut best);
                    }
                    for ck in COND_KINDS {
                        let mut g = cur.clone();
                        if let Node::Cond { cond, .. } = &mut g.nodes[i] {
                            *cond = ck;
                        }
                        consider(g, &mut best);
                    }
                }
            }
        }

        // Compensated framing inserts: build framing around the structure
        // without changing total duration, so it's a downhill step the
        // deterministic polish can take (a naive insert would desync the suffix
        // and be rejected). Try `set pins,{0,1}` at each gap, funded by a
        // top-level prim donor, at a few delay splits (a full byte-cell is 8
        // cycles ⇒ delay 7).
        let framing = [
            Op::Set { dst: SetDst::Pins, data: 0 },
            Op::Set { dst: SetDst::Pins, data: 1 },
        ];
        for donor in top_prim_indices(&cur.nodes) {
            let dd = prim_delay_of(&cur.nodes[donor]).unwrap();
            if dd < 1 {
                continue;
            }
            let delays = [dd - 1, dd.min(8) - 1, dd.min(maxd + 1).saturating_sub(1)];
            for pos in 0..=cur.nodes.len() {
                for op in &framing {
                    for &d in &delays {
                        if let Some(g) = compensated_insert_candidate(&cur, donor, pos, op.clone(), d) {
                            consider(g, &mut best);
                        }
                    }
                }
            }
        }

        // 2-opt kick: when single edits stall, try the joint changes that sit
        // behind a 2-coordinate barrier — re-counting a loop *and* deleting a
        // node (the "loop-of-N-1 plus a tail bit" trap), or deleting two nodes
        // at once. Reserved for the stall so it stays cheap.
        if best.is_none() {
            let loops: Vec<usize> =
                (0..cur.nodes.len()).filter(|&i| matches!(cur.nodes[i], Node::Loop { .. })).collect();
            for &li in &loops {
                for c in 0..=31u8 {
                    for j in 0..cur.nodes.len() {
                        if j == li {
                            continue;
                        }
                        let mut g = cur.clone();
                        if let Node::Loop { cond, counter_init, .. } = &mut g.nodes[li] {
                            if cond.is_counted() {
                                *counter_init = Some(c);
                            }
                        }
                        g.nodes.remove(j);
                        consider(g, &mut best);
                    }
                }
            }
            for i in 0..cur.nodes.len() {
                for j in (i + 1)..cur.nodes.len() {
                    let mut g = cur.clone();
                    g.nodes.remove(j);
                    g.nodes.remove(i);
                    consider(g, &mut best);
                }
            }
        }

        match best {
            Some((g, c)) => {
                cur = g;
                cur_cost = c;
            }
            None => return cur,
        }
    }
}

/// Shared blackboard for async island migration between the concurrent chains
/// of a stage (ticket 001). One slot per chain holds that chain's most-recently
/// posted *current* gene and its cost (`None` until the chain first posts).
struct Migration {
    slots: Vec<Mutex<Option<(Gene, f64)>>>,
    cfg: MigrateCfg,
}

impl Migration {
    fn new(n: u32, cfg: MigrateCfg) -> Self {
        Migration { slots: (0..n).map(|_| Mutex::new(None)).collect(), cfg }
    }

    /// Publish this chain's current state to its own slot.
    fn post(&self, idx: usize, g: &Gene, cost: f64) {
        *self.slots[idx].lock().unwrap() = Some((g.clone(), cost));
    }

    /// Snapshot a random peer's posted state (`None` if no peer or peer hasn't
    /// posted yet). Static all-to-all topology: any chain may be sampled.
    fn sample_peer(&self, idx: usize, rng: &mut Rng) -> Option<(Gene, f64)> {
        let n = self.slots.len();
        if n < 2 {
            return None;
        }
        let mut j = rng.below(n as u32) as usize;
        if j == idx {
            j = (j + 1) % n;
        }
        self.slots[j].lock().unwrap().clone()
    }
}

/// One annealing chain: a single Metropolis run with geometric cooling from
/// `start` (or a fresh random gene). Returns the chain's best gene.
///
/// With `migrate = None` the chain is stateless and independent (the
/// reproducible baseline). With `Some((board, idx))` it participates in async
/// island migration: it posts its current gene to `board[idx]` every
/// `post_rate` iters and, every `poll_rate` iters, may **adopt** a better
/// peer's current gene. The adoption rule (mlx86-derived) accepts a strictly
/// better peer with probability `1 - exp(-intensity·gap / t)` — so adoption
/// intensifies both with the size of the improvement and as the chain cools.
/// Late in the run this collapses the chains onto the best-found basin
/// (late-stage consensus); `local_best` is never overwritten, so a chain's own
/// best discovery can't be lost to a migration.
fn anneal_chain(
    config: Config,
    start: Option<Gene>,
    golden: &[u32],
    mask: &[u32],
    spec: &RunSpec,
    params: &Params,
    obj: &Obj,
    seed: u64,
    migrate: Option<(&Migration, usize)>,
) -> Gene {
    let mut rng = Rng::new(seed);
    let mut cur = start.unwrap_or_else(|| random_gene(config, &mut rng));
    let mut cur_cost = cost_gene(&cur, golden, mask, spec, obj);
    let mut local_best = (cur.clone(), cur_cost);
    for i in 0..params.iters {
        let frac = i as f64 / params.iters as f64;
        let t = params.t0 * (params.t_end / params.t0).powf(frac);
        let cand = mutate_gene(&cur, &mut rng);
        let cand_cost = cost_gene(&cand, golden, mask, spec, obj);
        let d = cand_cost - cur_cost;
        if d <= 0.0 || rng.unit() < (-d / t).exp() {
            cur = cand;
            cur_cost = cand_cost;
        }
        if let Some((board, idx)) = migrate {
            let cfg = board.cfg;
            if i % cfg.post_rate == 0 {
                board.post(idx, &cur, cur_cost);
            }
            if i % cfg.poll_rate == 0 {
                if let Some((peer_g, peer_c)) = board.sample_peer(idx, &mut rng) {
                    if peer_c < cur_cost {
                        let gap = (cur_cost - peer_c) * cfg.intensity;
                        if rng.unit() < 1.0 - (-gap / t).exp() {
                            cur = peer_g;
                            cur_cost = peer_c;
                        }
                    }
                }
            }
        }
        if cur_cost < local_best.1 {
            local_best = (cur.clone(), cur_cost);
        }
    }
    local_best.0
}

/// Top-`n` distinct genes by ascending cost (an elite set, deduplicated so the
/// next stage keeps real diversity rather than n copies of one champion).
fn elite(mut scored: Vec<(Gene, f64)>, n: usize) -> Vec<Gene> {
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<Gene> = Vec::new();
    for (g, _) in scored {
        if out.len() >= n {
            break;
        }
        if !out.contains(&g) {
            out.push(g);
        }
    }
    out
}

/// One stage of the synthesis schedule: a tolerance radius `k` paired with a
/// size weight. The schedule sharpens `k` toward 0 (the strict, certifying
/// metric) while the size weight rises toward 1.
#[derive(Debug, Clone, Copy)]
pub struct Stage {
    pub k: usize,
    pub size_weight: f64,
}

/// The default coarse→fine schedule: start blurry (`k=8`, light size pressure)
/// so partially-correct structure gets graded credit and can climb; sharpen
/// step by step to strict (`k=0`) with full size pressure to certify and pack.
pub const DEFAULT_SCHEDULE: [Stage; 5] = [
    Stage { k: 8, size_weight: 0.25 },
    Stage { k: 4, size_weight: 0.25 },
    Stage { k: 2, size_weight: 0.5 },
    Stage { k: 1, size_weight: 0.5 },
    Stage { k: 0, size_weight: 1.0 },
];

/// Strict (`k=0`) score of a gene — the certifying metric.
fn strict_score(g: &Gene, golden: &[u32], mask: &[u32], spec: &RunSpec) -> Score {
    score_masked(&g.lower(), golden, mask, spec)
}

/// The reliable synthesizer: run every `(schedule, seed)` combination and keep
/// the strict-best gene.
///
/// Per-chain synthesis is low-rate and high-variance, and the right tolerance
/// schedule is target-dependent (a fixed starting radius over-blurs short
/// frames). A portfolio of diverse schedules crossed with multistart seeds
/// turns both into non-issues: different schedules cover different targets, and
/// the seeds cover per-instance variance. Each `synthesize_gene` is itself
/// parallel (chains per stage), so the combinations run sequentially here.
#[allow(clippy::too_many_arguments)]
pub fn synthesize_portfolio(
    config: Config,
    golden: &[u32],
    mask: &[u32],
    spec: &RunSpec,
    params: &Params,
    schedules: &[&[Stage]],
    max_len: usize,
    elite_n: usize,
    seeds: &[u64],
) -> (Gene, Score) {
    let mut best: Option<(Gene, Score)> = None;
    for sch in schedules {
        for &seed in seeds {
            let (g, s) = synthesize_gene(config, golden, mask, spec, params, sch, max_len, elite_n, seed);
            if best.as_ref().map_or(true, |(_, bs)| (s.correctness, s.size) < (bs.correctness, bs.size)) {
                best = Some((g, s));
            }
            if best.as_ref().is_some_and(|(_, s)| s.correctness == 0) {
                return best.unwrap();
            }
        }
    }
    best.expect("portfolio must have at least one schedule and seed")
}

/// Run `n` annealing+polish chains in parallel, each seeded round-robin from
/// `pool` (or random if empty). Returns each chain's polished gene and cost.
#[allow(clippy::too_many_arguments)]
fn run_chains(
    config: Config,
    pool: &[Gene],
    golden: &[u32],
    mask: &[u32],
    spec: &RunSpec,
    params: &Params,
    obj: &Obj,
    n: u32,
    seed: u64,
) -> Vec<(Gene, f64)> {
    // Shared migration blackboard for this batch of chains (ticket 001), or
    // `None` for the independent baseline. Lives across the whole scope so each
    // scoped thread can borrow it.
    let board = params.migrate.map(|cfg| Migration::new(n, cfg));
    let board_ref = board.as_ref();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..n)
            .map(|r| {
                let start = if pool.is_empty() { None } else { Some(pool[r as usize % pool.len()].clone()) };
                let cs = seed ^ (r as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let migrate = board_ref.map(|b| (b, r as usize));
                s.spawn(move || {
                    let g = anneal_chain(config, start, golden, mask, spec, params, obj, cs, migrate);
                    let g = polish_gene(&g, golden, mask, spec, obj);
                    let c = cost_gene(&g, golden, mask, spec, obj);
                    (g, c)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

/// Count of structurally-distinct genes in a scored set.
fn distinct_count(scored: &[(Gene, f64)]) -> usize {
    let mut uniq: Vec<&Gene> = Vec::new();
    for (g, _) in scored {
        if !uniq.iter().any(|u| *u == g) {
            uniq.push(g);
        }
    }
    uniq.len()
}

/// Annealed-tolerance synthesis with **per-stage elitism** (graduated
/// optimization + a (μ+λ) evolutionary outer loop).
///
/// Each stage runs `params.restarts` independent annealing chains **in
/// parallel** (the thread-local emulator makes this free), polishes each, and
/// keeps the top `elite_n` *distinct* genes as the seed population for the next,
/// sharper stage. Carrying a diverse elite — not one champion — stops a chain
/// from committing to a single stage-1 basin (the funnel that drove the high
/// variance). Temperature re-heats each stage so the search re-settles into the
/// freshly-sharpened landscape.
///
/// Adaptive: the **strict** (`k=0`) score of every elite is tracked across all
/// stages, and the globally strict-best is returned — so a correct solution
/// found early can't be lost to a later blurry stage (the easy-target
/// regression), and the search exits as soon as it certifies correctness 0.
pub fn synthesize_gene(
    config: Config,
    golden: &[u32],
    mask: &[u32],
    spec: &RunSpec,
    params: &Params,
    schedule: &[Stage],
    max_len: usize,
    elite_n: usize,
    seed: u64,
) -> (Gene, Score) {
    let mut global_best: Option<(Gene, Score)> = None;
    // Fold a fresh elite set into the running strict-best; returns true if a
    // certified-correct (strict 0) solution is now in hand.
    let absorb = |pop: &[Gene], gb: &mut Option<(Gene, Score)>| -> bool {
        for g in pop {
            let s = strict_score(g, golden, mask, spec);
            if gb.as_ref().map_or(true, |(_, bs)| (s.correctness, s.size) < (bs.correctness, bs.size)) {
                *gb = Some((g.clone(), s));
            }
        }
        gb.as_ref().is_some_and(|(_, s)| s.correctness == 0)
    };

    // Stage 0 — adaptive diversity gathering. Keep launching batches of chains
    // until we've collected `diversity_target` structurally-distinct elites or
    // hit the chain cap. Front-loading basin diversity here is what later
    // stages need to avoid committing to one stage-1 champion.
    let s0 = schedule[0];
    let obj0 = Obj { w: params.w, size_weight: s0.size_weight, max_len, k: s0.k, metric: params.metric };
    let diversity_target = 2 * elite_n;
    let max_chains = params.restarts * 8;
    let mut scored: Vec<(Gene, f64)> = Vec::new();
    let mut ran = 0u32;
    while distinct_count(&scored) < diversity_target && ran < max_chains {
        let batch = run_chains(
            config, &[], golden, mask, spec, params, &obj0, params.restarts,
            seed ^ (ran as u64).wrapping_mul(0x1000_0000_0000_0001),
        );
        scored.extend(batch);
        ran += params.restarts;
    }
    let mut population = elite(scored, elite_n);
    if absorb(&population, &mut global_best) {
        return global_best.unwrap();
    }

    // Later stages — sharpen with a larger batch seeded from the elite pool.
    let late_batch = params.restarts * 2;
    for (idx, stage) in schedule.iter().enumerate().skip(1) {
        let obj = Obj { w: params.w, size_weight: stage.size_weight, max_len, k: stage.k, metric: params.metric };
        let batch = run_chains(
            config, &population, golden, mask, spec, params, &obj, late_batch,
            seed ^ ((idx as u64).wrapping_mul(0xD1B5_4A32_D192_ED03)),
        );
        population = elite(batch, elite_n);
        if absorb(&population, &mut global_best) {
            break;
        }
    }

    global_best.expect("schedule has at least one stage")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Insn, MovDst, MovOp, MovSrc, Op, OutDst, SetDst, SideCfg};
    use crate::program::*;
    use crate::run::run;

    const TX: u8 = 0;

    fn uart_cfg() -> Config {
        Config {
            side: SideCfg::NONE,
            clkdiv_int: 1,
            shift: ShiftCfg { pull_threshold: 8, out_dir: ShiftDir::Right, ..ShiftCfg::default() },
            pins: PinMap { out_base: TX, out_count: 1, set_base: TX, set_count: 1, ..PinMap::default() },
            ..Config::default()
        }
    }

    fn out_bit(d: u8) -> Node {
        Node::Prim(Insn { op: Op::Out { dst: OutDst::Pins, count: 1 }, delay: d, sideset: None })
    }
    fn set_pins(v: u8, d: u8) -> Node {
        Node::Prim(Insn { op: Op::Set { dst: SetDst::Pins, data: v }, delay: d, sideset: None })
    }
    fn pull() -> Node {
        Node::Prim(Insn::plain(Op::Pull { if_empty: false, block: true }))
    }
    fn counted(k: u8, body: Vec<Node>) -> Node {
        Node::Loop { cond: LoopCond::CountX, counter_init: Some(k - 1), body, jmp_delay: 0 }
    }

    /// Reference data loop as a gene: pull / set x,k-1 / out[6] / jmp x--.
    fn data_loop_gene(k: u8) -> Gene {
        Gene { config: uart_cfg(), nodes: vec![pull(), counted(k, vec![out_bit(6)])] }
    }

    /// Reference UART (8n1, k data bits) as a gene: pull / set0[7] / counted
    /// loop of out[6] / set1[7].
    fn uart_gene(k: u8) -> Gene {
        Gene {
            config: uart_cfg(),
            nodes: vec![pull(), set_pins(0, 7), counted(k, vec![out_bit(6)]), set_pins(1, 7)],
        }
    }

    fn spec(cycles_per_byte: u64) -> RunSpec {
        RunSpec {
            block: 0,
            sm: 0,
            inputs: vec![0x55, 0x3C, 0xF0, 0x41],
            output_pins: vec![TX],
            capture_pins: vec![TX],
            cycles: 4 * cycles_per_byte,
        }
    }

    fn full_mask(golden: &[u32]) -> Vec<u32> {
        vec![u32::MAX; golden.len()]
    }

    // ---- DME (Differential Manchester / biphase-mark) benchmark ----
    //
    // The harder-than-UART target (tickets 001 / the v2-IR motivation). Line
    // level is tracked in Y and driven to the pin; a transition is `mov y,~y`
    // (flip the level) + `mov pins,y` (drive it). Biphase-mark: a transition at
    // every bit boundary (the clock), plus an extra mid-bit transition iff the
    // data bit is 1 — the latter a *data-conditional* `Cond`, the structure UART
    // lacked and v1 couldn't express.

    fn dme_cfg() -> Config {
        Config {
            side: SideCfg::NONE,
            clkdiv_int: 1,
            // 5-bit 4B/5B line codes, shifted LSB-first (matches the real impl).
            shift: ShiftCfg { pull_threshold: 5, out_dir: ShiftDir::Right, ..ShiftCfg::default() },
            pins: PinMap { out_base: TX, out_count: 1, set_base: TX, set_count: 1, ..PinMap::default() },
            ..Config::default()
        }
    }

    fn mov_y_inv() -> Node {
        Node::Prim(Insn::plain(Op::Mov { dst: MovDst::Y, op: MovOp::Invert, src: MovSrc::Y }))
    }
    fn drive(d: u8) -> Node {
        Node::Prim(Insn { op: Op::Mov { dst: MovDst::Pins, op: MovOp::None, src: MovSrc::Y }, delay: d, sideset: None })
    }
    fn out_x() -> Node {
        Node::Prim(Insn::plain(Op::Out { dst: OutDst::X, count: 1 }))
    }

    /// Reference biphase-mark DME encoder, half-bit = `h` cycles. Tracks level
    /// in Y. Per bit: boundary transition + first-half hold, fetch the data bit,
    /// a conditional mid-bit transition (`if x-- {toggle}`, `skip_delay=1` to
    /// balance the 0/1 paths to equal duration), then the second-half hold.
    fn dme_ref(h: u8) -> Gene {
        let cell = vec![
            out_x(),       // data bit -> X (consumed by the Cond)
            mov_y_inv(),   // boundary transition (clock edge)
            drive(h - 1),  // drive + hold first half (h cycles)
            Node::Cond {
                cond: CondKind::XPostDec, // taken iff bit == 1
                then: vec![mov_y_inv(), drive(0)], // mid-bit transition
                els: vec![],
                dispatch_delay: 0,
                skip_delay: 1, // balance: 0-path (dispatch+skip) == 1-path (dispatch+toggle+drive)
            },
            drive(h - 1), // second-half hold (re-drives current level, both paths)
        ];
        Gene {
            config: dme_cfg(),
            nodes: vec![
                pull(),
                Node::Loop { cond: LoopCond::UntilOsrEmpty, counter_init: None, body: cell, jmp_delay: 0 },
            ],
        }
    }

    const DME_H: u8 = 4;
    /// Locked capture window: covers the 4-code corpus (active to cycle 272) with
    /// a small tail, so "correctness" isn't inflated by a long constant stall.
    const DME_CYCLES: u64 = 278;

    /// The locked DME benchmark: (spec, golden, full mask). Golden is the
    /// reference's own output (self-consistent oracle), captured under the locked
    /// window — exactly the reference-oracle approach UART uses.
    fn dme_golden() -> (RunSpec, Vec<u32>, Vec<u32>) {
        let sp = dme_spec(DME_CYCLES);
        let golden = run(&dme_ref(DME_H).lower(), &sp);
        let mask = full_mask(&golden);
        (sp, golden, mask)
    }

    /// Bit-0 transition edges of a waveform: `(cycle, rising?)`.
    fn dme_edges01(wave: &[u32]) -> Vec<(usize, bool)> {
        let mut out = Vec::new();
        let mut prev = 0u32;
        for (i, &s) in wave.iter().enumerate() {
            let v = s & 1;
            if v != prev {
                out.push((i, v == 1));
                prev = v;
            }
        }
        out
    }

    /// Print a champion's boundary(clock)/mid(data-conditional) edge breakdown.
    /// `boundaries` = edges of the all-zeros-corpus reference (the clock grid).
    fn dme_diagnose_wave(golden: &[u32], cwave: &[u32], mask: &[u32], boundaries: &[(usize, bool)]) {
        let ge = dme_edges01(golden);
        let ce = dme_edges01(cwave);
        let near = |c: usize| boundaries.iter().any(|&(e, _)| e.abs_diff(c) <= 2);
        let (mut bt, mut bh, mut mt, mut mh) = (0, 0, 0, 0);
        for &(c, dir) in &ge {
            let matched = ce.iter().any(|&(cc, cd)| cd == dir && cc.abs_diff(c) <= 3);
            if near(c) {
                bt += 1;
                bh += matched as i32;
            } else {
                mt += 1;
                mh += matched as i32;
            }
        }
        let spurious = ce.iter().filter(|&&(cc, cd)| !ge.iter().any(|&(c, d)| d == cd && c.abs_diff(cc) <= 3)).count();
        eprintln!(
            "    edges {} | boundary(clock) {bh}/{bt} | mid(data) {mh}/{mt} | spurious {spurious} | strict edge-cost {:.1}",
            ce.len(),
            edge_cost(golden, cwave, mask, 0)
        );
    }

    /// A multi-code corpus (diverse 4B/5B data codes, varied bit patterns and
    /// popcounts) so a thin oracle can't be gamed — the recurring overfitting
    /// hazard. Processed back-to-back: the loop wraps to `pull` between codes.
    fn dme_corpus() -> Vec<u32> {
        vec![0x1E, 0x0A, 0x15, 0x09] // codes 0,4,3,1; lsb bits 01111/01010/10101/10010
    }

    fn dme_spec(cycles: u64) -> RunSpec {
        RunSpec {
            block: 0,
            sm: 0,
            inputs: dme_corpus(),
            output_pins: vec![TX],
            capture_pins: vec![TX],
            cycles,
        }
    }

    /// PROBE: find the active length (last transition) of the reference on the
    /// corpus, so the locked capture window covers the data with minimal stall
    /// tail. Run: `cargo test --release -- --ignored dme_probe --nocapture`
    #[test]
    #[ignore = "probe; run with --release ... --nocapture"]
    fn dme_probe() {
        let g = dme_ref(DME_H);
        let wave = run(&g.lower(), &dme_spec(400));
        let last_edge = wave.windows(2).rposition(|w| (w[0] ^ w[1]) & 1 != 0).map(|i| i + 1).unwrap_or(0);
        let edges = wave.windows(2).filter(|w| (w[0] ^ w[1]) & 1 != 0).count();
        let total_pop: u32 = dme_corpus().iter().map(|c| (c & 0x1F).count_ones()).sum();
        eprintln!("corpus={:02X?}  lowered_len={}", dme_corpus(), g.lower().size());
        eprintln!("last transition at cycle {last_edge}; total edges={edges} (5*4 boundaries + {total_pop} mids = {})", 20 + total_pop);
        eprintln!("suggest DME_CYCLES = {}", last_edge + DME_H as usize + 2);
    }

    /// LOCK GUARD (runs in the normal suite): the reference is legal and scores
    /// 0 against its own locked golden, and the golden carries the expected
    /// data-conditional structure — one mid transition per 1-bit. This pins the
    /// benchmark so later changes can't silently move it.
    #[test]
    fn dme_reference_scores_zero() {
        let (sp, golden, mask) = dme_golden();
        let g = dme_ref(DME_H);
        assert!(g.validate().is_ok(), "{:?}", g.validate());
        assert_eq!(g.lower().size(), 10, "reference is 10 slots");
        assert_eq!(
            score_masked(&g.lower(), &golden, &mask, &sp).correctness,
            0,
            "reference must match its own golden"
        );
        // Structural: 5 boundaries/code (minus the 1 pre-window startup edge) plus
        // one mid transition per 1-bit across the corpus — the v2-IR property.
        let edges = golden.windows(2).filter(|w| (w[0] ^ w[1]) & 1 != 0).count();
        let pop: u32 = dme_corpus().iter().map(|c| (c & 0x1F).count_ones()).sum();
        let boundaries = 5 * dme_corpus().len() as u32 - 1;
        assert_eq!(edges as u32, boundaries + pop, "boundary + popcount-mid structure");
    }

    /// HEADROOM (ticket 001 gate): does the baseline gene search — now with the
    /// `insert_cond` move — actually solve DME, or does it have headroom? If the
    /// baseline reliably hits 0 it's too easy (like UART) and can't discriminate
    /// PT; if it plateaus, DME is a real testbed. This is the gate before any
    /// migration A/B is meaningful.
    ///
    /// Run: `cargo test --release -- --ignored dme_headroom --nocapture`
    #[test]
    #[ignore = "DME headroom; run with --release ... --nocapture"]
    fn dme_headroom() {
        let (sp, golden, mask) = dme_golden();
        let params = Params { iters: 6000, restarts: 8, ..Params::default() };
        let seeds: Vec<u64> = (0..8u64).map(|i| 0x0D3E ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();
        let (solved, corrs) = solve_rate(dme_cfg(), &golden, &mask, &sp, &params, 16, &seeds);
        eprintln!("\nDME baseline (n={}, max_len=16): solved {solved}/{}", seeds.len(), seeds.len());
        eprintln!("  correctness spread: {corrs:?}");
        eprintln!("  (golden has {} active cycles; reference is 10 slots)", DME_CYCLES);
    }

    /// HEADROOM++ : throw real compute at DME — a portfolio of schedules
    /// (including a wider starting blur, since the DME frame is ~3x UART) ×
    /// seeds, higher iters/restarts, larger length cap. Reports the best
    /// correctness reached and the structure of the best gene, to see whether
    /// the thin search ever approaches a solution with effort, and whether it's
    /// reaching for the toggle/conditional cell at all.
    ///
    /// Run: `cargo test --release -- --ignored dme_compute --nocapture`
    #[test]
    #[ignore = "DME big-compute frontier; run with --release ... --nocapture"]
    fn dme_compute() {
        let (sp, golden, mask) = dme_golden();
        let params = Params { iters: 18000, restarts: 16, ..Params::default() };
        let portfolio = [
            sched(&[(8, 0.25), (4, 0.25), (2, 0.5), (1, 0.5), (0, 1.0)]),
            sched(&[(16, 0.25), (8, 0.25), (4, 0.5), (2, 0.5), (1, 0.25), (0, 1.0)]), // wider start for the long frame
        ];
        let seeds: Vec<u64> = (0..8u64).map(|i| 0x0D3E ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();
        let mut best: (u32, u8, Option<Gene>) = (u32::MAX, 0, None);
        let mut solved = 0;
        for (si, s) in portfolio.iter().enumerate() {
            for &seed in &seeds {
                let (g, sc) = synthesize_gene(dme_cfg(), &golden, &mask, &sp, &params, s, 20, 4, seed);
                if sc.correctness == 0 {
                    solved += 1;
                }
                if (sc.correctness, sc.size) < (best.0, best.1) {
                    best = (sc.correctness, sc.size, Some(g));
                }
                eprintln!("  sched{si} seed{seed:#x}: correctness={} size={}", sc.correctness, sc.size);
            }
        }
        eprintln!("\nDME big-compute: solved {solved}/{} | best correctness={} size={}", 2 * seeds.len(), best.0, best.1);
        if let Some(g) = best.2 {
            eprintln!("  best gene: {}", show_gene(&g));
        }
    }

    /// OBJECTIVE A/B: does the transition-event metric dissolve the deceptive
    /// `out Pins` basin? Same search, same compute — only `Params.metric`
    /// differs. Both arms report the *strict level-Hamming* correctness of the
    /// best gene found (the unchanged certifier), so this measures whether
    /// edge-guided search actually reaches a level-correct DME waveform.
    ///
    /// Run: `cargo test --release -- --ignored dme_metric_ab --nocapture`
    #[test]
    #[ignore = "objective A/B; run with --release ... --nocapture"]
    fn dme_metric_ab() {
        let (sp, golden, mask) = dme_golden();
        let lvl = Params { iters: 8000, restarts: 8, ..Params::default() };
        let edge = Params { metric: Metric::Edge, ..lvl };
        let seeds: Vec<u64> = (0..8u64).map(|i| 0x0D3E ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();

        // Best gene per arm, judged by the arm's OWN metric, plus a cross-readout
        // of the other metric, so we see what each is actually steering toward.
        let run_arm = |name: &str, p: &Params| {
            let mut best: Option<(f64, Gene)> = None; // by this arm's metric (window 0)
            for &seed in &seeds {
                let (g, _) = synthesize_gene(dme_cfg(), &golden, &mask, &sp, p, &DEFAULT_SCHEDULE, 16, 4, seed);
                let w = run(&g.lower(), &sp);
                let self_cost = match p.metric {
                    Metric::Edge => edge_cost(&golden, &w, &mask, 0),
                    Metric::LevelTolerant => hamming_tolerant(&golden, &w, &mask, 0),
                };
                if best.as_ref().map_or(true, |(bc, _)| self_cost < *bc) {
                    best = Some((self_cost, g));
                }
            }
            let (bc, g) = best.unwrap();
            let w = run(&g.lower(), &sp);
            let lvl = crate::cost::hamming(&golden, &w);
            let edg = edge_cost(&golden, &w, &mask, 0);
            eprintln!("\n  {name}: best self-metric={bc:.1} | strict level-Hamming={lvl} | strict edge-cost={edg:.1}");
            eprintln!("    {}", show_gene(&g));
        };
        eprintln!("\nDME objective A/B (n={}); golden has ~30 edges, 278 cycles:", seeds.len());
        run_arm("level-tolerant", &lvl);
        run_arm("edge", &edge);
    }

    /// POINT 2: the flat-slot edge-objective search on DME — the creativity
    /// substrate (arbitrary jumps, no priors/macros) running on the edge metric.
    /// Compares against the gene-search results: does removing the structural
    /// constraints (and keeping the fixed objective) find more DME structure?
    ///
    /// Run: `cargo test --release -- --ignored dme_flat --nocapture`
    #[test]
    #[ignore = "flat+edge DME; run with --release ... --nocapture"]
    fn dme_flat() {
        use crate::program::Program;
        let (sp, golden, mask) = dme_golden();
        let template = Program::empty(dme_cfg());
        let space = crate::search::Space {
            slots: 18,
            side: SideCfg::NONE,
            search_wrap: true,
            genes: crate::search::Genes::default(), // config fixed (pins/shift from template)
        };
        let params = Params { iters: 8000, restarts: 16, ..Params::default() };
        let windows = [8usize, 4, 2, 1, 0];
        let seeds: Vec<u64> = (0..6u64).map(|i| 0xF1A7 ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();

        let mut best: Option<(u32, f64, Program)> = None;
        for &seed in &seeds {
            let (p, s) = crate::search::synthesize_flat(&template, &space, &golden, &mask, &sp, &params, &windows, seed);
            let w = run(&p, &sp);
            let ec = edge_cost(&golden, &w, &mask, 0);
            eprintln!("  seed{seed:#018x}: strict level-Hamming={} edge-cost={ec:.1} size={}", s.correctness, s.size);
            if best.as_ref().map_or(true, |(bc, _, _)| s.correctness < *bc) {
                best = Some((s.correctness, ec, p));
            }
        }
        let (bc, be, bp) = best.unwrap();
        let mut parts = Vec::new();
        for i in 0..32usize {
            if let Some(insn) = &bp.slots[i] {
                parts.push(format!("{i}:{}", brief_insn(insn)));
            }
        }
        eprintln!("\nflat+edge DME best: level-Hamming={bc} edge-cost={be:.1} (gene+edge was edge-cost 34)");
        eprintln!("  [{}]  wrap {}..{}", parts.join("  "), bp.wrap_bottom, bp.wrap_top);
    }

    /// POINTS 2+3: the parallel/elitist/PT flat engine on DME, at scale —
    /// migration off vs on. Tests whether the full mlx86-style flat search
    /// (parallel chains + diverse elitism + island migration, edge objective,
    /// no priors) escapes the degenerate traps the bare `synthesize_flat` fell
    /// into, and whether migration (PT) helps on the flat substrate.
    ///
    /// Run: `cargo test --release -- --ignored dme_flat_pt --nocapture`
    #[test]
    #[ignore = "flat PT engine on DME; run with --release ... --nocapture"]
    fn dme_flat_pt() {
        use crate::program::Program;
        let (sp, golden, mask) = dme_golden();
        let template = Program::empty(dme_cfg());
        let space = crate::search::Space {
            slots: 18,
            side: SideCfg::NONE,
            search_wrap: true,
            genes: crate::search::Genes::default(),
        };
        let base = Params { iters: 12000, restarts: 16, ..Params::default() };
        let migr = Params { migrate: Some(MigrateCfg::default()), ..base };
        let windows = [8usize, 4, 2, 1, 0];
        let seeds: Vec<u64> = (0..4u64).map(|i| 0xF1A7 ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();

        let run_arm = |name: &str, p: &Params| {
            let mut best: Option<(u32, f64, Program)> = None;
            let mut corrs = Vec::new();
            for &seed in &seeds {
                let (prog, s) = crate::search::synthesize_flat_pt(&template, &space, &golden, &mask, &sp, p, &windows, 4, seed);
                let w = run(&prog, &sp);
                let ec = edge_cost(&golden, &w, &mask, 0);
                corrs.push(s.correctness);
                if best.as_ref().map_or(true, |(bc, _, _)| s.correctness < *bc) {
                    best = Some((s.correctness, ec, prog));
                }
            }
            let (bc, be, bp) = best.unwrap();
            let mut parts = Vec::new();
            for i in 0..32usize {
                if let Some(insn) = &bp.slots[i] {
                    parts.push(format!("{i}:{}", brief_insn(insn)));
                }
            }
            corrs.sort_unstable();
            eprintln!("  {name}: best level-Hamming={bc} edge-cost={be:.1} | spread {corrs:?} | wrap {}..{}", bp.wrap_bottom, bp.wrap_top);
            eprintln!("    [{}]", parts.join("  "));
        };
        eprintln!("\nDME flat PT engine (n={} seeds; gene+edge ref edge-cost=34):", seeds.len());
        run_arm("flat baseline ", &base);
        run_arm("flat migration", &migr);
    }

    /// POINT 3: crank the scale. The flat PT engine does a full search in ~2s,
    /// so throw 20-40x compute at it — big iters/restarts/elite, a wider window
    /// start — to see whether scale alone drives the flat+edge search toward a
    /// solve (edge-cost → 0), and whether migration earns its keep with more
    /// chains. Run: `cargo test --release -- --ignored dme_flat_scale --nocapture`
    #[test]
    #[ignore = "flat PT engine at scale; run with --release ... --nocapture"]
    fn dme_flat_scale() {
        use crate::program::Program;
        let (sp, golden, mask) = dme_golden();
        let template = Program::empty(dme_cfg());
        let space = crate::search::Space {
            slots: 20,
            side: SideCfg::NONE,
            search_wrap: true,
            genes: crate::search::Genes::default(),
        };
        let base = Params { iters: 80_000, restarts: 32, ..Params::default() };
        let migr = Params { migrate: Some(MigrateCfg::default()), ..base };
        let windows = [16usize, 8, 4, 2, 1, 0];
        let seeds: Vec<u64> = (0..3u64).map(|i| 0x5CA1 ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();

        let run_arm = |name: &str, p: &Params| {
            let mut best: Option<(f64, u32, Program)> = None;
            let mut ecs = Vec::new();
            for &seed in &seeds {
                let (prog, s) = crate::search::synthesize_flat_pt(&template, &space, &golden, &mask, &sp, p, &windows, 6, seed);
                let w = run(&prog, &sp);
                let ec = edge_cost(&golden, &w, &mask, 0);
                ecs.push(ec);
                if best.as_ref().map_or(true, |(bc, _, _)| ec < *bc) {
                    best = Some((ec, s.correctness, prog));
                }
            }
            let (be, bl, bp) = best.unwrap();
            let mut parts = Vec::new();
            for i in 0..32usize {
                if let Some(insn) = &bp.slots[i] {
                    parts.push(format!("{i}:{}", brief_insn(insn)));
                }
            }
            eprintln!("  {name}: best edge-cost={be:.1} (level-Hamming {bl}) | edge spread {ecs:?} | wrap {}..{}", bp.wrap_bottom, bp.wrap_top);
            eprintln!("    [{}]", parts.join("  "));
        };
        eprintln!("\nDME flat PT at scale (iters=80k, restarts=32, n={} seeds; golden ~30 edges):", seeds.len());
        run_arm("baseline ", &base);
        run_arm("migration", &migr);
    }

    /// THE NEW PATH: continuous cross-breeding island engine (no staging,
    /// densified edge objective, recombination instead of copy-migration). Does
    /// it birth the data-conditional mids the staged engine never reached, and
    /// beat the ~22 plateau? Reports the boundary/mid breakdown.
    ///
    /// Run: `cargo test --release -- --ignored dme_breed --nocapture`
    #[test]
    #[ignore = "cross-breeding island engine; run with --release ... --nocapture"]
    fn dme_breed() {
        use crate::program::Program;
        let (sp, golden, mask) = dme_golden();
        let zsp = RunSpec { inputs: vec![0, 0, 0, 0], ..sp.clone() };
        let boundaries = dme_edges01(&run(&dme_ref(DME_H).lower(), &zsp));

        let template = Program::empty(dme_cfg());
        let space = crate::search::Space { slots: 20, side: SideCfg::NONE, search_wrap: true, genes: crate::search::Genes::default() };
        let params = Params { iters: 200_000, ..Params::default() };
        let windows = [8usize, 8, 6, 6, 4, 4, 2, 2, 1, 1, 0, 0]; // fixed window ladder, 12 islands
        let seeds: Vec<u64> = (0..3u64).map(|i| 0xB433 ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();

        let mut best: Option<(f64, Program)> = None;
        for &seed in &seeds {
            let (champ, _) = crate::search::synthesize_flat_breed(&template, &space, &golden, &mask, &sp, &params, &windows, seed);
            let cw = run(&champ, &sp);
            let ec = edge_cost(&golden, &cw, &mask, 0);
            eprintln!("  seed{seed:#018x}:");
            dme_diagnose_wave(&golden, &cw, &mask, &boundaries);
            if best.as_ref().map_or(true, |(bc, _)| ec < *bc) {
                best = Some((ec, champ));
            }
        }
        let (be, bp) = best.unwrap();
        let mut parts = Vec::new();
        for i in 0..32usize {
            if let Some(insn) = &bp.slots[i] {
                parts.push(format!("{i}:{}", brief_insn(insn)));
            }
        }
        eprintln!("\nbreed best edge-cost={be:.1} (staged plateau was ~22) wrap {}..{}", bp.wrap_bottom, bp.wrap_top);
        eprintln!("  [{}]", parts.join("  "));
    }

    /// PLATEAU DIAGNOSIS: take a flat-engine champion and decompose its edge
    /// errors against golden. Golden edges are classified **boundary** (the
    /// data-independent clock — present in an all-zeros-corpus reference) vs
    /// **mid** (the data-conditional transition — the extra edges real data
    /// adds). Answers: does the champion have the clock but miss the data
    /// modulation? Run: `cargo test --release -- --ignored dme_diagnose --nocapture`
    #[test]
    #[ignore = "plateau diagnosis; run with --release ... --nocapture"]
    fn dme_diagnose() {
        use crate::program::Program;
        // (cycle, rising?) edges of bit 0.
        fn edges01(wave: &[u32]) -> Vec<(usize, bool)> {
            let mut out = Vec::new();
            let mut prev = 0u32;
            for (i, &s) in wave.iter().enumerate() {
                let v = s & 1;
                if v != prev {
                    out.push((i, v == 1));
                    prev = v;
                }
            }
            out
        }
        let near = |edges: &[(usize, bool)], c: usize, tol: usize| edges.iter().any(|&(e, _)| e.abs_diff(c) <= tol);

        let (sp, golden, mask) = dme_golden();
        // Boundary grid: same corpus length, all-zero codes → only boundary edges.
        let zsp = RunSpec { inputs: vec![0, 0, 0, 0], ..sp.clone() };
        let bwave = run(&dme_ref(DME_H).lower(), &zsp);
        let boundaries = edges01(&bwave);

        // A representative plateau champion.
        let template = Program::empty(dme_cfg());
        let space = crate::search::Space { slots: 20, side: SideCfg::NONE, search_wrap: true, genes: crate::search::Genes::default() };
        let params = Params { iters: 40_000, restarts: 32, ..Params::default() };
        let (champ, _) = crate::search::synthesize_flat_pt(&template, &space, &golden, &mask, &sp, &params, &[16usize, 8, 4, 2, 1, 0], 6, 0x5CA1);
        let cwave = run(&champ, &sp);

        let ge = edges01(&golden);
        let ce = edges01(&cwave);
        eprintln!("\ngolden: {} edges ({} boundary-grid)  champion: {} edges  edge-cost={:.1}", ge.len(), boundaries.len(), ce.len(), edge_cost(&golden, &cwave, &mask, 0));

        let (mut b_tot, mut b_hit, mut m_tot, mut m_hit) = (0, 0, 0, 0);
        let mut misses = Vec::new();
        for &(c, dir) in &ge {
            let is_boundary = near(&boundaries, c, 2);
            let matched = ce.iter().any(|&(cc, cd)| cd == dir && cc.abs_diff(c) <= 3);
            if is_boundary { b_tot += 1; b_hit += matched as i32; } else { m_tot += 1; m_hit += matched as i32; }
            if !matched { misses.push((c, if is_boundary { 'B' } else { 'M' }, if dir { '^' } else { 'v' })); }
        }
        // spurious: champion edges with no golden edge of same dir nearby
        let spurious = ce.iter().filter(|&&(cc, cd)| !ge.iter().any(|&(c, d)| d == cd && c.abs_diff(cc) <= 3)).count();

        eprintln!("  boundary (clock) edges matched: {b_hit}/{b_tot}");
        eprintln!("  mid (data-conditional) edges matched: {m_hit}/{m_tot}");
        eprintln!("  spurious champion edges: {spurious}");
        eprintln!("  unmatched golden edges (cycle/class/dir): {misses:?}");
        let mut parts = Vec::new();
        for i in 0..32usize { if let Some(insn) = &champ.slots[i] { parts.push(format!("{i}:{}", brief_insn(insn))); } }
        eprintln!("  champion [{}] wrap {}..{}", parts.join("  "), champ.wrap_bottom, champ.wrap_top);
    }

    /// DIAGNOSTIC: run the DME reference on a few line codes and print the pin
    /// waveform + per-bit transition counts, to eyeball correctness and tune
    /// timing. Run: `cargo test --release -- --ignored dme_inspect --nocapture`
    #[test]
    #[ignore = "diagnostic; run with --release ... --nocapture"]
    fn dme_inspect() {
        let h = 4u8;
        let g = dme_ref(h);
        eprintln!("dme_ref(h={h}): {}", show_gene(&g));
        eprintln!("validate: {:?}", g.validate());
        // One code at a time so the transition structure is legible.
        for &(name, code) in &[("0=11110", 0x1Eu32), ("F=11101", 0x1D), ("1=01001", 0x09), ("Idle=11111", 0x1F), ("Q=00000", 0x00)] {
            let sp = RunSpec {
                block: 0,
                sm: 0,
                inputs: vec![code],
                output_pins: vec![TX],
                capture_pins: vec![TX],
                cycles: 100,
            };
            let wave = run(&g.lower(), &sp);
            let levels: String = wave.iter().map(|s| if s & 1 != 0 { '#' } else { '_' }).collect();
            let edges = wave.windows(2).filter(|w| (w[0] ^ w[1]) & 1 != 0).count();
            // low 5 bits LSB-first are the line bits actually shifted out
            let bits: String = (0..5).map(|i| if (code >> i) & 1 != 0 { '1' } else { '0' }).collect();
            let pop = (code & 0x1F).count_ones();
            let want = 5 + pop; // 5 boundary edges + one mid edge per 1-bit
            let ok = if edges as u32 == want { "OK" } else { "??" };
            eprintln!("  {name:>10} bits(lsb→)={bits} edges={edges} want={want} {ok}  {levels}");
        }
    }

    /// Sanity: reference genes lower to legal programs that score 0 vs their own
    /// golden, and to the expected sizes.
    #[test]
    fn reference_genes_are_correct() {
        for k in [1u8, 4, 8] {
            let g = uart_gene(k);
            assert!(g.validate().is_ok(), "{:?}", g.validate());
            assert_eq!(g.lowered_len(), 6, "pull/set0/setx/out/jmp/set1");
            let sp = spec(18 + 8 * k as u64);
            let golden = run(&g.lower(), &sp);
            assert_eq!(score_masked(&g.lower(), &golden, &full_mask(&golden), &sp).correctness, 0);

            let d = data_loop_gene(k);
            assert_eq!(d.lowered_len(), 4);
            let dsp = spec(2 + 8 * k as u64);
            let dgolden = run(&d.lower(), &dsp);
            assert_eq!(score_masked(&d.lower(), &dgolden, &full_mask(&dgolden), &dsp).correctness, 0);
        }
    }

    /// GENE SYNTHESIS (the payoff test): two-phase synthesis over the structured
    /// genome. The data loop is the spine (loop now native); the full UART is
    /// the real test — can the search compose framing around a protected loop
    /// node and reach 0, where the flat-array macro stalled at 21/44?
    ///
    /// Run: `cargo test --release -- --ignored gene_synthesis --nocapture`
    #[test]
    #[ignore = "gene synthesis; run with --release ... --nocapture"]
    fn gene_synthesis() {
        // Per-stage budget (5 stages in the default schedule); temperature is
        // re-heated each stage.
        let params = Params { iters: 5000, restarts: 8, ..Params::default() };
        let sched = &DEFAULT_SCHEDULE;

        for k in [4u8, 8] {
            // Data loop (spine).
            let dsp = spec(2 + 8 * k as u64);
            let dgolden = run(&data_loop_gene(k).lower(), &dsp);
            let dmask = full_mask(&dgolden);
            // Length cap = reference size (4) + slack; bounds bloat like the slot window.
            let (dg, ds) = synthesize_gene(uart_cfg(), &dgolden, &dmask, &dsp, &params, sched, 7, 4, 0xDA7A + k as u64);
            eprintln!("\ndata_loop k={k}: correctness={} size={}", ds.correctness, ds.size);
            eprintln!("  {}", show_gene(&dg));

            // Full UART (spine + framing).
            let usp = spec(18 + 8 * k as u64);
            let ugolden = run(&uart_gene(k).lower(), &usp);
            let umask = full_mask(&ugolden);
            // Length cap = reference size (6) + slack.
            let (ug, us) = synthesize_gene(uart_cfg(), &ugolden, &umask, &usp, &params, sched, 10, 4, 0x0A27 + k as u64);
            eprintln!("UART k={k}: correctness={} size={}", us.correctness, us.size);
            eprintln!("  {}", show_gene(&ug));
        }
    }

    /// Run `seeds.len()` independent syntheses of one target in parallel and
    /// summarize the spread. Each thread reuses its own thread-local emulator,
    /// so there's no shared mutable state.
    fn reliability(
        label: &str,
        golden: &[u32],
        mask: &[u32],
        spec: &RunSpec,
        params: &Params,
        max_len: usize,
        seeds: &[u64],
    ) {
        // Each synthesis is internally parallel (chains per stage), so run the
        // seeds sequentially to avoid nested thread oversubscription.
        let results: Vec<(u32, u8, Gene)> = seeds
            .iter()
            .map(|&seed| {
                let (g, sc) =
                    synthesize_gene(uart_cfg(), golden, mask, spec, params, &DEFAULT_SCHEDULE, max_len, 4, seed);
                (sc.correctness, sc.size, g)
            })
            .collect();
        let solved = results.iter().filter(|(c, _, _)| *c == 0).count();
        let mut corrs: Vec<u32> = results.iter().map(|(c, _, _)| *c).collect();
        corrs.sort_unstable();
        let best = results.iter().min_by_key(|(c, _, _)| *c).unwrap();
        eprintln!(
            "\n{label}: solved {solved}/{} | correctness {:?} | best size={}",
            seeds.len(),
            corrs,
            best.1
        );
        eprintln!("  best: {}", show_gene(&best.2));
    }

    /// DIAGNOSTIC: why does the `data_loop` k=4 near-miss
    /// `pull / set y,3 / out[3] / jmp y--[3]` score 2, not 0, against the
    /// reference `pull / set x,3 / out[6] / jmp x--[0]`? They look cycle-for-
    /// cycle timing-equivalent. Print both waveforms and mark the differing
    /// cycles — evidence before theories.
    ///
    /// Run: `cargo test --release -- --ignored diagnose_residual --nocapture`
    #[test]
    #[ignore = "diagnostic; run with --release ... --nocapture"]
    fn diagnose_residual() {
        let sp = spec(2 + 8 * 4);
        let reference = data_loop_gene(4);
        let near = Gene {
            config: uart_cfg(),
            nodes: vec![
                pull(),
                Node::Loop {
                    cond: LoopCond::CountY,
                    counter_init: Some(3),
                    body: vec![Node::Prim(Insn {
                        op: Op::Out { dst: OutDst::Pins, count: 1 },
                        delay: 3,
                        sideset: None,
                    })],
                    jmp_delay: 3,
                },
            ],
        };
        let g = run(&reference.lower(), &sp);
        let c = run(&near.lower(), &sp);
        let n = g.len().max(c.len());
        let lvl = |w: &[u32], i: usize| if (w.get(i).copied().unwrap_or(0)) & 1 != 0 { '#' } else { '_' };
        let oe = |w: &[u32], i: usize| if (w.get(i).copied().unwrap_or(0) >> 16) & 1 != 0 { '#' } else { '_' };
        let refl: String = (0..n).map(|i| lvl(&g, i)).collect();
        let nearl: String = (0..n).map(|i| lvl(&c, i)).collect();
        let diff: String = (0..n).map(|i| if g.get(i) != c.get(i) { '^' } else { ' ' }).collect();
        eprintln!("ref  lvl: {refl}");
        eprintln!("near lvl: {nearl}");
        eprintln!("diff:     {diff}");
        eprintln!("ref  oe : {}", (0..n).map(|i| oe(&g, i)).collect::<String>());
        eprintln!("near oe : {}", (0..n).map(|i| oe(&c, i)).collect::<String>());
        let ndiff = (0..n).filter(|&i| g.get(i) != c.get(i)).count();
        eprintln!("differing cycles (fresh): {ndiff}");

        // Determinism probe: score `near` repeatedly, each time after running a
        // DIFFERENT program on the same reused thread-local emulator. If the
        // score wanders, reset() is leaking state between runs.
        let golden = run(&reference.lower(), &sp);
        let mask = full_mask(&golden);
        let others = [uart_gene(8).lower(), uart_gene(4).lower(), data_loop_gene(8).lower(), near.lower()];
        eprintln!("\ndeterminism probe (near's strict correctness, each after a different prior run):");
        for (i, other) in others.iter().enumerate() {
            let other_spec = spec(40 + i as u64 * 7);
            let _ = run(other, &other_spec); // perturb the reused emulator
            let s = score_masked(&near.lower(), &golden, &mask, &sp).correctness;
            eprintln!("  after prior run {i}: near correctness = {s}");
        }
    }

    fn sched(ks: &[(usize, f64)]) -> Vec<Stage> {
        ks.iter().map(|&(k, size_weight)| Stage { k, size_weight }).collect()
    }

    /// PORTFOLIO: run diverse schedules × seeds and take the strict-best. The
    /// sweep showed no single schedule is reliable and the right starting radius
    /// is target-dependent; a portfolio covers both without knowing it. Reports
    /// the combined solve over the whole (schedule × seed) set per target.
    ///
    /// Run: `cargo test --release -- --ignored gene_portfolio --nocapture`
    #[test]
    #[ignore = "portfolio; run with --release ... --nocapture"]
    fn gene_portfolio() {
        let params = Params { iters: 5000, restarts: 8, ..Params::default() };
        let seeds: Vec<u64> = (0..8u64).map(|i| 0x5EED ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();
        // Two diverse starting radii: covers short frames (k4 start) and long (k8 start).
        let portfolio = [
            sched(&[(8, 0.25), (4, 0.25), (2, 0.5), (1, 0.5), (0, 1.0)]),
            sched(&[(4, 0.25), (2, 0.5), (1, 0.5), (0, 1.0)]),
        ];
        for k in [4u8, 8] {
            let usp = spec(18 + 8 * k as u64);
            let ugolden = run(&uart_gene(k).lower(), &usp);
            let umask = full_mask(&ugolden);
            let mut best = u32::MAX;
            let mut runs = 0;
            let mut first_solve = None;
            for (si, s) in portfolio.iter().enumerate() {
                for &seed in &seeds {
                    let c = synthesize_gene(uart_cfg(), &ugolden, &umask, &usp, &params, s, 10, 4, seed).1.correctness;
                    runs += 1;
                    if c < best {
                        best = c;
                    }
                    if c == 0 && first_solve.is_none() {
                        first_solve = Some((si, runs));
                    }
                }
            }
            eprintln!(
                "UART k={k}: combined best={best} over {runs} runs | first solve at {:?} (schedule_idx, run#)",
                first_solve
            );
        }
    }

    /// PARAMETER SWEEP: use parallelism to find a better tolerance schedule, and
    /// to test the k=4<k=8 inversion theory (a fixed large starting radius
    /// over-blurs the shorter frame and smears its data). Solve rate over seeds
    /// for several schedules × both UART targets.
    ///
    /// Run: `cargo test --release -- --ignored gene_param_sweep --nocapture`
    #[test]
    #[ignore = "parameter sweep; run with --release ... --nocapture"]
    fn gene_param_sweep() {
        let params = Params { iters: 5000, restarts: 8, ..Params::default() };
        let seeds: Vec<u64> = (0..6u64).map(|i| 0xBEEF ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();
        let schedules: Vec<(&str, Vec<Stage>)> = vec![
            ("A k8 5-stage (default)", sched(&[(8, 0.25), (4, 0.25), (2, 0.5), (1, 0.5), (0, 1.0)])),
            ("B k4 4-stage", sched(&[(4, 0.25), (2, 0.5), (1, 0.5), (0, 1.0)])),
            ("C k2 3-stage", sched(&[(2, 0.5), (1, 0.5), (0, 1.0)])),
            ("D k8 7-stage fine", sched(&[(8, 0.25), (6, 0.25), (4, 0.25), (3, 0.5), (2, 0.5), (1, 0.5), (0, 1.0)])),
            ("E k1 2-stage", sched(&[(1, 0.5), (0, 1.0)])),
        ];

        for k in [4u8, 8] {
            let usp = spec(18 + 8 * k as u64);
            let ugolden = run(&uart_gene(k).lower(), &usp);
            let umask = full_mask(&ugolden);
            eprintln!("\n=== UART k={k} ===");
            for (name, s) in &schedules {
                let solved = seeds
                    .iter()
                    .filter(|&&seed| {
                        synthesize_gene(uart_cfg(), &ugolden, &umask, &usp, &params, s, 10, 4, seed).1.correctness == 0
                    })
                    .count();
                eprintln!("  {name:<24}: solved {solved}/{}", seeds.len());
            }
        }
    }

    /// RELIABILITY: how often does each target solve across seeds? Characterizes
    /// the variance of the annealed-tolerance schedule (single-seed runs showed
    /// UART k=8 solving but easy targets sometimes regressing). Parallel.
    ///
    /// Run: `cargo test --release -- --ignored gene_reliability --nocapture`
    #[test]
    #[ignore = "reliability sweep; run with --release ... --nocapture"]
    fn gene_reliability() {
        let params = Params { iters: 5000, restarts: 8, ..Params::default() };
        let seeds: Vec<u64> = (0..8u64).map(|i| 0xC0FFEE ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();

        for k in [4u8, 8] {
            let dsp = spec(2 + 8 * k as u64);
            let dgolden = run(&data_loop_gene(k).lower(), &dsp);
            let dmask = full_mask(&dgolden);
            reliability(&format!("data_loop k={k}"), &dgolden, &dmask, &dsp, &params, 7, &seeds);

            let usp = spec(18 + 8 * k as u64);
            let ugolden = run(&uart_gene(k).lower(), &usp);
            let umask = full_mask(&ugolden);
            reliability(&format!("UART k={k}"), &ugolden, &umask, &usp, &params, 10, &seeds);
        }
    }

    /// Run `seeds` syntheses and return (solved_count, sorted correctness).
    fn solve_rate(
        config: Config,
        golden: &[u32],
        mask: &[u32],
        spec: &RunSpec,
        params: &Params,
        max_len: usize,
        seeds: &[u64],
    ) -> (usize, Vec<u32>) {
        let mut corrs: Vec<u32> = seeds
            .iter()
            .map(|&seed| {
                synthesize_gene(config, golden, mask, spec, params, &DEFAULT_SCHEDULE, max_len, 4, seed)
                    .1
                    .correctness
            })
            .collect();
        let solved = corrs.iter().filter(|&&c| c == 0).count();
        corrs.sort_unstable();
        (solved, corrs)
    }

    /// TICKET 001 A/B: independent chains (baseline) vs. async island migration,
    /// at *equal compute* (same iters/restarts/seeds — migration only changes
    /// whether the concurrent chains of a stage share a blackboard). Reports
    /// solve-rate for each arm on the two UART targets (the hard cases).
    ///
    /// Migration is non-deterministic (adoption depends on thread timing), so
    /// numbers will vary run to run; look at the aggregate solve-rate, not a
    /// single seed.
    ///
    /// Run: `cargo test --release -- --ignored migration_ab --nocapture`
    #[test]
    #[ignore = "ticket 001 A/B; run with --release ... --nocapture"]
    fn migration_ab() {
        let base = Params { iters: 5000, restarts: 8, ..Params::default() };
        let migr = Params { migrate: Some(MigrateCfg::default()), ..base };
        let seeds: Vec<u64> = (0..12u64).map(|i| 0xA1B2 ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();

        for k in [4u8, 8] {
            let usp = spec(18 + 8 * k as u64);
            let ugolden = run(&uart_gene(k).lower(), &usp);
            let umask = full_mask(&ugolden);

            let (sb, cb) = solve_rate(uart_cfg(), &ugolden, &umask, &usp, &base, 10, &seeds);
            let (sm, cm) = solve_rate(uart_cfg(), &ugolden, &umask, &usp, &migr, 10, &seeds);
            eprintln!("\nUART k={k} (n={})", seeds.len());
            eprintln!("  baseline   : solved {sb}/{} | {cb:?}", seeds.len());
            eprintln!("  migration  : solved {sm}/{} | {cm:?}", seeds.len());
        }
    }
}

/// Compact rendering of one instruction (op + delay + side-set).
fn brief_insn(ins: &Insn) -> String {
    let d = if ins.delay > 0 { format!("[{}]", ins.delay) } else { String::new() };
    let ss = match ins.sideset {
        Some(v) => format!(".s{v}"),
        None => String::new(),
    };
    let op = match &ins.op {
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

/// Compact one-line rendering of a gene's node tree, for experiment logs.
pub fn show_gene(g: &Gene) -> String {
    fn render(nodes: &[Node], out: &mut Vec<String>) {
        for node in nodes {
            match node {
                Node::Prim(i) => out.push(brief_insn(i)),
                Node::Loop { body, cond, counter_init, jmp_delay } => {
                    let init = match counter_init {
                        Some(n) => format!("={n}"),
                        None => "=data".into(),
                    };
                    let mut inner = Vec::new();
                    render(body, &mut inner);
                    let jd = if *jmp_delay > 0 { format!("[{jmp_delay}]") } else { String::new() };
                    out.push(format!("loop({cond:?}{init}){{ {} }}jmp{jd}", inner.join("  ")));
                }
                Node::Cond { cond, then, els, dispatch_delay, skip_delay } => {
                    let mut t = Vec::new();
                    render(then, &mut t);
                    let dd = if *dispatch_delay > 0 { format!("[{dispatch_delay}]") } else { String::new() };
                    let sd = if *skip_delay > 0 { format!("[{skip_delay}]") } else { String::new() };
                    if els.is_empty() {
                        out.push(format!("if({cond:?}){dd}{{ {} }}{sd}", t.join("  ")));
                    } else {
                        let mut e = Vec::new();
                        render(els, &mut e);
                        out.push(format!("if({cond:?}){dd}{{ {} }}else{{ {} }}{sd}", t.join("  "), e.join("  ")));
                    }
                }
            }
        }
    }
    let mut parts = Vec::new();
    render(&g.nodes, &mut parts);
    format!("len={} [{}]", g.lowered_len(), parts.join("  "))
}
