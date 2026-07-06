# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-06.

## Scoreboard (DME TX under the spec oracle + certifier)

| size | status |
|------|--------|
| ≤4   | **PROVEN IMPOSSIBLE** — enumeration, alphabet v2 incl. NOP landing pads (672M structures, 12.3B timing evals, `runs/enum-len4-v2/`) |
| 5    | OPEN — decided by the len-5 fleet sweep (~15 core-days), a lucky anneal, or the new SMT track (which also covers side-set — the sweep doesn't) |
| 6    | **EXISTS, CERTIFIED, twice independently** — pin-as-state (`mov Pins,!Pins`), no Y reg, interior-NOP branch pad; e.g. `[1:out X,1[6] 2:mov Pins,!Pins[5] 3:jmp X-->5 4:jmp NotY->1[1] 6:mov Pins,!Pins] wrap 1..6` |
| 8    | hand-written seed `dme_spec_ref()` (fixtures.rs, cert-locked by tests) |

Three tracks: **compression** (STOKE-style anneal from certified seed),
**enumeration** (exhaustive small-body sweep), and NEW **SMT synthesis**
(b73939f): PIO semantics mirrored in z3 bitvectors, program words as solver
variables — can return UNSAT proofs SA can't, and side-set costs bits, not
~3^len. `pio_superopt/src/smt.rs`, feature-gated (`--features smt`, system
libz3; default fleet builds unaffected). Mirror is differentially tested
against the emulator (60-case + 2000-case fuzz tiers, mutation-verified).
∃-direction proven end-to-end (`synthesize_len1_toggler`). CEGIS loop
(solver proposes / certifier refutes on example frames) is NOT built yet.

## Paused runs (user resumes in own shell)

- **compress2** (fixed footprint metric, cycle ~50, champ 6, snapshot saved):
  `cd pio_superopt && ./target/release/superopt compress --seed 5EED --trace runs/compress2-0x5eed.jsonl`
- **len-5 fleet sweep**: server here, workers on each box (see docs/fleet.md):
  `./target/release/superopt serve --len 5 --out runs/enum-len5`
  `./target/release/superopt work --server http://<serverbox>:7787`
  Workers: 1st signal drains, 2nd aborts + releases leases (safe daily kill).

## Watch out for

- **SMT UNSAT verdicts are only as good as the mirror** — models get
  certifier-checked, refutations don't. Before trusting any UNSAT: rerun
  `cargo test --release --features smt differential_fuzz -- --ignored`,
  and never extend the modeled subset without extending diff coverage.
- **Metric gaming**: size() is full footprint since 6b41592; treat any
  too-good champion with suspicion first.
- Side-set: emulator-safe (368e499); excluded from *enumeration* for cost
  only. The SMT track includes it from day one.

## Next actions

1. User runs compress2 + len-5 fleet to completion.
2. SMT: build the CEGIS engine (solver ∃ on accumulated frames, certifier
   as ∀-oracle, failing frame → new constraint), then race len-5 vs fleet.
3. RX testbed infra (flagship real-firmware target, dme_pio.rs RX 32/32).
4. Flagged: `algorithm_word` worktree + `gene-v2-ir` branch review-or-delete.
