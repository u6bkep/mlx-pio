# Tiny-spec champion mining for direct eyeballing

**Date:** 2026-07-14
**Test:** `tiny_champion_eyeball` in `pio_superopt/tests/narrow_engine.rs`
(`cargo test --release --test narrow_engine -- --ignored tiny_champion_eyeball --nocapture`)
**Total runtime:** ~4.5 s (largest single search 4.2 s) — far under budget.

Purpose: champion sets small enough to read *in full*, as raw material for
canonicalization lemmas. Prior censuses were too big to eyeball, and mining
by proxy (counts, histograms) loses the structure. Two tiny specs, each
searched at its minimal length L\* and at L\*+1, full champion enumeration
(no cap hit anywhere), every champion re-verified by the concrete runner.

## Common setup

- Config: 1-pin machine — `set_base=0/set_count=1`, `out_base=0/out_count=1`,
  `in_base=0` (the IN/MOV-src-PINS window reads pin 0 back, i.e. loopback),
  no side-set, clkdiv 1. Wrap: `(0,0)` for L=1, `(0,1)` for L=2 (only the
  natural bracket at L=2; brackets that make slot 1 reachable only via JMP
  were not run).
- Machine reset contract (matters for reading the families): **the pin value
  latch idles ALL-ONES** (pin 0 drives HIGH before anything writes it),
  `x = y = isr = osr = 0`, `isr_count = 0`, `osr_count = 32` (OSR empty).
- Champion notation: `value/decided` per slot in hex; `*` in the
  disassembly = don't-care field (never consulted on any surviving path);
  `bf=true` = binding-free, the X/Y register-mirror of the program is also
  a certified solution (so Y-spelled twins are represented, not missing).
- **Family tiers.** STRICT = identical pin trace + identical out-of-footprint
  flag + identical **final** machine state, across all four FIFO contexts,
  over a **2x horizon** (twice the spec trace — same-spec champions coincide
  on the spec trace by construction). LOOSE = traces + OOB only. Caveat:
  STRICT compares the *final* state, not stepwise state — members may differ
  in transient register values mid-run (flagged below where it happens).
- Fingerprint FIFO contexts: `[]` (empty),
  `[D3, 4A, FF, 00]` (the search payload),
  `[AAAA5555, 00000000, FFFFFFFF, 123456B1]`,
  `[00000001, 80000000, 55555555, CAFEF00D]`.

---

## Spec 1: TOGGLER (output-only, secondary)

Pin 0 toggles every cycle. Reference: `mov pins, !pins` (0xA008), wrap
(0,0) — the loopback trick: read the pin, invert, write it back. (PIO
cannot read the *output latch*; it can read the pin *level* through the
input mapping, which is the driven value here.) 17 cycles, no FIFO input.

Expected capture (pin 0, cycle 0 first): `01010101010101010`
(the pin idles HIGH pre-run, so the first executed write produces 0).

### L\* = 1 (wrap 0..0, 17 cycles): 1 champion, 1 strict family

```
[a008/ffff]  mov pins, !pins   bf=true
```

Fully decided including delay=0 (any delay breaks the every-cycle cadence).
The earlier exact census (`mov_toggle_l1_census_exact`) confirms this is the
only word in the whole 65,536-word space that reproduces the trace. At L=1
the space is already canonical — nothing to learn.

### L\*+1 = 2 (wrap 0..1, 17 cycles): 260 champions, **1 strict family** (also 1 loose)

Search: 12,417 items, <0.1 s. All 260 champions are strictly equivalent —
same trace, same final state, every context. The full set is an (almost)
clean cross product **{slot 0: "write 0 to pin 0"} x {slot 1: "write 1 to
pin 0"}**, all with delay 0 everywhere:

Slot 0 alphabet — 43 spellings of "drive pin 0 low this cycle":

| class | words | count |
|---|---|---|
| SET immediate | `set pins, 0` (e000) | 1 |
| MOV of a known-zero value | `mov pins, {x, null, isr, osr}` (a001, a003, a006, a007) | 4 |
| MOV bit-reverse of known-MSB-zero value | `mov pins, ::{pins, x, null, isr, osr}` (a010, a011, a013, a016, a017) | 5 |
| MOV invert of pin (reads 1, writes 0) | `mov pins, !pins` (a008) | 1 |
| OUT from a zero OSR | `out pins, N` for **every** N in 1..=32 (6000..601f) | 32 |

Slot 1 alphabet — 7 spellings of "drive pin 0 high this cycle":

| class | words | count |
|---|---|---|
| SET immediate | `set pins, 1` (e001) | 1 |
| MOV invert of known-zero | `mov pins, !{x, null, isr, osr}` (a009, a00b, a00e, a00f) | 4 |
| MOV invert of pin (reads 0, writes 1) | `mov pins, !pins` (a008) | 1 |
| MOV invert of Y | `mov pins, !y` (a00a) | (see below) |

260 = 43 x 6 + 2: `mov pins, !y` appears only behind the two slot-0 words
that name X (`mov pins, x`, `mov pins, ::x`) — the binding order (X must be
named first) admits a Y-spelling only after X is bound; everywhere else the
!y spelling is represented by its bf mirror `!x`.

What varies inside the family (= what the current quotient does NOT
collapse):

1. **Known-zero source aliasing**: `x`, `null`, `isr`, `osr` are all 0 on
   every execution, so 4 MOV sources (x5 with `::`, x5 with `!`) spell the
   same constant write. `word_canon` correctly keeps them apart in general
   (they differ when the registers differ) — the equivalence is
   *state-conditioned*, invisible to any per-word quotient.
2. **OUT from an empty/zero OSR**: with autopull off, `out pins, N` shifts
   N zeros out of a zero OSR and writes 0; `osr_count` is already saturated
   at 32, so there is *no state change at all* — for **all 32 counts**.
   One behavior, 32 spellings, again state-conditioned.
3. **Pin-loopback reads**: `mov pins, !pins` / `mov pins, ::pins` read the
   *current* pin level, which the trace pins down exactly at each slot's
   execution parity — data-flow through the pad that no static quotient sees.

Note what does NOT vary: no champion touches x/y/isr/osr/FIFOs (this family
is stepwise-clean, not just final-state-clean), and no delay is nonzero.
`set pins, {2,4,...}` spellings are already collapsed by `word_canon`
(set_count=1 masks the data field), which is why only `set pins, 0/1`
appear — the mechanized word quotient is visibly doing its job; everything
left is state-conditioned.

---

## Spec 2: BIT-COPIER (data-driven, primary)

Pin 0 = next bit of the TX FIFO, LSB first, 2 cycles per bit, autopull on,
`pull_threshold = 8` (each 32-bit FIFO word contributes its low byte).
Reference: `out pins, 1 [1]`, wrap (0,0). 44 cycles = 2 full words + 6 bits
of the third. Search FIFO: `[0xD3, 0x4A, 0xFF, 0x00]`.

Expected capture: `0xD3` LSB-first is 1,1,0,0,1,0,1,1, each bit held 2
cycles, then `0x4A`, then 6 bits of `0xFF`:

```
11110000110011110011001100001100111111111111
```

Data-dependence is asserted, not assumed: flipping the consumed byte of
FIFO word 0 or word 1 changes the reference trace (both checked). Every
champion's canonical program is additionally re-run under four alternative
seedings (`[AAAA5555,0,FFFFFFFF,123456B1]`, `[1,80000000,55555555,CAFEF00D]`,
all-zeros, all-ones) against the reference's trace under that seeding —
the *cross-context validity* columns below.

### L\* = 1 (wrap 0..0, 44 cycles): 1 champion, valid in all contexts

```
[6101/ffff]  out pins, 1 [1]   bf=true
```

The space at L=1 is exactly canonical for this spec, delay included.

### L\*+1 = 2 (wrap 0..1, 44 cycles): 332 champions, 123 strict families (4 multi), 3 loose families

Search: 13.5 M items, 4.2 s. Cross-context validity: **331/332** reproduce
the reference under every alternative seeding; **1 champion is a data
coincidence of the search context** (details below). Since 332 > 200, the
listing below gives complete *family* characterizations with member counts
instead of 332 raw lines (nothing is omitted; the classes are exact — they
were tallied mechanically from the full dump).

All 332 champions have the shape `out pins,1 ; <filler>` (slot 0 fully
decided as 6001, delay 0) **except** one member of S00: `out pins,1 [1] ;
out pins,1 [1]` (6101/6101), the two-slot respelling of the L=1 champion.
The pin work is done by slot 0; slot 1 is a 1-cycle filler and the families
classify the fillers.

**S00 — 112 members.** Fillers with *no net state effect at the horizon*
(strictly equivalent to the L=1 champion's behavior):

| filler class | members | why it's a no-op here |
|---|---|---|
| `wait 0 gpio N`, N=1..31 | 31 | pins 1..31 never driven -> condition already met (N=0 excluded: that's the data pin) |
| `wait 0 pin N`, N=1..31 | 31 | same, through the in_base mapping |
| `wait 0 irq N`, N=0..7 | 8 | no IRQ stimulus, flags stay 0 |
| `irq clear N`, N=0..7 (bit 7 of the operand is a certified don't-care) | 8 | clearing an already-clear flag |
| `jmp x!=y, *` / `jmp x--, *` — **target is a don't-care** | 2 | x==y==0 -> never taken (x-- does not decrement from 0), so the target field is never consulted |
| `push iffull block/noblock` | 2 | isr_count(0) < threshold(32) -> guard fails, no push |
| `mov pins, pins` | 1 | reads the pin, writes the same value back |
| `mov x, {x, y, null, isr, ::x, ::y, ::null, ::isr, !x}` | 9 | writes 0 to x (already 0); `!x` toggles x but an even execution count returns it to 0 by the horizon |
| `mov isr, {x, null, isr, ::x, ::null, ::isr, !isr}` | 7 | writes 0 to isr (also resets isr_count, already 0); `!isr` toggles, even count |
| `mov pc, {x, null, isr, ::x, ::null, ::isr, ::pins}` | 7 | pc := 0 — a *control-flow* spelling of the wrap (`::pins` reverses the pin into bit 31 -> masked to 0) |
| `mov pindirs, {!x, !null, !isr}` | 3 | writes all-ones dirs; pin 0 dir was already 1, others unobserved |
| `set x, 0` / `set pindirs, 1` | 2 | x already 0 / dir already 1 |
| `out pins,1 [1] ; out pins,1 [1]` | 1 | the structural respelling: both slots emit a bit, delay keeps cadence |

CAVEAT on `mov x,!x` / `mov isr,!isr`: these are final-state-equal but not
stepwise-equal (x is transiently all-ones). An interchange lemma citing S00
membership needs a deadness side condition, not just this fingerprint.

**S01 — 96 members.** `in {x, null, isr}, N` for N=1..32: shifts a zero
into the (zero) ISR — isr stays 0 but **isr_count saturates to 32**. One
behavior, 96 spellings; strictly separated from S00 *only* by final
isr_count. (`in y,N` is represented via the bf mirror of `in x,N`.)

**S02 — 3 members.** `mov x, !{null, isr, y}`: three spellings of
"x := all-ones". (Not `!x` — that one toggles and lands in S00.)

**S03 — 2 members.** `mov isr, !{null, x}`: "isr := all-ones, isr_count := 0".

**119 singleton families** (each a distinct final state or trace, tallied
exactly):

| filler class | families | what distinguishes them |
|---|---|---|
| `in pins, N`, N=1..32 | 32 | shifts *pin data* into ISR — final isr is data- and N-dependent |
| `in osr, N`, N=1..32 | 32 | shifts OSR residue (data) into ISR |
| `set x, N`, N=1..31 | 31 | final x = N (N=0 is in S00) |
| `irq set N`, N=0..7 | 8 | final irq_flags bit N |
| `mov {x, isr}, {pins, ::pins, !pins, osr, ::osr, !osr}` | 12 | register := pin/OSR data at the last execution |
| `pull ifempty block` | 1 | externally invisible for 88 cycles but distinct stall/final state after FIFO exhaustion |
| `pull ifempty noblock` | 1 | **loose-distinct** (L02): after the FIFO drains (cycle ~64 of the 2x horizon) the noblock pull loads X into OSR and the pin keeps emitting instead of freezing — caught only by the extended horizon |
| `mov pc, ::osr` | 1 | **the CTX-DEPENDENT champion** (loose L01) |

**The data-coincidence champion** — `out pins,1 ; mov pc, ::osr`: jumps to
`reverse(osr) & 0x1F`. For the search payload (and for all-zero data) the
reversed OSR residue happens to have zero low bits at every execution, so it
always jumps to 0 and matches; under `[AAAA5555,...]` and the other
seedings it jumps into the weeds. A single-seeded search *did* admit a
data-coincidence at L=2 — the multi-seeding mandate is empirically
load-bearing, not theoretical.

Loose tier: 3 families — L00 = the other 330 champions (all externally
identical over 4 contexts x 88 cycles), L01 = `mov pc,::osr`, L02 =
`pull ifempty noblock`. So at the externally-observable level the true
redundancy is 330:1.

---

## First-pass analysis: what the families expose

The mechanized per-word quotient (`word_canon`, P2/P4) is visibly tight:
no self-move or vacuous-jmp or set-data respellings survive. P3 keeps all
delays front-loaded-normal. P1/binding keeps Y-first spellings out except
where X is already named. **Everything that remains is either
state-conditioned (a per-word quotient cannot see it) or cross-slot
(structure vs delay).** Sources, in order of mass:

1. condition-already-satisfied ops used as fillers (waits, guarded
   push/pull, irq clear, never-taken jmp): ~111 of the 332 bit-copier
   champions, 0 of the toggler's;
2. known-zero register data flow (mov/out/in reading a register that is
   provably 0 at every execution): the entire toggler slot-0/slot-1
   alphabets minus SET, plus S01/S02/S03;
3. dead final-state writes separating otherwise-identical programs
   (set x,N singletons; in pins/osr,N; irq set N) — the strict/loose gap,
   123 vs 3 families;
4. structure-vs-delay respelling (`out [1] ; out [1]` vs `out ; filler`);
5. control-flow spelling of fall-through (`mov pc, <known-0>` = wrap).

### Candidate canonicalization lemmas

Stated as LHS ~ RHS + side conditions. "At every execution" means at every
cycle the slot executes, in every admissible context — these are
*state-conditioned* lemmas: to use them as prunes the engine needs the
condition as an invariant of the candidate partial program (value-class
analysis / ticket-011-style superposition), or they can serve as
post-search champion-set quotients proven per-spec against the z3 mirror.

- **CL1 (known-zero source aliasing).** `mov DST, [op] SRC` ~
  `mov DST, [op] null` when SRC's read value is 0 at every execution
  (SRC in {x, y, isr, osr}; op in {plain, !, ::}). Analogously
  `in SRC, N ~ in null, N`. Collapses 9 of the toggler's 10 slot-0 MOVs,
  4 of its 6 slot-1 MOVs, S01's 96 -> 32, S02 -> 1, S03 -> 1.
- **CL2 (OUT from exhausted zero OSR).** With autopull off, `out pins, N` ~
  `mov pins, null` for **any** N, when `osr == 0` and `osr_count == 32` at
  every execution (no pin difference — shifts zeros; no state difference —
  count saturated). Collapses 32 toggler slot-0 spellings to 1. The
  bit-count field is effectively an unrecognized don't-care here.
- **CL3 (satisfied-condition ops are NOPs).** `wait 0 SRC IDX` ~ `nop`
  when the waited level is already 0 at every execution (never-driven,
  never-stimulated pin; clear IRQ flag); `irq clear N` ~ `nop` when flag N
  is clear; `push iffull` / `pull ifempty` (block or not) ~ `nop` when the
  guard fails at every execution; `jmp COND, T` ~ `nop` (T a don't-care)
  when COND is false at every execution — the engine already leaves T
  undecided (cond-lazy demand), but does not fold the cond spellings
  (`x!=y` vs `x--` from x==y==0) together. Collapses 111 S00 members.
- **CL4 (dead-store quotient — the strict/loose gap).** `set x, N` /
  `in SRC, N` / `irq set N` / `mov REG, SRC` ~ `nop` when the written
  state is dead (never read downstream, not part of the observable
  contract). This is a *liveness* side condition on the surrounding
  program, not the state — it is what separates 123 strict families from
  3 loose ones, and is only usable where the spec's contract declares
  final state unobservable.
- **CL5 (delay-vs-structure normal form).** A wrap body of k executing
  slots each completing in c_i cycles is respelled by any split of the
  same cycle budget across slots (`out[1] ; out[1]` vs `out ; nop-filler`
  — note the two spellings differ in FIFO cadence only off-horizon).
  Candidate normal form: fewest executing slots, then P3 front-loading.
  Needs care: at L=2 the `out ; filler` and `out[1] ; out[1]` spellings
  are NOT stepwise identical (the second emits bits from both slots), only
  trace-identical — a footprint-metric tie-breaker, not an interchange.
- **CL6 (pin-loopback constant folding).** `mov pins, !pins` /
  `mov pins, ::pins` / `mov pins, pins` fold to constants when the trace
  pins the pin level at every execution of the slot (the toggler's parity
  argument). Subsumed by CL1 if "known-value" analysis includes the pad.

Suggested priority: CL1 + CL2 + CL3 are all instances of one mechanism —
**a known-value/satisfied-guard invariant over the candidate's reachable
states** — and together they collapse the toggler L=2 set 260 -> ~4
(set/mov-null/loopback/out-zero classes -> 1 with CL2+CL1 both) and the
bit-copier S00+S01 mass 208 -> ~3. That single analysis is the biggest
canonicity lever these specs expose.

### Method notes for the next mining run

- The 2x fingerprint horizon and the 4-seed battery each caught a distinct
  near-miss (`pull ifempty noblock` and `mov pc,::osr` respectively); keep
  both. Neither is caught at 1x horizon / 1 seed.
- STRICT-tier membership is final-state, not stepwise; `mov x,!x`-style
  members mean a lemma derived from a family needs its own side condition
  audit (or a stepwise-state fingerprint tier).
- L=2 searches at these trace lengths run in seconds; the same battery at
  L=3 with the tightened bracket is the obvious next rung if these lemmas
  get mechanized and we want a harder residual.
