# 008 — Outcome-grouped forking (+ predicate-valued memo patterns)

**Status:** open · **Source:** shard-search-playground second mining
pass (2026-07-12); unifies with STATUS "predicate-valued patterns".

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
