# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-15 (evening).

## L=3 ladder: 5 of 6 PROVEN — only 0..2 still running

**1..2 REFUTED** (2026-07-15, 9.7h): 363.97B items, 657.1M memo hits,
cap_hit=false, all 1,383,452 units settled. 0..0, 0..1, 1..1, 2..2
already proven. **If 0..2 refutes: footprint ≤3 impossible for tx_a;
L=4 rediscovery ladder unlocks.**

## RUN RESUMED: 0..2 monster (unit `pio-l3-w02-resume2`), ETA ~00:40

Paused 75.5%→resumed ~22:10 from the runtree (launch binary, header
accepted at 1,043,952 settled). systemd-run needs
`-p WorkingDirectory=<runtree>/pio_superopt` (units do NOT inherit
shell cwd — the first resume attempt failed on that). Old engine on
purpose; do NOT rebuild the runtree binary.

## 012 STAGE E1 LANDED + MERGED (9b5ded1) — engine now constraint-capable

Met-now WAIT grouping + full value-set constraint substrate (Item/
Champion/seeds/memo set-conds with P⊆S probing/frame-open drop rule/
junk_walk contract). Independently verified after agent handoff:
full suite green (77+2+6+16+11), L=2 smokes byte-identical
(0..1 29,055,279 / 1..1 21,591,943, −35.7%/−35.8%), L=3 gates
0..1 700.56M→571.12M (−18.5%, 25s) / 1..1 632.8M→510.90M (−19.3%,
21s), verdicts unchanged everywhere, frontier −19.5%. Red-green
S1-analog canary pinned (31/32 champions lost under the wrong
variant). irq-absence puzzle RESOLVED with measurement (jmp-back
re-execution; see ticket 012). NOTE: 008 §3b re-measurement trigger
fired AGAIN (partition adds ≤32 still_stalled calls per WaitIdx
demand). Known flaky: lib gene_search hung once at --test-threads=4.

## w12 trace mined (docs/analysis/narrow-split-w12-unit-mining.md)

- **Recursive-unit-split driver: NOT justified** — 28.0/28 effective
  cores, max unit 3.1% of wall. At L=4 prefer deeper frontier cycle.
- **71.1% of CPU was byte-identical repeat subtrees**; seed-orbit
  analysis (docs/analysis/w12-seed-orbits.md) NAMED it: **~87% of the
  redundant mass is slot-0 PROLOGUE respelling** — satisfied-WAIT
  no-ops (CL3, config-exact: pin-idle map + DE + irq stim all
  visible in the exclusions), self-moves (CL1/CL6), and a d ≡ d+24
  delay congruence (new CL7, spec-period, unverified). Lever = seed
  quotient before phase 2 via proven rules; CL3 needs equiv()
  supported_config extension (tx_a blocked on it → raises queue #5).
- Fork attribution: Delay 73.1%, BitCount 11.8%, WaitIdx 4.8%.
- Unit-keying soundness checked: identical unit counts across w12/w02
  are benign (wrap-back invisible on a 2-cycle prefix; units re-run
  the full spec with true wrap).

## Queued (priority order)

1. **Bank 0..2 verdict when it lands** (~22:30); then STATUS/journal/
   memory + ladder close-out. Mine its trace:
   `python3 tools/mine_narrow_split.py <trace>` (stdout only, never
   into runs/).
2. DONE (seed orbits named — see above). Follow-ups: verify the 24-
   cycle congruence against spec.expected (cheap); confirm self-sync
   mechanism on one delay-only group; then DISCUSS seed-quotient
   lever design (one-way door: verdict replication across proven-
   equivalent seeds).
3. T1 open follow-up: adversarial rig for tag-blind projection.
4. 012 stage 1 (JMP zero-test predicates on tags).
5. Mirror config-coverage extension for equiv() — now doubly
   motivated (CL3-with-preconditions on tx_a config is the seed-
   quotient's proof engine); then rule library.
6. 008 §3b re-measurement (trigger fired); sequential instrumented
   1..2 slice; ≤4 impossibility re-proof via equiv; runner header
   rev-pinning fix (build-time rev, see mining doc §5).

## Ops rules

Big searches serialized + gated (systemd-run --user, MemoryMax=48G,
MemorySwapMax=0). Magnitude gates = idle-box WALL-CLOCK + items.
`systemctl --user` unreliable from monitor shells — use log mtime.
Runner resume: same command, same rev, same params.
**Runtree convention (2026-07-15): long runs LAUNCH FROM and RESUME
FROM `../pio_optimization.runtree`** — before each launch, `git -C
../pio_optimization.runtree checkout <launch rev>` and build (or copy)
the binary there; the run's resume identity (runtime HEAD + dirty) is
then pinned to the runtree, and master/main tree stay free. Never
commit/build in the runtree mid-run. Details: docs/architecture.md.
