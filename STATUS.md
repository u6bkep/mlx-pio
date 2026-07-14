# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-13 (night).

## SOUNDNESS: all 6 findings resolved; L=3 ladder RE-CERTIFIED

Codex review (gpt-5.6-sol, one-shot) found 6 holes; all now fixed and
merged with red-green adversarial micro-specs (gate suite 12 -> 17
tests incl. new narrow_soundness.rs): S4 decided-delay walk records
(1544a45, confirmed by re-derivation); S1 cross-fork Prov::Field loss
(RED: memo-off 8 champions vs memo-on 0; flush_prov at all segment
ends); S3 bound-record-refutes-unbound-prober (RED: champion vanished
memo-on; Rec::bound flag, -3% hits); S2 binding-frame mixed-root
records (confirmed flow-gap, REFUTED-as-exploitable by an
identity-completeness argument — 4 blocked constructions — but fixed
conservatively anyway: binding frames unrecordable + upward
recordability poisoning, **-41% memo hits on 2..2 — REVIEWABLE
RELAXATION CANDIDATE after verdicts**, argument documented in the S2/S3
merge + test comments); S5/S6 seeded-search guards (P2/P4 off in
seeded slots; validate_seed whole-field check; 3 RED micro-specs).
Purge plateau hardened (bounded multi-pass, provable cap). Early
re-verify pre-S2/S3: 0..1 764.9M/29s, 1..1 698.4M/26s, verdicts hold.
**Verdict campaign result: ALL THREE proven brackets RE-REFUTE on the
repaired memo — 0..0 560.2M items/94s (was 5.33B/26min pre-stage-2:
9.5x items, 17x wall from the day's compounded levers), 0..1
781.9M/57s, 1..1 704.8M/38s. CAVEAT LIFTED for the proven brackets.**

**OVERNIGHT RUN (2026-07-13→14): unit pio-overnight, detached, 10h
cap, runs/overnight_monsters.log — all five rest brackets in order
(0..1, 1..1, then MONSTERS 2..2, 1..2, 0..2) on the repaired engine.
If all refute: footprint <=3 impossible, L=4 ladder unlocks. Check
this log first next session.**

## Superposition (ticket 011) — v1 scope NEEDS RE-CUT after census

Dead-demand census (merged ff6a4b3): **BitCount (28.6% of fork mass)
is ~0% dead** — shift chains re-read within cycles; dead-demand
deletion gets nothing there (needs 008-B outcome-predicate reads).
Collapsible = MOV->reg copies (75% dead x ~15.4% mass ~= 11.5%) +
SetData (~100% dead, tiny). Amend ticket 011 v1 before building:
target = MOV/SetData class; BitCount + the 86% state-div wall class
need outcome-predicate read semantics (008-B), not laziness. The
suspicious exact 0.7500 MovSrc dead fraction needs a structural look.

## Headline: realness tests point the monsters at DATA-PLANE SUPERPOSITION

Two measurements on the 2..2 wall (probes 2acc442, journal "night"):
(1) **86.0%** of single-conflict cond-misses are output-equal but
STATE-divergent (capture divergence 52/19.7M) — the wall is dead-state
divergence downstream of data-plane forks; conditioned word-interchange
lemmas cap at **13.3%**. (2) Fork attribution: Delay 44% (stage-2
residue), **BitCount 28.6%, MovSrc 15.2%** — data-plane fields carry
~35-45% of fork mass. **Next flagship: provenance-tag symbols for
x/y/ISR/OSR/counters** (design → staged build; subsumes 008-original
outcome-grouping; enables multi-case specs → RX flagship). Control
plane (pc/delay/stall/latches/clkdiv) stays concrete.

## Working mode (user, 2026-07-13): parallel worktree waves

Agents implement in isolated worktrees (specs + fast gates); merges
serialize through review + idle-box verdict runs. Wave 1: pair census
MERGED (47.2M canon-rep pairs → 684K strict fingerprints, ~69:1;
99.68% mass cross-spelling — champion/L=4-side prize); 008 stage 4
MERGED (912cb77: ISR_CNT CntProv provenance — bracket-neutral,
−26% memo entries, first instance of the superposition tag pattern);
009 gap check + data-driven serializer battery entries in flight.

## L=3 ladder: 3 of 6 REFUTED; monsters await superposition

0..0 (5.33B/26min), 0..1 (782.9M/29s), 1..1 (716.5M/26s, post-stage-4
counts) proven. Monsters 2..2/1..2/0..2: 2..2 = 27% settled at the
50-min gate on the stage-2 engine (~3h full).

## Ticket 008: stages 1,2,4 landed; walk chapter closed (3b/3d
reverted, post-mortems in ticket; 3b has USER-DIRECTED re-evaluation
triggers: larger L, or any evaluator cycle-optimization pass).

## Champion-family census (canonicalization program, step 1 done)

Solution-dense battery: champion sets collapse ~250,000:1 (L=3);
input variation splits only ~0.3% (output-only specs — serializer
entries in flight); strict (+final state) tier still leaves 62K-71K
member lemma-grade families. Readable conditioned lemmas surfaced
(pin-writer spellings; jmp !x ≡ always @x==0; OUT count saturation).
Dumps: runs/champ_mine.jsonl, runs/pair_census.jsonl.

## Ops rules

Big searches serialized + gated (`systemd-run --user -p MemoryMax=48G
-p MemorySwapMax=0 -p RuntimeMaxSec=3000`); >50 min = too slow, build
the lever. Magnitude gates = idle-box WALL-CLOCK + item counts (±2s
bracket noise observed; byte-identical code measured 24-49s across
box states). `systemctl --user` unreliable from monitor shells — use
log mtime. Worktree agents: smokes ≤150s single-threaded only.

## Queued

Superposition design doc + staged build (flagship); Codex one-shot
engine review (gpt-5.6-sol, single message, after wave 1 settles);
ladder-subsumption design doc (length-reducing pair rules); monsters
verdict once superposition lands. Then ticket 010 / multi-case specs
→ phase-invariant RX.

## Shard twin — COMPLETE (2a3a2e7); hand shard_pio/ to Christian

## Bench (idle) / paused

-0 pinger / -1 responder on [3][4][4][4], worktree
Raven-Firmware.single-sm-tx-bench UNPUSHED @350ede86. Paused: SMT
len-4, compress2, len-5 fleet, ticket 006 runner migration.
