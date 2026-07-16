# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-16 (early).

## L=3 LADDER COMPLETE: 6/6 REFUTED — footprint ≤3 IMPOSSIBLE for tx_a

0..2 landed 2026-07-15 23:23 REFUTED (426.66B items, champions=0,
cap_hit=false, all 1,383,452 units settled; run rev 024bb2a clean).
All six wrap brackets proven. Caveat as always: current-fidelity
semantics (post shift-counter fix). **L=4 rediscovery ladder is
unlocked.** No run in flight; box is idle.

## w02 mined + 013 EVIDENCE GATE RUN — CL7 SOLVED, tickets recut

docs/analysis/w02-mining-and-orbits.md (§CORRECTION is authoritative;
evidence bin: pio_superopt/src/bin/evidence013.rs):

- Duplicate CPU 63.4% (prediction "well below 71%" wrong in spirit);
  126-word no-op alphabet is LOOP-INVARIANT (24.5% of dup, 7×126
  orbits); E1's met-now grouping kills its 124 WAIT members.
- **CL7 ≠ phase congruence. `.side_set 1 opt` ⇒ field =
  enable(12)|side(11)|delay(10:8); "+24" = side-1 spelling.** The
  twin monsters = shipped slot-0 `wait 1 irq 0 side 1` vs plain.
  Latch-conditioned known-value write (CL1 family): diverges on 30%
  of random completions; unit-identity via golden-trace conditioning.
- **"Delay-only" class NEVER EXISTED: re-census = side-only 18.16%
  (w12) / 28.90% (w02), true-delay-only 0 groups in both.**
- **013 v2 mechanism CONFIRMED** (stall absorbs delay shifts exactly)
  but its class is dead → ticket 013 RECUT REQUIRED; v1's remaining
  target = in-unit Delay wall (74% of forks), magnitude unknown.
- **New top lever candidate: 012 stage E2 (sketched, NOT ruled on):**
  write-side E1 analog — Side-field outcome partition vs concrete
  latch; reuses E1 substrate verbatim; named mass 18–29% split-layer
  + unmeasured in-unit. AWAITING USER RULING: E2 vs 013 v1 order.
- Scheduling profile = w12 (top 1% ≈ 91% CPU); recursive-split verdict
  unchanged. Effective-cores metric is bogus on resumed traces.
- Reproduction gotcha: orbit joins for pre-E1 traces need a pre-E1
  dump_seeds (post-E1 master → 1,113,608 units ≠ trace's 1,383,452).

## 012 E1 merged (9b5ded1) — see journal 07-15 for verification detail

L=2 −35.7%/−35.8%, L=3 −18.5%/−19.3%, frontier −19.5%, verdicts
unchanged. Wall-clock magnitude gates now UNBLOCKED (box idle).
008 §3b re-measurement trigger has fired twice.

## Queued (priority order)

1. **USER RULING NEEDED: next engine lever = 012 E2 (Side outcome
   partition, sketched in ticket 012) vs 013 v1 (window-cleanliness
   relaxation, evidence recut).** E2 has the named mass and reuses
   E1 machinery; v1 targets the 74% in-unit Delay wall with unknown
   magnitude. Evidence gate is DONE (2026-07-16 journal entry).
2. E1 idle-box wall-clock magnitude gates + 008 §3b re-measurement
   (revert 95b1b32 to test) — box is now free; fold into the next
   measurement session to amortize setup.
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
