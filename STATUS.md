# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-13.

## L=3 ladder: 3 of 6 REFUTED; monsters still gated on a real lever

0..0 (5.33B items/26min), 0..1 (784.0M/27s), 1..1 (716.3M/24s)
proven. Remaining: 2..2 / 1..2 / 0..2 (all-slots-live monsters).
2..2 stage-2-engine baseline: 27% settled at the 50-min gate, 23.3B
worker items (~3h full).

**Ops rules:** big searches serialized + gated — `systemd-run --user
-p MemoryMax=48G -p MemorySwapMax=0 -p RuntimeMaxSec=3000`. Any run
>50 min is too slow — build the lever instead. `systemctl --user`
checks are unreliable from background monitor shells — watch the run
log's mtime. **Magnitude gates need idle-box WALL-CLOCK, not item
counts alone** (3b's item halving hid a 2.8x slowdown behind a
contended baseline).

## Ticket 008 — stages 1+2 landed; walk chapter CLOSED (3b + 3d reverted)

Stage 1 (12ec1cb): lazy JMP target demand. Stage 2 (89f97c9):
junk-window collapse, 4.4x at L=3; delay-agnostic walk records
(2a4c5fa). **3b (per-pop subtree walk) reverted: items halved,
wall-clock 2.8x worse. 3d (once-per-family record generalization)
reverted: ~zero cost but ZERO conversion** — census families group by
(core, conflict slot+mask) while members differ freely on other
decided bits, so the generalized record's conds (everything the
family-wide walk consulted) match a near-empty sub-family. The 84.5%
cond-miss transferability (probes KEPT: 96f0372/8e69ac8/456f829) is
inherently PER-MEMBER; per-member re-proof is 3b. Full post-mortems:
ticket 008 §3b/§3d. Remaining record-side idea = outcome-class conds
(design B, big surgery) — parked.

## NEW DIRECTION (user, 2026-07-13): static canonicalization program

(1) Champion-rich targets at reachable lengths — solution-dense
L=2-3 specs + existing exact censuses; dump full champion sets, group
by EXTENDED-stimulus fingerprints (same-spec champions coincide on
the spec trace by construction). (2) Pair canonicalization = 009 at
arity 2: mine pair-enumeration fingerprint census + champion families
for schema candidates, prove with the existing z3 mirror (bounded
equivalence, UNSAT = theorem), land within-L sibling-dedup rules
census-gated; length-reducing pair→single+delay = ladder subsumption
(design doc first: wrap/jmp-target shifts). Static rules are blind to
doomed-window equivalences (the wall) but shrink the space
multiplicatively at zero runtime cost and compound toward L=4.
Sequencing proposed to user; 008 stage 4 (ISR_CNT) + Codex review
still queued.

## Emulator fidelity fixed (e4a4860, a810ec5) — holds

MOV→ISR/OSR resets shift counter, OUT→ISR sets it, PUSH IFFULL/PULL
IFEMPTY are guards. NOP_CANON = mov x,x. Old ≤4-word impossibility
proof carries a caveat.

## Next (in expected-value order)

1. **2..2 / monsters under stage 3b** → all-six-refuted ⇒ L=4 ladder.
2. **008 stage 3c/4** + Codex engine review.
3. **Ticket 010 + multi-case specs** (flagship RX prereq).

## Flagship (unchanged): phase-invariant RX

Spec/oracle must quantify duty skew (±8ns measured / ±24 design) ×
parked phase; battery = pio_harness/tests/rx_bench_repro.rs +
ro_sampled fixtures. Engine needs multi-case specs first.

## Shard twin — COMPLETE (2a3a2e7); hand shard_pio/ to Christian

## Bench (idle) / paused

-0 pinger / -1 responder on [3][4][4][4], worktree
Raven-Firmware.single-sm-tx-bench UNPUSHED @350ede86. Paused: SMT
len-4, compress2, len-5 fleet, ticket 006 runner migration.
