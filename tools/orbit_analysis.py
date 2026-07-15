#!/usr/bin/env python3
"""Join narrow-split per-unit fingerprint groups with phase-1 seeds.

Usage: python3 tools/orbit_analysis.py <trace.jsonl> <seeds.jsonl>
(seeds from the `dump_seeds` bin at the trace's engine rev + params).

For each duplicate group: which seed components are constant, which
vary, and in which instruction fields (opcode / delay / operand-hi /
operand-lo, per slot). w12 findings: docs/analysis/w12-seed-orbits.md.
"""
import json
import sys
from collections import defaultdict

TRACE = sys.argv[1]
SEEDS = sys.argv[2]

OPC = ['JMP', 'WAIT', 'IN', 'OUT', 'PUSHPULL', 'MOV', 'IRQ', 'SET']

def fields(mask):
    """Name the field regions a 16-bit mask touches."""
    out = []
    if mask & 0xE000: out.append('opcode')
    if mask & 0x1F00: out.append('delay')
    if mask & 0x00E0: out.append('op-hi')
    if mask & 0x001F: out.append('op-lo')
    return '+'.join(out) if out else 'none'

def dis(decided, value):
    op = OPC[(value >> 13) & 7] if decided & 0xE000 == 0xE000 else '???'
    return f"{op} v={value:04x}/d={decided:04x}"

# unit -> seed (tuple of (slot, decided, value))
seeds = {}
with open(SEEDS, 'rb') as f:
    for line in f:
        d = json.loads(line)
        seeds[d['unit']] = tuple((s, dm, v) for s, dm, v in d['seed'])

groups = defaultdict(list)   # fp -> [unit ids]
ms_of = {}
with open(TRACE, 'rb') as f:
    f.readline()
    for line in f:
        if line.startswith(b'{"telem"') or line.startswith(b'{"done"'):
            continue
        d = json.loads(line)
        st = d['stats']
        fp = (d['items'], d['refuted'], st['cycles_run'], st['walk_cycles'],
              st['tags_created'])
        groups[fp].append(d['unit'])
        ms_of[d['unit']] = d['ms']

def group_shape(units):
    """Classify seed variation across a group's members."""
    ss = [seeds[u] for u in units]
    # seeds may differ in slot-structure; group by (slots, decided-masks)
    struct = defaultdict(list)
    for s in ss:
        struct[tuple((sl, dm) for sl, dm, _ in s)].append(s)
    var_bits = [0, 0, 0]          # OR of value-XOR per slot vs rep
    same_struct = len(struct) == 1
    if same_struct:
        rep = ss[0]
        for s in ss[1:]:
            for (sl, dm, v), (_, _, v0) in zip(s, rep):
                var_bits[sl] |= v ^ v0
    return same_struct, struct, var_bits

# ---- census over all multi-groups, weighted by redundant ms ----
cls_ms = defaultdict(float)
cls_groups = defaultdict(int)
tot_red = 0.0
for fp, units in groups.items():
    c = len(units)
    if c < 2:
        continue
    sm = sum(ms_of[u] for u in units)
    red = sm - sm / c
    tot_red += red
    same_struct, struct, var_bits = group_shape(units)
    if not same_struct:
        # mixed decided-structure: mirror twins / cross-shape orbit
        key = f'mixed-structure ({len(struct)} shapes)'
        if len(struct) == 2 and all(len(v) == c // 2 for v in struct.values()):
            key = 'two-shapes (mirror-like halves)'
        cls_ms[key] += red
        cls_groups[key] += 1
        continue
    key = 'vary: ' + ' | '.join(
        f's{sl}:{fields(var_bits[sl])}' for sl in range(3) if var_bits[sl])
    if not any(var_bits):
        key = 'identical seeds?!'
    cls_ms[key] += red
    cls_groups[key] += 1

print(f"total redundant ms: {tot_red/1000:.0f}s")
print("\n== redundant-CPU census by seed-variation class ==")
for k, v in sorted(cls_ms.items(), key=lambda kv: -kv[1]):
    print(f"  {v/tot_red*100:6.2f}%  {v/1000:>8.0f}s  groups={cls_groups[k]:>6}  {k}")

# ---- detail on top 10 groups by total ms ----
print("\n== top groups by total ms — seed anatomy ==")
top = sorted(groups.items(), key=lambda kv: -sum(ms_of[u] for u in kv[1]))[:10]
for fp, units in top:
    c = len(units)
    sm = sum(ms_of[u] for u in units)
    print(f"\n-- count={c} sum={sm/1000:.0f}s per-copy={sm/c/1000:.1f}s items={fp[0]}")
    same_struct, struct, var_bits = group_shape(units)
    if not same_struct:
        print(f"   {len(struct)} decided-structures:")
        for st_key, members in list(struct.items())[:4]:
            m0 = members[0]
            print(f"     x{len(members)}: " + '; '.join(
                f"s{sl} {dis(dm, v)}" for sl, dm, v in m0))
        continue
    rep = seeds[units[0]]
    print(f"   structure: " + '; '.join(f"s{sl} {dis(dm, v)}" for sl, dm, v in rep))
    for sl in range(3):
        if var_bits[sl]:
            vals = sorted({s[i][2] for s in (seeds[u] for u in units)
                           for i in range(len(s)) if s[i][0] == sl})
            print(f"   slot {sl} varies in {fields(var_bits[sl])} "
                  f"(xor-or={var_bits[sl]:04x}); {len(vals)} distinct values: "
                  + ' '.join(f'{v:04x}' for v in vals[:20])
                  + (' ...' if len(vals) > 20 else ''))
    const = [sl for sl in range(3)
             if any(s[0] == sl for s in rep) and not var_bits[sl]]
    if const:
        print(f"   constant slots: {const}")
