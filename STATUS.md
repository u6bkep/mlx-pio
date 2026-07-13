# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-13 (night).

## SOUNDNESS ALERT (Codex review, 2026-07-13 late night)

One-shot gpt-5.6-sol review found 4 credible memo soundness holes;
**all L=3 verdicts carry a caveat until fixed + re-run**: S1
cross-fork Prov::Field loss (SET-immediate conds omitted, mechanism
confirmed); S2 binding frames recordable with mixed-root proofs; S3
bound records refuting unbound probers at x==y; **S4 decided-delay
walk records — CONFIRMED + FIXED (1544a45)**. S5/S6 = seeded-search
canonicity/mirror holes (resynthesis-track scope, documented). Wave 3
in flight: adversarial micro-spec + fix agents for S1 and S2/S3; then
re-run all L=3 verdicts (0..0 26min + fast brackets). Full review
text in the session transcript; findings being folded into tickets.
Review also cleared: word_canon lemmas 7-11, CntProv local soundness,
core determinism. Perf findings queued: purge-loop O(n²) plateau,
per-unit workspace rebuild in split mode, probe projection caching.

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
