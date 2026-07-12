status: open
priority: high

# Consulted-state memo keys — generalization instead of eviction

## Problem

The consulted-set memo (a69b9d5, benefit-gated in the follow-up) keys
failure records on the ENTIRE flat `NState`, byte for byte. That is the
coarsest sound key: two search nodes share a record only when every
component — both scratch registers, both shift registers, both FIFOs
including stale buffer slots and head rotation, IRQ flags, everything —
is bit-identical. But a subtree's verdict can only depend on state it
actually READS. A toggle loop that never touches X gets refuted
identically from x=0 and x=17; today those are separate records (or one
record plus a full re-exploration). Every unread component that differs
is a lost share.

Observed cost (tx_a L=3 wrap 0..0, 2026-07-11): the v1 memo froze at
its 1M cap and hits flatlined — the DFS had wandered into a state
region whose IRRELEVANT components had shifted, so nothing matched.
The benefit histogram shows 99.3% of records summarize <16-item
subtrees; the value concentrates in a few thousand records whose reach
is needlessly narrowed by the monolithic key. Capacity work (purge and
raise the bar) treats the symptom; SEARCH.md's doctrine names the cure:
"canonicalization is the whole cost of search" — make things identical
upstream, don't evict downstream.

## Why this is the playground's own mechanism

In shard-search-playground, a memo key is a thunk's identity
`(expr, env)` under hash-consing: state the computation never consulted
is STRUCTURALLY ABSENT from the key. Nobody designed key minimization;
laziness gave it away. Our flat evaluator traded that for memcpy forks.
This ticket restores the missing half, symmetric to what the engine
already does for program fields (consulted `(mask, value)` conditions,
quantifying the failure claim over everything else).

## Design

**Component partition** of `NState` (~14 components):
pc, x, y, isr, osr, isr_count, osr_count, delay_count, stall,
pending_exec, irq_flags, out_latch, dir_latch, clk_acc, tx, rx
(plus cycle and next_input, which are already key parts).

**Split key:**
- Hash core (always read, every cycle): cycle, next_input, pc,
  clk_acc, out_latch, dir_latch — fetch reads pc, the divider reads
  clk_acc, capture/compose read the latches unconditionally. Also
  delay_count and stall discriminant (the step entry path consults
  both every cycle).
- Pattern conditions (read-mask + values of masked components only):
  x, y, isr, osr, counts, pending_exec, irq_flags, tx, rx, stall
  payload.

**Records** become (read_mask: u16 bitset, masked component values,
program conds, benefit). **Probe**: hash on the core, then scan the
key's records comparing only components in each record's mask — the
same shape as the existing per-slot program-cond scan.

**Read tracking**: frames accumulate a subtree state-read mask exactly
like `consulted_mask` accumulates program fields; a record's mask is
the union over its subtree. Soundness argument is the standard one:
a component read on NO path of the subtree cannot influence any
path's outcome, so the failure claim legitimately quantifies over it.

**Read-effect source — never instrument step()**: a static per-opcode
read table (sibling of `writes_pin_latch`), derived line-by-line from
`exec_op` and `still_stalled`:
- JMP: cond 1/2 read x, 3/4 read y, 5 reads both, 6 reads gpio
  (→ latches + stim), 7 reads osr_count.
- WAIT: gpio/pin read gpio; irq reads irq_flags (and stall re-checks
  read their subject every stalled cycle).
- IN: source reg/isr/osr; autopush reads isr_count, rx level.
- OUT: osr, osr_count; autopull reads tx; dst EXEC writes pending.
- PUSH/PULL: rx/tx levels; pull-empty reads x (the P1 channel).
- MOV: source component; STATUS reads tx/rx level per status_sel.
- IRQ: irq_flags.
- SET: nothing.
- pending_exec execution: decode the pending word the same way.
The asymmetry is the whole audit: OVER-approximating reads is sound
(fewer hits); UNDER-approximating is a false impossibility. Same
treatment as the P1 asymmetry audit.

## Payoff

- Records generalize over unread components: fewer records, each
  covering strictly more probers — memory compression WITHOUT
  information loss (the "clever approach" this replaces eviction with).
- Cross-region sharing: the v1 flatline mode disappears for subtrees
  whose diverging components are unread.
- Incidentally fixes the Fifo hash wart (stale buf slots / head
  rotation hash observably-equal FIFOs apart) wherever the FIFO is
  unread. A read-normalized Fifo digest (level + live words in order)
  handles the rest.

## Sequencing / data feed

The memo-hit dump (PIO_NARROW_DUMP) logs convergence clusters. Before
building: skim the dump from the current tx_a L=3 run and check WHICH
components block sharing in the hot clusters — that data pins the core
vs pattern partition (e.g. if clusters differ only in clk_acc phase,
the core needs thought there).

## Gates

- The four L=1 censuses (brute-force 65,536 words, quotient-exact
  coverage) run memo-on and must stay green.
- memo_on_off_equivalence: identical champion LISTS.
- New targeted gate: two seeded prefixes differing only in a NEVER-READ
  register must produce a memo hit (shared record); a pair differing in
  a READ register must not.
- narrow_diff unaffected (evaluator untouched).

## Size

Read table + audit is the bulk; record format and probe changes are
mechanical. ~a day with gates. Interaction with the P1 unbound-prober
restriction and binding-fork conflict poisoning carries over unchanged
(both are about program spellings, not state components).
