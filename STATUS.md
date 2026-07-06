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
**enumeration** (exhaustive small-body sweep), and **SMT synthesis**
(b73939f mirror + 40bb822 CEGIS): PIO semantics mirrored in z3 bitvectors,
program words as solver variables — returns UNSAT proofs SA can't, and
side-set costs bits, not ~3^len. `pio_superopt/src/smt/`, feature-gated
(`--features smt`, system libz3; default fleet builds unaffected). Mirror
differentially tested (60-case + 2000-case fuzz, mutation-verified). CEGIS
loop WORKS end-to-end: solver proposes on accumulated frames, real
emulator+certifier battery refutes (32 singles, 1024 pairs, both corpora,
16 random streams), divergence guard aborts if the two worlds disagree.
Found = battery-certified (mirror-independent). Hole-refill test re-derives
a freed seed slot in 2 iters (~15 s; once found the novel `mov Y, Pins`
loopback alternative). SSA interning (`unroll_interned`) was 60x on solve
time. Runner: `superopt smt-synth --len N [--side …]` (not resumable).
CEGIS subset ⊋ enumeration alphabet (adds PULL, all delays, out counts
to 32) — so a len-4 UNSAT independently corroborates the enumeration proof,
and a len-4 SAT would be a real discovery.

**SMT perf frontier (the open problem):** full-free len-4 iters took 16 min
(iter 2) then 6+ h (iter 3, killed) on the default solver. Levers added
d22dad5 (QF_BV logic, z3 parallel mode, pin-write pruning); probe restarted
2026-07-06 eve, running detached: `pio_superopt/runs/smt-synth-len4-none.log`
(check `tail` + `ps -C superopt`; NOT resumable — a rerun starts over).
If still crawling, next levers: diverse seed examples, shorter frame
windows (phi_max), structure constraints (canonical slot ordering), or
bitwuzla via SMT-LIB dump.

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
2. SMT: finish the len-4 cross-validation probe (`runs/smt-synth-len4-*.jsonl`),
   then len-5 (race vs fleet), then len-5/6 WITH side-set (`--side 1`).
3. RX testbed infra (flagship real-firmware target, dme_pio.rs RX 32/32).
4. Flagged: `algorithm_word` worktree + `gene-v2-ir` branch review-or-delete.
