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

use crate::cost::{hamming_tolerant, score_masked, Score};
use crate::gene::{Gene, LoopCond, Node};
use crate::ir::{Insn, Op, OutDst, SetDst, SideCfg};
use crate::program::Config;
use crate::rng::Rng;
use crate::run::{run, RunSpec};
use crate::search::{
    random_delay, random_sideset, Params, IN_SRCS, MOV_DSTS, MOV_OPS, MOV_SRCS, OUT_DSTS, SET_DSTS,
    WAIT_SRCS,
};

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
    match rng.below(9) {
        0 => {
            let pos = rng.below(n as u32 + 1) as usize;
            m.nodes.insert(pos, Node::Prim(random_prim_insn(rng, &m.config.side)));
        }
        1 => {
            let pos = rng.below(n as u32 + 1) as usize;
            insert_serializer(&mut m.nodes, pos, rng, &m.config.side);
        }
        2 => {
            m.nodes.remove(rng.below(n as u32) as usize);
        }
        3 => {
            let i = rng.below(n as u32) as usize;
            m.nodes[i] = if rng.boolean() {
                Node::Prim(random_prim_insn(rng, &m.config.side))
            } else {
                random_loop(rng, &m.config.side)
            };
        }
        // Timing-aware moves: restructure at constant total duration.
        4 => insert_compensated(&mut m.nodes, rng, &m.config.side),
        5 => remove_compensated(&mut m.nodes, rng),
        6 => shift_cycles(&mut m.nodes, rng),
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
/// - `k` is the tolerance-band radius (see [`hamming_tolerant`]): large early
///   to smooth the landscape, annealed to 0 (strict) to certify.
#[derive(Debug, Clone, Copy)]
pub struct Obj {
    pub w: f64,
    pub size_weight: f64,
    pub max_len: usize,
    pub k: usize,
}

/// Scalar cost: `∞` if the gene exceeds `max_len` or lowers to an illegal
/// program, else `w·correctness + size_weight·lowered_len`, where correctness
/// is the tolerance-band metric at radius `obj.k`.
fn cost_gene(g: &Gene, golden: &[u32], mask: &[u32], spec: &RunSpec, obj: &Obj) -> f64 {
    if g.lowered_len() > obj.max_len || g.validate().is_err() {
        return f64::INFINITY;
    }
    let wave = run(&g.lower(), spec);
    let corr = hamming_tolerant(golden, &wave, mask, obj.k);
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

/// One annealing chain: a single Metropolis run with geometric cooling from
/// `start` (or a fresh random gene). Returns the chain's best gene. Stateless
/// across chains, so chains parallelize freely.
fn anneal_chain(
    config: Config,
    start: Option<Gene>,
    golden: &[u32],
    mask: &[u32],
    spec: &RunSpec,
    params: &Params,
    obj: &Obj,
    seed: u64,
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
    let mut population: Vec<Gene> = Vec::new();
    let mut global_best: Option<(Gene, Score)> = None;

    for (idx, stage) in schedule.iter().enumerate() {
        let obj = Obj { w: params.w, size_weight: stage.size_weight, max_len, k: stage.k };
        let pop = &population;
        let scored: Vec<(Gene, f64)> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..params.restarts)
                .map(|r| {
                    let start = if pop.is_empty() { None } else { Some(pop[r as usize % pop.len()].clone()) };
                    let chain_seed = seed
                        ^ ((idx as u64).wrapping_mul(0x1000_0000_0000_0001))
                        ^ (r as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                    s.spawn(move || {
                        let g = anneal_chain(config, start, golden, mask, spec, params, &obj, chain_seed);
                        let g = polish_gene(&g, golden, mask, spec, &obj);
                        let c = cost_gene(&g, golden, mask, spec, &obj);
                        (g, c)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        population = elite(scored, elite_n);

        // Track the globally strict-best across every elite, and early-exit on
        // a certified-correct solution.
        for g in &population {
            let s = strict_score(g, golden, mask, spec);
            let better = global_best.as_ref().map_or(true, |(_, bs)| {
                (s.correctness, s.size) < (bs.correctness, bs.size)
            });
            if better {
                global_best = Some((g.clone(), s));
            }
        }
        if global_best.as_ref().is_some_and(|(_, s)| s.correctness == 0) {
            break;
        }
    }

    global_best.expect("schedule has at least one stage")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Insn, Op, OutDst, SetDst, SideCfg};
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
            }
        }
    }
    let mut parts = Vec::new();
    render(&g.nodes, &mut parts);
    format!("len={} [{}]", g.lowered_len(), parts.join("  "))
}
