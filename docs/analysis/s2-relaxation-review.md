# S2 relaxation review — can binding-fork memo poisoning be lifted?

Review brief, 2026-07-14. Engine state reviewed: master `719d66c`
(S2/S3 fixes landed in `baef5fa`, merged as `44030df`). All line
numbers refer to `pio_superopt/src/narrow/engine.rs` at that commit.

**Question under review.** The S2 conservative fix — (a) binding-fork
frames are unrecordable, (b) unrecordability poisons every ancestor
frame — costs −41% memo hits on monster workloads (`baef5fa` cost
table). The implementer argued S2 is a flow gap but not exploitable
("identity-completeness", four blocked constructions) and poisoned
anyway as directed. Can records be re-enabled on the strength of that
argument? S3's fix (`Rec::bound`) is out of scope: it was demonstrated
red and must stay.

**Verdict up front.** The identity-completeness argument is *sound for
the binding-fork frame itself* (fix (a) can be lifted). For *ancestor
frames* the review found two qualifications. First, a full revert is
unsound: a concrete cross-sibling construction (§5.1) shows the
poison-propagation half of (b) must survive in narrowed form. Second,
the argument's wall 1 ("the subtree fully enumerates the frame key's
space") is **false for ancestor frames as a matter of evidence
discipline**: the P1 register-symmetry prune discards spellings whose
reads are never charged to the consulted set, so records generalize
over a state component (Y) that a pruned-but-covered spelling reads.
This was **demonstrated live on master** with a micro-spec + probe-log
instrumentation (§6.2-§6.3): records with no SC_Y in their pattern,
whose champion-free proof silently leaned on P1 coverage valid only at
y==0, pattern-matched and cond-scanned a *bound* register-mirror twin
carrying y==1. No champion was lost in the rig — but the analysis of
why (§6.3) shows the remaining protection is a coincidence of
heuristic constants (`min_benefit == 4` vs a 3-item frame, plus fake-
value degeneracy), not an invariant. The hole needs no binding fork
below the recording frame, so it is live today independent of S2; the
relaxation matters because relaxed ancestor records are `Rec::bound`
and refute exactly the bound-twin probers that exhibit it.
Recommendation (§7): fix the P1 accounting first (one-line, mirrors
the existing pin-prefilter pattern), then relax (a) and narrow (b) to
conflict-poison propagation only, keeping `Rec::bound` and the S6
seed validation.

---

## 1. The S2 mechanism, precisely

### 1.1 Background: what a record claims

The consulted-state memo is failure-only. When a frame's subtree
closes champion-free, `close_child` (line 1822) records at key

- **core** `key_core(cycle, next_input, st)` (line 1167): pc, delay,
  stall, pending_exec, clk_acc, pin latches. Scratch registers X/Y,
  ISR/OSR and their counters, IRQ flags, and FIFO contents are *not*
  in the core;
- **pattern** `state_reads` (SC_* bits, line 1141): the union of state
  components any run in the subtree read, with values projected from
  the frame's key state (`project_state`, line 1184);
- **conds**: consulted program fields ∩ fields decided at frame open
  (`f.consulted_mask[s] & f.decided[s]`, line 1851). Fields forked
  *below* the frame drop out — the subtree is presumed to have
  explored all their values.

The soundness argument for a probe hit: a matching prober's every
concrete completion, executed from the matching state, replays some
run of the recorded subtree read-for-read (core pins the always-read
components, pattern pins the read state, conds pin the read program
fields, and fields forked below were exhaustively enumerated), and
every such run was refuted.

### 1.2 The binding fork

P1 register symmetry (module doc, lines 32-48): while an item is
`unbound` and `named == false`, values that name only Y are pruned at
fork generation (line 4001); the item stands for both its words and
their register-mirror. X/Y renaming is not a free ISA symmetry — two
asymmetric channels exist (PULL-on-empty reads physical X,
`is_pull_empty_read` line 1048; a `pending_exec` word from data is not
renamed, `word_touches_regs` line 1031). When one is about to execute
on an unbound item, the engine forks (line 4262): the **identity**
child keeps the words and state S; the **twin** child becomes an
ordinary concrete item — decided searched slots mirrored via
`mirror_word` (line 899), x/y swapped, i.e. it runs from σ(S). Both
children are bound. Push order (lines 4285, 4299): identity first,
twin second — LIFO, so the twin's subtree closes first.

### 1.3 The flow gap (finding S2)

Pre-fix, the binding fork pushed a frame with `recordable: true`, and
`recordable` never propagated upward. Two consequences:

1. **The binding frame itself** united consulted (mask, value) pairs
   from runs rooted at *two different states with two word spellings*
   (identity at S, twin at σ(S)) under one record keyed at the
   identity core. Twin-side conds carry mirrored field values; twin-
   side reads are projected at the identity key state.
2. **Ancestor frames** received those mixed-spelling conds through
   `close_child`'s upward merge (line 1892) with no marker that the
   union was heterogeneous. A frame poisoned by the existing value-
   conflict check (`Frame::merge`, line 1735) still merged its final
   conds into its parent, and the parent could record.

That is finding S2: records mix conds from mirrored and unmirrored
subtrees. Whether any *wrong refutation* can be built from the mixture
is the question the identity-completeness argument answers.

## 2. What the conservative fix changed (`baef5fa`)

Three changes; (a)+(b) are the S2 poisoning under review, (c) is the
S3 fix and stays:

- **(a)** the binding-fork frame is created `recordable: false`
  (line 4320) and `bound_seen: true` (line 4322);
- **(b)** `close_child` propagates `parent.recordable &=
  f.recordable` (line 1891) — every ancestor of a binding fork (and of
  any conflict-poisoned frame) is unrecordable. Together (a)+(b) mean
  *one binding fork anywhere below kills all records on the path to
  the root* — on PULL-heavy monster specs this is the −41%;
- **(c)** per-frame `bound_seen` (opened on a bound item, is a binding
  fork, or one closed below; propagated at line 1890), stored as
  `Rec::bound` (line 1553); probe skips bound records for unbound
  probers (line 3782, mirrored in `find_single_conflict` line 2286);
  subsumption in `RecList::insert` respects the flag (line 1621).

Measured cost (commit message, 150s sequential `tx_a_l3_22_mine`):
(c) alone −3% hits; (a)+(b) −41% hits (1.95% → 1.16%).

## 3. The identity-completeness argument, spelled out

Source: `baef5fa` commit message (the four "walls" that blocked four
red-construction attempts); reconstructed here against the code with
the invariants each leg actually relies on.

**Claim.** A record created at a binding-fork frame (or an ancestor),
keyed at the identity core with the mixed consulted union, never
wrongly refutes a prober, *given* `Rec::bound` excludes unbound
probers.

**Wall 1 — identity enumeration is complete.** The identity child is
always pushed (line 4285; the twin is conditional on `twin_differs`,
line 4288). The identity subtree runs from exactly the frame's key
state S with the frame's decided fields, and enumerates every
completion of the undecided fields. Both children are bound, so the
P1 prune (which requires `unbound`) is off everywhere below; the
remaining prunes (P2 canonical nop, P4 vacuous JMP, 009 quotient,
pin-write pre-filter) are justified by *state-independent* behavioral
equivalence to same-fork siblings or by exact lookahead, and the
pre-filter charges the reads of filtered values to the consulted set
(line 4060). So the identity subtree alone proves "champion-free from
S under these decided fields" — the same claim an ordinary frame at
that key would prove.

**Wall 2 — merge order: twin closes first.** LIFO makes all twin-side
merges into the binding frame happen strictly before any identity-side
merge; `Frame::merge` overwrites overlapping bits (line 1744), so any
bit consulted by *both* sides ends holding the identity value.

**Wall 3 — decided-bit conflicts poison.** `Frame::merge` (line 1739)
compares each incoming (mask, value) against the already-merged union
on bits in `frame.decided`; a disagreement sets `recordable = false`.
Because a field decided at frame-open has one fixed value across all
identity descendants (decided bits are immutable once set — forks only
add), and one fixed *mirrored* value across all twin descendants, the
only possible disagreement source is identity-vs-twin on a register-
naming field — exactly the dangerous mixture. Either the two sides
agree on every co-consulted decided bit, or no record forms.

**Wall 4 — a cond-matching bound prober replays identity.** A bound
prober is one concrete spelling standing only for itself (S3 removed
the unbound residue). Its run from the matching state crosses the
asymmetric event concretely — i.e. it does what the *identity* child
did, never what the twin did. By walls 2+3, every identity-consulted
decided bit in the record holds the identity value, so a prober that
satisfies the union conds a fortiori satisfies the identity-only
conds, and its completions are covered by wall 1's enumeration.
Twin-only contributions — conds on bits identity never consulted,
pattern components identity never read — can only *shrink* the set of
matching probers (identity's verdict provably did not depend on them),
i.e. they over-restrict, never under-restrict.

**Corrections found by this review** (they do not break the argument,
but the load-bearing structure differs from the commit's phrasing):

- Wall 2 (LIFO order) is *not* load-bearing for the binding frame's
  own record: wall 3's conflict check is order-independent (any
  disagreement poisons, whichever side merged first). Where order
  *does* matter is the value that a *poisoned* frame propagates upward
  (§4, §5.1) — there identity-merges-last guarantees the parent
  inherits identity values on identity-consulted bits.
- Wall 1's "alone fully enumerates" is exactly true for the binding
  frame (bound below ⇒ P1 off) but is **not** true for ancestor
  frames, whose subtrees contain the unbound prefix region where P1
  prunes — see §5.2. This is the gap.

**The four blocked constructions** (mapped to the walls that block
them):

1. *Missing-run construction* — find a matching bound prober with a
   completion no recorded run covers. Blocked by wall 1: the identity
   subtree covers every completion from the key state.
2. *Wrong-value construction* — get a twin value stored on a bit an
   identity run's refutation depended on. Blocked by walls 2+3:
   co-consulted bits either agree or poison.
3. *Mixed-cond construction* — smuggle a self-inconsistent cond set
   (both spellings) into one record. Blocked by wall 3 plus the
   decided filter (bits not in `frame.decided` never become conds).
4. *Divergence construction* — a prober whose future forks away from
   every identity run at the asymmetric event. For bound probers there
   is no fork: the event executes concretely, identical to identity
   (wall 4). For unbound probers this is real — it is S3, blocked by
   `Rec::bound`, not by the argument.

## 4. What a relaxation would change, concretely

The candidate relaxation examined here (§7 refines it):

- **Lift (a):** line 4320 `recordable: false` → `true`. The binding
  frame records when champion-free and conflict-free; its record
  carries `Rec::bound = true` (line 4322 `bound_seen: true` is
  untouched), so it refutes bound probers only.
- **Lift (b) — but not fully:** line 1891 `parent.recordable &=
  f.recordable` currently propagates two distinct things: (i) the
  unconditional binding-fork poison from (a), and (ii) genuine value-
  conflict poison from `Frame::merge`. Lifting (a) makes (i)
  disappear on its own; **(ii) must keep propagating** — removing
  line 1891 entirely reverts to the pre-fix flow and is unsound
  (§5.1). So the relaxed semantics of `recordable` becomes exactly:
  "no unresolved mixed-spelling value conflict in this subtree."
- **Untouched:** `bound_seen` propagation (line 1890), `Rec::bound`
  probe/subsumption guards (S3), the `Frame::merge` conflict check,
  S6 whole-field seed validation (`validate_seed`, line 3546 comment)
  — wall 3 and the twin's mirror-under-unchanged-mask soundness
  depend on it — and the S5 seeded-slot canonicity disarms.

Expected recovery: most of the −41%, since actual identity/twin value
conflicts require both sides to consult the same register-naming
decided field and are far rarer than binding forks themselves. This
should be re-measured on `tx_a_l3_22_mine` with the same protocol as
`baef5fa`.

## 5. Attacks

Constructed against the argument as directed; strongest first. 5.1
shows full removal of the propagation line is unsound (bounding the
relaxation); 5.2 is the real finding.

### 5.1 Cross-sibling clobber at an ancestor (kills the *full* revert)

Take a parent frame P (key S_P, unbound) with two children whose
subtrees each contain a binding fork, and a register-naming field b
decided *above* P (so b survives every decided filter up to P's
parent):

- fork-1: identity and twin both consult b — identity value v_i, twin
  value v_t = mirror(v_i) ≠ v_i. Its frame conflicts (wall 3) and is
  poisoned; by LIFO its final propagated value on b is v_i.
- fork-2: *only its twin* consults b (mirror control flow diverges —
  twin state has x/y swapped, so e.g. a `jmp x--` goes the other way
  and reaches a slot identity never fetches). No conflict at fork-2's
  frame; it propagates (b, v_t).

P merges v_i then v_t → conflict → P is poisoned (correct). But with
line 1891 deleted, P's *grandparent* receives P's final clobbered
conds — (b, v_t) if fork-2's child closed last — with no poison flag,
and records. That record's cond (b, v_t) admits bound probers spelled
v_t on b, whose completions were only ever refuted through fork-1's
identity runs *at v_i*: an unexplored future, potentially a champion.
This is precisely the "recordable never propagated upward" hole the
review flagged; the argument's four walls do not cover cross-sibling
composition because wall 2's ordering guarantee holds within one
binding frame, not across sibling frames. **Consequence: the
relaxation must keep conflict-poison propagation** (§4). With it, P's
poison reaches every ancestor and this construction dies.

### 5.2 The P1-coverage hole (breaks wall 1 for ancestors; live today)

Wall 1 extends to an ancestor frame F only if F's whole subtree
enumerates F's key space. It does not: in the unbound prefix region,
P1 (line 4001) prunes every value that names only Y, with **no
accounting of the pruned spelling's reads** — compare the pin-write
pre-filter, which carefully `consume_reads`s the reads of values it
kills (lines 4057-4063). P1's coverage claim is *state-dependent*: an
unnamed unbound item has x == y (registers can only diverge through
naming fields, seeds, or exec'd data words, all of which set `named`
or bind), so at the *recorded* state the pruned Y-spelling is mirror-
equivalent to the explored X-spelling. But the record's pattern only
pins the components some *explored* run read. If no explored sibling
reads Y, the record generalizes over Y — while its champion-free claim
silently covers a Y-reading spelling that was only proven at y = 0.

A bound prober can arrive at the same core with y ≠ 0 (registers are
not in the core; SC_Y is absent from the pattern; `mirror_blocked` is
unbound-only, line 3759; `Rec::bound` is false — the recorder never
crossed a binding event). It matches and is refuted by a proof that
does not cover it; if the killed subtree contains a champion, the
champion is lost silently.

Three mechanisms make the hole narrow; the review measured all three
(§6.3), and none is a designed invariant:

- *Pattern closure via post-naming reads.* If any explored sibling or
  any post-naming descendant reads Y (`jmp x!=y` is its own mirror and
  always explored; `mov x,·` names X, after which its `src=y` sibling
  is explored and reads y), SC_Y = 0 enters that frame's pattern and
  y ≠ 0 probers state-miss. This closes every frame sitting *above* a
  register-naming field fork. It does **not** close frames below a
  non-naming dst (`mov pins,·`, `mov osr,·`, …), whose src fork
  P1-prunes `src=y` with siblings that never read Y — those frames
  record SC_Y-free (observed directly, §6.3).
- *The benefit gate.* The one SC_Y-free frame whose conds pin the
  champion-bearing branch (`mov pins,·`) has all its src children
  killed by the pin-write pre-filter — killed values are never pushed,
  never counted in `stats.items` — leaving a 3-item subtree against
  `min_benefit = 4` (line 3673). One notch of a hardcoded heuristic
  constant is the difference between green and a lost champion.
- *The prober must be bound with y ≠ x.* Unbound probers with x ≠ y
  are blocked by `mirror_blocked`; unbound probers with x == y are
  genuinely covered (symmetric state). The reachable victims are
  binding-fork twins — which is why this surfaced in the S2 review:
  **the S2 poisoning (a)+(b) currently suppresses every ancestor-of-a-
  binding-fork record, hiding part of this hole's surface; relaxing
  (b) re-exposes it.** But records from unbound subtrees *without* a
  binding fork below are `Rec::bound = false`, refute bound probers
  freely, and are not suppressed by the S2 fix — the wrong-transfer
  refutations happen on master now (§6.3).

This is a new finding (proposed: **S7**), filed against the memo/P1
interaction rather than against S2. Even without a demonstrated
champion loss it should be treated as a soundness bug: the memo's
correctness argument is an evidence-discipline argument ("the verdict
is a function of what was read, and everything read is in the
record"), and S7 is a demonstrated violation of that discipline whose
harmlessness rests on `min_benefit`'s current value and on MovOp
having exactly 3 values. It must be fixed before the ancestor half of
the relaxation ships, or the relaxation widens its surface.

### 5.3 Attacks that bounced (for completeness)

- *Nested binding forks* (twin conds mirrored twice): unreachable —
  binding forks fire only on unbound items and both children are
  bound, so ≤1 binding fork per root-to-leaf path.
- *Twin-only pattern components projected at the identity state*
  (SC_Y from a twin read, valued at identity's y): over-restrictive
  only; identity's verdict did not depend on the component.
- *Silent clobber without conflict*: impossible on decided bits — a
  disagreeing overlap always trips wall 3 before mattering; bits not
  in `frame.decided` are dropped at finalize.
- *Twin-side sub-frame records* (frames inside the twin subtree):
  ordinary bound frames keyed at real twin states with the twin's own
  spellings — sound independently, already recorded today.
- *Partial-field mirror scrambling*: blocked at entry by S6
  `validate_seed`; engine forks decide whole fields by construction.
- *Subsumption laundering a bound record into an unbound one*:
  `RecList::insert` only lets A absorb B when `!A.bound || B.bound`.
- *`twin_differs == false` fork* (x == y and register-free decided
  words): single identity child; the frame is a plain identity record.

## 6. Red-green gates

### 6.1 S2-relaxation gate (expected GREEN if walls 1-4 hold)

No S2 red exists (consistent with the implementer's four failed
attempts and this review's §5.3). The gate for shipping the relaxation
is therefore: (i) the S3 rig (`narrow_soundness.rs::
s3_bound_record_must_not_refute_unbound_prober`) stays green — it
exercises bound-record-vs-unbound-prober directly and would go red if
the relaxation ever recorded without `bound_seen`; (ii) a new rig in
the same style targeted at the *relaxed* surface: seeded scaffold
forcing a binding fork below a small recording frame, twin control
flow diverging so the record carries twin-only conds (§5.1 fork-2
shape), then a bound prober matching the identity conds — assert
memo-on == memo-off champion lists. If the identity-completeness
argument is wrong anywhere reachable, list divergence surfaces here.
(iii) the §5.1 construction as its own micro-spec once the relaxation
lands, pinning the conflict-propagation semantics of line 1891.

### 6.2 S7 rig (built and run during this review)

`p1_pruned_reads_must_be_consulted` (appendix A). Space: slot 0 seeded
`mov osr,~null` (registerless — root stays unnamed, P1 armed), slots
1-3 free, 1-bit mandatory side-set on pin 2 with alternating expected
values (pins every slot's side bit and kills every delay>0 spelling,
keeping the space enumerable), 4 cycles, empty TX, pins idle high.

- **Recorder** `[_ / set pins,3 / pull noblock / <fork>]`: unnamed,
  unbound (x==y=0 at the pull ⇒ no binding demand; the empty-TX pull
  still does osr ← physical X = 0, normalizing OSR), arrives at core
  K = (cycle 3, pc 3) with x=y=0, osr=0, cnt=0. Its slot-3 MOV src
  fork P1-prunes `mov pins,y`; the explored siblings (pins, x, null,
  status, isr, osr) never read Y and all fail the cycle-3 capture
  (expected pins01 = 01; they drive 00, 11, or nothing against
  idle-high 11). Intended record: conds pinning slot 3 to [MOV,
  dst=pins, op=none], pattern SC_X = 0, SC_OSR = 0, … and **no
  SC_Y**; `Rec::bound = false`. (§6.3: this exact record is
  benefit-gated out; its dst=osr/isr/pc/exec siblings do form.)
- **Prober** `[_ / out x,1 / pull / …]`: `out x,1` sets x=1 (the
  `out y,1` spelling is P1-pruned — it exists only as the twin), so
  the slot-2 pull demands a binding fork. The twin
  `[_ / out y,1 / pull]` arrives at K **bound** with x=0, y=1,
  osr ← physical X = 0, cnt=0: every recorded component matches, both
  probe guards pass (bound prober ⇒ no `mirror_blocked`; unbound
  record ⇒ no `Rec::bound` skip). Its [MOV, pins, op=none] item
  would be refuted before the src fork, losing `mov pins,y` — which
  reads y=1, drives pins 01, and is the expected trace's only
  producer.
- Assert memo-off finds the twin champion and memo-on == memo-off.

### 6.3 Observed result: GREEN, and exactly why

Run on master `719d66c` (2026-07-14, review worktree):

```
expected: [00070003, 00070007, 00070003, 00070005]
p1 rig: off champs=440839 items=214106319 |
        on  champs=440839 items=29981610 hits=2652823 entries=171972
test p1_pruned_reads_must_be_consulted ... ok
```

Champion lists identical — no red. A second, memo-on-only run under
`PIO_NARROW_PROBE_LOG` + `PIO_NARROW_DUMP` (28.8 GB probe stream,
grepped for the twin lineage `slot1 = ffff/7041`) shows precisely
where the construction gets pinched:

1. **The SC_Y-free P1-hole records exist and pattern-match the y=1
   twin.** The twin's `[slot3 = MOV,pins,op-none]` item (prober
   `["ffff/a0eb","ffff/7041","ffe0/8080","f0f8/b000"]`, state x=0,
   y=1, osr=0) probed the recorder's key and reached **cond-scan** in
   the `mask=13` (SC_X|SC_ISR|SC_OSR — *no SC_Y*) and `mask=15` sets:
   `{"mask":13,"recs":6,"fails":[{"slot":3,"m":61664,"want":45280,…}`
   — six records formed on the unnamed recorder line, under MOV-dst
   forks whose `src=y` was P1-pruned, whose patterns do not mention Y,
   matched at a state whose y differs from the recorder's. The
   evidence-discipline violation is real and observed; those records
   refute the twin's `[MOV, dst=osr/isr/pc/exec]` items (kills whose
   lost `mov <dst>,y` completions happen to be non-driving and
   genuinely doomed — wrong mechanism, harmless content).
2. **The champion-bearing record is missing by one item.** The only
   frame that would pin `dst=pins` without SC_Y is the MovOp fork
   frame under `[MOV, dst=pins]`: its src children are all killed by
   the pin-write pre-filter (never pushed, never counted), so its
   subtree is 3 popped items — below `min_benefit = 4`. Every frame
   above it (`F_dst`, `F_op`) inherits SC_Y = 0 from the `dst=x →
   src=y` post-naming exploration and from `jmp x!=y`, putting those
   records in value-keys the y=1 twin never matches (confirmed: no
   `[3,0xF000,0xB000]`- or `[3,0xF0E0,0xB000]`-cond record exists in
   any set the twin can see).
3. **Value degeneracy blocks the workarounds.** Widening the MovOp
   frame to 4 items requires a `mov pins,src` that survives cycle 3
   on the recorder line, i.e. a recorder-state source driving the
   champion's pin value — and the reachable twin y' values (all-ones
   shifts: 1, 3, …) collide with `~x`/`pins`-idle fakes everywhere
   except y'=1, where nothing on the all-zero recorder state can
   drive 01. The pinch is exact, and it is exactly as strong as
   `min_benefit == 4` and `|MovOp| == 3`.

So: **no champion-divergence red on master today**; the rig is kept
as a canary (it flips red if the benefit gate, the MovOp/field
arities, or the pre-filter counting ever shift), and the probe-log
methodology above is the reproduction recipe for the wrong-transfer
kills.

### 6.4 The S7 fix (out of scope here, but one line by design)

Treat a P1 prune like a pre-filter kill: charge the pruned value's
reads before `continue` at line 4001 —
`consume_reads(word_state_reads(pruned_word, &cfg), …)` — routing
SC_Y through the normal provenance path exactly as lines 4060-4063 do
for pin-prefiltered values. Records above unnamed naming forks then
pin y at the recorded state (where x == y made the coverage valid),
and the §6.2 twin state-misses every set. Cost: a slightly wider
pattern on frames above unnamed naming forks; no new flags, no
probe-side change. (Alternatives considered and rejected: skip P1
when `memo_on` — loses real pruning; a `p1_seen` frame poison — far
blunter than needed.)

Verification that the fix bites (since the rig is green either way):
rerun the §6.3 probe-log recipe and confirm the twin's
`[MOV,pins,op-none]` probe **state-misses** the former `mask=13`/
`mask=15` sets (their masks now include SC_Y, bit 1), instead of
reaching cond-scan.

## 7. Recommendation

1. **Fix S7 first** (§6.4), verified by the §6.3 probe-log check,
   with the §6.2 rig kept as a permanent canary, plus the standard
   determinism/locking tests.
2. **Then relax S2 in the bounded form** (§4): lift (a) at line 4320;
   keep line 1891 (its propagated value is then conflict-poison only);
   keep `bound_seen`/`Rec::bound`/S6/S5 untouched. Do not delete the
   propagation line — §5.1 is a concrete unsoundness against the full
   revert.
3. **Ship with gates**: S3 rig green, §6.2 rig green, a §6.1(ii)
   twin-divergence rig green, memo-on == memo-off champion-list
   equality on the existing micro-spec battery, and the L=3 ladder
   re-certification (the `4794ee8` protocol) before any monster
   verdict is trusted.
4. **Re-measure** the hit-rate on `tx_a_l3_22_mine` with `baef5fa`'s
   like-for-like protocol; expected: most of the −41% returns, since
   conflict poison requires identity and twin to consult the same
   register-naming decided field.

The identity-completeness argument, so bounded, holds in my judgment:
wall 1 is airtight for the binding frame (bound ⇒ P1 off below), and
the ancestor extension is exactly as strong as the enumeration-
completeness of the unbound region — which is S7's, not S2's, to
guarantee.

---

## Appendix A — the S7 rig source

Built against `narrow_soundness.rs`'s helpers (`cfg_for`, `covered`,
`mirror_program` unused, constants `OUT_Y_1`, `PULL_NOBLOCK`,
`NOP_WORD`). Kept out of the committed test battery on purpose (this
review is document-only; it runs ~90 s and is green — a canary, not a
gate); paste it back verbatim to reproduce §6.3.

```rust
const S1: u16 = 0x1000; // side-set value 1 (side_count=1, en=false)
const MOV_OSR_INV_NULL: u16 = 0xA0EB; // mov osr, ~null (osr=FFFF_FFFF, cnt=0)
const MOV_PINS_Y: u16 = 0xA002; // mov pins, y

fn p1_rig() -> (EngineSpec, [u16; 32]) {
    let config = Config {
        side: pio_superopt::ir::SideCfg { count: 1, en: false },
        pins: PinMap {
            out_base: 0,
            out_count: 2,
            set_base: 0,
            set_count: 2,
            sideset_base: 2,
            ..PinMap::default()
        },
        ..Config::default()
    };
    let mut spec = EngineSpec {
        cfg: cfg_for(config, 0, 5),
        slots: 4,
        cycles: 4,
        inputs: vec![],
        output_pins: vec![0, 1, 2],
        capture_pins: vec![0, 1, 2],
        stim: Stim::default(),
        irq_sets: vec![],
        expected: vec![],
        seed: vec![(0, 0xFFFF, MOV_OSR_INV_NULL)], // registerless: root stays unnamed
        memo_cap: 0,
    };
    let mut w_target = [NOP_WORD; 32];
    w_target[0] = MOV_OSR_INV_NULL; // side 0
    w_target[1] = OUT_Y_1 | S1; // twin spelling; identity spells out x,1
    w_target[2] = PULL_NOBLOCK; // side 0; empty TX -> osr <- physical X
    w_target[3] = MOV_PINS_Y | S1; // y=1 -> pins 01, the differentiator
    spec.expected = run_spec(&spec, w_target);
    (spec, w_target)
}

#[test]
fn p1_pruned_reads_must_be_consulted() {
    let (mut spec, w_target) = p1_rig();
    eprintln!("expected: {:08x?}", &spec.expected);
    assert_eq!(spec.expected[3] & 0x7, 0x5, "target trace must end side=1 pins=01");
    assert_eq!(spec.expected[2] & 0x7, 0x3, "cycle-2: pins idle high, side 0");

    let off = search(&spec, 1_000_000);
    assert!(!off.champion_cap_hit);
    assert!(
        covered(&off.champions, &w_target),
        "memo-off search failed to find the twin champion at all — rig broken"
    );

    spec.memo_cap = 1 << 20;
    let on = search(&spec, 1_000_000);
    assert!(!on.champion_cap_hit);
    eprintln!(
        "p1 rig: off champs={} items={} | on champs={} items={} hits={} entries={}",
        off.champions.len(),
        off.stats.items,
        on.champions.len(),
        on.stats.items,
        on.stats.memo_hits,
        on.stats.memo_entries
    );
    for ch in &off.champions {
        if !on.champions.contains(ch) {
            eprintln!("  lost champion: v={:04x?} bf={}", &ch.value[..4], ch.binding_free);
        }
    }
    assert!(
        covered(&on.champions, &w_target),
        "P1 RED: memo record with unaccounted P1-pruned reads killed a bound twin prober"
    );
    assert_eq!(
        off.champions, on.champions,
        "P1 RED: champion lists diverge between memo off/on"
    );
}

/// Diagnostic: memo-on run of the P1 rig for env-driven dumps.
#[test]
#[ignore]
fn p1_rig_dump() {
    let (mut spec, w_target) = p1_rig();
    spec.memo_cap = 1 << 20;
    let on = search(&spec, 1_000_000);
    eprintln!(
        "diag: champs={} hits={} target_covered={}",
        on.champions.len(),
        on.stats.memo_hits,
        covered(&on.champions, &w_target)
    );
}
```
