# 010 — Multi-SM factorization blueprint (from compile⊗exec)

**Status:** open (design note; blocked on multi-case specs) ·
**Source:** playground joint-synthesis arc (docs/joint-log.md there).

The playground's coupled-synthesis experiment is structurally our
multi-SM endgame: two holed programs, no per-program tests, one
correctness theorem. Their measured arc on a 1.9e16 coupled space:
direct coupled narrowing 157s → enumerate-small-side + digest-quotient
22.9s → determinism prefilter 1.2s → trie-factored 0.4s. Transfers:

1. **Factor by interface:** SM-A's probe-observable surface is its
   pin/IRQ/FIFO trace. Enumerate/search SM-A, quotient by interface
   digest, run one SM-B search per class. Exactness: classes partition
   SM-A's space; per-class search settles SM-B's space; product =
   joint total.
2. **Determinism prefilter:** two spec rows demanding different
   outputs from identical (interface, state) admit no partner program
   — kills classes searchlessly (97.5% at their depth 2). Also
   applies to multi-case spec self-consistency.
3. **Trie-share class prefixes:** order cases smallest-first, group
   classes by induced interface per case, pay shared-prefix narrowing
   once (their 2.9M → 331K steps).
4. **Measured warning:** strongest-case-first ordering was 4x WORSE
   (wide forking before failure). Order cases cheapest-first.

Prereq: multi-case specs (sequential trace concatenation + state
reset) — the same feature the flagship RX resynthesis needs.
