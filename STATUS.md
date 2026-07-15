# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-15 (evening).

## L=3 ladder: 5 of 6 PROVEN — only 0..2 still running

**1..2 REFUTED** (2026-07-15, 9.7h): 363.97B items, 657.1M memo hits,
cap_hit=false, all 1,383,452 units settled. 0..0, 0..1, 1..1, 2..2
already proven. **If 0..2 refutes: footprint ≤3 impossible for tx_a;
L=4 rediscovery ladder unlocks.**

## RUN LIVE: 0..2 monster (unit `pio-l3-monsters`, detached, resumable)

Started ~12:20 after 1..2 finished; ~49% settled at 17:26, ETA
~22:30–23:00. Log `/data/pio_optimization/runs/l3_monsters.log` (check
mtime); trace `narrow-split-l3-w02.jsonl`. **RESUME CAVEAT: the header
pins runtime HEAD (024bb2a) + dirty flag — this session's doc commits
break a naive resume. If resume is needed:**
`git stash && git checkout 024bb2a`, rerun the exact command (handoff
doc §1), then return to master. Do NOT rebuild the binary.

## w12 trace mined (docs/analysis/narrow-split-w12-unit-mining.md)

- **Recursive-unit-split driver: NOT justified** — 28.0/28 effective
  cores, max unit 3.1% of wall. At L=4 prefer deeper frontier cycle.
- **71.1% of CPU was byte-identical repeat subtrees** (6,925 distinct
  stat fingerprints / 1.38M units; seven 126-copy orbits @122s = 11%
  of the run). Sound exploitation = seed-level quotient via the
  equiv()/rule-library track — unit-granularity canonicalization is
  now the top measured lever (3.5x ceiling on this workload).
- Fork attribution: Delay 73.1%, BitCount 11.8%, WaitIdx 4.8%.
- Unit-keying soundness checked: identical unit counts across w12/w02
  are benign (wrap-back invisible on a 2-cycle prefix; units re-run
  the full spec with true wrap).

## Queued (priority order)

1. **Bank 0..2 verdict when it lands** (~22:30); then STATUS/journal/
   memory + ladder close-out. Mine its trace:
   `python3 tools/mine_narrow_split.py <trace>` (stdout only, never
   into runs/).
2. Seed-orbit identification: re-derive phase-1 seeds (deterministic),
   join on unit index, name what the 126-orbits respell → feeds the
   unit-quotient design.
3. T1 open follow-up: adversarial rig for tag-blind projection.
4. 012 stage 1 (JMP zero-test predicates on tags).
5. Mirror config-coverage extension for equiv(); then rule library.
6. 008 §3b re-measurement (trigger fired); sequential instrumented
   1..2 slice; ≤4 impossibility re-proof via equiv; runner header
   rev-pinning fix (build-time rev, see mining doc §5).

## Ops rules

Big searches serialized + gated (systemd-run --user, MemoryMax=48G,
MemorySwapMax=0). Magnitude gates = idle-box WALL-CLOCK + items.
`systemctl --user` unreliable from monitor shells — use log mtime.
Runner resume: same command, same rev, same params.
