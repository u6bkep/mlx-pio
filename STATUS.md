# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-15 (~3am).
> **Session handoff (read first): `docs/handoff-2026-07-15.md`** — overnight
> run details, resume commands, queued decisions.

## OVERNIGHT RUN LIVE: unit `pio-l3-monsters` (detached, 24h cap, RESUMABLE)

Brackets 1..2 then 0..2 via the new `narrow-split` runner subcommand.
Log: `/data/pio_optimization/runs/l3_monsters.log`; unit-level JSONL
traces `narrow-split-l3-w12.jsonl` / `-w02.jsonl` alongside. If it
dies, rerun the same command (in handoff doc) — it resumes, skipping
settled units. Do NOT rebuild the binary before resuming (trace header
pins the engine rev).

## L=3 ladder: 4 of 6 proven; monsters running on a much faster engine

0..0, 0..1 (700.56M/29s post-tags), 1..1 (632.8M/25s), 2..2
(132.0B/2h55m, pre-tags engine) REFUTED. Tonight's engine gained:
011(b) x/y Field tags (−10.3% items at L=3, wall-neutral), S2
relaxation (memo hits 1.366%→1.938% on the 2..2 mine — the −41%
poisoning fully recovered), S7 soundness fix. All proven-bracket
verdicts re-held on the launch tree.

## Landed tonight (all gated, all merged to master)

- **011(a)+(b)**: OSR-count CntProv twin; x/y Field(slot,mask) tags,
  die-on-transform. Ticket 011 re-cut (stops at (b); Fn1 → 012 stage 4).
- **S7** (new soundness finding from the S2 review): P1 prune + pin
  pre-filter didn't charge killed words' reads to the FORK frame's own
  record (segment-only was insufficient — deviation from review doc
  documented in commit). Red-green canary in narrow_soundness.rs
  (NOTE: fast suite now ~5 min because of it).
- **S2 relaxation**: binding frames record again; upward poisoning
  narrowed to exact conflict-poison. Rec::bound/S5/S6 untouched.
- **012 ticket** (outcome-predicate reads design) + **stage-0 census**
  (PIO_NARROW_PRED_CENSUS=1, sequential only): first table says 2-way
  predicate kinds = 5.4% of collapses at 16x; in-sub 51% at 1.1x;
  ctr-thresh/out-pinvis have no surface until OSR tags (stage 4).
- **Proof engine Layer 1**: `equiv()` driver (smt/equiv.rs) — 3-valued,
  ∀-inputs (symbolic FIFO words+occupancy), preconditions, loose/strict
  tiers, counterexample replay on the real emulator. 2,144 word_canon
  pairs PROVEN universal; CL1/CL2 conditional lemmas proven; 0 mirror
  divergences. Config coverage (not opcodes) is the binding limit.
- **Runner `narrow-split`**: unit-level resumable bracket searches,
  per-unit telemetry JSONL, byte-identical to the test path.
- **Tiny-champion eyeball** (docs/analysis/): toggler+bit-copier full
  champion dumps; candidate lemmas CL1-CL6; multi-seed + 2x horizon
  both caught real impostors.

## Queued (priority order — see handoff doc for detail)

1. Bank overnight verdicts; per-unit tail analysis from the traces.
2. T1 open follow-up: adversarial rig for tag-blind projection
   (011(b) agent couldn't make it red; invariant shipped defensively).
3. 012 stage 1 (JMP zero-test predicates on tags).
4. Mirror config-coverage extension (68.7K battery-only pairs blocked
   on supported_config, only 17.5K on opcodes); then rule library.
5. 008 §3b re-measurement trigger FIRED (evaluator-adjacent cost
   changed tonight) — re-test walk economics when convenient.
6. Sequential instrumented 1..2 slice (pred-census + near-miss probe
   on a monster wall); ≤4 impossibility re-proof via equiv.

## Ops rules

Big searches serialized + gated (systemd-run --user, MemoryMax=48G,
MemorySwapMax=0). Magnitude gates = idle-box WALL-CLOCK + items.
`systemctl --user` unreliable from monitor shells — use log mtime.
Runner resume: same command, same rev, same params.

## Shard twin — COMPLETE (2a3a2e7); bench/paused items unchanged
