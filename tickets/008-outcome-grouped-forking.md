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
3b. **RE-SCOPED by the recursive pair races (2026-07-13): generalized
   subtree walk ("junk-wall collapse").** Cross-opcode conflict class
   race-sized honestly (PairRace probe, 96f0372 + 8e69ac8): 770K
   opcode-conflict races on 2..2, deep region — **84.5% share_co**
   (whole joint subtree co-refutes, 100.0% latch-quiet), 2.2% bad
   (nearly all one-side-OOB; true capture divergence 37/770K), 13.3%
   mismatch (conflict slot executes, different field demands), budget
   binds 29/770K at 16K steps. steps_avg=391 two-machine ⇒ ~200
   single-machine. KEY INSIGHT: the race observes the prober refute
   at every leaf — the record machine is redundant. Lever = bounded
   concrete DFS of the item's own subtree at fork points (emulator
   steps only, enumerate fork values, superset of engine enumeration
   since canon prunes not applied): all leaves refute within budget ⇒
   kill item without forking. No record needed, no theorem — direct
   observation. Binding edge / surviving leaf / budget ⇒ fall back.
   Keeps stage-2 junk_walk as the cheap fast path for pure delay
   families. Undecided (walk-forked) bits contribute NO conds
   (universally quantified); decided consulted bits merge as usual.
   **Stage 3c (user, 2026-07-13): adaptive walk budget** — make the
   step cap a runtime tunable driven by a value heuristic (kill-rate ×
   steps-per-kill EMA per firing site), with occasional exploratory
   deep walks during a run to re-probe whether longer walks pay.
   Build after fixed-budget 3b proves out.
4. ISR_CNT provenance (prov becomes a small field-SET: MOV→ISR
   resets, OUT→ISR sets from a field, IN accumulates a field).
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
