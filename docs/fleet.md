# Multi-machine runs (fleet orchestration)

Two long-run kinds exist; they distribute differently. Enumeration fans out
across machines with zero coordination; compression stays on one machine at
a time but migrates freely.

## Enumeration (`superopt enumerate`) — embarrassingly parallel

The unit of work is a **shard** (= first-slot opcode index, `0..alphabet`).
Each completed shard writes `shard-NNNN.json` into `--out` (atomic rename);
a rerun skips shards whose file exists. That file set is the ONLY state —
there is no coordinator, no locks, no partial-progress files.

### Setup (each machine)

```sh
git clone <repo> && cd pio_optimization/pio_superopt
git checkout <REV>          # SAME commit everywhere — see invariants below
cargo build --release
```

Copying the `target/release/superopt` binary to same-arch machines works
too and is the safer way to guarantee the invariants.

### Split the shard space

Give each of `M` machines a residue class. Machine `k` (0-based) runs:

```sh
./target/release/superopt enumerate --len 5 \
    --shard-mod M --shard-rem k \
    --threads <cores> --out runs/enum-len5
```

`--out` is machine-local. Uneven machines? Give a fast box two residues
(run it twice with different `--shard-rem`, sequentially or in parallel
with split `--threads`).

### Collect + aggregate

Copy every machine's shard files into one directory (names never collide —
each shard number is produced by exactly one residue class):

```sh
rsync -av fastbox:pio_optimization/pio_superopt/runs/enum-len5/ runs/enum-len5/
```

Then rerun the plain command on the collecting machine:

```sh
./target/release/superopt enumerate --len 5 --out runs/enum-len5
```

It finds 0 shards to do and prints the aggregate (structures / screened /
pattern-pass / timing evals / SURVIVORS) over all files present. Survivor
details (words, wrap, delays) are inside the shard JSONs.

### Pause / resume / failure

- Ctrl-C (or a crash, or a reboot) any time: completed shards are durable,
  the in-flight shards' work is lost, rerunning the same command redoes
  only those. Shard granularity: ~80 core-seconds at len 4, ~4–5 core-hours
  at len 5 — so at len 5 an interruption costs up to `--threads` ×
  a few hours. Finer sharding (two-op prefix) is the planned fix if this
  bites.
- A machine dying mid-campaign needs no cleanup: reassign its residue to
  any other machine.

### Invariants (the sharp edges)

1. **Same commit everywhere.** The alphabet ORDER is the shard-numbering
   contract (`alphabet_size_is_pinned` test guards the sizes, not the
   order). Mixing binaries from different revs silently mislabels shards.
   Check `git rev-parse HEAD` matches before launching; when in doubt,
   ship one binary to every machine.
2. **One `--len` per `--out` dir.** Shard numbering restarts per len.
3. Every machine must use the same `--len`; `--shard-mod/rem` are the only
   flags that may differ.

### Budget cheat-sheet (measured at len 4, extrapolated)

| len | structures | est. cost | verdict |
|-----|-----------|-----------|---------|
| 4 | 6.6e8 | ~3.5 core-hours | one machine, background |
| 5 | 1.3e11 | ~15 core-days | a weekend on 3–4 boxes |
| 6 | 2.9e13 | ~8 core-years | NOT yet — needs prefix pruning / scaffolds first |

## Compression (`superopt compress`) — single-machine, migratable

State = the trace file (`runs/compress-<seed>.jsonl`); it embeds inline
snapshots and the run header. To migrate:

1. Ctrl-C the run (it snapshots at the next checkpoint and exits).
2. Copy the `.jsonl` to the new machine (same commit/binary — resume is
   byte-identical only against identical code; the header check catches
   parameter drift but NOT code drift).
3. Rerun the exact same command there; it auto-resumes from the last
   snapshot. `--restarts` is baked into the snapshot, so the thread count
   of the anneal does not change with the machine — a bigger box just runs
   the same 32 restarts faster.

Do not run two resumed copies of the same trace concurrently — both would
append to their own copy and the histories fork silently (that fork is how
we deterministically read out the overnight run's fate on 2026-07-05, so
it is a feature — but only on a COPY, never the original path).
