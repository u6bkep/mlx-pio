# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-16 (early).

## L=3 LADDER COMPLETE: 6/6 REFUTED — footprint ≤3 IMPOSSIBLE for tx_a

0..2 landed 2026-07-15 23:23 REFUTED (426.66B items, champions=0,
cap_hit=false, all 1,383,452 units settled; run rev 024bb2a clean).
All six wrap brackets proven. Caveat as always: current-fidelity
semantics (post shift-counter fix). **L=4 rediscovery ladder is
unlocked.** No run in flight; box is idle.

## w02 mined (docs/analysis/w02-mining-and-orbits.md)

- 012 prediction slot filled: duplicate CPU 63.4% (predicted "well
  below 71%" — WRONG in spirit). Prologue story dead; mass mutated:
  **delay-only class 18.1%→28.8% (largest)**; the 126-word no-op
  alphabet reappeared IN-LOOP (7×126 orbits, 24.5%) → alphabet is
  loop-invariant; E1 + 013 apply to ALL brackets.
- **CL7 (d≡d+24) now on STALLING waits**: the bracket's two heaviest
  units (517.3M items each, byte-identical) are `wait 1 irq 0` seeds
  24 apart in delay — the 013 v2 shift-absorption shape at the top of
  the cost table. Verification is cheap and unlocks v2's evidence gate.
- Scheduling profile = w12 (top 1% ≈ 91% CPU); recursive-split verdict
  unchanged. Effective-cores metric is bogus on resumed traces.
- Reproduction gotcha: orbit joins for pre-E1 traces need a pre-E1
  dump_seeds (post-E1 master → 1,113,608 units ≠ trace's 1,383,452).

## 012 E1 merged (9b5ded1) — see journal 07-15 for verification detail

L=2 −35.7%/−35.8%, L=3 −18.5%/−19.3%, frontier −19.5%, verdicts
unchanged. Wall-clock magnitude gates now UNBLOCKED (box idle).
008 §3b re-measurement trigger has fired twice.

## Queued (priority order)

1. **013 evidence gate + v1 build** (user-ruled next engine work):
   verify CL7 24-cycle congruence against spec.expected + step the
   emulator on one delay-only group (self-sync) and on the wait-irq
   twins; then v1 (constant-read windows). Delay is 74% of forks and
   delay-shaped classes are ~60% of redundant CPU — top lever.
2. E1 idle-box wall-clock magnitude gates + 008 §3b re-measurement
   (revert 95b1b32 to test) — box is now free; fold into the first
   013 measurement session to amortize setup.
3. L=4 ladder design: deeper frontier cycle (3+) before recursive
   split; decide bracket order; seed-quotient DISCUSSION (one-way
   door: verdict replication across proven-equivalent seeds) —
   equiv() supported_config extension is its proof engine.
4. T1 adversarial rig for tag-blind projection; 012 stage 2 (JMP
   zero-tests); runner header rev-pinning fix (build-time rev).

## Ops rules

Big searches serialized + gated (systemd-run --user, MemoryMax=48G,
MemorySwapMax=0). Magnitude gates = idle-box WALL-CLOCK + items.
`systemctl --user` unreliable from monitor shells — use log mtime.
systemd-run units do NOT inherit shell cwd — pass
`-p WorkingDirectory=`. Runner resume: same command, same rev, same
params. **Runtree convention: long runs LAUNCH FROM and RESUME FROM
`../pio_optimization.runtree`** — checkout the launch rev there, build
there, never commit/build in it mid-run. Details: docs/architecture.md.
Known flaky: lib gene_search hung once at --test-threads=4.
