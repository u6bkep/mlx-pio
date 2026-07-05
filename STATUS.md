# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-05 (eve).

## Scoreboard (DME TX under the spec oracle + certifier)

| size | status |
|------|--------|
| ≤4   | **PROVEN IMPOSSIBLE** — enumeration, alphabet v2 incl. NOP landing pads (672M structures, 12.3B timing evals, `runs/enum-len4-v2/`) |
| 5    | OPEN — decided by the len-5 fleet sweep (~15 core-days) or a lucky anneal |
| 6    | **EXISTS, CERTIFIED, twice independently** — pin-as-state (`mov Pins,!Pins`), no Y reg, interior-NOP branch pad; e.g. `[1:out X,1[6] 2:mov Pins,!Pins[5] 3:jmp X-->5 4:jmp NotY->1[1] 6:mov Pins,!Pins] wrap 1..6` |
| 8    | hand-written seed `dme_spec_ref()` (fixtures.rs, cert-locked by tests) |

Strategy pivot (this morning, after the 6th oracle-and-autopull-independent
L=6 stall killed from-scratch synthesis): **compression** (STOKE-style anneal
from a certified seed, champion moves only through the certifier) +
**enumeration** (exhaustive small-body sweep; structure/timing decomposition
— delays factor out because nothing in the alphabet stalls mid-frame).

## Paused runs (user resumes in own shell)

- **compress2** (fixed footprint metric, cycle ~50, champ 6, snapshot saved):
  `cd pio_superopt && ./target/release/superopt compress --seed 5EED --trace runs/compress2-0x5eed.jsonl`
- **len-5 fleet sweep**: server here, workers on each box (see docs/fleet.md):
  `./target/release/superopt serve --len 5 --out runs/enum-len5`
  `./target/release/superopt work --server http://<serverbox>:7787`
  Workers: 1st signal drains, 2nd aborts + releases leases (safe daily kill).

## Watch out for

- **Metric gaming**: the anneal exploited occupied-span size (`wrap`/`jmp`
  into empty slots = hidden NOPs); size() is now full footprint (6b41592).
  Treat any future too-good champion with suspicion first.
- Side-set: emulator bug FIXED long ago (368e499) — excluded from
  enumeration only for cost (~3^len). **Option on the table: restart
  compression with side-set enabled** (cheap probe of the side-set world;
  pico-style encoders shave toggles that way).

## Next actions

1. User runs compress2 + len-5 fleet to completion.
2. Decide: side-set-enabled compression run (new trace, new space).
3. RX testbed infra (flagship real-firmware target, dme_pio.rs RX 32/32):
   RunSpec pin-stimulus + RX FIFO capture + golden-equivalence battery.
4. Flagged: `algorithm_word` worktree + `gene-v2-ir` branch review-or-delete.
