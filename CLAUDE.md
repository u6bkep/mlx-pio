# pio_optimization — agent guide

## Read this first

- **`STATUS.md`** — the current frontier. Read it at the start of every
  session; it is short by design.
- **`docs/architecture.md`** — goal, pipeline, durable lessons. Read when you
  need design context; skim the "Durable lessons" section before proposing
  search/oracle changes.
- **`docs/journal.md`** — session history. Do NOT read it whole; search it
  for provenance when you need to know why something is the way it is.
- **`tickets/`** — the issue tracker; ticket 005 (spec oracle) is the active
  design doc.

## Writing conventions (replaces the old SCRATCH.md)

- **`STATUS.md` is REWRITTEN, not appended.** At the end of a work session,
  update it to reflect the new current state and keep it under ~50 lines.
- **`docs/journal.md` is append-only, newest entry at top.** Move the session
  narrative there (what happened, findings, verdicts, commit hashes).
- Durable lessons and pipeline changes go in `docs/architecture.md`.
- Do not recreate SCRATCH.md.

## Operational notes

- Long runs go through the runner binary: `cargo run --release --bin
  superopt -- spec-ladder ...` in `pio_superopt/`. It writes a resumable
  JSONL trace under `runs/`; Ctrl-C snapshots and exits, rerunning the same
  command resumes byte-identically. Older experiments still run as
  `#[ignore]` tests: `cargo test --release -- --ignored <name> --nocapture`
  (migrating them into the runner is planned — see STATUS.md).
- Long runs must follow the observability directive in
  `docs/architecture.md` (stderr heartbeat + JSONL trace), and should write
  logs/traces under `pio_superopt/runs/`, not the crate root.
- The search is deterministic in independent mode; same seed ⇒ same champion.
  Preserve that when touching engine code (there are locking tests).
