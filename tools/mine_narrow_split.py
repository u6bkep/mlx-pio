#!/usr/bin/env python3
"""Mine a narrow-split unit trace (runs/narrow-split-*.jsonl).

Usage: python3 tools/mine_narrow_split.py <trace.jsonl>

Prints: run summary, per-unit ms/items percentiles + tail-mass shares,
scheduling verdict inputs (effective cores, max-unit vs wall), top-20
units, aggregate fork-kind attribution (named), and the duplicate-
subtree analysis (stat-fingerprint groups → redundant CPU share).

Read-only; write its stdout OUTSIDE the trace directory (analysis
never goes into runs/). First used on w12 2026-07-15 — findings in
docs/analysis/narrow-split-w12-unit-mining.md.
"""
import json
import sys
from collections import defaultdict

FORK_KIND_NAMES = [  # mirror of engine.rs FORK_KIND_NAMES (index-stable)
    "Side", "Opcode", "JmpCond", "WaitPol", "WaitSrc", "WaitIdx", "InSrc",
    "BitCount", "OutDst", "PushPullBits", "MovDst", "MovOp", "MovSrc",
    "IrqBits", "IrqIdx", "SetDst", "SetData", "Delay", "JmpTarget",
]

path = sys.argv[1]

ms, items, hits, unit_ids = [], [], [], []
fork_kinds_sum = None
groups = defaultdict(lambda: [0, 0])  # fingerprint -> [count, sum_ms]
done = None
n_champ = 0

with open(path, 'rb') as f:
    header = json.loads(f.readline())
    for line in f:
        if line.startswith(b'{"telem"'):
            continue
        if line.startswith(b'{"done"'):
            done = json.loads(line)['done']
            continue
        d = json.loads(line)
        ms.append(d['ms'])
        items.append(d['items'])
        hits.append(d['memo_hits'])
        unit_ids.append(d['unit'])
        n_champ += d['champions_found']
        st = d['stats']
        fp = (d['items'], d['refuted'], st['cycles_run'],
              st['walk_cycles'], st['tags_created'])
        g = groups[fp]
        g[0] += 1
        g[1] += d['ms']
        fk = st['fork_kinds']
        if fork_kinds_sum is None:
            fork_kinds_sum = [0] * len(fk)
        for i, v in enumerate(fk):
            fork_kinds_sum[i] += v

n = len(ms)
print(f"header: {json.dumps(header['narrow_split'], sort_keys=True)}")
print(f"units settled in trace: {n}; champions found: {n_champ}")
if done:
    print(f"done: verdict={done['verdict']} items={done['items']} "
          f"memo_hits={done['memo_hits']} secs={done['secs']:.0f} "
          f"cap_hit={done['cap_hit']}")

def pct(sv, p):
    return sv[min(n - 1, int(p / 100.0 * n))]

for name, vals in (('ms', ms), ('items', items)):
    sv = sorted(vals)
    tot = sum(vals)
    print(f"\n== per-unit {name} == total={tot} mean={tot / n:.1f}")
    for p in (50, 90, 99, 99.9, 99.99):
        print(f"  p{p}: {pct(sv, p)}")
    print(f"  max: {sv[-1]}")
    for share in (0.0001, 0.001, 0.01, 0.10):
        k = max(1, int(n * share))
        print(f"  top {share * 100:g}% ({k} units) hold "
              f"{sum(sv[-k:]) / tot * 100:.2f}% of total")
    print(f"  bottom 50% hold {sum(sv[:n // 2]) / tot * 100:.2f}% of total")

tot_ms = sum(ms)
print(f"\ncpu = {tot_ms / 1000:.0f}s = {tot_ms / 3.6e6:.2f} core-hours")
if done:
    wall = done['secs']
    print(f"wall = {wall:.0f}s; effective cores = {tot_ms / 1000 / wall:.1f}")
    print(f"largest unit = {max(ms) / 1000:.1f}s "
          f"= {max(ms) / 1000 / wall * 100:.3f}% of wall")

print("\ntop 20 units by ms (ms, items, memo_hits, unit_id):")
for i in sorted(range(n), key=lambda i: ms[i], reverse=True)[:20]:
    print(f"  {ms[i]:>8} ms  {items[i]:>12} items  {hits[i]:>8} hits  "
          f"unit {unit_ids[i]}")

print("\nfork-kind attribution (sum over units):")
tot_fk = sum(fork_kinds_sum)
for i, v in sorted(enumerate(fork_kinds_sum), key=lambda kv: -kv[1]):
    if v / tot_fk >= 0.001:
        print(f"  {FORK_KIND_NAMES[i]:<12} {v:>16}  {v / tot_fk * 100:6.2f}%")

print(f"\n== duplicate-subtree analysis == "
      f"(fingerprint = items,refuted,cycles_run,walk_cycles,tags_created)")
dup_ms = 0
dup_units = 0
for fp, (c, sm) in groups.items():
    if c > 1:
        # cost of all copies beyond one, charged at the group mean
        dup_ms += sm - sm / c
        dup_units += c - 1
print(f"distinct fingerprints: {len(groups)} / {n} units; "
      f"redundant copies: {dup_units} units, "
      f"{dup_ms / 1000:.0f}s = {dup_ms / tot_ms * 100:.1f}% of CPU")
sizes = sorted((g[0] for g in groups.values()), reverse=True)
print(f"group sizes: max={sizes[0]} p50={sizes[len(sizes) // 2]}")
print("top 10 groups by total ms:")
for fp, (c, sm) in sorted(groups.items(), key=lambda kv: -kv[1][1])[:10]:
    print(f"  count={c:>7}  sum_ms={sm:>11}  per-copy={sm // c:>8}ms  "
          f"items/copy={fp[0]:>12}")
