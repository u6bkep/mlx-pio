# 011 — Data-plane superposition (provenance-tag symbols)

**Status:** RE-CUT 2026-07-14 — this ticket now ENDS at stage (b);
see "Re-cut" below · **Source:** realness tests on the 2..2 wall +
champion census (journal 2026-07-13 evening/late-evening/night); user
design thread "late evening". Subsumes ticket 008's ORIGINAL
outcome-grouped-forking design; enables multi-case specs (ticket 010
prereq, RX flagship adjacency).

## Re-cut (2026-07-14): this ticket stops at stage (b)

The dead-demand census (merged ff6a4b3) refuted stage (c) as BARE
laziness: **BitCount forks are ~0% dead** — in a wrap loop the shift
chain re-reads the shifted register within a few cycles (IN reads
SC_ISR, OUT reads SC_OSR unconditionally, `word_state_reads`), so
"don't fork until read" gains nothing there; the collapsible-by-pure-
laziness class is only **MOV/SetData ≈ 11.5%** of fork mass. Deferral
without a predicate/sub-value read rule cannot touch the 28.6%
BitCount mass.

Consequences (per ticket 012 §5 — 011 and 012 are ONE lattice: tags =
where a value comes from, value-set constraints = what is known about
the defining field; the predicate read is the rule connecting them):

- **Stage (c) is DELETED from this ticket.** Its `Fn1` machinery moves
  to **012 stage 4**, where the transform is paired with the OUT
  pin-visible predicate classes that make it pay.
- **Stages (a) and (b) land here as planned** (they own the 11.5%
  MOV/SetData slice and force the Item/memo-projection/binding/
  junk_walk decisions that 012 builds on).
- **Stage (d) (`Input(k)`, multi-case specs) is unchanged** and
  follows later — after 012's ladder — under this ticket's design.
- Ticket 008 closes into 011(a,b) + 012, as recorded in 012 §5.

## Measured motivation

Four measurements, all 2026-07-13:

1. **86.0% of the 2..2 wall's single-conflict cond-misses are
   output-equal but STATE-divergent** (realness test 1, probes
   2acc442: of 18.6M single-conflict cond-misses lock-step raced to
   the conflict slot's completion, equiv 13.3%, state_div 86.0%,
   capture divergence 52 of ~19.7M sampled). Conditioned
   word-interchange lemmas cap at **13.3%** of the wall; the other
   86% needs a LIVENESS argument — "the diverged state is dead" —
   that no static or conditioned interchange lemma can express.
2. **Fork attribution: Delay 44%, BitCount 28.6% (155.6M), MovSrc
   15.2% (82.8M)**, JmpTarget 3.7%, MovOp 2.6% (realness test 2,
   `Stats::fork_kinds`, engine.rs:147-152). Data-plane fields
   (BitCount + MovSrc/MovOp/InSrc + OutDst-partial + SetData) carry
   **~35-45% of fork mass**.
3. **Champion census: ~250,000:1 dead-effect multiplicity on
   output-only specs** (sq4_l3: 223,223 champions → top family
   223,128 = 99.96%; pulse14_l3: 248,978 → 1 family). The mega
   families decompose as dead-effect instructions (registers written,
   never observed), equivalent pin-writers, and delay placement. On
   DATA-DRIVEN serializer specs the density collapses ~3 orders of
   magnitude (**340 autopull / 2 explicit-pull champions**) —
   registers become live, killing the dead-effect class. The 250K:1
   prize is output-only-specific; on protocol targets the same
   mechanism's value concentrates on the refutation wall instead.
4. **84.5% of cond-miss subtrees provably co-refute** (recursive pair
   races, 770K deep races, 100% latch-quiet, true divergence 37) —
   the state divergence downstream of data-plane forks is dead: the
   refutation arrives before anything reads the diverged registers.

All four point at one mechanism: the engine forks data-plane decode
fields into concrete values at consult time even when the consult is
only a WRITE into a register nothing ever reads. "Consulted ≠
mattered" — the same gap on the champion side (multiplicity) and the
memo side (state-divergent cond-misses).

## The concept

Registers **x, y, ISR, OSR, both shift counters, and TX-FIFO words**
become provenance-tagged SYMBOLS in the item:

```
Tag ::= Concrete(u32)            -- today's world
      | Field(slot, mask)        -- the undecided field's future value
      | Fn1(op, captured, slot, mask)  -- ONE transform level: a closed
                                 --  function of one undecided field,
                                 --  all other operands captured
                                 --  concrete at write time
      | Input(k)                 -- k-th spec input word (stage d;
                                 --  multi-case: resolves per case)
```

The CONTROL PLANE — pc, delay counter, stall, clkdiv accumulator, IRQ
flags, pin latches, `pending_exec` — **stays concrete**, exactly the
`KeyCore` component set (engine.rs:851-876).

- A **write** of an undecided field into a register stores the tag
  WITHOUT forking the field. `set x, <imm>` with the immediate
  undecided becomes ONE child with `x = Field(slot, 0x001F)` instead
  of 32 children (today: `demand()` engine.rs:407-452 forks the
  SetData field eagerly at fetch, engine.rs:439).
- A **read** of a symbolic register demands the defining bits — fork
  the field THEN (deferred, exactly the pattern 008 stage 1 built for
  JMP targets, engine.rs:3590-3645) and replay the stored transform
  per child. A later refinement forks only the outcome predicate the
  read needs (`jmp !x` needs `x==0`, not x) — this is 008's original
  outcome-grouped forking, subsumed as a refinement of the read rule.
- An **unread** register leaves its defining field undecided forever:
  champion don't-cares widen (the field never enters `decided`), memo
  records stop conditioning on it (it never reaches `seg_mask` or
  `state_reads`), and the 86% state-divergent conflict class becomes
  symbol-equal memo probes.
- `Input(k)` symbols additionally give **multi-case specs**: a
  program is input-phase-invariant iff it never demands the
  case-distinguishing symbol (ticket 010's determinism prefilter and
  trie-shared prefixes fall out of the same mechanism).

This is `Prov`/`CntProv` (engine.rs:995-1006, 1022-1092) promoted
from memo-side accounting to the value representation itself. 008
stage 4 (CntProv, merged 912cb77, −26% memo entries) is the proof of
the pattern at its smallest: a field-set provenance that a read
consults instead of the state component. Superposition puts the tag
IN the register and lets it suppress the fork, not just the record
condition.

---

## 1. Representation

**Where tags live.** Two options:

- (a) Inline: widen `NState`'s `x/y/isr/osr: u32` to the Tag enum.
  Rejected for v1: `NState` (112 bytes measured) is shared by
  `step()`/`exec_op` (mod.rs:322-677), `run_spec*`
  (engine.rs:4120-4244), `junk_walk`, the pin-write pre-filter's
  scratch state (engine.rs:3535), the probes, and the KeyCore/
  MemoKey — every one of them would pay the enum, and the hot
  concrete path stops being the certified twin-spec interpreter.
- (b) **Side-band in `Item` (recommended v1):** `NState` stays
  byte-identical; `Item` (engine.rs:201-225) gains a small tag block:
  a `sym: u8` bitmap (which of x/y/isr/osr/isr_cnt/osr_cnt is
  symbolic) plus tag payloads. A register whose bitmap bit is set has
  garbage in its `NState` slot and its truth in the tag. `sym == 0`
  ⇒ the item is all-concrete and takes today's exact code path.

**Size/Copy budget.** Measured on the current build (temporary
size-probe test, this worktree): `Item` = **252 bytes** (`decided` 64
+ `value` 64 + `NState` 112 + cycle/next_input 8 + 3 flags + pad),
`NState` = 112, `Fifo` = 36, `KeyCore` = 32, `Frame` = 328,
`CntProv` = 18. A fork is a struct copy plus one field write
(engine.rs:201-202, `let mut child = it;` at 3552/3615/3829). Tag
block budget: bitmap (1) + 4 register tags (Fn1 needs op + captured
u32 + slot/mask ≈ 8-10 bytes each, so ≈ 40) + 2 counter tags
(CntProv-shaped, 18 each — or reuse the existing per-segment CntProv
and add only a persistent base tag) ≈ **+48-80 bytes, Item ≈
300-330**. At ~1.3µs/item (008 §3b post-mortem numbers) an extra
~64-byte copy per fork is noise in isolation, but stack traffic and
L1 pressure are exactly where this engine lives (IPC 4.2,
L1-resident — architecture.md "The working pipeline") — the wall-
clock gate below is mandatory, not a formality.

TX-FIFO words (stage d only): a pushed word is always
`spec.inputs[next_input]` at push time (engine.rs:3291-3294,
3412-3416; mod.rs Fifo), so a symbolic TX word is always `Input(k)`
with k = the push position — one u8 bitmap over the 8 buf slots,
reusing the `buf` u32 to hold k. RX-FIFO words need NO tags: no ISA
path reads RX contents (project_state comment, engine.rs:909-917);
pushing a symbolic ISR stores a dummy and keeps the level concrete.

**One transform level.** `Fn1` exists because the two serializer-
shaped writes are not identity: `out x, n` writes
`shift(osr_snapshot, n)` and `in isr` accumulates
`shift(isr_snapshot) | src_bits`. The rule that keeps this closed:
**a tag may be a function of at most ONE undecided field, with every
other operand captured concrete at write time** — then a read can
fork that field and REPLAY the function per child. `Input(k) >> n`
(concrete n, symbolic base) is the mirror case for stage d. Chains
(second transform on an already-symbolic value, two undecided fields
feeding one write) are not representable.

**Collapse-to-concrete policy.** Any operation the tag grammar cannot
express **collapses its symbolic inputs first**: fork the defining
field(s) at that point (deferred demand — the fork the write
originally dodged, paid only now, only on paths that reached the
unsupported op), replay to concrete, then execute. This is strictly
lazier than today (today the fork happens at fetch, unconditionally)
and never lazier than sound. Collapse sites are enumerable: see §2.

## 2. Demand semantics — read-site enumeration

Classification of every read site in `exec_op` (mod.rs:382-677) and
the engine cycle loop. "Demand" = collapse the tag (fork defining
field, v1) or, as a later refinement, fork only the outcome classes
of the listed predicate.

Always-concrete sites (control plane / capture — no change):

- `step` entry: `delay_count`, stall re-check (`still_stalled`
  mod.rs:283-298 reads FIFO LEVELS — levels stay concrete — plus
  gpio/irq), fetch pc. All KeyCore.
- Capture: `capture_word`/`compose` (engine.rs:802-814, mod.rs:684)
  read only `out_latch`/`dir_latch` + external stim — latches are
  always concrete because every latch WRITE demands concrete data
  (below).
- Side-set (mod.rs:350-359): decode-field-driven latch write, always
  concrete (it is a code field, not a register).

Register read sites and their v1 rule:

| Site | Reads | v1 rule / eventual predicate |
|---|---|---|
| JMP `!x`/`!y` (mod.rs:390-391, 398) | x or y | demand full value; predicate = `v == 0` (2 outcome classes vs up-to-32-way fork — the 008-B refinement) |
| JMP `x--`/`y--` (mod.rs:392-397) | x/y, then writes v−1 | demand full value in v1. OPEN QUESTION: `Fn1` cannot hold "Field − k" across iterations (loop = chain); predicate forking needs value-SET tags (008-B proper) |
| JMP `x!=y` (mod.rs:406) | both | demand both unless tag-identical (same tag ⇒ equal ⇒ not taken) |
| JMP `!osre` (mod.rs:408) | osr_count | counter predicate `cnt < pull_threshold` — CntProv-style consult if count symbolic |
| IN src x/y/isr/osr (mod.rs:459-466) | low `bc` bits of src | demands only the bits it shifts: for `Field(s,m)` source, a SUB-FIELD demand of the low-bc defining bits is legal; v1 demands the whole field (simpler, sound) |
| IN old-ISR shift (mod.rs:470-483) | isr | if ISR symbolic and dest chain stays dead: write `Fn1(shift_or, captured, bc-field)` when exactly one operand is undecided; else collapse |
| IN isr_count (mod.rs:450, 484-490) | counter | today already lazy without autopush (`word_state_reads` engine.rs:942-957, 008 stage 4); autopush threshold compare = counter predicate |
| OUT data shift (mod.rs:509-527) | osr, osr_count, bc | dest x/y/isr with concrete osr + undecided bc → `Fn1(shift, osr_snapshot, bc-field)`; symbolic osr + undecided bc = two unknowns → collapse |
| OUT dst PINS/PINDIRS (mod.rs:530-540) | data → latch | **demand concrete** (capture reads latches) |
| OUT dst PC (mod.rs:541) | data → pc | **demand concrete** (control plane) |
| OUT dst EXEC (mod.rs:548) | data → pending_exec | **demand concrete** — `pending_exec` sits in KeyCore (engine.rs:858); a symbolic pending word would contaminate core equality |
| PULL empty-read of X (mod.rs:570) | x | binding-relevant: see §4; the value itself may move as a tag copy into osr |
| PULL/PUSH guards (mod.rs:562, 579) | osr_count/isr_count | counter predicate (`cnt >= threshold`) |
| MOV src x/y/isr/osr (mod.rs:610-618) | src | op none → tag COPY (free); op `!`/`::` (mod.rs:621-625) on `Field` → fold into `Fn1`; on `Fn1` → second level → collapse |
| MOV dst PINS/PINDIRS/PC/EXEC (mod.rs:628-633) | val | **demand concrete** (latch/control) |
| MOV dst x/y/isr/osr (mod.rs:629-643) | — | tag write; counter resets stay concrete effects |
| IRQ / WAIT (mod.rs:417-443, 649-661) | irq/gpio | control plane, unchanged |
| SET dst x/y (mod.rs:669-670) | — | **the tag creation site**: `Field(slot, 0x001F)`; today this forks 32× at fetch |
| SET dst PINS/PINDIRS (mod.rs:668, 671) | imm → latch | concrete (already footprint-visible; `fetch_footprint` engine.rs:2888 shows the imm-laziness precedent for the X/Y case) |

Engine-side (outside exec_op) read sites:

- `jmp_taken` peek (engine.rs:760-772), used by the deferred-target
  fork (3591) and junk_walk (3005): same predicate rules as JMP.
- `is_pull_empty_read` (745-755): osr_count guard + TX level —
  counter predicate + concrete level.
- Binding demand `it.st.x != it.st.y` (3678, and twin construction
  3689): §4.
- Pin-write pre-filter (3529-3551): only fires when `writes_pin_latch`
  — whose data path demands concrete anyway, so the scratch `step`
  stays on concrete state. No change, but the firing condition must
  check `sym`-cleanliness of the consulted registers (a MOV PINS from
  a symbolic src must demand first, not pre-filter).
- Memo probe `project_state` (3318): §3.
- `word_state_reads` (926-987) stays the read TABLE; what changes is
  the routing in `consume_reads` (1123-1144): today X/Y route through
  segment-local `Prov`, resetting to `Fork` at every pop (995-997).
  Under superposition the tag IS the provenance and PERSISTS across
  segments — `Prov::Fork` ("consult the state pattern") applies only
  to `Concrete` registers, `Prov::Field` generalizes to the tag's
  defining field, and the segment-reset disappears for tagged
  registers.

## 3. Memo interplay (the hardest part)

Current structure: `MemoEntry` keyed by `KeyCore` (851-876), sets of
(read-mask, `Vec<u32>` state projection → `RecList`) (1337-1339),
probe at engine.rs:3298-3396, record insert in `close_child`
(1474-1486) projecting the FORK-TIME state under the subtree's
accumulated `state_reads`.

**KeyCore is unchanged.** Every KeyCore component is control-plane
and stays concrete — core equality survives verbatim. This is the
payoff of drawing the plane boundary exactly at the KeyCore set.

**Patterns mostly stay concrete too**, by the routing argument: a
subtree read of a TAGGED register consults the defining field
(program cond via `seg_mask`) — not `SC_X` — so a state-reads bit is
set only when the read target was CONCRETE at the read. The projected
value for a read component is therefore a real u32 in the common
case. Symbols reach patterns only at the edges:

- **Probe side:** the prober's state under a record's mask may hold a
  tag where the record read a concrete value. v1: **skip the set** —
  treat as state-miss. Exact precedent: `mirror_blocked` skips any
  set whose mask touches SC_X|SC_Y (3314).
- **Record side:** the frame's fork-time state (f.key.2) may hold a
  tag in a component a DESCENDANT read after collapse (the collapse
  fork happened below this frame, so a deeper segment read concrete
  x while the frame's snapshot has the tag). Today's analog is a
  below-frame `SET x, imm` followed by a read: the outer frame
  over-conditions on its stale x — sound, just conservative. v1 for
  tags: encode projected values as tagged u64s (`Concrete(v)` ↔ v;
  `Field(s,m)` ↔ marker|s|m; `Fn1` ↔ marker|op|s|m|captured-hash) and
  define pattern equality as STRUCTURAL TAG EQUALITY: same tag
  matches, differing tags miss, concrete-vs-tag miss.

**Why same-tag matching is sound:** two items at the same core whose
x is the same `Field(s, m)`: if the record's subtree never read x, no
condition exists — hit is the prize and trivially sound. If it read x,
it collapsed: the fork below the record frame enumerated EVERY legal
value of (s,m) (`values_into`, 309-403) and the decided-filter
(1437-1443) drops the field from conds — the record's claim already
quantifies over the field, so it covers a prober whose (s,m) is
undecided-with-same-tag AND one whose (s,m) has since been decided to
any enumerated value. (A decided prober's tag must still encode
(s,m) identically — tags are never rewritten by later decisions,
which keeps this true by construction.)

**What v1 loses vs gains.** Gains: the 86% class — records over
subtrees whose diverged registers were dead stop conditioning on them
(they were never read ⇒ never in mask ⇒ symbol-equal probes hit);
and, bigger, most of those sibling items NEVER EXIST because the
write didn't fork. Loses: (i) concrete-vs-tag misses — a population
mixing collapsed and never-collapsed items at one core bifurcates the
table; a concrete prober cannot use a record whose pattern holds a
tag even when its decided value is one the record covered
(recoverable later: "tag subsumes any enumerated concrete value on
the same field" is a sound one-directional widening — OPEN QUESTION
whether the probe-side check stays cheap); (ii) two different tags
denoting the same runtime value miss (symbol-equality coarser than
value-equality in both directions — see §7).

## 4. P1 mirror / binding interplay

P1 machinery: unbound items stand for their register-mirror twin
(Item.unbound, 218-224; doc header engine.rs:32-48); binding demand
fires on PULL's physical-X empty read or a register-touching
`pending_exec` word (3661-3719), gated by `it.st.x != it.st.y`
(3678); the twin materializes by mirroring decided words
(`mirror_word`, 695-720) and swapping x/y (3699).

- **Tags mirror trivially.** `mirror_word` rewrites only register-
  SELECT fields (JMP cond, the 3-bit src/dst codes); tags are created
  from DATA fields (SetData 0x1F, BitCount 0x1F, MOV-op bits) whose
  bit positions and values are mirror-invariant. A `Field(s, m)` tag
  in the twin denotes the mirrored word's same data field — same
  (s, m), same value under any completion. `std::mem::swap(x, y)` on
  the tag block swaps tags like values. No new mechanism.
- **The predicates are the problem**: `x != y` at the binding demand
  (3678) and `twin_differs` (3689), plus `mirror_blocked = unbound &&
  x != y` at the probe (3307), are undecidable on symbolic x/y.
  **v1 rule: any symbolic x or y at a binding-relevant event →
  collapse to concrete first** (fork the defining fields, re-enter
  the cycle — the cycle-loop re-entry is already idempotent by
  design, 3406-3417). For `mirror_blocked` only, a cheaper sound rule
  exists: tag-identical x and y ⇒ provably equal ⇒ not blocked;
  anything else ⇒ treat as blocked (blocking more only loses
  sharing, never soundness — same one-sidedness as today's rule).
- P1's first-naming prune (3486-3488) and `names_regs` (285-301)
  operate on candidate FIELD VALUES, not register contents —
  unaffected.

## 5. junk_walk and the emulator hot path

`step()` runs at ~13ns on concrete u32s (008 §3b post-mortem:
walk-step ~13ns vs item ~1.3µs; perf attribution: step 13.8% of
search CPU, journal "small hours, later"). A tag check inside every
register op would tax every one of the billions of concrete cycles
(L=3 0..0 ran 5.33B items) to serve the minority of tag-live ones.

Proposal:

- **Two-lane execution.** `step()`/`exec_op` stay byte-identical —
  the certified twin (docs/evaluator-spec.md, tests/narrow_diff.rs)
  is untouched. A new `step_sym(st, tags, cfg, gpio_in) ->
  SymOutcome` interprets the same semantics tag-aware and reports
  demands instead of stepping through them. The cycle loop dispatches
  on `it.sym == 0` (one predictable branch per cycle): all-concrete
  items — the overwhelming majority even under superposition, since
  tags only exist between a lazy write and its read/refutation —
  never leave today's path.
- **junk_walk** (2938-3097) must mirror tag transitions exactly as it
  already mirrors CntProv (3053-3075) — that mirroring discipline is
  the stage-4 precedent. A walk encountering a READ of a live tag
  bails as a fork edge (return false, caller forks normally — same
  contract as the fetch-demand bail at 2996). Writes of new tags
  during a walk are fine (they suppress nothing the walk needed).
  OPEN QUESTION: whether tag-live windows are common enough at delay
  post-forks to dent the stage-2 collapse rate (44% of fork mass is
  Delay; stage 2 must not regress).
- **Wall-clock gate (REQUIRED):** the emulator microbenchmark must
  show the concrete lane within noise of baseline BEFORE any search
  gate is consulted, and every stage below carries an idle-box
  wall-clock gate per the standing ops rule (STATUS "Ops rules";
  measurement lesson of 008 §3b). Note: **ticket 008 §3b's
  re-evaluation trigger fires on ANY evaluator change** — a
  superposition step that changes evaluator cost in either direction
  re-opens the 3b walk-economics question (user directive, ticket
  008 §3b: "break-even is steps/kill × ns/step vs items-saved ×
  µs/item"). Log the re-measurement, don't skip it.

## 6. Staged build ladder with gates

Every stage: one commit, 12 fast gates (censuses L1/L2 exact, memo
on/off, split-vs-sequential, instrumentation-inert) + the stage's
magnitude gates. Fallback for every stage = revert the commit; tags
are additive machinery, no stage rewrites a prior one's semantics.

**(a) Extend CntProv to OSR count.** Mirror of 008 stage 4:
`word_cnt_writes` twin for the OSR counter (OUT accumulates its
BitCount, PULL/MOV→OSR reset — mod.rs:527, 574-575, 642-643;
`word_state_reads` engine.rs:958 currently returns SC_OSR_CNT for
every OUT — make it autopull-conditional exactly as IN's was made
autopush-conditional, 942-957), mirrored in junk_walk. Gates:
like-for-like 150s 2..2 mine — expect memo-entry reduction of stage-4
shape (stage 4: 656K→484K, −26%) with wall neutral; bracket verdicts
byte-identical. Effect bound: osr_count-only near-misses (the
isr twin was 35% of state misses). Fallback: revert; zero coupling.

**(b) x/y value tags `Field(slot, mask)`, die-on-transform.** Tag
block in Item + two-lane step + SET x/y stops forking SetData at
fetch (`demand` engine.rs:439 loses the SetData/… arm for reg dsts,
like the JMP-target arm was removed in stage 1, 12ec1cb); MOV x←y
op-none copies tags; EVERYTHING else collapses (incl. JMP reads —
full-field deferred fork, no predicates yet). Memo: probe-side
skip-set rule + tagged-u64 projection (§3). Binding: collapse rule
(§4). Gates: emulator microbench (concrete lane ≤ noise); L1/L2
censuses UNCHANGED (they brute-force coverage both directions —
tests/narrow_engine.rs census_l1:117-152 directly validates widened
don't-cares); NEW widening census: a spec with a provably-dead
`set x, imm` must produce champions with the imm UNDECIDED (assert
via Champion.decided), plus `assert_champions_sound` on every
materialization corner (see §7); idle-box L=3 0..1/1..1 wall-clock +
item counts; 50-min gated 2..2 units-settled. Expected effect:
SetData eager-fork mass + the subset of MovSrc 15.2% that is
dead-copy; memo cond-miss composition shift (state_div class starts
hitting). Honest expectation: stage (b) alone captures the SMALL end
of the 35-45% — SetData is not in the top-3 fork kinds — its job is
to land the machinery under the cheapest semantics. Fallback: revert
(the demand() arm restore is mechanical).

**(c) — DELETED (re-cut 2026-07-14).** Was: ISR/OSR data tags + one
transform level (`Fn1`) aimed at BitCount 28.6%. REFUTED as bare
laziness by the dead-demand census (BitCount forks ~0% dead — the
wrap-loop shift chain re-reads the register within cycles), so
deferral alone collapses nothing there. The Fn1 machinery moves to
**ticket 012 stage 4**, paired with the OUT pin-visible predicate
classes that make the transform pay. See 012 §5 (composition) and the
Re-cut section above.

**(d) `Input(k)` symbols + multi-case specs** (unchanged by the
re-cut; follows AFTER ticket 012's ladder). TX-FIFO tag bitmap;
pull/autopull moves `Input(k)` into OSR as a tag copy; a demand on an
Input symbol forks the CASE axis (per-case children), which is
outcome-grouped forking over cases; spec gains a case battery
(shared cycles/expected per case — ticket 010's "sequential trace
concatenation + state reset" is the fallback encoding if per-case
forking interacts badly with KeyCore.cycle — OPEN QUESTION). A
champion that never demanded any Input symbol is input-invariant BY
CONSTRUCTION — the census's 99.6-99.7% input-survival measurement
(late-evening entry) becomes an engine-visible bit, and
phase-invariance for the RX flagship is the same bit over
phase-distinguishing stimulus. Gates: multi-case census vs N
independent single-case searches (verdict product must agree);
input-battery equivalence on the champ-mine specs. Fallback: revert;
stages a-c don't reference Input.

Stage order rationale: (a) is measurement-grade cheap and de-risks
the junk_walk mirroring; (b) lands representation+memo+binding rules
on the simplest tag; the effect-size stage is now 012 stage 4 (where
(c)'s Fn1 lives); (d) is the flagship-enabling stage and touches the
spec surface, so it goes last, after 012.

## 7. Risks

- **Expression growth.** Bounded by construction: one transform
  level, collapse on anything deeper. The failure mode is collapse-
  storms (tags created and immediately collapsed, paying tag overhead
  plus the fork anyway). Measure: collapse-site counters per stage
  (new Stats fields, instrumentation-inert).
- **Memo hit-rate regression.** Symbol-equality is coarser than
  value-equality in BOTH directions: concrete-vs-tag misses (§3) can
  REDUCE absolute hits even as records generalize; stage 4 showed the
  healthy version (−26% entries, +8% hits). Gate on hit-density AND
  wall-clock, not entries. The purge/benefit machinery (1444-1487)
  is agnostic and needs no change.
- **Champion materialization / don't-care widening soundness.**
  `Champion::words()` materializes don't-cares as 0 (113-115) — a
  widened champion claims MORE concrete programs. The claim is only
  sound if the register truly was never read on THIS champion's
  surviving path AND the trace check never consulted it — which is
  the engine's own invariant, but it must be gated: (i) census_l1 /
  L2 exact censuses already assert coverage ⇔ trace-match over the
  whole space — they catch over-widening outright; (ii) EXTEND
  `assert_champions_sound` (tests/narrow_engine.rs:45-61) to also
  materialize each champion at a NONZERO don't-care assignment (e.g.
  all-ones-under-mask, plus a seeded-random one) — today it checks
  only the canonical zero materialization, which would miss a
  widening bug on exactly the newly-widened bits; (iii) the
  solution-dense battery re-run: family count per behavior must be
  UNCHANGED while champion-record count drops (the multiplicity
  collapses into don't-cares — that is the point).
- **Split-driver interaction.** search_split's phase 1 expands to a
  128×-threads frontier (48f236e); superposition REDUCES fork width,
  so phase 1 must run deeper to hit the same frontier target —
  unit-count and balance may shift. Items stay Copy; the worker
  protocol is untouched. Gate: split-vs-sequential equivalence test
  (already in the 12) + split 0..1 wall-clock.
- **Determinism.** Fork order changes (fewer, later forks) ⇒
  different champion ENUMERATION ORDER and counts, same coverage.
  The locked determinism tests must keep passing per build; any
  verdict change on the proven brackets (0..0/0..1/1..1) is a bug,
  full stop.
- **Two-lane drift.** step_sym re-implements semantics; its
  concrete-projection must be differentially tested against step()
  on random tag-free states AND on collapse-then-step vs
  step-then-collapse commutation (new fast gate).

## 8. What it subsumes / retires — and what it does not

Subsumes:

- **Ticket 008's ORIGINAL design (outcome-grouped forking /
  value-set items, 008 "The idea")**: the write side is fully
  subsumed (don't fork what isn't read); the read side arrives as
  the predicate refinement of the demand rule (§2) — 008-B's
  "records conditioned on outcome classes" becomes "fork outcome
  classes at the read", one mechanism. Ticket 008 should be closed
  into this ticket once stage (b) lands.
- **The dead-effect champion-multiplicity class** (census class (a),
  evening entry): dead writes stop deciding fields, so the 250K:1
  collapses into don't-care widening at generation time instead of
  post-hoc census grouping.

Does NOT address:

- **Delay forks, 44% of fork mass** — time-plane, not data-plane;
  stage 2's junk-window collapse already owns the refutation side and
  its residue (SET-pair delay conds, cross-opcode families) stays.
- **The static/conditioned lemma path** — its measured ceiling on the
  refutation wall is 13.3%, and its real prize (pin-writer spellings,
  cross-opcode word classes, 99.68% cross-spelling pair mass) is
  champion-side / L=4 rediscovery. That program continues in
  parallel (009/pair census), unchanged by this ticket.
- **Latch-visible divergence** — anything that writes pins stays
  fully concrete forever; superposition buys nothing on pin-dense
  spaces where most instructions are latch writers.

## v1 scope decision (superseded by the re-cut — kept for provenance)

Build **stages (a) + (b) only**, as two commits: (a) OSR-count
CntProv (pure stage-4 mirror, bracket-neutral expected), then (b)
x/y `Field` tags with die-on-transform, skip-set memo rule,
collapse-at-binding. No Fn1, no predicates, no Input symbols, no
counter VALUE tags beyond the existing CntProv pattern. Rationale:
(b) is the smallest stage that forces every hard design decision
(Item layout, memo projection encoding, binding collapse, junk_walk
mirroring, census extensions) while its semantics stay simple enough
to audit against exec_op line-by-line; its expected effect size is
modest and that is acceptable — the gates it must pass are exactly
the gates 012's ladder needs, and 012 stage 4 is where the 28.6%
BitCount / 86% conflict-class prize lives.

The dead-demand census this section asked for WAS RUN (merged
ff6a4b3, journal 2026-07-13) and is what forced the re-cut: it
converted "fork attribution 35-45%" into "collapsible-by-laziness
mass ≈ 11.5% (MOV/SetData)", refuted (c) as bare laziness (BitCount
~0% dead), and routed the Fn1 machinery into 012 stage 4 where it
pairs with predicate classes. See the Re-cut section at the top and
012 §5.
