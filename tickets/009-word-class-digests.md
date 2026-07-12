# 009 — Word behavioral quotient (digest classes)

**Status:** open — design settled 2026-07-12 (see "Design decisions"
below); implementation paused mid-session by the emulator-fidelity
discovery its first lemma triggered (journal 2026-07-12 eve), resuming
on corrected semantics. · **Source:** playground factorization (45,878
compile spellings → 13,335 probe-observable classes via 128-bit
behavior digests, quotiented BEFORE search).

## Design decisions (2026-07-12)

- **Digests propose, lemmas prove.** Class merges feed impossibility
  verdicts, so they must be proven, not fuzzed. The SMT mirror cannot
  prove the full space (no WAIT/IRQ/IN/PUSH, 1-pin configs only), so:
  battery digest groups candidates; only merges verified by a small
  library of auditable, per-config lemmas (checked against exec_op
  source) enter the table; unproven candidates stay singletons (sound
  — less pruning, never wrong verdicts) and are LOGGED as lemma TODOs.
- **Fork-time sibling dedup** is the sound generation insertion point:
  within one field-fork loop, skip a candidate iff an already-PUSHED
  sibling has the same behavior signature under the child's decided
  mask. Dynamically sound by construction (the kept sibling provably
  survived P1/side/P2/P4 filters); no static representative choice, no
  P1 interaction bugs. Precompute per-realizable-mask signature tables
  (demand order makes realizable masks a small prefix set, ~30/config).
- **Memo cond canonicalization is likely moot within one search** once
  generation dedups spellings; measure the cond-miss composition from
  the census run before building it.
- **Known real classes to encode first**: SET pins/pindirs data
  masking by set_count (16x on 1-pin configs); config-dependent pin
  aliasing. NOTE: `mov null,src` does NOT exist (MOV dst 011 is
  PINDIRS); ISR/OSR self-moves are REAL ops under corrected semantics.
- Spec-relative quotient (space excludes all shift-count observers) is
  a legitimate extension for seeded spaces.

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
