# 012 stage E2 — Side-field outcome partition

Date: 2026-07-16  
Worktree: `pio_optimization.codex-e2` / branch `codex-e2`

## Result

E2 is implemented and all requested correctness and search gates pass. The
tx_a verdict remains **REFUTED** in every requested bracket. Item counts fall
13.9–62.0% on the four full gates, and the baseline-compatible 0..2 frontier
falls 74.8%.

The source and tests are present in this worktree but are **not committed**.
The sandbox exposes this worktree's actual Git metadata through the main
tree at
`/home/ben/Documents/programmingSync/pio_optimization/.git/worktrees/pio_optimization.codex-e2`,
which is read-only and outside the permitted worktree. Both staging and
committing fail before any index update with:

```text
fatal: Unable to create '.../.git/worktrees/pio_optimization.codex-e2/index.lock': Read-only file system
```

I did not touch the main tree or `../pio_optimization.runtree`, and did not
attempt to bypass that boundary.

## What changed

### Engine mechanism

`pio_superopt/src/narrow/engine.rs` now:

- recognizes an optional value-driving Side demand whose result is a pure
  comparison with the current output latch;
- computes the legal current Side set without admitting optional-enable-clear
  dead value spellings;
- groups `{no side-set, enabled side == current latch}` into one
  `Constraint(slot, side_mask, allowed)` child;
- forks every latch-changing enabled value concretely and in ascending-minimum
  canonical class order;
- leaves delay bits entirely to the existing Delay demand (`0x1800` is the
  tx_a Side mask; `0x0700` is never included);
- re-consults constrained Side fields on every fetched re-arrival;
- snapshots the pre-partition constraint set in the fork frame and charges
  the deciding Side-field consult to that frame itself (S7);
- reuses the E1 cap-three/collapse-oldest policy, set-cond packing, P-subset-S
  probing, frame-open emission/drop rule, mask-only parent propagation,
  champion materialization, and split-seed transport;
- validates Side constraints in seeds against the config-specific legal raw
  set and the partition scope;
- adds `side_partitions` and `side_all_no_change` counters. No new fork kind
  was introduced: children remain attributed to the existing stable `Side`
  index, so `FORK_KIND_NAMES` and `tools/mine_narrow_split.py` did not change.

### Scope guards

E2 always falls back to the existing concrete Side fork for
`side_pindir=true`.

There is one additional load-bearing guard from the emulator's execution
order: the opcode executes before side-set. Comparing side data with the
fork-time latch is therefore valid only if the opcode cannot overwrite a
side pin first. The implementation proves that either:

1. the config's OUT and SET value-latch windows are both disjoint from the
   side window (the tx_a case: both counts are zero), or
2. the slot's opcode/destination is already decided and proves that this
   instruction does not write an overlapping output-latch window.

Otherwise it uses today's concrete forking. This is not a deviation from the
requested rule; it is the required “anything else that makes the outcome not
a pure latch-value compare” guard made explicit.

## Soundness reasoning

### Node-local identity and re-arrival

At a valid E2 node, disabled side-set leaves the output latch unchanged, and
the one enabled spelling equal to the current side-window latch writes the
same bits already present. Their complete post-side-set machine state is
therefore identical. Every other enabled spelling writes a different value
to at least one side latch and remains concrete.

The identity is deliberately not treated as loop-invariant. A grouped field
stays undecided under a constraint, so fetching that slot again necessarily
re-enters E2 before execution. E2 intersects/repartitions the current allowed
set against the latch at that later state. In the red rig the first arrival
creates `{no-side, side=1}` while the latch is high; the loop body drives it
low; the second arrival splits that old class into concrete no-side and
side=1. Only side=1 survives the requested trace.

The same rule applies to split seeds: a seed carrying the first-arrival set is
not trusted as a permanent equivalence. It is re-consulted at search entry and
again after wrap-back.

### Latches in the memo core versus E1's external schedule

`KeyCore` already contains both `out_latch` and `dir_latch`. Consequently, a
failure record made under one latch value cannot match a prober at the same
cycle/control state with a different latch: the core lookup misses before
conditions are considered. E2 therefore needs no new `SC_*` projected-state
bit for the latch read.

That structural fact does **not** remove the need for set-valued program
conditions. A record made in a frame that opened under a constrained Side set
is relative to that spelling set. `close_child` must emit the frame-open set,
and enclosing frames must receive only the consulted mask so that they derive
their own frame-open knowledge. Probe matching remains `P ⊆ S`. This is the
same E1 rule and covers records reached through carried Side constraints even
though latch changes themselves are separated by the core key.

### S7

The Side raw values and concrete latch decide which children represent the
exhaustive outcome partition. The whole Side mask is merged into the
enclosing segment and, at an actual Side partition fork, into the new fork
frame's own consulted set. The latch read is already represented in that
frame's `KeyCore`. Thus a record for the fork cannot generalize over either
the spelling knowledge or the latch that justified grouping killed/collapsed
values.

### Constraint overflow

The cap stays three and overflow still collapses the oldest constraint by
forking its allowed members concretely. Overflow attribution now recognizes
whether that oldest field is Side or WaitIdx; the policy and ordering are
otherwise unchanged. Observed overflow rates were:

- L2 0..1: 32 (0.000169% of items)
- L2 1..1: 32 (0.000172%)
- L3 0..1: 160,548 (0.071754%)
- L3 1..1: 165,808 (0.085446%)

## Red-green micro-specs

Added to `pio_superopt/tests/narrow_soundness.rs`:

1. `side_no_change_grouping_matches_exact_enumeration`
   - latch stays high at every arrival;
   - exact Side searches have verdicts `[survives, refutes, survives]` for
     no-side / side=0 / side=1;
   - grouped search preserves the verdict and a known NOP champion covering
     no-side plus side=1 under allowed raw set `0x00000009`;
   - items drop from 2,015 (sum of exact searches) to 1,333.
2. `side_constraint_reconsults_after_latch_changes_on_wrap`
   - side=1 equals the latch at the first arrival, the loop body drives the
     latch low, and side=1 differs on wrap-back;
   - the correct search preserves the sole side=1 champion with memo off and
     on, and also when started directly from the carried first-arrival Side
     constraint;
   - the deliberately stale minimum/no-side representative produces a
     different trace, so a no-reconsult implementation visibly loses that
     champion.
3. `side_pindir_falls_back_to_concrete_forking`
   - asserts zero E2 partitions and a nonzero ordinary Side fork count in
     PINDIR mode.

The existing `constraint_frame_open_set_conds` canary continues to lock the
generic frame-open set-cond rule used by both E1 and E2.

## Gates

All Cargo commands used `--offline` because this sandbox cannot resolve
crates.io. Cached dependencies were sufficient.

### Full suite

Command:

```text
cargo test --offline --release -- --test-threads=2
```

Result: **PASS**, 115 passed, 0 failed, 25 ignored. The existing
`p1_pruned_reads_must_be_consulted` adversarial test took 205.88s; the suite
completed normally. Memo on/off, split/sequential, instrumentation inertness,
resume/determinism, and the new E2 tests are green.

### tx_a narrowing gates

Runner target was 4096 on the sandbox's 32 reported threads. All runs used
460 cycles, memo cap 2,097,152, champion cap 5, and ended with zero champions
and `cap_hit=false`.

| Length / wrap | E1 baseline items | E2 items | Delta | Delta % | Time | Verdict |
|---|---:|---:|---:|---:|---:|---|
| L2 / 0..1 | 29,055,279 | 18,970,614 | -10,084,665 | -34.7085% | 3.27s | REFUTED |
| L2 / 1..1 | 21,591,943 | 18,586,940 | -3,005,003 | -13.9172% | 3.28s | REFUTED |
| L3 / 0..1 | 571,116,778 | 223,747,402 | -347,369,376 | -60.8228% | 9.90s | REFUTED |
| L3 / 1..1 | 510,896,872 | 194,050,234 | -316,846,638 | -62.0177% | 8.48s | REFUTED |

Artifacts:

- `pio_superopt/runs/e2-l2-01.jsonl` and `.result.json`
- `pio_superopt/runs/e2-l2-11.jsonl` and `.result.json`
- `pio_superopt/runs/e2-l3-01.jsonl` and `.result.json`
- `pio_superopt/runs/e2-l3-11.jsonl` and `.result.json`

### 0..2 frontier

To compare directly with the supplied 28-thread baseline, I used the same
frontier target, 3584:

```text
cargo run --offline --release --bin dump_seeds -- \
  --len 3 --wrap-lo 0 --wrap-hi 2 --target 3584
```

Result: 280,384 units at frontier cycle 2 (196,228 pre-mirror), versus the E1
baseline 1,113,608: **-833,224 units (-74.8220%)**.

## Deliberate differences and open questions

- No fork-kind/stat-name mirror was appended because E2 is a new partition
  policy for the existing `Side` field, not a new field fork kind. This keeps
  the miner's stable indexes unchanged.
- PINDIR partitioning remains deliberately unsupported as requested.
- The only unresolved deliverable is commit creation, blocked by the
  read-only external Git metadata described at the top. No code or gate issue
  remains open.
