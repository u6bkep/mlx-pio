# 009 — Word behavioral quotient (digest classes)

**Status:** open · **Source:** playground factorization (45,878
compile spellings → 13,335 probe-observable classes via 128-bit
behavior digests, quotiented BEFORE search).

## The idea

Mechanize P2/P4: precompute, per SM config, the quotient of all 65,536
instruction words by observable behavior (context-free equivalence —
words that behave identically in ALL states). Playground method: hash
each word's behavior on a sampled state battery (digest), then verify
candidate-equal pairs exactly (we have exec_op + census machinery to
prove quotients; census_l1 already checks a hand-rolled one).

Two applications:

1. **Generation:** fork only class representatives where equivalence
   is context-free (P2/P4 are two hand-found classes; the table finds
   them all). Sound by the same argument as P2/P4.
2. **Memo cond canonicalization:** insert AND probe canonicalize
   consulted field values through the table, so records match across
   spellings — aimed directly at the cond-miss signal (the dominant
   miss class in the gate: 95,670 cond vs 16,968 state misses).

## Notes

- Equivalence must be per-config (side-set layout, pin counts change
  behavior); table is 64K u16 class ids per config — build once per
  search, cache.
- Partial words: canonicalize only fully-decided consulted fields, or
  build the table at field granularity (opcode+field context).
- Cheaper and lower-risk than 008; can land first and 008 can reuse
  the partition machinery.
