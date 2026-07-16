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

## 012 E1+E2 merged (9b5ded1, 2089f47) — engine collapses read AND write outcomes

E1: L=2 −35.7%/−35.8%, L=3 −18.5%/−19.3%, frontier −19.5%.
**E2 (Side outcome partition, implemented by Codex gpt-5.6-sol —
user tooling test, one-shot success; docs/analysis/codex-e2-report.md):
on top of E1, L=2 0..1 −34.7% / 1..1 −13.9%; L=3 0..1 −60.8%
(223.7M) / 1..1 −62.0% (194.1M); 0..2 frontier 1,113,608 → 280,384
(−74.8%).** Combined vs pre-E1: L=3 0..1 700.6M → 223.7M (3.13x).
Verdicts unchanged everywhere. Codex found the opcode-before-side-set
ordering hazard itself (overwrite guard). Verified independently:
suite green, smokes byte-identical. Codex ops gotchas: reasoning
effort defaults to NONE (override to high); worktree git metadata is
outside its sandbox (commit on its behalf).

## 008 §3b re-measured (2026-07-16): STILL LOSES — walk chapter closed TERMINALLY

Both standing triggers consumed. Under E1+E2: L=3 wall 2.9–3.0x
worse, 2..2 slice 2.52x fewer units settled; steps/kill improved
(340) but items-saved-per-kill collapsed to 0.72 — E1/E2 ate the
co-refuting redundancy the walk harvested. Port needed 3 drift fixes
(tag-gate sw_fire — S7 canary caught the raw port concrete-walking
011b placeholders; conservative read accounting). Branch measure-3b
kept for provenance. Detail: journal + ticket 008. Bonus: master-side
runs = the deferred E1+E2 idle-box wall gates (L=3 9.5s/8.3s; 2..2
~40min-pace vs 2h55m pre-E1).

## Queued (priority order)

1. **013 v1 ruling** (recut ticket: in-unit Delay wall 74% is the
   remaining named fork mass; v2 mechanism proven, its class dead;
   split-layer evidence void). User call on build vs re-mine first
   (a fresh 2..2 full run is ~40 min post-E2 and would give a
   post-E1+E2 wall census for the recut).
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
