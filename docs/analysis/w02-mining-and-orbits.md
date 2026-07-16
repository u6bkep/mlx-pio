# w02 (L=3, wrap 0..2) trace mining + seed orbits — 2026-07-15

> **CORRECTED 2026-07-16 — read §CORRECTION first.** The "delay-only
> class" and "CL7 d≡d+24 congruence" interpretations below are WRONG:
> under `.side_set 1 opt` the 5-bit field is enable(12)|side(11)|
> delay(10:8), and the re-census with side bits separated shows the
> entire "delay-only" class is SIDE-SPELLING (side-1-vs-none); the
> true-delay-only class has ZERO groups. Measured numbers stand;
> interpretations are superseded by the CORRECTION section.

Companion to `narrow-split-w12-unit-mining.md` and `w12-seed-orbits.md`.
Trace: `/data/pio_optimization/runs/narrow-split-l3-w02.jsonl` (1.09 GB,
complete). Engine rev **024bb2a (pre-E1)** — same engine as w12, so the
two brackets are directly comparable. Seeds re-derived with a pre-E1
`dump_seeds` build (scratch worktree at eb0b801; engine untouched
between 024bb2a and eb0b801); unit count matched the trace exactly
(1,383,452; frontier cycle 2, 1,002,852 pre-mirror).

## Verdict

**REFUTED** — 426.66B items, 372.42B refuted, 814.3M memo hits,
cap_hit=false, champions=0, all 1,383,452 units settled. This was the
last open bracket: **the L=3 ladder is 6/6 REFUTED → footprint ≤3 is
impossible for tx_a** (standing caveat: current-fidelity semantics,
post shift-counter fix). L=4 rediscovery ladder unlocks.

## Headline numbers vs w12

| metric | w12 (1..2) | w02 (0..2) |
|---|---|---|
| items | 363.97B | 426.66B |
| CPU | 271.3 core-h | 292.0 core-h |
| duplicate-CPU fraction | 71.1% | **63.4%** |
| delay-only redundancy class | 18.1% | **28.8%** |
| top-1% units' CPU share | 92.0% | 90.6% |
| largest unit | 1,087s | 1,264s (×2 twins) |
| Delay fork share | 73.1% | 74.1% |

Same violently bimodal skew as w12; largest unit ≈3.4% of an ideal
28-core wall (~10.4h), so the w12 scheduling verdict (recursive unit
split not justified; deeper frontier first at L=4) carries over.
Caveat: "effective cores" is **meaningless for resumed runs** — the
trace holds all units but wall clock covers only the resumed segment
(the naive number came out 105.3 on 28 cores). Use CPU totals.

## The no-prologue prediction: WRONG in spirit (63.4%, not "well below 71%")

w02 has no prologue slot — wrap 0..2 puts every slot in the loop — so
the w12 story ("~87% of redundancy is slot-0 PROLOGUE respelling")
predicted a large drop. It dropped 7.7 points. The mass survived by
changing shape:

1. **s0:delay-only — 28.8% of redundant CPU (1,600 groups; was 18.1%
   in w12).** Now the largest class. Same-seed-shape units differing
   only in slot-0 delay bits doing byte-identical work.
2. **The 126-word no-op alphabet at slot 0 — 24.5% (7 orbit groups of
   exactly 126 units, per-copy ~180–195s).** The SAME alphabet found
   in the w12 prologue (124 satisfied-WAITs + 2 self-moves; xor-or
   spans opcode+delay+op fields), one orbit per representative delay
   value b9xx..bfxx. **The alphabet is loop-invariant, not
   prologue-specific** — these words are no-ops on every loop
   iteration, not just on a once-executed prefix.
3. Mixed delay+op classes (delay+op-hi 13.0%, delay+op-lo 11.3%,
   delay+op-hi+op-lo 5.5%) — delay again, entangled with low-bit
   op respelling.

Virtually all redundancy is still **slot-0 (split-boundary) variation**;
multi-slot classes are noise (<1% combined).

## CL7 (d ≡ d+24) evidence strengthens — now on STALLING waits

The two heaviest units in the whole bracket (581417 and 1273143;
1,264s and 517.3M items EACH, byte-identical fingerprints) are
`wait 1 irq 0`-shaped seeds (v=0x38C0 vs 0x20C0, decided 0xF8FF, low
3 delay bits free) whose decided-high delay bits differ by exactly
**24** — the same congruence w12 flagged on prologue no-ops (CL7,
spec-period, still unverified against spec.expected). This pair is a
*stalling* wait: consistent with the 013 v2 shift-absorption
hypothesis, with the 24-periodicity of the irq/stim schedule selecting
which delays pair. Verify cheaply per queue item (spec.expected
period check + emulator step on one pair).

## Implications (ranked)

1. **013 is now the top lever, and v2 specifically.** The delay-only
   class grew to 28.8% and the CL7 twins put the shift-absorbing-stall
   shape at the very top of the unit cost table. v1 (constant-read
   windows) still applies (pin-idle windows exist in-loop pre-DE).
2. **E1 (already merged, 9b5ded1) attacks class 2 everywhere.** 124 of
   the 126 alphabet members are satisfied WAITs — exactly what met-now
   grouping collapses into one constrained child. This run predates
   E1; future brackets get the collapse for free. The −18..−36% item
   drops already measured on L=2/L=3 gates are this effect.
3. **Scheduling: profile matches w12** (top 1% ≈ 91% of CPU in both;
   largest unit ~3% of ideal wall). The w12 verdict stands: recursive
   unit split not justified at L=3; at L=4 prefer deeper frontier
   cycle first.
4. **Seed quotient before phase 2** remains the general split-layer
   answer (all redundancy is s0-variation); its proof engine is still
   the equiv() supported_config extension (queue #5).

## CORRECTION (2026-07-16) — CL7 solved; the "delay-only" class never existed

Evidence session (bin: `pio_superopt/src/bin/evidence013.rs`; scripts:
`tools/find_delay_pairs.py`, `tools/side_vs_delay_census.py`):

**The field decode.** tx_a is `.side_set 1 opt`: the 5-bit shared
field is enable(bit12) | side(bit11) | delay(bits 10:8). "+24" in that
field is 0b11000 = **side-set enable + side 1**, not delay arithmetic.
The orbit classifier's "delay" region (0x1F00) conflated them.

**CL7 resolved.** The twins are `wait 1 irq 0 side 1` (the shipped
tx_a slot-0 instruction) vs plain `wait 1 irq 0` (encodings confirmed:
0x38C0/0x20C0; side0=0x30C0). The irq schedule has NO 24-cycle period
(gaps 15,15,30,12,8,...). CL7 is a **known-value side-set write
no-op** — DI idles high, `side 1` re-writes 1 — the CL1/CL2
state-conditioned family, not a phase congruence.

**Re-census with side bits separated (both brackets):**

| bucket | w12 | w02 |
|---|---|---|
| side-only (bits 12:11) | **18.16%** | **28.90%** |
| TRUE-delay-only (bits 10:8) | **0 groups** | **0 groups** |
| other bits involved | 71.35% | 58.27% |
| shape-varies | 10.49% | 12.83% |

The old "delay-only" classes (18.1% / 28.8%) match the side-only
buckets to the decimal — they were side-spelling all along. The
heavy-pair scan found 2,969 same-shape pairs in w02, every one with
xor exactly 0x1800; zero true-delay pairs.

**The identity is latch-conditioned, not universal.** Trace
differential over sampled completions: side1-vs-plain diverges on
968/3200 (side0 control: 3200/3200) — `side 1` is invisible only
while the DI latch holds 1; wrap-back after a completion drove DI low
exposes it. Unit-level byte-identity holds anyway because surviving
items must match the golden trace, which pins the latch at every
arrival. The engine knows the latch concretely at fork time, so an
**E1-shaped outcome partition on the Side field** ({no side-set,
side==latch} → one constrained child; {side≠latch} → concrete) is
computable exactly, reusing the landed constraint substrate.

**013 v2 self-sync mechanism: CONFIRMED at the emulator** — with a
stalling WAIT downstream, slot-0 delay shifts d=1..7 produce zero
trace divergence; with a visible write first, all 7 diverge. But the
split-layer class it was hypothesized to explain is gone; v2's value
is now in-unit only, unquantified.

**Revised lever ranking (supersedes "Implications" above):**
1. **Side outcome rule** (write-side E1 analog) — named split-layer
   mass 18–29% of redundant CPU, plus unmeasured in-unit subtree
   duplication (Side forks are only 0.03% of forks but sit high in
   trees, doubling everything beneath them).
2. 013 recut required: v1's split-layer evidence was a side-artifact;
   its remaining target is the in-unit Delay fork wall (74%). v2:
   mechanism proven, motivating class dead.
3. Seed-quotient CL7 lemma = known-value write (CL1 family) — likely
   provable with the existing conditional-lemma machinery, no
   supported_config extension needed for THIS class.

## Fork attribution (sum over units)

Delay 74.07%, BitCount 10.01%, SetData 4.10%, WaitIdx 4.05%,
MovSrc 2.67%, JmpTarget 2.43%, rest <1% each.

## Reproduction

```
python3 tools/mine_narrow_split.py /data/pio_optimization/runs/narrow-split-l3-w02.jsonl
<pre-E1 build>/dump_seeds --len 3 --wrap-lo 0 --wrap-hi 2 > w02_seeds.jsonl
python3 tools/orbit_analysis.py <trace> w02_seeds.jsonl
```
(dump_seeds must be built at a pre-E1 rev — post-E1 master decomposes
this bracket into 1,113,608 units, not the trace's 1,383,452.)
