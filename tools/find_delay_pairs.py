#!/usr/bin/env python3
"""Find w02 fingerprint-group unit pairs whose seeds differ ONLY in s0
TRUE-delay bits (10:8), vs pairs differing ONLY in side bits (12:11).
Usage: find_delay_pairs.py <trace.jsonl> <seeds.jsonl>"""
import json, sys
from collections import defaultdict

trace, seedsf = sys.argv[1], sys.argv[2]

seeds = {}
with open(seedsf, 'rb') as f:
    for line in f:
        d = json.loads(line)
        s = d['seed']
        if len(s) == 1 and s[0][0] == 0:  # single-slot-0 seeds only
            seeds[d['unit']] = (s[0][1], s[0][2])  # (decided, value)

groups = defaultdict(list)
with open(trace, 'rb') as f:
    f.readline()
    for line in f:
        if line.startswith(b'{"telem"') or line.startswith(b'{"done"'):
            continue
        d = json.loads(line)
        if d['items'] < 1000:  # skip trivial groups
            continue
        st = d['stats']
        fp = (d['items'], d['refuted'], st['cycles_run'], st['walk_cycles'], st['tags_created'])
        groups[fp].append((d['unit'], d['ms']))

TRUE_DELAY = 0x0700
SIDE = 0x1800
delay_pairs, side_pairs = [], []
for fp, members in groups.items():
    if len(members) < 2:
        continue
    ms = [(u, m) for u, m in members if u in seeds]
    for i in range(len(ms)):
        for j in range(i + 1, len(ms)):
            (u1, m1), (u2, m2) = ms[i], ms[j]
            d1, v1 = seeds[u1]
            d2, v2 = seeds[u2]
            if d1 != d2:
                continue
            x = v1 ^ v2
            if x and (x & ~TRUE_DELAY) == 0:
                delay_pairs.append((fp[0], m1, u1, d1, v1, u2, v2))
            elif x and (x & ~SIDE) == 0:
                side_pairs.append((fp[0], m1, u1, d1, v1, u2, v2))
        if len(delay_pairs) > 400 and len(side_pairs) > 400:
            break

for name, pairs in [("TRUE-DELAY-only (bits 10:8)", delay_pairs), ("SIDE-only (bits 12:11)", side_pairs)]:
    pairs.sort(key=lambda p: -p[1])
    print(f"== {name}: {len(pairs)} pairs (heaviest first) ==")
    for p in pairs[:8]:
        items, ms, u1, dec, v1, u2, v2 = p
        print(f"  items={items:>12} ms={ms:>8} units {u1}/{u2} decided={dec:04x} v1={v1:04x} v2={v2:04x} xor={v1^v2:04x}")
    print()
