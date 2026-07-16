# 012 — 008-B: outcome-predicate reads (value-set constraints)

**Status:** stage 1 (E1) LANDED 2026-07-15 late (worktree commits
1e9a7d9 substrate+partition, 1ca971c frame-open set conds red-green,
3bdd880 walk rule; L=2 smokes −35.7%/−35.8% items, verdicts
unchanged; irq-absence open check RESOLVED below) · **Source:** dead-demand
census (merged ff6a4b3, journal 2026-07-13) + realness tests (2acc442);
the read-rule half of ticket 011's superposition program. Line numbers
below are against master @ 719d66c (engine.rs ~4750 lines,
mod.rs `exec_op` 382-677).

---

## AMENDMENT 2026-07-15 (evening) — E1 re-cut: environment-read predicates land FIRST

**Source:** w12 monster trace mining + seed-orbit analysis
(docs/analysis/narrow-split-w12-unit-mining.md, w12-seed-orbits.md).
User rulings: E1 = new stage 1; seeds carry constraints; junk-window
cleanliness relaxation = separate ticket after E1; implement before
re-measuring (long runs are wall-clock expensive — land known-positive
narrowing levers first).

### Why the stage-0 census missed the mass

Stage 0 instrumented 011(b)'s TAG-collapse sites; a WAIT never touches
a tag, so the environment-read fan was invisible to it BY
CONSTRUCTION. The w12 mining is the complementary measurement: 71% of
the 1..2 monster's CPU was byte-identical repeat subtrees, ~87% of the
redundant mass slot-0 prologue respelling, and the biggest exact
orbits are the satisfied-WAIT alphabet (124 of 126 members; the
excluded indices reproduce the config's pin-idle/DE/irq-stim map
precisely). There are two read planes: (i) tagged-register reads
(the original ladder, needs 011), and (ii) reads of the CONCRETE
environment through undecided SELECT fields (WaitPol/WaitSrc/WaitIdx
choose WHICH known signal to read). Plane (ii) carries the monster
mass and needs no tags at all.

### §1 taxonomy amendment

Reclassify `WAIT gpio/pin` (mod.rs:421-432) from "Ctl/external,
non-target" to **Pred(met-now) on the select fields**: at fork time
the gpio state is concrete in the item, so met(pol, src, idx) is a
deterministic per-raw-value evaluation — the §2 fork rule applies
with the environment in place of a tag replay. `WAIT irq` stays
excluded: pol=1 clears a per-idx flag (the original reason), and
pol=0 irq waits were absent from the w12 orbits for a reason not yet
understood (see open checks) — do not scope irq until that is
explained.

### Stage E1 — met-now WAIT grouping (the new stage 1)

Mechanism, per the §2 fork rule with these specifics:

- WaitPol/WaitSrc keep today's eager concrete forks (2- and 3-wide —
  cheap, and forking them first makes the idx partition a clean
  SINGLE-FIELD set per (pol, src) branch; no cross-field constraints,
  §7's exclusion stands).
- At the WaitIdx demand with pol/src concrete: evaluate the wait
  condition per allowed idx against the item's concrete gpio state
  (reuse the emulator's own condition eval — do NOT reimplement the
  in_base mapping). Partition = {met-now} ∪ {unmet, individually}.
  Met arm writes nothing (gpio/pin sources), so met-now is ONE
  outcome class: push one child with Constraint(slot, idx-fmask,
  met_set) (singleton ⇒ decide as today; all-met ⇒ no fork at all,
  §2 rule 2). Unmet values fork concretely as today — their stall
  futures genuinely diverge (DE toggles, self-driven pins).
- Re-execution (loop return or jmp back into the slot) re-partitions
  the CURRENT allowed set at the new cycle — §2 "later predicate
  read intersects" verbatim; the met set can only shrink.
- Fork-frame read accounting: the partition evaluation consults the
  gpio state and the field — charge per the S7 discipline (the
  fork frame's OWN record carries the killed/grouped values' reads).

E1 delivers the identical substrate the original stage 1 was designed
to force — Item constraint block, class enumeration, set-valued conds
+ P ⊆ S probing + partition drop rule (§3), champion sets (§4),
junk_walk single-class rule (§4) — with NO 011(b) dependency and a
monster-scale measured payoff (the original stage 1's honest
expectation was "small"). Original stages renumber: zero-test JMPs →
stage 2, dec transform → stage 3, counter thresholds → stage 4, OUT
pin-classes → stage 5.

### Scope delta: seeds carry constraints (REQUIRED, was deferred)

§ "Split driver" deferred constraint-carrying seeds. E1 cannot leave
it deferred: phase 1's truncated search is exactly where the met fan
explodes into 60+ frontier units, and if frontier champions cannot
carry their constraint the fan re-materializes as seeds and the
split layer keeps paying it. Frontier units (Champion) and seed
tuples gain the constraint block; `validate_seed` extends to check
constraint well-formedness (S6-style: within values_into, nonempty,
fmask consistent with the slot's decided bits); unit resume traces
serialize it (Champion already derives serde). This is the one
genuinely new soundness surface of the re-cut — S1-analog red-green
micro-spec FIRST, per §3.

### Explicitly out of E1

- Stall-class future-grouping (grouping unmet waits by known
  completion cycle from the stim timeline) — needs future-environment
  reasoning; a later stage if the residue measures large.
- WAIT irq (above). Open check RESOLVED (2026-07-15, measured): the
  w12 body can `jmp 0` back into the prologue slot (JMP targets
  enumerate 0..slots), so orbit membership required vacuousness at
  EVERY reachable execution cycle, not just the prologue cycle.
  Unstimmed gpio/pin levels are time-invariant, but the irq plane is
  written by both the environment AND the program: flag 0 is
  stim-latched high from cycle 10 (pol-0 wait on it stalls on any
  jmp-back re-execution), and flags 1..7 — never stimulated — are
  settable by body `IRQ set k` words, so `wait 0 irq k` re-executed
  after one stalls too. Evidence (master engine, re-derived w12
  decomposition, byte-identical 1,383,452 units): unit fingerprints
  (items, refuted, cycles_run, walk_cycles, tags_created) —
  `wait 0 gpio 1 [1]` ≡ `mov pins,pins [1]` EXACTLY
  (43,367,431 items, all five counters equal — the orbit);
  `wait 0 irq 1 [1]` differs by −12,115 items (the irq-set→jmp-back
  paths); `wait 0 irq 0 [1]` by −291,000 items and −125,200 tags (the
  stim-latched flag). Not an engine artifact — a true semantic
  non-equivalence. Consequence for any future irq scoping: pol-0 irq
  grouping would have to condition on flag-plane WRITES (program +
  stim), not just current flag state; E1's gpio/pin-only scope stands.
- Delay ladders (73% of monster forks): the companion junk-window
  cleanliness relaxation ("reads with window-invariant values don't
  dirty the window") — separate ticket, sequenced after E1. The
  deeper "delay as incremental threshold predicate" unification is
  REJECTED for now: delay lives in KeyCore and this must not grow
  into a memo-key redesign.

### E1 gates (amendments to §6's standing gates)

- Standing gates as written (fast suite, L1/L2 exact censuses, memo
  on/off, split-vs-sequential, instrumentation-inert, determinism
  locks, proven-bracket VERDICT identity — items are expected to DROP
  on WAIT-bearing brackets; report deltas, verdicts must not change).
- The widening micro-spec becomes a wait-set spec: a config where
  `wait 0 <pin>` survives for a known index set ⇒ ONE champion whose
  constraint covers the set, coverage = |set| balanced against the
  exact census (vs |set| separate champions today).
- Seed round-trip gate: split a WAIT-bearing bracket, SIGINT, resume
  — byte-identical final stats with constraints in the trace.
- Wall-clock magnitude gates DEFERRED to idle box / b-srv0 (user
  ruling: implement first; the box is running the 0..2 monster).
  Verdict/item gates run now at low test-thread counts.
- Prediction logged pre-measurement (falsifiable): w02 (wrap 0..2, NO
  prologue slot) should show duplicate-CPU fraction well below w12's
  71%. Record the measured value here when the trace is mined:
  **MEASURED 2026-07-15: 63.4% — prediction WRONG in spirit.** The
  fraction fell only 7.7 points. The prologue *story* died with the
  prologue (w12's ~87%-of-redundancy prologue-respelling class cannot
  exist in w02), but the redundant mass largely survived by changing
  shape: the s0:delay-only class GREW 18.1%→28.8%, and the 126-word
  no-op alphabet reappeared at slot 0 *inside the loop* (7×126-unit
  orbits, 24.5% of redundant CPU) — the alphabet is loop-invariant,
  not prologue-specific. Consequence: E1's met-now grouping (which
  collapses the 124 satisfied-WAIT members) and 013 (delay classes)
  matter for ALL brackets, not just prologue-bearing ones; the
  delay-only/self-sync class (013 v2) is now the single largest named
  redundancy class. Details: docs/analysis/w02-mining-and-orbits.md.

---

## Why this exists (measured)

1. **86.0%** of single-conflict cond-misses on the 2..2 monster wall are
   output-equal but STATE-divergent — dead-register divergence
   downstream of data-plane forks (realness test 1).
2. Fork mass: **Delay 44%, BitCount 28.6%, MovSrc 15.2%** (realness
   test 2).
3. The dead-demand census killed the pure-laziness hope for BitCount:
   **BitCount forks are ~0% dead** — in a wrap loop the shift chain
   RE-READS the shifted register within a few cycles (IN reads SC_ISR,
   OUT reads SC_OSR|SC_OSR_CNT unconditionally, `word_state_reads`
   engine.rs:1245-1261). Ticket 011's deferral (don't fork until read)
   gains nothing when the read always comes. The collapsible-by-laziness
   class is only **MOV/SetData ≈ 11.5%** of fork mass.

The census verdict is precise: the value IS read — but almost never as
a full 32-bit value. A serializer reads its OSR **one pin-visible
sliver at a time** plus a **threshold compare**; a loop reads its
counter register as **`== 0`**. 008-B forks on the predicate OUTCOME
(or the consumed sub-value), not on the full field value. This is the
lever the census says the 28.6% + the 86% class actually need.

## Investigation: the exact 0.7500 MovSrc dead fraction

The census reported MovSrc/MovOp tracked 36,820, live 9,204, dead
27,616 — dead-frac exactly 0.7500 (both rows identical). Structural
explanation, from the census code and fork machinery:

- **Tracked MOV chains come in quadruples.** `dd_classify`
  (engine.rs:356-395) tags a MovOp/MovSrc fork only when the
  already-decided MOV dst (read from the partial word, `(w>>5)&0x7`)
  is a register: **x(1), y(2), isr(6), osr(7) — 4 of the 8 MovDst
  values**. MovDst itself is not a census kind; per MOV context the
  MovDst fork spawns exactly 4 tracked chains, one per register dst.
  The MovOp fork is the fresh tag and the MovSrc fork at the same pc
  MERGES into it, counted once per chain (engine.rs:3949-3964) — hence
  the identical MovSrc/MovOp rows. 36,820 = **4 × 9,205 contexts**.
- **Each chain's first outcome is produced by the stage-2 junk_walk of
  its first-explored MovSrc child — which is always `src = osr`.**
  Children are pushed in `values_into` order ([0,1,2,3,5,6,7],
  engine.rs:582-586) onto a LIFO stack (push at 4104), so raw 7 = OSR
  pops first. That child completes the MOV (tag arms, 4343-4351),
  survives the capture check (a register MOV moves no latches), and
  hits the delay post-fork, where `junk_walk` fires (4415-4439).
- **The tracked population is dominated by wrap-slot MOVs** (bracket
  2..2 loops slot 2 onto itself; prologue-slot forks happen once per
  decided prefix, structurally rare — thousands of contexts reach the
  slot-2 fork, a handful reach slots 0/1). The walk therefore
  immediately RE-EXECUTES the MOV: `word_state_reads(mov dst, osr)` =
  **SC_OSR only** (engine.rs:1273-1286). The walk's dd hook
  (3436-3440) sets `dd_walk_read` iff that intersects the tag; the
  window is clean (no latch writes, `reads_external` false for
  src=osr) so the family co-refutes within a few cycles when the
  expected trace toggles.
- Therefore the chain resolves **LiveRead ⟺ dst = osr**; the dst =
  x/y/isr chains resolve DeadRefute — the loop's only register read is
  the SRC, and the first-explored src is OSR. Exactly **1 live + 3 dead
  per quadruple**: 9,204 live + 27,616 dead = 9,204 × (1+3) + one
  exceptional all-dead quadruple (27,616 = 3 × 9,205 + 1; plausibly a
  prologue-slot MOV or a first-child capture refute on a
  trace-transition cycle). Dead fraction = 3/4 **by construction**.

Two consequences worth recording:

- **0.7500 is a census artifact of first-outcome-seen + LIFO order**,
  not a fine-grained liveness measurement. It measures "dst ≠ the
  first-explored src (osr)". The per-(dst,src) truth — a MOV→reg in a
  tight loop is dead unless the loop re-reads that register, i.e.
  dst == src self-moves or a downstream reader — is if anything MORE
  favorable to laziness than 0.75 (4 self-read combos of 28). The
  ticket-011 v1 scope (MOV/SetData ≈ 11.5%) stands; treat 0.75 as a
  lower-bound-shaped estimate, not a precision number.
- **The BitCount ~0.0001 rows are the SAME mechanism with the sign
  flipped**: IN/OUT self-loops read their own register unconditionally
  (`word_state_reads` returns SC_ISR / SC_OSR|SC_OSR_CNT for every
  IN/OUT), so the walk always reports live. Those live reads are
  one-hop self-chain reads — exactly the reads this ticket converts
  into sub-value/predicate demands.

---

## 1. Read taxonomy (line-by-line against `step()`/`exec_op`)

Classification of every state interaction. **Full** = the read needs
the whole value (collapse under 011). **Pred(p)** = only predicate p's
outcome matters this cycle. **Sub** = only a bit-slice/sub-value
matters. **Sink** = written somewhere nothing ever reads (free).
**Ctl** = control plane, always concrete (KeyCore set, engine.rs:
1155-1179), out of scope by the 011 plane boundary.

Cycle machinery (`step` mod.rs:322-367, engine loop):

| Site | Class |
|---|---|
| delay countdown, stall re-check (`still_stalled` mod.rs:283-298: FIFO LEVELS, gpio, irq), fetch pc, clkdiv | Ctl |
| capture/compose (latches + stimulus) | Ctl |
| side-set latch write (mod.rs:350-359) | Ctl (code field, latch-feeding) |

`exec_op`:

| Opcode / site | Reads | Class |
|---|---|---|
| JMP cond 0 (always) mod.rs:389 | — | none |
| JMP cond 1/3 (`!x`/`!y`) mod.rs:390,398 | x or y | **Pred(v == 0)** — 2 classes |
| JMP cond 2/4 (`x--`/`y--`) mod.rs:391-405 | x/y; writes v−1 when ≠0 | **Pred(v == 0)** + transform write (dec) on the ≠0 arm |
| JMP cond 5 (`x!=y`) mod.rs:406 | both | **Pred(x == y)**, relational; tag-identity ⇒ equal (011 §4); one-side-concrete c ⇒ Pred(v == c); both-symbolic ⇒ collapse |
| JMP cond 6 (pin) mod.rs:407 | gpio | Ctl/external |
| JMP cond 7 (`!osre`) mod.rs:408 | osr_count | **Pred(cnt < pull_threshold)** — counter threshold |
| JMP target (taken) mod.rs:412 | 5-bit field → pc | Full (feeds pc = Ctl; already consult-deferred, 008 stage 1, engine.rs:4137-4214) |
| WAIT gpio/pin mod.rs:421-432 | gpio | Ctl/external |
| WAIT irq mod.rs:433-440 | irq flag | Ctl (flags concrete); the undecided IDX field is NOT outcome-groupable — the met arm clears a DIFFERENT flag per idx. Non-target. |
| IN autopush pre-flush mod.rs:450-458 | isr_count, RX full | **Pred(cnt >= push_threshold)** + Ctl level; the flushed ISR → RX is **Sink** (below) |
| IN src x/y/isr/osr mod.rs:459-467 | low `bc` bits of src | **Sub** — only bits 0..bc of the source are consumed |
| IN old-ISR shift mod.rs:470-483 | isr | Full into the new ISR (Fn1 accumulate or collapse — 011 (c) territory) |
| IN isr_count accum mod.rs:484 | counter | write; CntProv-carried (008 stage 4), read only via the threshold predicates |
| IN post-shift autopush mod.rs:486-490 | isr_count, RX full | **Pred(cnt >= push_threshold)**; pushed ISR is **Sink** |
| OUT autopull mod.rs:497-508 | osr_count, TX | **Pred(cnt >= pull_threshold)** + Ctl level; popped word = Input(k) (011 (d)) |
| OUT shift-out mod.rs:509-527 | osr, bc | **Sub** — splits OSR into consumed `data` (bc bits) + residual; residual is a tag write (Fn1 shift), the consumed part classifies per dst below |
| OUT dst PINS/PINDIRS mod.rs:530-533,537-540 | data → latch | **Sub of the Sub**: the latch takes `cfg.out_count.min(bc)` bits of data — the only capture-visible part. THE key partial-bit read (see §2 classes). |
| OUT dst x/y mod.rs:534-535 | data | Full move of the consumed sub-value → tag write (Fn1) |
| OUT dst NULL mod.rs:536 | — | discard: the shift itself is the only effect — pure residual/counter write |
| OUT dst PC mod.rs:541 | data → pc | Full + must concretize (Ctl) |
| OUT dst ISR mod.rs:544-547 | data | Full move → tag write; counter := bc (CntProv Set) |
| OUT dst EXEC mod.rs:548 | data → pending_exec | Full + must concretize (pending_exec is KeyCore, engine.rs:1161) |
| OUT osr_count accum mod.rs:527 | counter | write; read via threshold predicates only (stage (a) of 011 makes `word_state_reads`' SC_OSR_CNT autopull-conditional) |
| PULL IFEMPTY guard mod.rs:562 | osr_count | **Pred(cnt < pull_threshold)** — guard-skip vs proceed |
| PULL empty/nonblock mod.rs:565-573 | TX level, x | Ctl level; X read is Full (moves into OSR as a tag COPY — 011) + the P1 binding predicate (§4) |
| PUSH IFFULL guard mod.rs:579 | isr_count | **Pred(cnt >= push_threshold)** |
| PUSH mod.rs:590-592 | isr → RX | **Sink**: no ISA path reads RX contents; `project_state` stores RX LEVEL only (engine.rs:1212-1220). A symbolic ISR can be pushed as a dummy word with a concrete level — no collapse, ever. (The capture never sees it either.) |
| MOV src x/y/isr/osr mod.rs:610-618 | src | Full — but a register dst makes it a tag COPY (free, 011); op `!`/`::` mod.rs:621-624 folds into Fn1 |
| MOV src STATUS mod.rs:613-616 | FIFO level | Ctl (level predicate on concrete state) |
| MOV dst PINS/PINDIRS/PC/EXEC mod.rs:628-633 | val | Full + must concretize (latch/Ctl) |
| MOV dst x/y/isr/osr mod.rs:629-643 | — | tag write; counter resets concrete effects |
| IRQ mod.rs:649-661 | irq flags | Ctl |
| SET dst x/y mod.rs:669-670 | — | tag creation (011); `fetch_footprint` already treats the imm as lazy (engine.rs:3290-3293) |
| SET dst PINS/PINDIRS mod.rs:668,671 | imm → latch | Ctl (latch-feeding; stays eager + pre-filtered) |

Engine-side (outside `exec_op`):

- `jmp_taken` peek (engine.rs:1063), used by the deferred-target fork
  and junk_walk — same Pred rules as JMP.
- `is_pull_empty_read` (1048) — level (Ctl) + the IFEMPTY counter Pred.
- Binding demand `x != y` (4262-4270) — relational Pred; 011 §4 rules
  (tag-identity ⇒ equal; else collapse). Unchanged here.
- Pin-write pre-filter (4034-4084) — needs concrete consulted
  registers; under a constraint whose class is singleton it may run,
  otherwise skip (sound: the filter is an optimization).
- Stall re-checks (`stall_state_reads` 1489-1496) — levels/flags, Ctl.

The predicate inventory is small and closed: **zero-test (x/y),
equality-to-concrete, counter-threshold (4 sites: JMP !osre, PULL
IFEMPTY, PUSH IFFULL, autopush/autopull entry), and the OUT
pin-visible sub-value.** Everything else is Full, Sink, or Ctl.

## 2. Fork semantics: value-set constraints

**The representation insight: every forkable field has at most 32
legal raw values** (`values_into`, engine.rs:513-607 — 5-bit fields).
Any subset of a field's candidate set is exactly **one u32 bitmask
over raw values**. Define:

```
Constraint ::= (slot: u8, fmask: u16, allowed: u32)   // bit i ⇔ raw i legal
```

Today's item knowledge per field is two-valued: undecided (allowed =
all of values_into) or decided (`decided`/`value` bitmasks, Item
engine.rs:201-238 — allowed = singleton). Constraints are the
intermediate lattice points: **decided ⊂ constrained ⊂ free**, ordered
by set inclusion. Nothing about `decided`/`value` changes; the
constraint block is additive:

- `Item` gains a small fixed array of Constraints (cap 3 suggested,
  ~8 bytes each ≈ +28 bytes with a count byte; forks stay struct
  copies). **Overflow policy: collapse the oldest constraint** (fork
  its allowed set fully, then drop it) — bounded and deterministic.

**Fork rule at a predicate read of a tag** (008-B's core). A read
classified Pred(p) hits a register whose tag (011) is
`Field(s, m)` or `Fn1(op, cap, s, m)`:

1. Enumerate the field's current allowed set (values_into ∩ any
   outstanding constraint), replay the tag transform per raw value,
   evaluate p — **≤32 cheap concrete evaluations at fork time**,
   deterministic, no solver. This yields the outcome partition
   {S_true, S_false} (counter thresholds: two intervals; OUT
   pin-classes: k sets, below).
2. Push one child per NONEMPTY class. A singleton class decides the
   field outright (today's machinery). A non-singleton class writes a
   Constraint. **If only one class is nonempty, no fork happens at
   all** — the predicate outcome is implied; execution continues with
   the constraint unchanged (constraint propagation for free).
3. The child executes the predicate's arm concretely (the outcome is
   now known). Writes on that arm follow 011 rules: `x--`'s ≠0 arm
   writes `Fn1(dec, s, m)` (needs `dec` in the transform table —
   stage 2); a second transform collapses (011's die-on-transform).

**OUT pin-visible classes** (the BitCount prize). `out pins, bc` with
concrete OSR and undecided bc: the capture-visible effect is
`write_pin_field(data(bc), out_base, out_count.min(bc))` and the
stall/counter effect is the threshold compare. Group the 32 bc raws by
the pair (latch-write result, threshold outcome); the residual OSR is
`Fn1(shift, osr_snapshot, bc)` and the counter accumulates the
constrained field (CntProv accum, engine.rs:1385-1394). Class count
is config-dependent and this is where the honesty lives: **right-shift
with out_count=1 (the tx_a serializer shape) gives ONE latch class**
(`data & 1 = osr & 1` for every bc ≥ 1) × 2 threshold outcomes;
left-shift or wide out_count degrades toward 32 classes = today.

**Later reads against an outstanding constraint:**

- A later **full-value read** (unsupported op, MOV to pins, binding
  event...) collapses: enumerate values_into ∩ allowed, one child per
  survivor, field becomes decided, constraint slot freed. The word-
  quotient sibling dedup (engine.rs:4020-4033) applies inside this
  enumeration exactly as at a fetch fork.
- A later **predicate read** of the same field intersects: partition
  the CURRENT allowed set. Classes shrink monotonically; loops that
  re-test the same predicate re-derive a single nonempty class and
  stop forking (the `jmp !x` spin on x∈{≠0} tests {≠0}: one class,
  no fork, no item growth — this is what makes predicate constraints
  strictly better than eager re-collapse in loops).
- Constraints never relax; they are refined or consumed. Tags are
  never rewritten by decisions (011 §3 invariant), so a constraint's
  (slot, fmask) identity is stable for its lifetime.

## 3. Memo interplay

Current machinery: records keyed by KeyCore (1155-1179) → per
read-mask state-projection tables (MemoEntry 1686-1688) → RecList of
packed conds (`pack_cond` slot|mask|value u64, 1512-1516); probe match
= `decided & m == m && value & m == v` (3786-3790); insert-side
subsumption (1522-1542); the decided-filter drops conds on fields
forked below the frame (`consulted_mask & f.decided`, 1850-1855).

**KeyCore: unchanged** (all Ctl — same argument as 011 §3). **State
patterns: unchanged beyond 011** — a predicate read consults the
DEFINING FIELD (program cond), not the SC component, exactly the
CntProv routing precedent (`consume_reads` 1426-1447). Constraints
live entirely on the program-cond side. What changes:

- **Set-valued conds.** A subtree that refuted under constraint
  (s, fmask, S) records the cond "for all values of field s∈S" — this
  IS the outcome-grouped record ("refuted for all x==0" when S came
  from a zero-test; more generally "refuted for the whole ≠0 class").
  Packing: a second cond kind `(slot, fmask, allowed:u32)` — slot(5) +
  fmask(16) + set(32) = 53 bits, still one u64 with a discriminator
  bit. Value-conds are singleton set-conds; keep the old encoding for
  them so the hot path's common case is untouched.
- **Prober match = set inclusion.** Prober knowledge per field P
  (decided ⇒ singleton, constrained ⇒ its set, free ⇒ full): cond
  (s, fmask, S) matches iff **P ⊆ S**. Today's decided check is the
  singleton case verbatim. A free prober matches only S = full (which
  the decided-filter never emits) — same shape as today's
  "undecided prober misses". This is the wall conversion: two probers
  with DIFFERENT decided imms (5 vs 17) both satisfy an S = {≠0} cond
  where today they pairwise cond-miss.
- **Subsumption** (`subsumes`): cond A implied by cond B on the same
  (slot, fmask) iff S_B ⊆ S_A. Mixed value/set conds compare through
  the singleton embedding. The RecList cap/benefit machinery is
  agnostic.
- **Recording — the partition drop rule.** Today a field fully forked
  below the frame drops out of conds ("all values explored", decided-
  filter). Predicate forks explore an exhaustive PARTITION of the
  fork-time allowed set, and every deeper refinement (further
  predicate forks, collapses) is exhaustive over its class by
  induction — so the same drop is sound: **generalize
  `Frame.decided` to frame-open field knowledge** (decided mask +
  constraint snapshot) and drop any cond component not implied by
  frame-open knowledge. A cond on a field constrained ABOVE the frame
  survives as the frame-open set (the subtree's claim is relative to
  it), mirroring how decided-above conds survive today.
- **What probers must NOT match:** a record whose subtree read the
  predicate outcome under class S must not fire on a prober whose
  knowledge admits values outside S — guaranteed by P ⊆ S. And the
  S1-analog hazard: constraints, unlike Prov, are NOT segment-local —
  they live on the item and cross frames; the cond emission must
  therefore always express reads as frame-open-relative sets (the
  drop rule above), never as "current" sets, or an enclosing frame
  records a condition its own key state predates. This is the exact
  shape of soundness finding S1 (flush_prov, 1449-1471) and gets the
  same treatment: red-green micro-spec FIRST (narrow_soundness.rs
  convention), then the rule.

## 4. Interactions

- **junk_walk** (3343-3475): a walk reaching a predicate read of a
  live tag evaluates the class membership of the walk's allowed set:
  single nonempty class ⇒ continue concretely on that outcome (the
  walk stays representative for the whole family); otherwise bail as
  a fork edge (return false — same contract as the fetch-demand bail
  at 3406). Constraint/tag transitions mirrored exactly as CntProv is
  (the stage-4 discipline). The stage-2 collapse rate on Delay (44%
  of fork mass) must not regress — walk-bail frequency is a gated
  stat.
- **P1 mirror/binding** (011 §4 + S6): `mirror_word` (899-924)
  permutes register-SELECT field VALUES and never touches data
  fields. Constraints on data fields (SetData, BitCount) mirror as
  identity; a constraint on a select field (none planned in v1)
  mirrors by permuting the set — and unlike partial (mask, value)
  pairs (the S6 concern, 926-948), **explicit value-sets are closed
  under mirror_word's permutations**, so the whole-field invariant
  holds. Binding events collapse symbolic x/y first (011 rule);
  collapse enumerates the constrained set. `Rec::bound`/S3 logic is
  orthogonal and unchanged.
- **word_canon quotient** (754-897): sibling dedup applies inside
  constrained collapses via the same per-mask tables. Predicate
  classes are unions of behavior classes (quotient-equal words have
  equal execution, hence equal predicate outcomes), so class
  construction and the quotient never disagree. Champion-side
  coverage accounting must use |allowed ∩ enumerated| per field.
- **Champion materialization.** `Champion::words()` materializes
  don't-cares as 0 (108-122) — **wrong for a set excluding the
  0-completion** (e.g. surviving constraint {≠0}). Champion gains the
  constraint list; `words()` materializes the minimum allowed member;
  `assert_champions_sound` (tests/narrow_engine.rs:45-61) extends to
  materialize per-set extremes and a seeded-random member (the 011
  gate (ii) extension, which this ticket needs even more). L1/L2
  exact censuses (census_l1, tests/narrow_engine.rs:117-152) remain
  the coverage ⇔ trace-match oracle: a champion now covers
  Σ|allowed| completions and the census counts must balance exactly.
- **Split driver:** constraints are Copy, fixed-size; the worker
  protocol and frontier units are untouched. `validate_seed` (S6)
  gains nothing new in v1 (seeds stay whole-field decided; a
  constraint never enters via seed).
- **Determinism:** classes are derived by deterministic enumeration in
  raw-value order; children pushed in canonical class order (ascending
  minimum member). Locked determinism tests must pass; any verdict
  change on proven brackets (0..0/0..1/1..1) is a bug, full stop.

## 5. Composition with ticket 011 — one lattice, one program

Tags and constraints are **complementary axes of the same design, not
rival mechanisms**:

- 011's tag answers "WHERE does this register's value come from" — a
  register → defining-field pointer (plus one transform).
- 012's constraint answers "WHAT is known about that field's value" —
  a point on the per-field subset lattice, of which decided/value is
  the singleton level.
- The predicate read is the RULE THAT CONNECTS THEM: tag lookup →
  class computation on the field's allowed set → constraint write.

Neither half stands alone: without tags, predicate reads reach only
the direct decode-field consults (JmpCond outcome-grouping — measured
at 2.5% of the wall, which is why 008-original was demoted); without
constraints, tags collapse fully at every read and the census says
the reads always come (BitCount ~0% dead). **Recommendation: one
build program, two tickets, interleaved ladder** — concretely:

1. 011 stage (a) (OSR CntProv) and re-cut stage (b) (x/y Field tags,
   die-on-transform) land first, as planned. They force the Item/
   two-lane/memo-projection/binding decisions and own the 11.5%
   MOV/SetData slice.
2. **Amend ticket 011 to STOP at (b)**: its stage (c) (Fn1 as bare
   laziness) is refuted by the census (BitCount live-chain) and its
   Fn1 machinery lands here instead — inside stage 4 below, where the
   transform is paired with the predicate classes that make it pay.
   011 (d) (Input(k), multi-case) is untouched and follows later.
3. This ticket's stages run after 011(b), on its tag machinery.

Ticket 008 closes into 011 + this ticket (its original "value-set
items / outcome-classed records" sketch is §2/§3 here, made concrete;
its stages 1/2/4 are landed engine facts).

## 6. Staged build ladder (smallest-first, one commit + gates each)

Standing gates for every stage: the fast suite (17 incl.
narrow_soundness.rs), L1/L2 exact censuses, memo on/off equivalence,
split-vs-sequential, instrumentation-inert; magnitude gate = idle-box
WALL-CLOCK + item counts on L=2 and L=3 0..1/1..1 (proven-bracket
verdicts must be unchanged), 150s 2..2 mine for composition shifts.
Any evaluator-cost change re-opens the 008 §3b walk-economics
re-measurement (user directive; do not skip, log it). Fallback per
stage = revert the commit; constraints are additive machinery.

- **Stage 0 — predicate-read census (measurement before code, the
  dd-census convention).** Env-gated probe at 011(b)'s collapse
  sites: classify each would-be collapse by the §1 taxonomy and
  record the would-be class count (zero-test: 2; threshold: 2;
  OUT-pins: computed latch-class count under the live cfg). Output:
  fork-width saved per predicate kind on the 150s 2..2 mine. This
  converts "BitCount 28.6%" into "class-compressible fork mass X% at
  Y× average width reduction" and orders stages 3/4 by measured
  value. Gate: instrumentation-inert (items byte-identical flag-off).
- **Stage 1 — JMP `!x`/`!y` zero-test predicates on tagged x/y.** The
  cleanest slice: 2 classes, no writes on either arm, tag kinds
  limited to `Field` (SetData / MOV-copy — exactly 011(b)'s tags).
  {0} child decides the field (singleton); {≠0} child writes the
  first Constraint. Delivers the FULL new machinery at minimum
  semantics: Item constraint block, class enumeration, set-valued
  conds + P ⊆ S probing + partition drop rule, champion sets,
  junk_walk single-class rule. Extra gates: a red-green frame-
  soundness micro-spec (S1-analog, §3) BEFORE the feature commit; a
  widening micro-spec (spec where `set x, imm; jmp !x` survives only
  for imm≠0 ⇒ ONE champion with allowed={1..31}, coverage 31, vs 31
  champions today — assert via the extended assert_champions_sound).
  Honest effect expectation: small (it inherits 011(b)'s modest
  slice); its job is landing the representation, like (b)'s was.
  Note the deliberate split from the task's `x--` grouping: a
  zero-test child pair costs 2 pushes vs 32 with NO new transform;
  `x--` without a dec transform still pays 1+31 = 32 and is pure
  plumbing noise — it goes to stage 2 where the transform makes it
  2 pushes.
- **Stage 2 — `x--`/`y--` with a `dec` transform.** {0} arm: untaken,
  register unchanged (still 0). {≠0} arm: taken, register :=
  `Fn1(dec, s, m)` under the ≠0 constraint; a second decrement (loop
  iteration) collapses (die-on-second-transform — counting loops
  ultimately concretize, but the FIRST iteration's 32-way fork
  becomes 2, and refutation usually arrives first: 84.5% subtree
  co-refutation). This is the loop-counter shape (delay loops,
  `jmp x-- target`). Gate adds a counted-loop micro-spec (census-
  exact coverage of a `set x, n / jmp x--` delay loop).
- **Stage 3 — counter threshold predicates.** The four guard sites
  (§1) over SYMBOLIC counters: single-BitCount-fed counters (CntProv
  base-accounted + one field — the common serializer case) partition
  that field by the threshold compare; multi-field sums collapse in
  v1 (interval arithmetic is an open question, §7). Requires 011(a)
  (OSR-count CntProv twin) landed. Gates add the serializer battery
  (bd6f0ac specs): verdicts and champion sets IDENTICAL, wall within
  noise — registers are live there and the counters are exactly what
  these specs exercise.
- **Stage 4 — OUT pin-visible sub-value classes + Fn1(shift).** The
  BitCount 28.6% head-on: group bc by (latch write, threshold
  outcome); residual OSR becomes `Fn1(shift, snap, bc)` (the Fn1
  machinery arrives HERE, from 011(c), paired with the classes that
  pay for it); chained OUTs collapse at the second shift (two
  unknowns) — by which time the first OUT's capture check has already
  pruned on the pin-visible class. Gates: everything above + 2..2
  units-settled at the 50-min gate (the monster gate that stage-2
  junk-collapse set the precedent for) + the output-only 2..2 mine
  where the 86% class conversion should finally show as cond-miss →
  hit composition shift. Success here is what re-arms the monsters
  verdict campaign.

Stage order rationale: 0 is free and de-risks 3/4's sizing; 1 is the
smallest slice that forces every hard decision; 2 reuses 1's classes
with the first transform; 3 needs only CntProv plumbing; 4 is the
effect-size stage and takes the largest new machinery (Fn1) last.

## 7. Risks and open questions (honest)

- **Recording soundness is the hard part, again.** The partition drop
  rule and frame-open-relative set conds are exactly the terrain
  where S1/S2/S3 lived. Mitigation: red-green micro-specs BEFORE each
  feature commit (the soundness-suite convention), memo on/off
  equivalence as a standing gate. Expect at least one subtle hole;
  budget for it.
- **Hot-path cost.** The probe/subsume loops run on packed u64s and
  were ~55% of search CPU before packing (RecList header). Set-conds
  add a discriminator branch to the innermost cond check. The
  singleton fast path must stay byte-identical; idle-box wall-clock
  gates are mandatory per stage (the 008 §3b lesson: item counts
  alone lie).
- **Two-way forks that die anyway.** A predicate fork whose both
  classes refute quickly pays 2 pushes + a constraint where today's
  32 children might all die in the pre-filter for ~free. The stage-0
  census measures width-saved, not wall-saved; only the per-stage
  wall gates decide.
- **Constraint-block overflow storms.** Cap 3 with collapse-oldest
  could thrash on constraint-dense programs. Counter:
  `constraint_overflows` stat (instrumentation-inert), gate on it
  staying rare; bump the cap only with measured cause.
- **Config-dependence of the stage-4 prize.** The one-class collapse
  needs right-shift + narrow out_count. tx_a and the RS-485 firmware
  targets fit; wide-bus configs get little. State the win as
  config-conditional in any verdict claims.
- **Relational predicates are out.** `x != y` with both sides
  symbolic, and any cross-field constraint ("bc1 + bc2 >= 32"), are
  not representable — collapse. Multi-field counter sums (stage 3)
  likewise. If the stage-0 census shows large mass there, that is a
  different, solver-shaped ticket — do not grow this one into it.
- **Loops concretize eventually.** Constraints refine monotonically;
  a counted loop's k-th iteration has singleton knowledge — the win
  is bounded by refutation depth (84.5% co-refute early, which is
  the bet). If the monsters' refutations turn out DEEP on exactly
  the constrained paths, gains shrink; the 2..2 gate will say so.
- **Champion semantics ripple.** Coverage counting (Σ|allowed|),
  census tooling, champ-mine dumps, and the family-census scripts all
  assume decided-or-free fields. Budget a tooling pass; gate L1/L2
  census balance exactly.
- **Dependency risk.** Stages 1-4 sit on 011(a)/(b). If (b) misses
  its gates, this ticket has no substrate — the JmpCond-field-only
  fallback is measured at 2.5% and NOT worth building alone.
- **Delay stays untouched.** 44% of fork mass is time-plane; stage-2
  junk-collapse owns it. No claim here moves it.
