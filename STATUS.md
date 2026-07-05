# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-05.

## Where we are

**The L=6 wall is ORACLE-INDEPENDENT.** The spec-oracle long run's fate is
already known: a deterministic copy of its trace was resumed to completion
during eval-cache measurements — attempt 2 of L=6 stalls, ladder breaks,
**solved 4/13**, champion has the refill idiom two-thirds wired
(`jmp NotOsrEmpty` + second `pull`), cert FAIL(3)/FAIL(6). Same wall as
cycle-exact ⇒ basin topology around the OSR-refill conjunction, not the
reference's timing artifacts. The live run (if still going) is only
confirming this — safe to Ctrl-C.

**Branch `runner-restructure`** (worktree ../pio_optimization-restructure,
4 commits, all tests green) — READY TO MERGE:
- Phase 1: 34 concluded experiments + superseded engines deleted (-2.8k
  lines); 10 ignored keepers (6 canaries, 3 subcommand-candidates kept as
  tests, crack_l2).
- Phase 2: `Problem` trait (DmeSpec/DmeWave), `wave-ladder` + `diagnose`
  subcommands, `<trace>.result.json`. spec-ladder header verified
  byte-identical to the live run's (resume-safe across the merge).
- Early-exit eval (d1d1396): reject-bound hot path, ~13% shallow-stall,
  ~0% in hot phases; behaviorally transparent (verified via sorted-trace
  byte-equality).

## Next actions (agreed 2026-07-05)

1. Merge `runner-restructure` → master; remove worktree; stop the live run.
2. **Eval cache** (user-approved; design settled by measurement, see ticket
   004): thread-local direct-mapped table, 2^16 slots, EXACT keys
   (assembled words + wrap + config genes), values = per-group raw error
   vectors (weights applied at lookup). Measured: 32-39% duplicate rate
   across stall regimes; 16k-slot direct-mapped already captures 96% of
   the unbounded ceiling (no thrashing, no bloom admission needed).
   Expected ~1.5x on stalled rungs, composes with early-exit.
3. **Autopull as a config gene under the spec oracle** (ticket 005 step 4)
   — the actual lever against the wall: the spec oracle doesn't mandate
   dme_ref's +1-slip-per-word, so autopull-on may dissolve the refill
   spine the way autopull+wrap dissolved the UART spine. Needs the padded
   (FIFO-fed) dataset variant for autopull retries (`pad` flag exists).
4. Then per ticket 005: 32-slot window, multi-SM.
