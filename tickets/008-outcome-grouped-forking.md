# 008 — Outcome-grouped forking (+ predicate-valued memo patterns)

**Status:** in progress — stage 1 of 4 landed (12ec1cb) · **Source:**
shard-search-playground second mining pass (2026-07-12); unifies with
STATUS "predicate-valued patterns".

## Staging (agreed 2026-07-12; all four get built, one commit each)

1. **DONE (12ec1cb): lazy JMP target demand** — target forked at
   consult time (taken execution), like delay. L=3 brackets −1.4/−1.6%
   items, L=2 +2% (vacuous jmp-to-ft walks to first-taken instead of
   dying at fetch — bounded; the untaken-fan saving scales with L).
2. **RE-SCOPED by the delay-pair races (2026-07-12): time-shift-
   invariant refutation lookahead ("junk-window collapse").**
   Measured on 2..2 (28,672 sampled delay-conflict pairs, lock-step
   races): 95.8% co_refuted at avg 60.5cy, of which 100% latch-quiet
   and 99.95% external-free; ZERO diverged. Theorem: from state S, if
   the walk (undecided delays as 0) to a capture/expected mismatch at
   cycle F contains no pin-latch writes, no external-schedule
   consults (WAIT any src, JMP PIN), and no non-delay fork edges,
   then every spelling of every undecided delay field in the window
   executes the same latch-quiet instruction sequence time-shifted ⇒
   capture is the same static value on [t0,F] for all spellings ⇒
   all refute at exactly F. Hook at the delay post-fork: one scratch
   walk (cap ~128cy) refutes the entire 8^k delay-spelling subtree
   outright; records carry NO delay conds (kills the 44% record wall
   as a side effect). Walk must mirror the main loop's consulted/read
   accounting for the memo. Fallback to normal forking on any unclean
   window. The original value-set/outcome-class design attacks the
   2.5% cond slice — demoted behind stage 3.
3. **TRIED AND REVERTED (2026-07-13): demand hoisting.** On a clean
   walk prefix ending at a fork edge, fork the demanded FUTURE field
   at the parked point, delay left pending (attack on the
   post-collapse #1 class: SET-pair delay conds, 50% of the residual
   2..2 wall). FAILED the magnitude gate: L=3 0..1 785M→982M items
   (+25%); excluding side-field edges (hypothesis: hoists bypass the
   cycle-local side pre-filter) made it WORSE (1.09B) — hypothesis
   wrong, mechanism not understood. Lesson: don't reorder fork
   decisions past the cycle-local prunes without cost attribution.
3b. **TRIED AND REVERTED (2026-07-13): generalized subtree walk.**
   Race-sized first (PairRace probe, 96f0372 flat + 8e69ac8 recursive
   joint-fork races, both KEPT): cross-opcode conflicts ≈ the whole
   post-stage-2 wall; **84.5% of cond-miss subtrees fully co-refute**
   (770K deep races, 100% latch-quiet, true divergence 37/770K,
   budget-bind 29 at 16K steps, ~200 single-machine steps/kill-tree).
   Lever (a1477f3, reverted by its revert commit): bounded concrete
   DFS of the item's future at a COND-MISS pop, full value
   enumeration (strict superset of engine children ⇒ refutation
   transfers with no theorem), budget 768, per-branch depth cap 128.
   Kill rate 89%, **items halved (L=3 0..1 784.0M→388.1M, 1..1
   716.3M→353.1M), but WALL-CLOCK 2.8x WORSE (27s→76s, 24s→68s;
   2..2 27%→10.3% settled at 50min)**. Mechanism understood: the
   walk re-explores subtrees WITHOUT the memo/quotient/canon sharing
   the engine gets (~563 steps/kill vs ~100 needed to break even at
   ~13ns/step vs ~1.3µs/item), and sibling cond-miss pops re-walk
   overlapping futures — the exact redundancy the memo exists to
   remove. Firing-policy sub-lessons (all measured): ungated
   fork-site firing = 18% kills/416 avg steps (walks out-cost the
   main loop); remaining-trace gates never fire (search refutes
   early, never nears the horizon); item-count magnitude gates MUST
   be paired with idle-box wall-clock (the first "win" was against a
   contended 127s baseline; true baseline 27s). The 84.5%
   transferability is REAL — the exploitable form is record-side
   (weaken/condition conds so probers hit without re-proving), not
   walk-side re-proof. Candidate: linear lock-step race transfer at
   cond-miss (no forking, ~44cy, kills the 7.4% flat-co_refute slice
   cheaply) — size its wall-clock before building.
3c. **Adaptive walk budget (user, 2026-07-13) — MOOT while 3b is
   reverted**, but the two control knobs it would tune were
   identified empirically: where to fire (population kill-rate) and
   when to give up (per-branch depth). Revisit only if a walk-shaped
   lever returns.
3d. **TRIED AND REVERTED (2026-07-13): once-per-family walk-certified
   record generalization** (built f5a7c2b, reverted after the 15-min
   conversion run). Sizing (probe 456f829): 95.2% of cond-misses are
   single-conflict; ~2K distinct (core, conflict-slot+mask) families,
   wildly heavy-tailed (true mean ~96K members); family walk with the
   conflict UN-DECIDED kills 65.4% of miss-MASS at 6.4K steps.
   Firing-policy iterations (each measured): first-miss-per-entry
   arming = uniform-over-entries → 7.5% kills, and in split mode
   re-arms per unit (+50% wall on 0..1); 256-miss threshold arming =
   30% kills at ~zero cost. Kill path inserts the generalized record
   directly at the item's core (the frame system alone never keys a
   record there — first build bug). **CONVERSION VERDICT: zero.**
   15-min sequential 2..2: 64 kills/190 arms, items and memo-hit rate
   byte-flat vs baseline. **Root cause is structural, not tunable:
   the census family groups members by (core, conflict slot+mask),
   but members differ freely on OTHER decided bits; the walk's record
   must carry conds on everything its (family-wide) tree consulted,
   so it covers only the sub-family agreeing with the walked item
   outside the conflict — nearly empty. The 84.5% transferability is
   inherently PER-MEMBER (the pair race proved member-vs-record), and
   per-member re-proof is 3b — already proven uneconomical.** The
   walk/record-generalization chapter is CLOSED; the remaining
   record-side idea is outcome-class conds (design B, big surgery).
   Next direction chosen instead: static pair canonicalization + 
   champion-family mining (user proposal, 2026-07-13 — see journal).
4. ISR_CNT provenance (prov becomes a small field-SET: MOV→ISR
   resets, OUT→ISR sets from a field, IN accumulates a field).
   **NOW THE ACTIVE STAGE**, then the one-shot Codex engine review.
4. ISR_CNT provenance (prov becomes a small field-SET: MOV→ISR
   resets, OUT→ISR sets from a field, IN accumulates a field).

## The idea

The playground's narrowing engine forks a hole only into the
alternatives the consult DEMANDS. Our engine forks a demanded field
into all its CONCRETE values — but most consults don't need the value,
only its effect this cycle:

- A `JmpCond` consult in a given state has TWO outcomes (taken /
  fallthrough); we fork 8 values where 2 outcome-groups would do.
- `WaitPol`/`WaitSrc`/side-set in many states collapse similarly.
- `SetData` mostly doesn't collapse (each imm writes a distinct x) —
  the win is field- and state-dependent, which is exactly why it
  compounds on JMP-heavy spaces like tx_a.

An item's decided field becomes a VALUE-SET (generalizing don't-care
from "never consulted" to "consulted, but only its outcome class
mattered"). A later consult that distinguishes members (next loop
iteration, different state) refines the set — partition refinement,
lazily.

## Why it's the same abstraction as predicate-valued memo records

Fork side: fork on outcome classes, not values. Record side: condition
records on the outcome class the subtree tested (`x==0`, not `x==17`).
One equivalence-class machinery serves both; do them together.

## Sketch

- Item repr: per-slot decided mask stays; decided VALUE becomes either
  concrete or a small set/interval per field (needs a compact repr —
  per-field class id into a per-(field,state-context) partition table).
- exec_op consult sites return (outcome, distinguishing-partition);
  fork pushes one child per nonempty class.
- Champions materialize as sets (already have don't-cares; this adds
  subset-don't-cares).
- Memo conds gain class-valued entries; probe checks class membership.
- Census gates: L=1/L=2 exact-coverage censuses must still hold
  (census_l1 machinery); memo on/off equivalence; ladder verdicts.

## Expected value

Multiplicative fork-width reduction down the whole tree on JMP-heavy
spaces; directly attacks the L=3+ fan. Highest-ceiling engine change
known. Big surgery — item/champion/memo representation all touched.

## Evidence (L=3 probe census + snapshot, 2026-07-12)

Direct measurement says this ticket owns the deep-memo wall: of 798M
core-matched probes, 90.2% were COND misses, and 97% of sampled cond
fails are value conflicts on decided bits — probers and records hold
genuinely different slot-0/1 words (quotient respelling covers only
0.97%; filler slots never conflict). Sharing across different words
requires conditioning on the OUTCOME the subtree tested, exactly this
ticket's record side. Also queued here: **ISR_CNT provenance** — 35%
of state-miss near-diffs are isr_count-only; its value is usually
program-determined (count of INs since last push), the same
field-provenance chain that X/Y already have.

## Evidence refinement (2026-07-12 late, target/delay bit split)

Splitting the 94.9M sampled pure value conflicts by conflicting bits:
**JMP-pair delay-only conflicts are 41.5M (44% of the entire wall)** —
same JMP word, different delay spelling; cross-opcode pairs 39.2M
(41%); JMP cond-bit conflicts ~2.5%; JMP target-only 0.33% (why stage
1 measured only −1.4%: its record-side headroom was tiny); side-set
~0 (the fork-time side pre-filter already kills those subtrees).
Design consequence: stages 2/3 must center TIMING/outcome sharing —
delay-spelling equivalence (same state at same cycle via different
delay splits; P3 covers only consecutive canonical nops) and
cross-opcode outcome classes (untaken JMP ≡ nop ≡ guard-failed PULL ≡
met WAIT this cycle) — not cond-value grouping, which is 2.5% of the
wall.
