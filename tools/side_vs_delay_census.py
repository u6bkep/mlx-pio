#!/usr/bin/env python3
"""Re-census w02 duplicate-group variation with side bits (12:11) separated
from true delay (10:8). Buckets group variation (xor-union over member
seeds, all slots) into side-only / true-delay-only / mixed-field / other.
Usage: side_vs_delay_census.py <trace.jsonl> <seeds.jsonl>"""
import json, sys
from collections import defaultdict

trace, seedsf = sys.argv[1], sys.argv[2]

seeds = {}
with open(seedsf, 'rb') as f:
    for line in f:
        d = json.loads(line)
        seeds[d['unit']] = tuple((e[0], e[1], e[2]) for e in d['seed'])

groups = defaultdict(list)
with open(trace, 'rb') as f:
    f.readline()
    for line in f:
        if line.startswith(b'{"telem"') or line.startswith(b'{"done"'):
            continue
        d = json.loads(line)
        st = d['stats']
        fp = (d['items'], d['refuted'], st['cycles_run'], st['walk_cycles'], st['tags_created'])
        groups[fp].append((d['unit'], d['ms']))

SIDE, DELAY, FIELD = 0x1800, 0x0700, 0x1F00
buckets = defaultdict(lambda: [0, 0])  # name -> [groups, redundant_ms]
total_red = 0
for fp, members in groups.items():
    if len(members) < 2:
        continue
    sum_ms = sum(m for _, m in members)
    red = sum_ms - sum_ms // len(members)  # all-but-one-copy cost
    total_red += red
    ss = [seeds[u] for u, _ in members]
    # comparable only if same shape (slots+decided masks)
    shapes = {tuple((s, dec) for s, dec, _ in sd) for sd in ss}
    if len(shapes) != 1:
        buckets['shape-varies (decided masks differ)'][0] += 1
        buckets['shape-varies (decided masks differ)'][1] += red
        continue
    xu = 0
    base_v = ss[0]
    for sd in ss[1:]:
        for (_, _, v0), (_, _, v1) in zip(base_v, sd):
            xu |= v0 ^ v1
    if xu == 0:
        name = 'identical seeds?!'
    elif xu & ~SIDE == 0:
        name = 'side-only (12:11)'
    elif xu & ~DELAY == 0:
        name = 'TRUE-delay-only (10:8)'
    elif xu & ~FIELD == 0:
        name = 'field-mixed (side+delay)'
    else:
        name = 'other bits involved'
    buckets[name][0] += 1
    buckets[name][1] += red

print(f"total redundant ms: {total_red//1000}s over {sum(1 for m in groups.values() if len(m)>1)} groups")
for name, (g, ms) in sorted(buckets.items(), key=lambda kv: -kv[1][1]):
    print(f"  {ms/total_red*100:6.2f}%  {ms//1000:>8}s  groups={g:<7} {name}")
