# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-04.

## Where we are

**Oracle pivot (ticket 005) in progress.** The cycle-exact testbed is retired:
the gated ladder solves DME L=2..5 but stalled at L=6 four times (warm-start
lock-in; cross-pollination + mined macros didn't cross the moat — search-side
levers exhausted). Verdict: the wall is basin topology under the cycle-exact
oracle, and `dme_ref` itself is not spec-shaped (14-cycle cell, off-center
data, +1 slip/word), so every champion was forced to copy those artifacts.

**Spec oracle state** (migration plan in `tickets/005`):

1. Certifier — DONE, on master (`src/certify.rs`, mutant-tested, independent).
2. Oracle plumbing — DONE, on branch `spec-oracle`: dataset rows are
   `(RunSpec, Target)`, `Target::Wave` (cycle-exact, unchanged behaviour) or
   `Target::SpecBits` (nominal cell = 16 cycles, data at +8). Spec testbed +
   certifier-gated ladder mirror exist; fast tests green (50 pass).
3. A/B + re-tune under the spec oracle — IN FLIGHT. Finding: the tolerance
   band is charitable to a data-independent half-cell TOGGLER exploit; L=2
   cracks only at 32×4M. `dme_spec_densify_sweep` (densify_w × budget grid,
   TOGGLER/CONJUNCTION structural classifier) committed on `spec-oracle`;
   **sweep currently running**, results table to be filled, then merge.

## Active branches / worktrees

- `spec-oracle` @ `../pio_optimization-spec-oracle` — 3 commits ahead; merge
  to master after the densify sweep results land.
- `algorithm_word`, `gene-v2-ir` — stale, review-or-delete.

## Next actions

1. Fill `dme_spec_densify_sweep` results table; pick densify_w; merge
   `spec-oracle` → master.
2. Long run: `dme_spec_curriculum_gated` (lengths 2..=14) under the spec
   oracle — does the ladder climb past L=6 once refill artifacts aren't
   mandated?
3. Repo restructure (discussed 2026-07-04, do AFTER merge): runner binary
   replacing `#[ignore]` experiment tests; `Problem` trait; engine module
   split; per-run `runs/<date>-<name>/` artifact dirs. Needs a ticket.
4. Then per ticket 005: re-test config genes (clkdiv/autopull), 32-slot
   window, multi-SM.
