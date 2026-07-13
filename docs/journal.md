# Journal — session history

> Append-only, newest entry at top. Moved verbatim from the old `SCRATCH.md`
> on 2026-07-04. Not required reading — search it for provenance when needed.
> Current state lives in `STATUS.md`; durable design in `docs/architecture.md`.

## 2026-07-13 (evening) — champion-family census: 250,000:1 behavioral redundancy

First step of the static canonicalization program (test
champion_family_mine, 0f58474; dump champ_mine.jsonl 265MB with full
extended traces). Solution-dense battery, full champion enumeration,
extended-horizon (2x) fingerprint grouping:
sq2_l2 217→1 family; sq4_l2 224→1; pulse14_l2 217→1;
**sq4_l3 223,223 champions → 37 families, top = 223,128 (99.96%)**;
**pulse14_l3 248,978 → 1 family**. sq2_l3 = 0 champions (correct
parity impossibility: a wrap-0..2 loop cannot toggle at period 2 —
proven in 0.3s). 36 sq4_l3 tail families (95 champions) match the
spec trace but DIVERGE on the extended horizon — spec-coincident
impostors; the extended fingerprint is mandatory for theorem mining.

Mega-family structure (analyze_champ_fams.py): multiplicity is NOT
delay spelling (sq4_l3: 223,128 distinct even modulo delay fields;
all 8 opcodes appear in every slot; 88 opcode shapes). It decomposes
as (a) DEAD-EFFECT instructions — OUT/IN/MOV/SET/IRQ/passing-WAIT
whose written state is never observed downstream — each ≡ nop with
the same latency; (b) equivalent pin-writers (set pins,1 ≡ mov
pins,!null ≡ mov pins,!x @x=0 ≡ mov pins,!osr @osr=0); (c) delay
placement across dead runs (P3 generalization / static time-shift).
KEY IMPLICATION: the engine's demand mechanism forks decode fields at
EXECUTION even when the write target is dead — "consulted ≠
mattered" on the champion side, the same gap as the memo cond wall.
Class (a) dominates and is CONTEXT-dependent (deadness), which static
pair lemmas cannot fold — it is exactly ticket 008's ORIGINAL design
(outcome-grouped forking / value-set items), now with a measured
250,000:1 prize on solution-dense specs. Static lemmas still own the
state-independent slice (b): why does 009's word quotient NOT fold
set pins,1 with mov pins,!null? — cheap gap-check queued first.

## 2026-07-13 (afternoon) — family-record lever tried & reverted; walk chapter CLOSED

User approved the once-per-family walk-certified record generalization
(built f5a7c2b on sizing that looked decisive: ~2K families, 65% of
miss-mass killable, ~0.2s total walk cost). Three firing policies
measured on the way (first-miss-per-entry: 7.5% kills + per-unit
re-arming cost +50% wall in split; 256-miss threshold: 30% kills at
~zero cost) and one real bug fixed (kills must insert the record at
the ITEM's core — the frame system never keys one there). **Final
verdict: zero conversion** (15-min sequential 2..2: 64 kills/190
arms, items and hit-rate byte-flat) — REVERTED. Root cause is
structural: census families group by (core, conflict slot+mask), but
members differ freely on OTHER decided bits, and the walk's record
must condition on everything its family-wide tree consulted — it
covers only the near-empty sub-family agreeing with the walked item
outside the conflict. **The 84.5% transferability is inherently
per-member, and per-member re-proof is 3b — already uneconomical.
The walk/record-generalization chapter is closed** (probes kept:
PairRace 96f0372/8e69ac8, FamilyProbe 456f829). Remaining record-side
idea = outcome-class conds (008 design B, big surgery) — parked.

**Direction chosen (user, discussion): static canonicalization
program** — (1) champion-rich targets at reachable lengths (censuses
already give exact champion sets; solution-dense L=2-3 specs; group
champions by extended-stimulus fingerprints, not just the spec
trace); (2) pair canonicalization = 009 at arity 2 (within-L sibling
dedup rules first; length-reducing pair→single+delay = ladder
subsumption, needs a design doc for wrap/target shifts); (3) mine
theorem candidates from output-grouped short-search dumps + pair
enumeration fingerprint census, prove with the existing z3 mirror
(bounded equivalence queries, UNSAT = theorem — no new prover).
Static rules can't see the doomed-window (state-dependent)
equivalences that dominate the wall, but they shrink the space
multiplicatively everywhere at zero runtime cost and compound up the
ladder toward L=4 rediscovery.

## 2026-07-13 (midday) — stage 3b REVERTED: item halving hid a 2.8x wall-clock loss

The entry below celebrated 3b off a CONTENDED baseline (127s for
0..1). Idle-box re-measurement on the pre-3b commit: 0..1 = 784.0M
items in **27s**, 1..1 = 716.3M in **24s** — so 3b's 388.1M/76s and
353.1M/68s are a **2.8x wall-clock regression**, confirmed at scale
by 2..2 (10.3% vs 27% settled at the 50-min gate). Reverted
(a1477f3's revert); probes kept; byte-identical baseline reproduction
verified post-revert. Mechanism (understood, unlike hoisting): the
walk re-proves subtrees with NO memo hits, no quotient sibling
dedup, no canon prunes — ~563 steps/kill where ~100 breaks even
(walk step ~13ns vs item ~1.3µs) — and dense sibling cond-miss pops
re-walk overlapping futures, the exact redundancy the memo removes.
The 84.5% transferability stands; exploit it record-side, not by
re-proof. New ops rule: magnitude gates pair item counts with
idle-box wall-clock. Ticket 008 §3b has the full post-mortem.
Next: stage 4 (ISR_CNT provenance), then the Codex review.

## 2026-07-13 (day) — 008 stage 3b: generalized subtree walk, 2x on stage 2 [SUPERSEDED — see above]

**Morning: race-sized the cross-opcode class honestly** (the delay
chapter's measurement lesson applied). PairRace probe = DelayPair
generalized to ANY single decided-cond conflict, verdicts per kind
(delay/arg/opcode), commit 96f0372. Flat 20-min deep mine on 2..2:
opcode conflicts are 99.5% of raceable cond-misses, but 92% end
INCONCLUSIVE at a demand edge — with states still EQUAL in all but
24/709K. So the probe learned joint forks (8e69ac8): at an
equal-state demand edge both machines fork the SAME value (sound:
conflict bits all decided) and leaf verdicts aggregate (BAD<INC<CO).
Deep-region result: **share_co 84.5% of 770K races, 100.0%
latch-quiet, true divergence 37, budget-bind 29/770K at 16K steps,
steps_avg 391 (~200 single-machine)**.

**Key insight:** the race observes the PROBER refute at every leaf —
the record machine is redundant. Lever = single-machine bounded
concrete DFS ("generalized junk walk"): enumerate every fork edge
over the full value set (superset of engine children: P1/P2/P3/P4,
side pre-filter, 009 dedup all subset values_into) and kill the item
when every branch refutes. No theorem — direct observation. User
approved; also queued stage 3c: adaptive walk budgets (runtime
kill-rate×cost heuristic + occasional deep probe walks).

**Firing policy took three measured iterations** (a1477f3):
1. Ungated at fetch-demand + target fork sites: 18% kills, 416 avg
   steps, swcyc>main-loop cyc; split 0..1 catastrophically regressed
   (652M items live at 11min vs 784M total baseline). Killed.
2. Remaining-trace≤128 gate: NEVER fires — the search refutes early,
   it.cycle never approaches the horizon. Dead code.
3. **Cond-miss pop firing + per-branch depth cap 128** (failure cost
   = proving a branch survives; bound it at ~2x the mined 63cy leaf
   depth): 89% kill rate, search becomes walk-dominated (main loop
   cyc 33M vs walk 10.7B in the smoke; junk_walk nearly extinct).

**Magnitude gate: L=3 0..1 784.0M→388.1M items in 76s; 1..1
716.3M→353.1M in 68s; champions=0 both; all 12 fast gates pass**
(both exact censuses, memo on/off, split-vs-sequential). 2..2 gated
attempt launched on the 3b engine (txa_l3_swalk_gate2.log).
Commits: 96f0372, 8e69ac8, a1477f3.

Ops note: `systemctl --user is-active` is unreliable from background
monitor shells (false inactive) — watch the run log's mtime instead.

## 2026-07-13 (small hours, later) — hoisting tried & reverted; wall re-censused

Post-collapse mining (fresh 15-min instrumented 2..2 run): the old
44% JMP-delay conflict class is GONE (47M→6,954 sampled); residual
wall = SET-pair delay conds 50% (delay families whose walk hits a
fork edge — 93.6% of residual walks are demand_edge) + cross-opcode
41%. Collapse does 50% of all refutations (look=251.8M of 504.9M).

**Demand hoisting FAILED the magnitude gate and was reverted**: fork
the walk-edge's future field at the parked point, delay pending —
L=3 0..1 785M→982M (+25%); the side-filter-bypass hypothesis
(exclude side edges) made it worse (1.09B), so the mechanism is NOT
understood. Reverted byte-identically (785,290,370 items reproduce).
Negative result + lesson in ticket 008: don't reorder fork decisions
past the cycle-local prunes; next lever starts from cost attribution
(perf: search_impl 73.8% flat, step 13.8% — no machinery tax; the
cost is genuine frontier churn). Gated 50-min 2..2 attempt launched
on the stage-2 engine as benchmark/coup-de-grace (txa_l3_22try.log).

## 2026-07-13 (small hours) — 008 stage 2: junk-window collapse, 4.4x at L=3

**Mining loop under the new 50-min policy** (three iterations of
instrumented 10-15min 2..2 runs, delay-pair lock-step races): of
28,672 sampled delay-conflict probe pairs, 95.8% CO-REFUTE (identical
captures, same refutation cycle, avg 60.5cy), 100% of those with
latch-quiet windows, 99.95% external-free, ZERO diverged. First
classifier iteration was an artifact (states start equal — absorption
must require the delta to be EXPRESSED first); second starved on
unbound probers (~99.5% of delay-conflict probers are unbound — noted).

**Theorem operationalized** (89f97c9): in a window with no pin-latch
value changes, no external-schedule reads (WAIT, JMP PIN, IN/MOV
PINS, IRQ), and no fork edges, every spelling of every undecided
delay executes the same instruction sequence time-shifted, holds the
same static capture, and hits the same expected-trace mismatch cycle.
`junk_walk` at the delay post-fork refutes the whole 8^k family in
one representative walk; records carry no undecided-delay conds.
OOB fetches are execution-position-based, NOT shift-invariant near
the horizon — bounded per shift point; the L=1 exact census caught
that hole on the first build (word 0xa680), proving the census gates'
worth again. **L=3 0..1: 3.43B→785M items (4.4x); 1..1: 3.14B→717M;
L=2 1..1: 104.6M→40.2M (2.6x).** Verdicts unchanged, 12/12 gates.

Policy note: a bracket-measurement launch rolled into 2..2 un-gated
and blew the 50-min cap (user caught it) — every launch, including
"quick measurements", goes through the gated-unit incantation now.

## 2026-07-12 (late night) — OOB refutation, 008 stage 1, OOM incident, gated relaunch

**OOB refutation landed** (6b08a0b, user ruling): executing any slot
at/past the searched length is UB on hardware (the rest of the ROM
belongs to other programs) — the space is now "programs whose
execution stays within their L words for the whole horizon". Refuted
at fetch time (`fetch_pc >= spec.slots`); catches fall-through and
computed MOV/OUT PC; JMP targets were already generation-restricted.
Censuses validate the redefinition exactly; `run_spec_oob` flags OOB
for census callers. Prior refutations stand a fortiori.

**Split driver deepened** (48f236e): phase-1 frontier target 16x→128x
threads (shallow cycle-1 frontiers left near-unconstrained hours-deep
units; 0..1 fell 499s→68s with 43% fewer items), live progress
counter (AtomicU64, 2^20 chunks), per-worker quotient reuse.

**L=3 brackets 0..1 and 1..1 REFUTED**: 3.47B items/68s and 3.19B/61s
(28 threads). 1..2 revealed as a second monster (2-slot wrap loop):
52.3B live items at only 6% units settled before the run died.

**008 stage 1 — lazy JMP target demand** (12ec1cb): target demanded at
consult time (taken execution), like delay; fetch_footprint for JMP
shrinks to cond bits; P4 relocated into the deferred fork. Measured:
L=2 1..1 +2% items (vacuous jmp-to-ft now walks to first-taken
instead of dying at fetch), L=3 0..1 −1.4% (3.427B, verdict + item
count byte-identical across independent runs). Kept per the
build-every-lever ruling: the untaken-fan saving scales with L, the
walk overhead is bounded. Probe-log measurement that reshaped 008:
target-only conflicts are 0.33% of the wall — **the wall is JMP
delay-bit conflicts (41.5M/94.9M sampled = 44%, same word different
delay spelling) and cross-opcode pairs (41%)**; side-set conflicts
~0 (the fork-time side pre-filter already kills them). Stage 2/3
design must center timing/outcome sharing, not cond grouping (2.5%).

**OOM incident**: running the rest brackets + a 28-worker stage-1
measurement + the old census run concurrently OOMed the box and
killed everything including the Claude session (per-worker memos
multiply by threads × runs). Lessons operationalized: big searches
run SERIALIZED, and detached under
`systemd-run --user -p MemoryMax=48G -p MemorySwapMax=0` so a runaway
kills its own cgroup, not the system, and survives session death.
Rest-run relaunched as unit `pio-l3rest` (txa_l3_rest2.log), bracket
order now banks 2..2 before the 1..2 monster (7169b6c). The old
census run died at 6.65B items without its end-of-run census —
accepted (old semantics, verdict tainted anyway).

## 2026-07-12 (night) — THE L=3 WALL FELL + census analysis picks the next lever

**L=3 wrap 0..0 REFUTED**: 5,332,269,770 items exhausted in 1567s at
28 threads (search_split + 009 quotient, corrected semantics) — the
bracket that killed v1 at 5.67B (overnight), v4 at 1.42B, v5 at 230M
is a proven impossibility. Remaining five L=3 brackets launched
(txa_l3_rest.log). Straggler anatomy from the two attempts: 2044/2052
units settle in ~10s, the tail is a handful of deep units (playground
lesson holds); recursive unit splitting is the next driver lever.

**009 landed mid-wait** (a687b45): lemma word quotient (word_canon +
exact per-mask partial tables + fork sibling dedup) — tx_a folds
23,584/65,536 words, frontier 2052→1520 units, L=2 brackets ~1.9x
(195M→102.6M items, 20s→11.3s). Battery gate + censuses green.

**Census/snapshot analysis** (8GB probe samples, 8.4M-record purge
snapshot from the old-semantics instrumented run) — three verdicts:
(1) cond misses are 90.2% of core-matched probes; 97% of sampled
fails are value conflicts on DECIDED bits of slots 0/1 — genuinely
different programs. Quotient headroom measured at 0.97% ⇒ **memo cond
canonicalization is dead; ticket 008 (outcome-level records) owns the
deep-memo wall**. (2) 88.2% of all cond storage was always-match
filler-walk conds (mean 21.8/24.7 conds per record; deep subtrees
walk 29 nop slots after falling off the program) ⇒ stripped
spec-constant-slot conds (sound: filler is never forked NOR
twin-mirrored — the binding fork mirrors searched slots only).
(3) state misses are 94% single-component: RX 44% (contents are
ISA-unreadable ⇒ patterns now project level only), ISR_CNT 35%
(real reads; provenance chain queued into 008). Both levers landed
as 05c3303; L=1/L=2 verdicts + item counts byte-identical, wins are
memory (~8x smaller records) + scan cost + deep-region sharing.

## 2026-07-12 (eve) — EMULATOR FIDELITY: shift-counter semantics wrong in all 3 layers; fixed; verdicts re-established

Ticket 009's first lemma ("which words are no-ops") forced a datasheet
check that found TWO hardware divergences shared consistently by the
vendored emulator, the narrow twin, and the z3 mirror (so every
differential gate passed while all three were wrong vs silicon):

1. **MOV→ISR/OSR resets the shift counter** (empty/full) and **OUT→ISR
   sets isr_count to the bit count** (RP2350 ch.11 destination
   annotations). All layers left the counters untouched. Fixed in
   e4a4860. Fallout: `mov osr,osr` is NOT a hardware no-op (re-arms
   autopull, flips !OSRE) — NOP_CANON moved 0xA0E7 → **0xA021
   (`mov x,x`)**; P2 prunes only the y,y twin now; ISR/OSR self-moves
   enumerate as real ops. Cost: forking the nop sets `named`, so P1
   mirror-folding disarms in nop-carrying unbound subtrees (no
   register-free true nop exists on silicon; 008's outcome classes
   could recover this).
2. **PUSH IFFULL / PULL IFEMPTY are shift-count GUARDS** ("do nothing
   unless the count reached its threshold"), evaluated before any FIFO
   access. All layers consulted the bit only on a full/empty FIFO —
   e.g. `pull ifempty` with queued data below threshold popped the
   FIFO; on silicon it is a no-op. Fixed in a810ec5. Engine: PULL/PUSH
   read-masks gained SC_OSR_CNT/SC_ISR_CNT for the if-variants (memo
   soundness); `is_pull_empty_read` models the guard (binding fork).
   Four legacy vendored tests had PINNED the wrong behavior (citing
   the buggy lines); rewritten to the datasheet.

Blast radius: production dme_pio.rs unaffected (its `mov isr,osr` uses
are `mov; push` with no IN between — counters dead; NOT the deaf-RX
culprit). Certified DME encoder unaffected (movs to Y/Pins only; reads
don't reset). HIL-validated TX transform unaffected (empirical). The
OLD enumerate.rs track excludes ISR/OSR self-moves as "identity nops"
— **the ≤4-word impossibility proof now carries that caveat** (those
are real ops in the corrected semantics; re-proving needs the narrow
ladder anyway).

Verdicts re-established under corrected semantics (4b3bd26), all
refutations HOLD: L=1 0..0 18,357 items (was 14,141 — space grew);
L=2 0..0 (split gate), 0..1 195.6M items/20.0s, 1..1 195.4M/19.8s at
28 threads. L=3 0..0 relaunched with `search_split(28)`
(txa_l3_split_run.log, 2052 frontier units); the old-semantics
instrumented sequential run left running for its census/purge data
(verdict tainted, throughput benchmark still valid — it blew past
v4's 1.42B kill point at ~1.5M items/s sustained).

Answered: no config makes the ISR/OSR self-moves no-ops again — the
counter observers (!OSRE, ifempty/iffull, autopull/autopush) are word
encodings present in every config's space. The consulted-state memo
already recovers the cost dynamically wherever a subtree never reads
the counters; a spec-relative quotient (space excludes all observers)
is a legitimate 009 extension.

Full suites green everywhere: picoem 327, superopt default 95+ across
targets, smt feature 82 (differential cross-checks re-validate the
mirror against the corrected twin). Test authorship of the mechanical
layer fixes was delegated to a subagent (both fidelity commits) and
verified here.

## 2026-07-12 (midday) — mining flags + perf-guided memo rework: 6.7x

**Instrumentation flags (95710db).** Two env channels, both provably
search-behavior-free (`instrumentation_flags_do_not_change_search`:
champions + stats bit-identical flags-on/off): `PIO_NARROW_PROBE_LOG`
(per-cycle probe-outcome census nocore/state_miss/cond_miss/hit +
byte-budgeted sampled miss diagnostics — nearest-record component-diff
bitmask on a state miss, first failing cond on a cond miss; stride
doubles each time half the REMAINING budget spends, so deep regions
thin rather than truncate) and `PIO_NARROW_SNAPSHOT` (full memo-table
JSONL before each purge + at end; frames stack in the meta line). New
Stats: memo_state_misses/memo_cond_misses. Strict JSONL (the old
PIO_NARROW_DUMP hex literals aren't). Big dumps →
/data/pio_optimization/runs/ (2nd SSD), noted in architecture.md.

**Perf profile (100s L=3 0..0 slices, F=999).** 87% of CPU was memo
machinery, evaluator ~2%: probe-side linear rescan of per-record
heap-allocated conds Vecs ~55% (engine.rs conds loop), SipHash ~25%
(KeyCore + proj Vec, macros.rs compression rounds inlined), insert
dedup ~7%. Release profile now keeps debug symbols.

**Fixes (5807b93), all census/equivalence-gated:**
1. RecList — every record at a (core, mask, projected-values) key
   packs its conds into ONE contiguous u64 buffer (slot<<32|mask<<16|
   value); probe streams a single allocation.
2. FxHash for memo maps + irq_at + dump counters. No DoS surface;
   nothing decision-bearing consults iteration order (purge retention
   is a per-record predicate).
3. Insert-side subsumption (implied record fires strictly more often;
   drop the implied, fold benefits as purge priority) +
   REC_LIST_CAP=32 highest-benefit eviction. Swept 32/64/256:
   L=3-slice 1.72M/s vs 1.28M/s vs 0.69M/s at equal ~6.5% hit density;
   32 also won wall-clock on both L=2 brackets. New stats
   memo_rec_scans/recs_scanned/max_recs (heartbeat recs_avg/recs_max)
   showed lists pinned at whatever cap they're given — list growth was
   the old deep-region decay driver.

**Results (all verdicts reproduce, 0 champions):** same-slice L=3
throughput 257K → 1.72M items/s (6.7x, and >2x PLAIN DFS's 800K/s).
L=2 0..1: 157.3M items in 4.5min (old memo 40min; plain 3.3min — gap
now 1.35x, was 12x). L=2 1..1: 76s (was ~5min); its item count rose
23.9M → 56.9M — first-match record selection changed under
subsumption, NOT the cap (64 didn't recover it); wall-clock still
3-4x better. L=1 and L=2 0..0 item counts byte-identical (14,141 /
632,537). v4's L=3 kill point (1.42B items) is now ~15min of
wall-clock; overnight instrumented run queued as next step.

## 2026-07-11/12 — all levers landed; the conflict-scope bug; L=3 pushed to 1.4B+, still open

**Strategy (user):** goal is whole-32-word multi-SM programs; ALL
refutation weapons are indispensable at scale — build everything
visible, don't gate levers on which wins at L=3; long-running tests
mean more engine work, not runner work (runner integration dropped).

**Landed, each census-gated (0381fc4, 5819fe6, 4205bea, a69b9d5,
c7596e1, 3b1a540, 389bbfa, 44919a6, fe610b3):**
1. Pin-write pre-filter: exact one-cycle lookahead reusing step() when
   a fork value completes the consulted set and the (op,dst) writes a
   pin latch. Champion sets provably identical. Inert on tx_a's config
   (out_count=0) at L=2 — space-dependent, confirming build-them-all.
2. P1-full register symmetry, ALWAYS ON. Audit vs exec_op: exactly two
   asymmetric channels — PULL nonblock/if_empty on empty TX READS
   physical X into OSR; pending_exec words are data (never renamed).
   Unbound item = words + mirror twin; at an asymmetric event fork
   identity + twin-as-ordinary-item (decided words mirrored, x/y
   swapped = true twin state by equivariance). First design (swap once,
   mirror champions at report) was UNSOUND — conjugation breaks at the
   event; the twin must become a concrete item. EngineSpec::seed added
   (constrained resynthesis) and used to gate the fork end-to-end
   (pull_empty_binding_fork: each binding's trace finds exactly its own
   spelling).
3. Canon P2 (nop = mov osr,osr — register-free keeps P1 armed), P4
   (vacuous JMP-to-fallthrough, non-writing conds), P3 (front-loaded
   delays in runs of FRESHLY-FORKED consecutive nops; fresh-fork-only
   flag keeps seeds/wrap-revisits sound).
4. Consulted-set memo (failure-only, the playground translation):
   frame stack mirrors DFS; records = consulted (mask,value) ∩ fork's
   decided snapshot at (cycle, next_input, state); probe refutes items
   whose decided fields satisfy a record. Benefit-gated (subtree items
   = stats.items - items_at_open, O(1) by DFS contiguity), purge-and-
   raise at cap. Guards: unbound prober with x!=y can't hit (mirror
   twin at swapped state uncovered); binding-fork spelling conflicts
   poison recordability.
5. Ticket 007 (consulted-STATE keys) same session: key = always-read
   CORE (cycle, ni, pc, delay, stall, pending, clk_acc, latches) +
   read-masked patterns (x, y, isr, osr, counts, irq, fifos read-
   normalized); per-opcode read table (over-approx sound); segment-
   local X/Y provenance (set x,imm consults its field only if x is
   READ; sound because a write and its readers in one fork-free
   segment share all enclosing frames). Two-level index (per core:
   mask -> hash of projected values) after v3 showed 50x probe
   collapse; insert dedup killed ~94% of stored records.

**THE BUG (389bbfa):** Frame::merge's conflict detection compared
consulted values on the full overlap INCLUDING the field the frame
itself forked — children necessarily consult that field with different
values, so EVERY fork frame above leaf level was silently poisoned
unrecordable, from the memo's first commit. Only deepest conflict-free
frames (delay forks over refuted leaves) recorded — hence "99.3% of
records < 16 items" and the v1/v2 hit flatlines. Found by a
MEASUREMENT gate (consulted_state census showed 8.6% where 3-30x was
expected; core-match counter + a 12-line probe-miss trace localized
it). Fix: conflicts only on bits the frame keeps (decided at frame).
consulted_state gate: 830,875 -> 245,991 items (3.4x, asserted).

**New gates:** census_l1 (all 65,536 words, reproduces-trace IFF
covered under the P1+P2+P4 quotient; constant-HIGH census = 60,992
matching words, exact); memo_on_off_equivalence (champion LISTS);
pull_empty_binding_fork; p3_delay_normal_form;
consulted_state_shares_across_unread_register; tx_a_l3_first_bracket
(#[ignore], big-memo bracket experiment). PIO_NARROW_DUMP=<path>
streams memo-hit pairs (different partial programs, interchangeable
futures) + cluster summary; Stats::benefit_hist.

**tx_a ladder (verdicts all reproduce, 0 champions):** L=1 14,141
(was 16,735); L=2 0..0 632,537 (was 1,080,991); L=2 0..1 153,331,047
(was 220,364,655); L=2 1..1 23,890,049 (was 220,364,655 — 9.2x,
665K memo hits; the 0..1/1..1 wrap-invariance theorem is dead, the
memo exploits single-slot-loop convergence plain DFS can't).

**L=3 0..0 REMAINS OPEN — the wall moved but stands.** v1 (old
engine): abandoned >5.67B items. v2 (filters, memo frozen at 1M cap):
killed 3.25B+. v4 (full levers, 1M cap): killed 1.42B+; hits
flatlined at 2.57M once x4 purges pushed the bar to 1024. v5
(bracket-only, 8M entries ≈ user-approved ~1GB, x2 purge): TRIPLE the
hit density (7.4M hits @ 230M items) but also flatlined ~200M and
throughput fell to 17K items/s (giant-table probing). Conclusion:
capacity is NOT the lever — under value-exact patterns, deep regions
stop matching regardless of budget. Memory: ~100MB/1M records
measured (VmHWM), user's ballpark exact.

**Where the signal points next:** (1) mine runs/txa_l3_v5_hits.jsonl +
v4 dump (309K hit-pair lines) for what blocks sharing; (2) predicate-
valued patterns (records condition on x's exact value where the
subtree only zero-tested it — jmp !x cares about x==0, not x==17);
(3) probe throughput (fast hasher; memo currently LOSES wall-clock at
L=2 0..1: 153M items @ 64K/s vs v2's 159M @ 800K/s); (4) ISR/OSR
provenance, cond-lazy JMP targets; (5) multi-case specs for the
flagship RX (sequential trace concatenation + reset).

## 2026-07-11 (later) — fork engine v1 + first impossibility proofs; shard twin COMPLETE; L=3 wall → memo is the pivot

**Fork engine landed (aa489b3, cf892d0).** narrow/engine.rs: hole
programs (per-slot decided-mask/value), demand-driven forking at
bit-field granularity, DFS with NState checkpoint copies, per-cycle
trace refutation, don't-care champions. Statically dead bits never
fork. Two laziness levers: delay forks POST-step only on survived
cycles; asserted side-set values pre-filtered against expected[cycle]
(OE-gated soundness rule). Gates: L=2 square wave rediscovered;
L=1 period-3 duty proven impossible by exhaustion (first narrowing
impossibility proof). SOUNDNESS FIND: X/Y renaming is NOT a true ISA
symmetry — nonblocking/if_empty PULL on empty TX FIFO loads X — so
P1-lite pruning is a default-off flag; full P1 (virtual registers)
must model PULL's implicit X at the link binding.

**tx_a ladder (460-cycle oracle: DE stim + IRQ pulses, parity-absorber
cases).** Banked, zero champions everywhere: L=1 16,735 items; L=2
0..0 1,080,991; L=2 0..1 220,364,655; L=2 1..1 220,364,655 (eager
baselines 27,670 / 1,812,526 / 364,094,446 — levers ≈1.65x; eager and
lazy engines agree on every verdict). The identical 0..1/1..1 counts
are a THEOREM not a bug (probe: wrap IS applied, 1,364 vs 678
champions on a square-wave oracle with the same items/forks): for
2-slot spaces sharing an oracle, slot-consultation order is
wrap-invariant and wrap only acts after the last fork. L=3 0..0:
ABANDONED >5.67B items / ~90min, first of six brackets — plain DFS
without cross-item sharing hits the wall exactly where the playground
said: THE MEMO IS THE WHOLE GAME. User called the pivot. Next levers,
in expected-value order: consulted-set memoization, op-level pin-write
pre-filter (SET/OUT/MOV PINS determine captured OE-pinned levels like
side-set does), runner integration (resumable brackets).

**Shard twin COMPLETE and merged (2a3a2e7).** Sub-agent implemented
evaluator-spec.md in shard (shard_pio/: emulator 880 lines, runner,
vector generator): 101/101 certified vectors byte-identical (~0.5s via
the prebuilt bin/shard_eval — user's tip cut the full-closure check
from 30-60min to 32s), full closure checker-green (type gate,
structural measures, ZERO CANON advisories), gate discriminates
(seeded mutations fail exactly the right vectors). Verified 101/101
from master post-merge. Spec amendments from the twin adopted
(8c9b1af): driver/harness contract is now spec §9; OUT/MOV EXEC
truncate u16, OUT/MOV PC mask 0x1F; WaitIrq stores the resolved index.
PIO semantics now exist as checked shard definitions — Christian's
proof arc (2-SM ≡ 1-SM delayed bisimulation) is STATABLE; multi-SM
design notes in shard_pio/README.md (Block record lift, inter-SM
intra-cycle ordering to pin in spec + 2-SM vectors before proving).

## 2026-07-11 — narrowing engine begins: forkable evaluator landed; CANON.md canonicalization plan

Back on the optimizer per the 07-10 decision. Two threads:

**Canonicalization (Christian's shard CANON.md, now at
`~/Documents/programmingSync/computer-whisperer/shard/docs/CANON.md`).**
Read in full — v1 (C1–C10 recognizer + rewriter + exactness census +
content hash) is LANDED there, std at stage 2. Transfer analysis for
PIO agreed with user: canonicalization is a multiplier ON TOP of
narrowing (narrowing refutes duplicate spellings one waveform at a
time; measured there: quotient turned an unfinishable depth-4 space
into 4,095 steps). Adopted for our v1: fork-time leaf filters only
(D17's placement taxonomy: sibling-content constraints stay OUT of
generation), rules P1 register-canonicality / P2 canonical nop /
P3 delay-normal form / P4 vacuous control; every rule must be
LENGTH-NON-INCREASING on the representative or ≤N impossibility proofs
die (their depth-budget caveat, load-bearing for us); license =
fuzz-certified (no proof kernel here — weakest tier of CANON §6).
Register symmetry lands as VIRTUAL REGISTERS (user's call): candidates
written over r0/r1, deterministic link-time binding first-consulted→X,
ARM-side preloads are candidate holes and rename with the binding —
D16's "the freedom never exists". Exactness census at len≤2 is the
rule-set gate. ALSO ratified direction: a shard PIO emulator +
unbounded 2-SM-pair ≡ 1-SM-TX equality proof (delayed bisimulation,
constant 3-cycle lead — the invariant shape our emulator cert already
measured). Parking clarification: user misremembered the polarity
claim; Christian's actual requirement is that the implementations
MATCH (they do — parking covered by the emulator cert; shipped tx_a
parks DI high via idempotent force-highs, our mov pins,~null is
identical); a machine-checked proof is what would fully convince him.

**Evaluator v1 landed (4b9a364, 573f7b3).** `pio_superopt/src/narrow/`:
flat Copy NState (~120B — fork checkpoint = memcpy), total bit-field
decode (pending_exec can carry arbitrary words; no IR round-trip),
step() mirroring the vendored SM cycle ordering; `docs/
evaluator-spec.md` is the state/step contract written to double as the
shard twin's spec. Differential gate `tests/narrow_diff.rs`: DME
reference + ~2,500 random programs across side-set configs, config
genes, streaming, RX flavors (autopush thresholds × shift dirs × FIFO
joins), and per-cycle pin stimulus — all byte-identical to run::run.
The fuzz caught two contract facts now pinned in the spec: the pin
VALUE latch idles ALL-ONES, and osr_count resets to 32 (OSR empty at
reset — jmp !OSRE false, autopull fires on first OUT). Throughput
2.8x the fused vendored path (fat-LTO) on the DME workload — but the
structural win is the memcpy-able state the Bus-embedded vendored SM
can never give. Sub-agent semantic review launched at session end.

## 2026-07-10 (post-midnight wrap) — ground clip was the monster; bench truth

Continuation: user moved the Saleae ground clip off the differential leg
onto chassis ground. Re-measured with ro_sampler: skew collapsed from
±20ns to ~±4-8ns opposite sign, no vanishing pulses (lows 4-5/9-10,
highs 5-6/10-11; identical both boards; fixtures ro_sampled2_*). The
harness then found timings bit-perfect on BOTH regimes (the first grid
had capped d18 too low): flashed [3][4][4][4] to both boards (raven
350ede86).

Bench truth after the dust settled:
- Boards accept ~85-87% of the MAIN module's production traffic (its
  clkdiv-1.2 TX jitter smears parking per frame — friendly to receive).
- Parked-phase 125<->125 peer frames: ~10% at tonight's parking. The
  NS->NA exchange now works in BOTH directions (observed fd85bc52 ->
  NA len=86 twice in 60s), but pings stay red — lossy neighbor
  discovery never converges. Mechanism: 8ns sample grid vs ±6ns
  residual duty = ~1-cycle margins at some parked phases. That is the
  phase-invariant RX resynthesis spec, now with real fixtures.
- Production main (shipped fast RX) answered 0 of ~70 board NS —
  production firmware needs this fix class in both RX variants.
- Ops lessons: host tshark cannot arbitrate this bus (lan865x MAC
  filter); `pkill -f probe-rs` matches its own wrapper shell (exit 144)
  — use pgrep -x; defmt attach needs the EXACT flashed ELF (feature
  rebuilds shift the table).

Commits: raven 5a06f0eb + 350ede86; pio_optimization bfab6f2, ac3e3b0,
1c5de8c. Bench left: -0 duty-robust pinger -> -1 (detached), -1
duty-robust responder (attach bjoly1gxj).


## 2026-07-10 (late night) — duty distortion found: the RX mystery closed

Continuation of the "both RX fixes" bench session. Chain of discoveries:

1. **Phantom failure.** Built the missing model pieces (2-cycle sync-delay
   pipeline, carried frame state, candidate-C code, real current-bus capture
   — `rx_bench_repro.rs`): emulator clean at EVERY phase, all models. Audited
   the flash timeline: board -1's last flash (18:21) predated the startup
   revert edit (18:23) — the "reverted build still misses" observation was
   made on the [9][9] build; the committed build had never run. Lesson:
   verify `Compiling <crate>` appears in the flash log after any edit.
2. **Real build on hardware:** cross-clock frames pass CRC, but peer NS
   frames still never assemble; -0's pinger had also been down (probe
   teardown halted it) which masked cadence. Live-bus Saleae capture: 191
   bursts/8s (main module now on bus: PTP + 100ms telemetry + host), ALL
   FCS-clean on the wire.
3. **Register + DMA forensics (probe-rs reads on live target):** RX SM
   parked at `wait 0 pin 0` (25/25 samples), config registers all correct,
   FDEBUG.RXSTALL sticky-set, DMA write pointer advancing at 268 sym/s vs
   ~10k/s on the wire → the PIO itself misses frames; software exonerated.
4. **ro_sampler.rs** (new raven diagnostic): PIO1 raw-samples its own RO at
   125 Msps. Result: duty-cycle distortion ~±20ns — low runs 7-8/12-13
   samples, high runs 1-3/7-8 (some ONE sample), rising edges late. Wire
   pristine ⇒ skew lives in the transceiver-RO/pad path. Identical on both
   boards. Feeding sampled streams to the emulator reproduces the historic
   bench garbage head `01 12 02...` exactly. The aperture story was a red
   herring; uniform sync delay provably cancels.
5. **Fix:** polarity-asymmetric retiming (low test ~fall+80ns, high test
   rise+40ns = [4][1][9][4]), grid-searched against 7 real sampled captures
   + duty-distorted wire replays: 7/7 decode (3 bit-perfect) vs 0/7 shipped
   and 0/7 for the old aperture patch. Bench: -1 went ~2 → 533 CRC-valid
   frames/min; first-ever cross-board NS decode + NA reply (fd85bc52 →
   TX len=86).
6. **Ceiling:** pings still 0/71 — residual ~2% symbol errors are the
   vanished one-sample high pulses; P(clean 86B frame) ≈ 3%. Beating this
   needs the analog answer (Saleae ground clip? termination?) or RX
   resynthesis keyed on falling edges (lows never vanish) — the flagship
   superopt target, now with a real measured spec + fixtures.

Commits: pio_optimization bfab6f2 (harness + fixtures), raven 5a06f0eb
(dme_pio retiming + ro_sampler). Bench left running: -0 duty-robust pinger,
-1 duty-robust responder, both attached tasks stopped, -0 attach live.


## 2026-07-10 (night) — Survey vs production + K2L cross-check: RX bug refined, embassy-bump timeline, jitter direction tested

User challenged "production works reliably" vs our 100%-fail bench. Survey:
- crates/main runs embassy-DEFAULT **150 MHz** (system_freq commented out);
  only pneumatics pins 125. Production always pairs 150(fast RX, 1.2
  jittered TX) <-> 125(slow RX, 1.0 clean TX). All validated combos ever
  were cross-grid (also RP2040@125 <-> K2L in the Jan HIL report).
- No caller-side fixes missing: no sync-bypass/pad conditioning anywhere;
  raven-net sits above link decode.
- Timeline: embassy fork bump to upstream (51dff4da, 2026-04-07) is NOT
  in any release; R6-1 bringup (pneumatics-rp2350, the "requires 125 MHz"
  note) landed 2026-04-09 ON TOP of it — R6-1 only ever ran post-bump
  code, and no release tag contains R6-1 support. v0.1.3 worktree +
  submodules staged at /tmp/claude-1000/rf-v013 (unused so far).
- Bringup notes (Orbiter-Ultra-Hardware .../r6-1-firmware-bringup-notes.md)
  are pinout-only, no RS485 war story.

K2L USB 10BASE-T1S adapter added to the bus (enp8s0u1u1u3u3) = compliant
third-party referee + tshark visibility:
- K2L decodes the boards' stock TX PERFECTLY (clean NS frames at 3s
  cadence in tshark). TX fully exonerated again.
- Host->board ping: 100% loss; boards receive K2L frames as runts/CRC
  errors. Board RX fails against a real PHY too.
- Production clock pairing reproduced on bench (150 stock <-> 125 stock):
  STILL 0 valid frames. Clock config is not the masking factor.
- Jittered-TX direction (150/1.2 pinger -> 125 slow RX): runts + a
  KEY new failure mode: TWO full-length **926-byte assemblies failing
  only CRC** (the host's UDP spam) — alignment was CORRECT for those
  frames (misalignment dies in runts within symbols), so slow-RX CAN
  lock right vs the K2L; the residual is bit error(s) mid-frame. RX bug
  is at least two-factor: phase-dependent alignment AND occasional
  mid-frame corruption.
Open: K2L PLCA/beacon config unknown (could pollute the bus — check
raven_net_tests host setup); next move = replay real Saleae edge lists
through the emulated RX as pin stimulus (sub-cycle phase sweep, full
visibility) instead of more bench poking.

## 2026-07-10 (eve) — Bench session: single-SM TX hardware-validated; shipped RX bit-alignment bug root-caused

Bench = 2x pneumatics R6-1 (RP2354A), multiprobe `2e8a:000c-{0,1}:
E66368254F694937:0` (trailing :0 mandatory or probe-rs says "no probes").
Debug chronology, kept for method value:

1. Baseline (stock TX both boards) FAILED: 0 ping replies. Not our bug —
   single-SM was never flashed. Fixed two real test-firmware issues en
   route: embassy-default 150 MHz (product REQUIRES 125; at 150 the RX is
   completely dead — no RX-S DMA activity even for own echo) and
   identical MACs (fixed seed 0x1234_5678_9ABC_DEF0 → both boards same
   link-local; now OTP-chipid-derived).
2. At 125 MHz: RX pipeline alive; own echo decodes (MAC-matched, silently
   dropped by design — the discard_frame path is the ONLY fully silent
   one, everything else warns/counts). Cross-board: 100% loss, both
   directions, `rx_runts` ticking.
3. Saleae Logic Pro 8 on the bus (automation API :10430, scripts in
   session scratchpad): WIRE IS PERFECT. J J H H + scrambled payload +
   T R decode flawlessly at 500 MS/s; 40/80ns runs clean; both boards'
   crystals within ~1-25 ppm (baud from edge sums, ppm-level).
4. Emulator sweeps (rx_diag.rs): all whole-cycle phases and realistic ppm
   offsets decode 16/16 — but ONLY with the test helper's re-framing.
   Strict alignment check: emulator slow-RX delivers bit-offset 3 at
   EVERY phase; bench delivers offset 0 for own-echo, offset 1 cross.
   The -1 dump `01 12 02` = wire's `00011 00100 00100` with ONE extra
   bit prepended — bits perfect, grouping off by one, whole frame
   garbage since RXProcessor never re-frames. drift-cluster prediction
   (phase parks ~8s/cell at 1ppm) FALSIFIED by 6-min 0-reply run —
   consistent with alignment (not sampling margin) being the failure.
   Slow-variant startup paths also mistimed vs fast (1.6 vs 2.0 bit
   skips). PRODUCT IMPLICATION: pneumatics<->pneumatics rs485-eth cannot
   work; main<->pneumatics may be phase-lucky. Needs Christian/user
   disposition.
5. Single-SM TX verdict WITHOUT working RX: the Saleae is the oracle.
   Flashed single-sm-tx on -0; captured 3 NS frames at 3.000s cadence;
   structurally identical to stock capture (J J H H, clean runs, T R,
   990 bits, 12.5 MBd). mov pins,!pins pin-as-state toggling CONFIRMED
   on real silicon. Combined with emulator equivalence (incl. new 125MHz
   + slow-RX round trips, commit 4f2e195): single-SM TX is validated;
   ping-through blocked only by the pre-existing RX bug.

Also: slow RX variant is 31 instructions (dme_pio.rs "32" comment stale).
Harness lesson reinforced: tolerant test decoders hide alignment bugs —
rx_diag.rs now reports the raw bit offset (FW-GARBAGE marker).

## 2026-07-10 — Project pivot to the real firmware TX; single-SM TX transform certified in emulator + firmware built

Two things happened. First, direction discussion: the shard-search-playground
reference (needed narrowing / superposed evaluation — lazy hole-forking,
output-prefix pruning, consulted-set sharing) maps naturally onto PIO as a
third engine: holes = instruction slots/fields, demand = fetch, prune = per-
cycle pin-trace mismatch. Bit-level (fork on opcode/side-set/delay/operand
FIELDS at the cycle each is semantically consulted) beats whole-instruction
alphabets: side-set asserts on the first cycle even under stall, so for
dense waveforms the trace nearly determines those bits before the rest of
the instruction is ever forked. Decision: build our own demand-driven
narrowing evaluator (SMT-mirror pattern: differentially fuzzed accelerator,
emulator+certifier stay the soundness authority), NOT contort the vendored
emulator. Engine not started yet.

Second, target re-aim: the REAL rs485-eth firmware (copied to
reference/rs485-eth) doesn't use our single-SM DME encoder at all. TX is a
two-SM pair: tx_b (17 instr, sequencer, side-set=DE, fires IRQ 0 per DME
transition) + tx_a (4 instr, IRQ→DI toggle; its jmp-PIN-on-DE makes IRQs
while DE is low idempotent force-highs = parity absorber, parks DI high).
The split exists ONLY because side-set can't XOR — and our compress track's
`mov pins, !pins` pin-as-state discovery dissolves it. Hand transform:
irq set 0 → mov pins ~pins (DI as OUT+IN pin), final parking irq →
mov pins ~null (absolute high). 17 instr, one SM, IRQ gone, TX PIO 31→27.

Certified in emulator (pio_harness/tests/tx_single_sm.rs): DE cycle-exact,
DI edge-identical with constant 3-sys-cycle lead (tx_a's IRQ latency),
both clkdivs (1+51/256 @150MHz exact-6-cycle half-bits; 1+16/256 @133MHz
delta-sigma), multi-frame with silence gaps + parking; shipped 32-instr RX
decodes all 16 data codes from the single-SM waveform (round-trip test).
Gotcha logged: harness set_pin() is EXTERNAL stimulus and overrides PIO
output in the GPIO merge — preset latches via exec'd mov, not set_pin.

Hardware validation prepared in Raven-Firmware.main (UNCOMMITTED, user
reviews/flashes): rs485-eth gets feature `single-sm-tx` (tx_a left unused);
stale hil_testing/ethernet_tests/rs485_eth_test refreshed to current APIs
(embassy path-deps — old crates-io pins silently missed the [patch] after
the embassy bump and built TWO embassy copies; DMEPioHardware ctrl_dma/
ts_pio_sm fields; StaticConfigV6::single; dma::Channel::new + DMA_IRQ_0
bindings). Both variants release-build to thumbv8m ELFs. Plan: flash
--features single-sm-tx TX against known-good RX on the rig, ping test.

Scoreboard implication: DME single-SM results (≤4 impossible, 6-word
champion) demote to benchmark suite; ≤4 proof also predates side-set
enumeration. Next targets: tx_b-derived single-SM (17) compression, tx_a
optimality as narrowing-engine validation, RX (32/32) flagship later.
Single-SM-TX moonshot upgraded to "done pending hardware".

## 2026-07-06 (eve) — len-4 probe: full-free solves are the bottleneck; solver levers added (d22dad5)

First len-4 full-free probe (all 64 program bits free): iter 1 solved in
7 s, iter 2 in 16 min, iter 3 killed after 6+ h single-core CPU. The
hole-refill numbers (9 s) do NOT extrapolate — each battery counterexample
multiplies solve cost steeply when the whole program is free. Candidates
were entertainingly weird (`mov PinDirs,!Pins` direction-toggle encoding,
`mov Osr,!Osr` scramblers) — the certifier accepts direction-driven levels,
FYI, since it reads the merged pad like a receiver would.

Levers added (d22dad5): `Solver::new_for_logic("QF_BV")` (pin the
bit-blast→CDCL strategy), `parallel.enable` (still looked single-core in
the first minute — check overnight), and the sound enumerate-style
"some instruction must drive the pin" pruning constraint. Probe restarted
detached (survives session close): `runs/smt-synth-len4-none.log`. If it's
still crawling tomorrow: diverse seed examples, smaller phi_max windows,
canonical-ordering symmetry breaking, or dump SMT-LIB and try bitwuzla.

## 2026-07-06 (later) — CEGIS engine built and working (40bb822); len-4 cross-validation probe launched

`smt/cegis.rs` on top of the morning's mirror. `assert_frame` restates
`certify_dme` in bitvectors with ONE free variable per example (frame phase
φ): per-cycle biconditional "transition iff on the φ-grid" covers clock
edges, data-iff-bit, spurious edges, strict tail; window sized exactly like
`spec_certify_corpus`. Agreement with the real certifier pinned positive
(spec seed × 4 corpora) and negative (dme_ref, delay-mutated seed).

Loop: solver ∃ on accumulated examples → candidate → REAL emulator +
certifier battery (32 singles → 1024 pairs → train/held-out corpora → 16
random 8-word streams, cheapest first) → failing corpus becomes a new frame
constraint. Found = battery-certified, mirror-independent. Unsat = subset
impossibility resting on mirror fidelity. **Divergence guard**: if a
battery-refuted candidate is re-proposed (i.e. mirror and certifier
disagree), abort loudly rather than spin.

**Perf lesson (the big one):** naive nested-ite unrolling made a 1-free-word
solve take 565 s; SSA/BMC interning (`unroll_interned`: fresh constants per
state field per cycle, asserted equal) brought it to 9.3 s — 60x. Whole smt
suite: 14.5 s release. Equivalence to the pure unroll is a pinned test;
diff tests still use the pure form (ground simplify, no solver).

End-to-end test frees seed slot 1 and re-derives it in 2 iters. A pre-SSA
run instead invented `mov Y, NonePins` — reading the current line level back
through the GPIO loopback instead of tracking it in Y. The solver is already
finding non-obvious mechanisms; good omen for side-set worlds.

Runner: `superopt smt-synth --len N [--side none|1|2en] [--side-pindir]
[--no-autopull] [--max-iters] [--trace]` (needs `--features smt`; binary
must be REBUILT with the feature — plain `cargo build --release` silently
lacks the subcommand). NOT resumable; trace is observability only.

Launched: len-4, side none, autopull — the CEGIS subset is STRICTLY LARGER
than the enumeration alphabet (adds PULL, all delays, out counts to 32, any
set imm), so UNSAT corroborates the 12.3B-eval enumeration proof from a
fully independent engine, while SAT would be a real len-4 discovery outside
the alphabet. Trace: `runs/smt-synth-len4-none.jsonl` + `.log`.

## 2026-07-06 — SAT/SMT track opened: symbolic PIO semantics + differential harness (b73939f)

New third track alongside compression/enumeration, user-initiated ("the
landscape is so bumpy SA is proving to be hard"): encode PIO semantics into
SMT bitvectors and synthesize/refute by solver instead of search. Rationale:
the len-5 question is a *decision problem* — CDCL is enumeration-with-
learning and can return UNSAT proofs SA never can; side-set costs the
solver a few bits per slot instead of enumeration's ~3^len blowup, so it is
IN scope from day one (a len-5 UNSAT without side-set would be the weaker
claim, and the sweep already covers the no-side-set region).

Landed (`pio_superopt/src/smt.rs`, feature `smt`, links system libz3 —
default build unchanged for fleet boxes): `step()` mirrors picoem-common's
`execute_cycle` bit-for-bit for the single-SM/single-pin TX subset
(enumeration alphabet + PULL + side-set incl. pindir drive and
apply-on-stall ordering + autopull + delays + wrap + one-cycle GPIO
loopback; FIFO modeled as index into the concrete input stream — valid for
both preload and streaming paths per `stream_matches_fast`). `unroll()`
gives per-cycle (level, OE); `SymProgram::free` + `legal_word()` make the
program words solver variables confined to the modeled subset.

**Fidelity is the entire risk** (a wrong mirror poisons UNSAT verdicts
silently — models are certifier-checked, refutations are not). Gates:
dme_spec_ref cycle-exact over the full 278-cycle window; 60 random subset
programs across the side-set/autopull/shift-dir/threshold grid; 2000-case
`differential_fuzz` `#[ignore]` tier (passes clean, ~7.6 min, rerun before
trusting any UNSAT); two hand-planted mutations (X-- decrement-at-zero,
side-set-skipped-on-stall) both caught by the random tier.
`synthesize_len1_toggler` closes the ∃ loop: solver invents a word, real
emulator confirms the waveform.

Next: CEGIS engine (solver proposes against accumulated example frames,
certifier is the ∀-oracle, failures become new constraints), then aim at
len-5 DME as a race against the fleet sweep. User is learning SAT along
the way (reference/sat_playground is the sandbox; not used by the crate).

## 2026-07-05 (evening) — strategy pivot: compression + enumeration; certified 6-word encoder; len<=4 proven impossible

Morning queue executed: runner-restructure merged (090d423), eval cache
landed (6f0ce2f: per-restart direct-mapped 2^16, exact keys incl config,
per-group raw sums, hit==miss bit-identical; 29-30% hits, 1.2x; NOTE:
per-group summation changed float order — pre-6f0ce2f traces unresumable).

**Autopull experiment (d777208, spec-ap-ladder 0x5EED 32x4M): SIXTH L=6
stall.** RunSpec.autopull_pad applies FIFO padding per-candidate. The gene
WON (champion autopull-on with NO pull; 56/64 minima) and L=6 still stalled
all attempts at fe=8 (vs 11-12 plain spec, 63-80 cycle-exact); solved 4/13.
Refill-spine hypothesis dead: the wall is the word seam, not the idiom.

**Pivot (user-agreed): stop synthesizing, compress + enumerate.**
- Compression (1ba9ff8): dme_spec_ref() hand-written spec-shaped autopull
  seed (8 insns, certifies clean; dme_ref CANNOT certify — 14cy cell,
  data@+6, +1/word slip). synthesize_compress: cycles of reheat-and-cool
  from the champion, wandering cost W*err+size, champion moves only through
  the certifier (train; held-out reported). Reuses GatedSnapshot protocol;
  resume locked by test. CERTIFIER FIX: autopull_pad removed from cert
  corpora (pad = data an autopull champ transmits -> guaranteed strict-tail
  FAILs; yesterday's ap gates were inflated: re-cert FAIL(1)/FAIL(3)).
- Enumeration (e674016): exhaustive len-N bodies, alphabet jmp/out/mov/set
  (+NOP in v2), structure/timing decomposition — no alphabet op stalls
  mid-frame so the edge SEQUENCE is delay-independent: screen structures
  once at delay 0 (exact, no false negatives; zero-delay seed passes,
  pinned), brute-force delay tuples (sum<=15) only for pattern survivors,
  then full dataset + certifier. Sharded (first-slot op), file-per-shard
  manifest, --shard-mod/rem fleet split.

**Results:**
- compress run 1: certified 6 at cycle 47 — pin-as-state (mov Pins,!Pins,
  no Y reg), interior NOP as asymmetric branch pad (delays can't pad one
  branch outcome). That NOP trick exposed an enumeration alphabet hole ->
  alphabet v2 (+canonical NOP, 4688e18).
- **METRIC GAMING (cycle 49): "size 5" champion used wrap 0..5 + jmp->5
  with slot 5 EMPTY** — executes a real NOP at address 5, 6 words on
  hardware, reported 5; cost steered INTO the exploit. Fix (6b41592):
  size() = footprint (occupied ∪ wrap bounds ∪ jmp targets), locked by
  test. Poisoned run stopped (trace kept); compress2 restarted fresh and
  independently re-found an equivalent certified 6 (cycle 35, jmp NotY
  variant) — the 6-word design replicates across metric versions.
- **len-4 v1: 655M structures, 0 survivors; len-4 v2 (NOP alphabet): 672M
  structures, 12.3B timing evals, 0 survivors** => no 4-word encoder,
  unconditional over the v2 scope. Minimum is 5 or 6; len-5 decides.

**Fleet infra:** shard coordinator built by subagent in worktree, reviewed
(one fix: write shard file BEFORE marking done), merged (3d31ab1):
superopt serve/work, lease TTL requeue, 409 contract check, all state =
shard files. Graceful two-stage worker shutdown (d5f9c3d): drain ->
abort+release (no TTL stranding); ctrlc termination feature (SIGTERM ==
Ctrl-C everywhere). docs/fleet.md covers static split, coordinator mode,
compression migration. Live-smoke-tested with real signals.

**Corrections:** side-set emulator bug was already FIXED (368e499,
2026-06-21) — earlier session docs cited the stale memory headline;
enumeration excludes side-set for COST (~3^len), not fidelity (f7b5ce8).

**Firmware survey** (Raven-Firmware dme_pio.rs): TX block 31/32 = TX_A 4
(IRQ-toggle slave) + TX_B 17 (encoder, side-set DE, silence codepoint) +
timestamp 10; RX block 32/32 (unrolled 6-way jmp-pin ladder = 11 insns of
position-encoded cycle counting). Interfaces mostly SOFT (IRQ handshake,
sentinels, exec preloads) — per-SM compression with observational-
equivalence oracles (original program = golden). Flagship: RX. Harness
has multi-SM (from_shared) + per-cycle set_pin; RunSpec wiring needed.

Paused for user-driven runs: compress2 (snapshot at ~cycle 50, champ 6)
and the len-5 fleet sweep. Commits: 090d423..f7b5ce8 (13).

## 2026-07-05 — resumable runner, restructure, eval optimizations; L=6 wall is oracle-independent

- **Densify verdict + merge** (late 07-04): sweep table filled — champion
  class monotone in densify_w (TOGGLER→OTHER→CONJUNCTION); densify_w=1.0
  kills the toggler and cracks 2..=3 at 24×2M (5.3× cheaper than default
  0.5's 32×4M). INVERTS the cycle-exact densify lesson: under the tolerance
  band the spurious discount is what funds the toggler. spec-oracle branch
  merged to master; worktree/branch deleted.
- **Resumable runner** (c97055d on master): gated ladder snapshots full
  state (per-restart cur/best programs + RNG u64 + rung pool/lib/minima)
  inline into the JSONL trace at ~1/8-rung cadence and on Ctrl-C (stop flag
  latched by r0, all restarts quit in lockstep post-snapshot). `superopt
  spec-ladder` auto-resumes from the last snapshot, header-verified;
  byte-identical to an uninterrupted run (locked by test). Also fixed a
  latent heartbeat cadence bug (i%hb only fired when hb was a multiple of
  the checkpoint step — true only at the 4M budget).
- **Long spec run started** (user, own shell): lengths 2..=14, 32×4M,
  densify 1.0, seed 0x5EED.
- **Restructure** (branch `runner-restructure`, ticket 006): phase 1
  deleted 34 concluded experiments + orphaned engines (flat/PT, rainbow,
  novelty, ramp/stage, portfolio, meta_anneal) −2.8k lines; phase 2 added
  the `Problem` trait (DmeSpec/DmeWave), `wave-ladder` + `diagnose`
  subcommands, `result.json`. Engine-split dropped by agreement (deletion
  already halved the files). spec-ladder header byte-identical to the live
  trace's → resume-safe across the merge.
- **Early-exit eval** (d1d1396): reject when the partial row-sum exceeds
  cur+40·temp (acceptance < e^-40 < RNG resolution), consuming the
  Metropolis draw to keep the stream identical. Verified transparent
  (sorted traces byte-identical vs baseline binary). ~13% on a full
  shallow-stall schedule; ~0% in reheated hot phases — savings live in the
  cold half of each attempt.
- **Eval-cache measurements** (user's idea): duplicate-candidate rate is
  ~32% (shallow stall, 12M evals) and ~33-39% (deep stall, live-trace
  copy, 34M evals) — stable across regimes, warm≈random restarts; NOT the
  >90% a pinned-champ model predicted (cur wanders at stall temps).
  Key-stream replay vs simulated caches: **no thrashing** — a 16k-slot
  direct-mapped table hits 37.2% vs the 38.8% unbounded ceiling (96%);
  LRU/2-way/bloom-admission buy nothing. Design settled: thread-local
  direct-mapped 2^16, exact keys, per-group error vectors. ~1.5× on
  stalls. Approved, to implement next session (ticket 004).
- **FINDING — the L=6 wall is oracle-independent**: the deep-stall probe
  (deterministic copy of the live trace, resumed to completion) ended
  attempt 2 STALLED → solved 4/13, champion carrying the partial refill
  idiom (`jmp NotOsrEmpty` + second pull), cert FAIL(3)/FAIL(6). The live
  run will deterministically reach the same verdict. The wall is basin
  topology around the OSR-refill conjunction, not dme_ref's timing
  artifacts. Next lever: autopull as a config gene under the spec oracle
  (ticket 005 step 4).

## 2026-07-04 — L=6 verdict: STALL (4x), testbed retired; oracle pivot underway

- Overnight 2..=14 run (old engine): L=2..5 solved, L=6 stalled fe=80. Trace
  diagnosis: **warm-start lock-in** — all 64 warm restarts ended at EXACTLY the
  warm champ (0 improvements in ~256M evals, reheat to 0.79); all 32 random
  restarts plateaued 2.6-4x worse. Monoculture in a deep moat.
- Countermeasures built (commits 333747f, 8b3029d): **cross-pollinated
  retries** (restart minima deduped by op structure -> diverse warm pool) +
  **self-mined macro splices** (recurring 2-3 insn runs, jmp targets
  relativized, spliced ~1-in-8). Attempt 0 stays RNG-identical to old engine.
- Rerun (`xpoll-mined`): still 4/13, L=6 fe=80 all 3 attempts. Attempt 2 truly
  engaged (27 distinct minima, pool=8 lib=8); best incursion sel=13322 — ONE
  unit off the champ — nothing crossed. Frequency-mining harvested boilerplate,
  never the rare refill idiom (>=2-programs threshold). Lesson: the wall is
  basin topology at this cell geometry; search-side levers exhausted.
- **Oracle pivot started (ticket 005, all 4 design decisions resolved)**:
  - Real 802.3 Clause 147 numbers extracted (docs/802.3-clause147-dme-timing.md):
    T2=80ns±100ppm, T3=40ns band 38-42, jitter<5ns s2s. All SUB-CYCLE at PIO
    resolutions -> TX freedom = compliant FAMILY (phase/polarity/schedule),
    not edge fuzz; bands become load-bearing for the future RX direction.
  - **FINDING: dme_ref itself is NOT spec-shaped** (14-cycle cell, data at +6
    not centered, +1 slip per 5-bit word = pull latency). Pinned as test
    `dme_ref_is_not_spec_shaped`. Every cycle-exact champion was forced to
    copy these artifacts. Autopull/balanced-refill = the path to compliance.
  - Certifier landed on master (src/certify.rs, commit 4e6b317): independent
    receiver-style decoder, mutant-tested, no shared code with dme_ref/cost.
  - Decisions: clkdiv OUT of genes v1; bare bit-cells v1; in-repo certifier now
    (scope cross-check later); **nominal cell = 16 cycles, data at +8**.
  - Spec search metric + testbed implemented on branch `spec-oracle`
    (worktree ../pio_optimization-spec-oracle): Target plumbing (Wave|SpecBits),
    spec testbed, certifier-gated ladder. Finding: the tolerance band is
    charitable to a data-independent half-cell toggler (Thompson hazard as
    predicted) — L=2 cracks only at full 32x4M budget. densify_w sweep with a
    structural TOGGLER/CONJUNCTION classifier written to price the exploit out.

## 2026-07-03 — status + THE PATH FORWARD (agreed with user)

### What happened this session

- **Padding regression found & fixed** (commit 93900e2): the FIFO-fed padding
  added in c76ed4d ("harmless with autopull off") broke the conjunction crack —
  padded stalls L=2 (0/13, front_err=1.5); unpadded solves L=2..5, same
  seed/budget. Padding is now opt-in (`pad: bool`), required only for autopull
  retries. **The autopull "double-consume race" hypothesis is RETRACTED** —
  its L=2 evidence was the padding, so autopull+fed-FIFO is UNKNOWN, not negative.
- **Word-boundary wall replicated unconfounded**: unpadded extended ladder
  solves L=2..5, stalls L=6 (front_err=63). Crucially the stalled champion
  contains the refill idiom skeleton (`jmp NotOsrEmpty` + a second `pull`) —
  the search FINDS the mechanism idea unaided; it fails to finish wiring it.
- **Ladder engine fixes** (commit a911650): lexicographic rung selection
  (frontier-solvers can no longer lose selection to a lower-metric non-solver),
  stall recovery (2 retries, escalating reheat, half-random pool), deterministic
  early-stop (per-checkpoint barrier) + heartbeat. Determinism re-verified.
- **In flight**: the L=6 verdict run (2..=14, fixed engine), restarted with
  structured trace logging.

### The L=6 verdict decides the macro question

- **If it cracks**: curriculum pressure alone discovers mechanisms — no macro
  injection needed. Climb the ladder, then implement the END config-polish
  (flip autopull on, delete redundant pull/OSR-check, re-validate — still
  unimplemented), then STOP polishing this testbed (no meta-tune size-squeezing
  for its own sake) and pivot to the oracle work below.
- **If it stalls after all retries**: next lever is **self-mined macros**, NOT
  hand-authored ones — harvest recurring instruction subsequences from the
  elite pool / stalled champions (e.g. the `jmp !OSRE / pull` pair it already
  invented) and offer them as insertion moves. Historical note: hand macros
  already had an era here (cracked the UART spine, superseded by the atomic
  Gene-IR loop); the new constraint is the PROJECT GOAL — novel programs a
  human wouldn't write — so human-authored idiom injection is a last resort.
  Moves shape visit density, not reachability; but with finite compute, density
  is destiny, so keep the vocabulary machine-discovered.

### Ranked sources of human bias (why the next fronts are what they are)

User's goal: novel PIO (oversampled clocks, overlapped loops through
seemingly-unrelated code) for the real multi-SM 10BASE-T1S program. The
binding biases, strongest first:

1. **The oracle**: cycle-exact equality to the hand-written reference waveform
   at pinned clkdiv doesn't just bias toward human implementations — it
   MANDATES human timing bit-for-bit. Oversampling/retiming solutions score as
   garbage by construction. (This is why clkdiv-as-gene "failed": at window=0
   any clkdiv change is maximally wrong. The landscape, not the wiring.)
2. **slots=10 window**: overlapped-loop tricks need room + multiple entry
   paths; a 10-slot single-wrap window can't express them. Blocker for 32:
   polish is O(slots²·ops²), search budget vs space size.
3. **Single-SM harness**: the real target is multi-SM (IRQ handshakes, SM→SM
   signal chains); search + oracle plumbing are single-SM throughout.
4. (Distant fourth) macro vocabulary — see above.

### The next major front: SPEC-LEVEL ORACLE (design before code)

Replace "equals the reference waveform" with "satisfies the protocol":
- **Loose metric for search** (gradient): tolerance-band edge scoring —
  `edge_cost`'s timing `window` is already a banded aligner; free clkdiv.
- **Strict independent checker for certification**: a real protocol checker
  (edge positions within spec jitter of ideal cell boundaries, mid-bit
  presence per data bit, data recoverable), NOT the search metric. Split is
  mandatory: every softened oracle to date got gamed (Thompson hazard —
  `OUT PINDIRS`, counter-replay).
- Champions must still be HW-validated eventually.
- This is a correctness REDEFINITION and a one-way-door design — discuss the
  DME tolerance spec + exploit surface with the user before implementing.

Sequencing after the oracle: 32-slot window scaling (only pays once the
objective can reward overlap), then multi-SM harness, then the real
rs10base-t1s target (start with one SM's worth under the spec oracle).

## 2026-06-23 — Performance + determinism + meta-tuning resolved

Picked up the "NEXT" from 06-22 (profile the eval hot path) and it cascaded
into resolving the whole meta-tuning question. All committed.

### Eval perf — 3.4x on the real breed-step (ticket 004)
Profile-first with a Criterion suite (`benches/eval.rs`) + the `fixtures` module.
- **edge_cost 3.9x**: a full all-ones mask scanned all 32 bit-channels (30 empty)
  — intersect `care` with bits present in golden|candidate; rolled the DP onto
  two rows. 12.3→3.2µs.
- **Emulator 3x**: `emu.step()` ran both Cortex-M33 cores + all peripherals every
  PIO cycle. Vendored `rp2350-emu` and added `step_pio_only`/`tick_pio` (PIO
  slice only; skip cores/timers/IRQ, hoist the redundant GPIO refresh, skip reset
  blocks). Gated by `fast_step_matches_full` (DME ref + plateau + 300 random,
  byte-identical to the full step). 16.5→4.2µs.
- **Fat LTO**: the hot path crossed 4 crate boundaries through un-inlined pub fns
  (~15% call overhead). 5.2→4.2µs.
- **Cache hypothesis REFUTED**: perf shows IPC 4.2, L1d miss ~0% (Emulator is
  11.5 KiB, fits L1) — compute-bound, not cache-bound. At 32 threads IPC drops
  4.2→2.6 (SMT exec-unit contention), so 32 islands ≈ 20x aggregate, not 32x.
- Near the single-SM ceiling; more needs single-SM specialization (declined —
  multi-SM is wanted) or a JIT. The 4-SM-loop skip was measured neutral.

### Determinism — free, via the cooperation A/B (tickets 001/002)
- **Cross-breeding does NOT help on DME** (`dme_breed_ab`, n=16): independent
  restarts beat cooperative on best/median/mean. The n=6 look-positive was
  *noise* — same seed gave 17 then 23 (board↔thread timing). Slot crossover is
  destructive; disruption > benefit at DME complexity. (May still pay at higher
  complexity — not refuted, just below threshold here.)
- **Independent mode (`poll_rate=0`) is deterministic** (no board reads → pure
  seeded anneal; locked by `flat_breed_independent_is_deterministic`). So
  trustworthy meta-tuning needed no barrier-sync rewrite — just run independent.

### Meta-tuning — resolved (ticket 002)
- **Transfer trap was the `max_window`/ladder lever**: dropped it from the
  meta-genome → tuning transfers (deterministic: default 23.0 vs tuned 21.3 @800k).
- **`t_end` sweep**: optimum is budget-invariant (0.05 at both 150k and 800k) ⇒
  **short inner trials ARE representative**. The perf work let us *prove* this.
- **New weak link**: the meta-anneal under-optimizes (found t_end=0.54; sweep says
  0.05). 24 SA iters stall on a ~1-D landscape — a grid beats SA for small HP
  spaces. And at 800k the t_end curve is nearly flat: HP tuning's ROI on DME is
  thin; its real value is at higher complexity.

### NEXT (as recorded then)
DME is still unsolved (best edge-cost ~17–22, plateaued). Perf + determinism are
in hand. The open frontier is **why DME plateaus** — the search isn't reaching
edge-cost 0 regardless of scale/HPs. Likely a *structural/representational* gap
(the mids), not a tuning or speed gap. Candidates: richer move set / decomposition
for the data-conditional cell; or accept DME as a stepping stone and move toward
the real target (10BASE-T1S) where cooperation + meta-tuning should finally pay.
*(Resolution: the curriculum/conjunction work of 06-24..07-03, then the oracle
pivot — see later entries.)*

## 2026-06-22 — mlx86 detour → novelty pivot → flat edge engine

Surveyed `reference/mlx86` (a friend's x86-asm superoptimizer) for prior art →
`tickets/` (001 parallel tempering, 002 meta-tuning, 003 hash tabu). It searches
a flat raw-byte space yet solves hello-world / a 4-fn calculator and finds
genuinely creative solutions (e.g. misaligned jumps that re-decode bytes). Three
things that flowed from it:

**1. Targets: UART is solved; aim at DME.** UART has one hard structure (the
spine) and we already find good ones — it can't discriminate search ideas (PT
A/B on it was neutral-to-negative noise). The real target is 10BASE-T1S DME
(`reference/.../rs485-eth`). Stepping stone = **single-SM Differential
Manchester TX** (biphase-mark: transition every bit boundary + a *data-conditional*
mid-bit transition). The data-conditional mid-transition is the coupled second
structure UART lacked.

**2. v2 IR — structured conditional (`Node::Cond`, committed).** The gene IR v1
banned all non-structural JMPs, so it **could not express** DME's data-conditional
branch. Added `Cond{cond,then,els,…}` (structured selection, dual of `Loop`),
label-free, lowers with a past-end→wrap fixup. The DME reference encoder
(`dme_ref`, biphase-mark, tracks line level in Y, `mov y,~y`/`mov pins,y` to
toggle, `jmp x--` on the data bit) runs correctly: mid-transitions are
**popcount-exact**. Locked golden + corpus + guard test.

**3. The macro trap, and the real reframing.** Macros (counted-loop, etc.) were
right for UART — a *solved* problem where encoding the known structure is fine —
but they **inject human priors at the expense of the novelty we're after**. The
gene IR itself is a prior: it forbids the creative control-flow reuse mlx86
thrives on. Diagnosis, in order of leverage:
- **Objective (biggest).** We scored the cycle-by-cycle *level* trajectory;
  mlx86 scores *output*. Level-Hamming on a transition code is **deceptive**: a
  slowly-varying signal matches ~half the cycles for free, so the search falls
  into a `out Pins` data-dump basin (best ~31-44/278, never climbs). **Fix:
  `Metric::Edge`** (`cost.rs`) — represent each channel as a transition-event
  sequence (cycle,new_value; pre-level 0 ⇒ complete encoding) and score a banded
  edit distance (shift = Δ/(W+1), miss/spurious = 1; W annealed = old `k`).
  Edge-cost 0 at W=0 ⟺ exact waveform, so it also certifies. **A/B: the level
  metric steers to `out Pins`; the edge metric steers to a pin-toggling loop
  (`mov Pins,~Pins`) — a real transition generator.** Necessary, not sufficient:
  it reaches the periodic-clock skeleton, not yet the data-conditional mid.
- **Representation.** The creativity (instruction reuse, overlapping jumps) lives
  in the **flat slot search**, which the gene IR forbids. So: revive flat search.
- **Diversity/scale.** mlx86's flat power = PT + migration + millions of trials.

**Flat edge engine (`synthesize_flat_pt`, search.rs).** Parallel chains
(thread-local emu), per-stage diverse elitism, adaptive stage-0 diversity, ported
island migration (PT) — edge objective, window-annealed, **no priors/macros**,
arbitrary jumps. Selects/certifies by edge-cost (not level — fixed a straggler
bug). First A/B on DME (n=4): **flat+edge edge-cost ≈24 beats gene+edge ≈34**
(not compute-controlled, but 8 full runs in **15 s** — huge scale headroom).
**Migration still a wash** (24 vs 23, noise) — within-stage PT hasn't earned its
keep on either UART or flat-DME; if PT is to pay off it likely needs knob tuning
or the cross-*seed* variant (deferred "plan 2"), not more within-stage migration.
Neither solves yet (edge ~23, need 0).

### Plateau → diagnosis → the cross-breeding pivot

Flat+edge+PT+scale (the full mlx86 recipe) **plateaus at edge-cost ~22** —
7× iters barely moved it (24→22). **Diagnosis** (`dme_diagnose`, classify golden
edges as *boundary*/clock vs *mid*/data-conditional via an all-zeros reference):
the champion matches some boundaries but **0/11 mids, every seed** — the
data-conditional transition is **never born**. Two findings:
- The mid cell is **expressible** in the flat search (`out x`/`jmp x--` exist)
  but **gradient-free** — no partial credit until the whole timed cell is right,
  so local search never enters it. Scale can't manufacture a gradient.
- The edge metric has its **own trap**: missing = spurious = 1, so the safe move
  is *emit few edges, avoid spurious* (a champion hid at 7 edges / 0 spurious).

**The new path (committed) — staging overstayed its welcome.** Three changes,
all confirmed to matter:
1. **Densify** (`edge_cost_w`, spurious weight < 1): attempting an edge costs
   less than leaving one unmatched → the search densifies instead of hiding.
2. **Continuous cross-breeding islands** (`synthesize_flat_breed`): persistent
   parallel islands on a **fixed window ladder** (no staged re-election), each a
   long anneal, sharing a board and **recombining** (slot-range `crossover`) not
   copying — so a "has clock" and a "has mid fragment" island can yield a child
   with both, crossing the conjunctive gap.
3. **Scale**: 32 islands = 32 cores (was 12 — we were leaving 20 idle).

**BREAKTHROUGH:** the data-conditional mids — never born before — **are born**:
4-5/11, edge-cost trajectory **staged-plateau 22 → small-breed 23 → breed@scale
17** (5/11 mids, 12/20 boundaries, 1 spurious; a creative `mov Pins,~Pins` /
`~Osr` / `Isr` tangle, *not* the human reference). Still not solved (17, need 0)
and **high-variance** (1/3 seeds breaks through).

### Migration verdict
- **Within-stage island migration (copy) is a wash** — UART, flat-DME, and at
  scale. Superseded by **cross-breeding (recombination > copy)**. Ticket 001's
  copy-migration is retired; the useful descendant is the breeding board.

## Earlier (UART era, pre-2026-06-22)

### THE central problem (as it was then): assembling the counted-loop spine

Five decomposition experiments (in `search.rs`, all `#[ignore]`) triangulated
*what* makes UART hard — it is **not length, not framing-vs-data**:

| experiment | result | rules out |
|---|---|---|
| `uart_curriculum_bit_ramp` (warm-start k=1→8) | cliffs at rung 1 | length |
| `uart_k1_base_solvable` (1-bit frame cold) | plateau ~10 | "small ⇒ easy" |
| `uart_masked_curriculum` (framing-first mask) | degenerate oscillator trap | region-masking on a shared pin |
| `uart_data_loop_synthesizes` (spine, no framing) | plateau 6 / 26 | "it's just SPI" |
| `serializer_autopull_synthesizes` (no spine) | **clean 0, 20s** | isolates the cause |

**The obstacle is the counted-loop spine**: `pull` + `set x,N` counter + `out` +
`jmp x--` — ~4 instructions, **no partial credit until all present and wired**.
SPI/autopull-serializer synthesize only because **autopull+wrap dissolve the
spine**. UART can't use that escape: **framing needs the explicit count** to know
when to emit start/stop, so the spine is irreducible *and* shared between data
and framing. That's why nothing separates into independent fragments — the
"data fragment" still contains the whole spine (→ it plateaus, not 0).

Why masking failed specifically: framing and data multiplex **one pin**, so
"solve framing first" admits a degenerate pin-envelope oscillator (no FIFO
machinery) that traps the warm-start. Masking only separates sub-problems that
map to separable program pieces (e.g. *different pins*).

Two levers ruled out, one ruled in:
- **Curriculum / bit-count ramp** — dead: relocates the cliff to the base rung.
- **Region-masking decomposition** — dead for shared-pin protocols (oscillator trap).
- **Building-block / macro moves** — INDICATED. The failure is *coordination*
  (4 simultaneous mutations, no gradient); a macro move makes the spine one move.

### Building-block macro moves (slot search, committed 84e1de5)

Slot-search macro `insert_counted_loop` (self-sufficient `pull / set x,N /
out / jmp x--`, wrap-enclosed) + `MutateImmediate` (dial an immediate without
re-rolling the op). **Cracked the spine**: `data_loop` 6/26 → 0/0. Two lessons
paid for: a block must be immediately waveform-correct to survive selection
(the reward gives no credit for structure), and the count needs a tuning move.
But full UART only reached 21/44 — loops survived yet got **mangled and
couldn't integrate**: in a flat array a loop is divisible, so the search keeps
re-deranging it. → motivated the first-class IR.

### First-class IR + annealed tolerance metric (committed 59cc470)

**Gene IR** (`gene.rs`): genome = sequence of nodes, `Node = Prim | Loop{body,
cond, counter_init, jmp_delay}`. A loop is **atomic** (owns its back-jump), so
point moves can't dismantle it. **No labels** — plain-JMP targets are literal,
so structural loops lose no runtime novelty; data-driven axes kept
(`UntilOsrEmpty`, `counter_init=None`). Lowers to the existing `Program` path.

**Gene search** (`gene_search.rs`): SA over nodes; serializer macro; hard
**length cap** (gene analogue of the slot window — essential, reduced size
weight alone bloats); deterministic **polish** (refine + remove + compensated
framing insert); **timing-aware moves** (insert/remove-compensated, shift-cycles
over top-level prim delays) that restructure at *constant total duration* to
dodge the strict-Hamming phase cliff.

**Annealed tolerance-band metric** (`cost.rs::hamming_tolerant`, `k`): a
mistimed-but-present bit pays `δ/(k+1)` not 1; `k=0` == strict masked Hamming.
`synthesize_gene` anneals `k` 8→0 (graduated optimization: blurry→sharp, temp
re-heated each stage). Smooth metric finds the basin; strict k=0 certifies —
and the schedule *resolves* the smooth-vs-exploitable tension (smooth guides,
strict rejects gamed champions).

**FIRST FULL UART SYNTHESIS:** k=8 → correctness 0, size 6, **novel** —
`pull / mov Pins,BitReverse Null[7] (start) / loop(CountY=7){out[6]} /
mov Pins,Invert X[7] (stop)`. The framing bits are creative (BitReverse 0 = low;
Invert of zero-init X = high), reached only because of the annealed tolerance —
strict Hamming never got there.

### Reliability — solved via parallelism (committed 674bc37)

Per-chain synthesis is low-rate/high-variance and three search levers (elitism,
2-opt polish kick, adaptive diversity gathering) **plateaued** it — the
bottleneck is late-stage barrier-crossing, not stage-1 diversity. So reliability
comes from **parallelism**, not a higher per-chain rate:

- **k=4<k=8 inversion = starting-radius mismatch** (confirmed by sweep): a fixed
  radius-8 blur over-smears the *shorter* k=4 frame. Optimal starting radius
  **scales with the target** (k4 wants ~4, k8 ~8). No single schedule is best.
- **`synthesize_portfolio`**: run a portfolio of diverse schedules (varied
  starting radii) × multistart seeds, keep the strict-best. A 2-schedule × 8-seed
  portfolio **reliably synthesizes both UART targets** (combined correctness 0,
  first solve in 1-2 runs). Each `synthesize_gene` is internally parallel
  (chains/stage via `run_chains`).

Net: the engine is **capable + parallel-reliable**. Per-chain elitism/2-opt help
specific cases (data_loop k=8 7/8) but parallel portfolio is the mechanism.
