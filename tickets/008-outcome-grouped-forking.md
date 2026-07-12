# 008 — Outcome-grouped forking (+ predicate-valued memo patterns)

**Status:** in progress — stage 1 of 4 landed (12ec1cb) · **Source:**
shard-search-playground second mining pass (2026-07-12); unifies with
STATUS "predicate-valued patterns".

## Staging (agreed 2026-07-12; all four get built, one commit each)

1. **DONE (12ec1cb): lazy JMP target demand** — target forked at
   consult time (taken execution), like delay. L=3 brackets −1.4/−1.6%
   items, L=2 +2% (vacuous jmp-to-ft walks to first-taken instead of
   dying at fetch — bounded; the untaken-fan saving scales with L).
2. Outcome-grouped forking: value-set items (u32 bitmask per ≤5-bit
   field, small fixed array per item, concrete-fork fallback), lazy
   partition refinement at each re-consult via one-step outcome on a
   scratch state.
3. Predicate-valued memo records on stage-2's interned partitions.
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
