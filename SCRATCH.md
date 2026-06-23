# Superoptimizer — scratch / thinking doc

> Uncommitted working notes. Where we are, what's broken, directions to try.
> Not a spec; a place to think. Last updated 2026-06-22.

## The goal (unchanged)

Stochastic search (SA / MCMC) to **generate** RP2350 PIO programs, scored in a
cycle-accurate emulator against a known-correct reference waveform. The payoff is
**novel, short** PIO that humans wouldn't hand-write. North star: a custom
10BASE-T1S (DME) TX/RX. Modeled on STOKE; cousin to AlphaDev.

Objective: **minimize instruction slots in flash**, subject to (a) correctness
within tolerance and (b) a real-time deadline. Clock divider is a free axis.

## Where we are (working pipeline, all committed)

Genome → score → search, end to end, fast and deterministic:

- **Emulator**: vendored `picoem-common`, patched. clkdiv `INT==0`→65536 and the
  opt-side-set overlay bug both FIXED (same-pin side-set+OUT now faithful).
  Per-eval **emulator reuse** via `Pio::reset` → ~17× faster (~1µs/eval).
- **IR** (`pio_superopt`): typed `Op` enums = exactly the legal encodings;
  encoder+decoder round-trip-tested vs the emulator's decoder. `Program` =
  `[Option<Insn>;32]` + wrap + `Config` genome. Size = **occupied span**.
  `validate()` makes illegal states explicit (no silent legalization).
- **Oracle**: capture per cycle, per pin, **level AND output-enable (OE)**. OE
  killed the `OUT PINDIRS` exploit. `trace_pads` in the harness.
- **Metric**: strict cycle-aligned Hamming, **now with a masked variant**
  (`hamming_masked`/`score_masked`) — per-cycle/per-bit don't-care mask, the
  partial-credit primitive. Strict = all-ones mask.
- **Search**: range-aware moves; Metropolis + geometric cooling + restarts;
  cost = `W·correctness + size` (W>max size ⇒ STOKE two-phase). Temp `t0≈2W`.
  `anneal_masked`/`polish_masked` take a mask; strict versions are full-mask
  wrappers. Greedy polish (1-opt sweep + 2-opt kick) appended to annealing.
- **Optimization mode**: `seed_from_template` — restarts start from the reference.

### What's validated

- **SPI-TX: synthesis works.** From random, reliably finds correct size-2 SPI
  and creative variants (pin-feedback, `MOV PINS,BitReverse OSR`).
- **Autopull serializer: synthesis works** (correctness 0, size 2, ~20s). LSB-first
  serialize with autopull+wrap — no explicit pull/counter.
- **UART-TX: synthesis cliffs.** Optimization-mode (seed=reference) → 0; from
  scratch → plateau (~67 full, ~10 even for a 1-bit frame).

## THE central problem: assembling the counted-loop spine

Five decomposition experiments (in `search.rs`, all `#[ignore]`) triangulated
*what* makes UART hard — it is **not length, not framing-vs-data**:

| experiment | result | rules out |
|---|---|---|
| `uart_curriculum_bit_ramp` (warm-start k=1→8) | cliffs at rung 1 | length |
| `uart_k1_base_solvable` (1-bit frame cold) | plateau ~10 | "small ⇒ easy" |
| `uart_masked_curriculum` (framing-first mask) | degenerate oscillator trap | region-masking on a shared pin |
| `uart_data_loop_synthesizes` (spine, no framing) | plateau 6 / 26 | "it's just SPI" |
| `serializer_autopull_synthesizes` (no spine) | **clean 0, 20s** | isolates the cause |

**The obstacle is the counted-loop spine**: `pull` + `set x,N` counter + `out` +
`jmp x--` — ~4 instructions, **no partial credit until all present and wired**.
SPI/autopull-serializer synthesize only because **autopull+wrap dissolve the
spine**. UART can't use that escape: **framing needs the explicit count** to know
when to emit start/stop, so the spine is irreducible *and* shared between data
and framing. That's why nothing separates into independent fragments — the
"data fragment" still contains the whole spine (→ it plateaus, not 0).

Why masking failed specifically: framing and data multiplex **one pin**, so
"solve framing first" admits a degenerate pin-envelope oscillator (no FIFO
machinery) that traps the warm-start. Masking only separates sub-problems that
map to separable program pieces (e.g. *different pins*).

### Two levers ruled out, one ruled in
- **Curriculum / bit-count ramp** — dead: relocates the cliff to the base rung.
- **Region-masking decomposition** — dead for shared-pin protocols (oscillator trap).
- **Building-block / macro moves** — INDICATED (see below). The failure is
  *coordination* (4 simultaneous mutations, no gradient); a macro move makes the
  spine one move.

## Building-block macro moves (DONE — slot search, committed 84e1de5)

Slot-search macro `insert_counted_loop` (self-sufficient `pull / set x,N /
out / jmp x--`, wrap-enclosed) + `MutateImmediate` (dial an immediate without
re-rolling the op). **Cracked the spine**: `data_loop` 6/26 → 0/0. Two lessons
paid for: a block must be immediately waveform-correct to survive selection
(the reward gives no credit for structure), and the count needs a tuning move.
But full UART only reached 21/44 — loops survived yet got **mangled and
couldn't integrate**: in a flat array a loop is divisible, so the search keeps
re-deranging it. → motivated the first-class IR.

## First-class IR + annealed tolerance metric (committed 59cc470)

**Gene IR** (`gene.rs`): genome = sequence of nodes, `Node = Prim | Loop{body,
cond, counter_init, jmp_delay}`. A loop is **atomic** (owns its back-jump), so
point moves can't dismantle it. **No labels** — plain-JMP targets are literal,
so structural loops lose no runtime novelty; data-driven axes kept
(`UntilOsrEmpty`, `counter_init=None`). Lowers to the existing `Program` path.

**Gene search** (`gene_search.rs`): SA over nodes; serializer macro; hard
**length cap** (gene analogue of the slot window — essential, reduced size
weight alone bloats); deterministic **polish** (refine + remove + compensated
framing insert); **timing-aware moves** (insert/remove-compensated, shift-cycles
over top-level prim delays) that restructure at *constant total duration* to
dodge the strict-Hamming phase cliff.

**Annealed tolerance-band metric** (`cost.rs::hamming_tolerant`, `k`): a
mistimed-but-present bit pays `δ/(k+1)` not 1; `k=0` == strict masked Hamming.
`synthesize_gene` anneals `k` 8→0 (graduated optimization: blurry→sharp, temp
re-heated each stage). Smooth metric finds the basin; strict k=0 certifies —
and the schedule *resolves* the smooth-vs-exploitable tension (smooth guides,
strict rejects gamed champions).

**FIRST FULL UART SYNTHESIS:** k=8 → correctness 0, size 6, **novel** —
`pull / mov Pins,BitReverse Null[7] (start) / loop(CountY=7){out[6]} /
mov Pins,Invert X[7] (stop)`. The framing bits are creative (BitReverse 0 = low;
Invert of zero-init X = high), reached only because of the annealed tolerance —
strict Hamming never got there.

### Reliability — solved via parallelism (committed 674bc37)
Per-chain synthesis is low-rate/high-variance and three search levers (elitism,
2-opt polish kick, adaptive diversity gathering) **plateaued** it — the
bottleneck is late-stage barrier-crossing, not stage-1 diversity. So reliability
comes from **parallelism**, not a higher per-chain rate:

- **k=4<k=8 inversion = starting-radius mismatch** (confirmed by sweep): a fixed
  radius-8 blur over-smears the *shorter* k=4 frame. Optimal starting radius
  **scales with the target** (k4 wants ~4, k8 ~8). No single schedule is best.
- **`synthesize_portfolio`**: run a portfolio of diverse schedules (varied
  starting radii) × multistart seeds, keep the strict-best. A 2-schedule × 8-seed
  portfolio **reliably synthesizes both UART targets** (combined correctness 0,
  first solve in 1-2 runs). Each `synthesize_gene` is internally parallel
  (chains/stage via `run_chains`).

Net: the engine is **capable + parallel-reliable**. Per-chain elitism/2-opt help
specific cases (data_loop k=8 7/8) but parallel portfolio is the mechanism.

## mlx86 detour → novelty pivot → flat edge engine (2026-06-22)

Surveyed `reference/mlx86` (a friend's x86-asm superoptimizer) for prior art →
`tickets/` (001 parallel tempering, 002 meta-tuning, 003 hash tabu). It searches
a flat raw-byte space yet solves hello-world / a 4-fn calculator and finds
genuinely creative solutions (e.g. misaligned jumps that re-decode bytes). Three
things that flowed from it:

**1. Targets: UART is solved; aim at DME.** UART has one hard structure (the
spine) and we already find good ones — it can't discriminate search ideas (PT
A/B on it was neutral-to-negative noise). The real target is 10BASE-T1S DME
(`reference/.../rs485-eth`). Stepping stone = **single-SM Differential
Manchester TX** (biphase-mark: transition every bit boundary + a *data-conditional*
mid-bit transition). The data-conditional mid-transition is the coupled second
structure UART lacked.

**2. v2 IR — structured conditional (`Node::Cond`, committed).** The gene IR v1
banned all non-structural JMPs, so it **could not express** DME's data-conditional
branch. Added `Cond{cond,then,els,…}` (structured selection, dual of `Loop`),
label-free, lowers with a past-end→wrap fixup. The DME reference encoder
(`dme_ref`, biphase-mark, tracks line level in Y, `mov y,~y`/`mov pins,y` to
toggle, `jmp x--` on the data bit) runs correctly: mid-transitions are
**popcount-exact**. Locked golden + corpus + guard test.

**3. The macro trap, and the real reframing.** Macros (counted-loop, etc.) were
right for UART — a *solved* problem where encoding the known structure is fine —
but they **inject human priors at the expense of the novelty we're after**. The
gene IR itself is a prior: it forbids the creative control-flow reuse mlx86
thrives on. Diagnosis, in order of leverage:
- **Objective (biggest).** We scored the cycle-by-cycle *level* trajectory;
  mlx86 scores *output*. Level-Hamming on a transition code is **deceptive**: a
  slowly-varying signal matches ~half the cycles for free, so the search falls
  into a `out Pins` data-dump basin (best ~31-44/278, never climbs). **Fix:
  `Metric::Edge`** (`cost.rs`) — represent each channel as a transition-event
  sequence (cycle,new_value; pre-level 0 ⇒ complete encoding) and score a banded
  edit distance (shift = Δ/(W+1), miss/spurious = 1; W annealed = old `k`).
  Edge-cost 0 at W=0 ⟺ exact waveform, so it also certifies. **A/B: the level
  metric steers to `out Pins`; the edge metric steers to a pin-toggling loop
  (`mov Pins,~Pins`) — a real transition generator.** Necessary, not sufficient:
  it reaches the periodic-clock skeleton, not yet the data-conditional mid.
- **Representation.** The creativity (instruction reuse, overlapping jumps) lives
  in the **flat slot search**, which the gene IR forbids. So: revive flat search.
- **Diversity/scale.** mlx86's flat power = PT + migration + millions of trials.

**Flat edge engine (`synthesize_flat_pt`, search.rs).** Parallel chains
(thread-local emu), per-stage diverse elitism, adaptive stage-0 diversity, ported
island migration (PT) — edge objective, window-annealed, **no priors/macros**,
arbitrary jumps. Selects/certifies by edge-cost (not level — fixed a straggler
bug). First A/B on DME (n=4): **flat+edge edge-cost ≈24 beats gene+edge ≈34**
(not compute-controlled, but 8 full runs in **15 s** — huge scale headroom).
**Migration still a wash** (24 vs 23, noise) — within-stage PT hasn't earned its
keep on either UART or flat-DME; if PT is to pay off it likely needs knob tuning
or the cross-*seed* variant (deferred "plan 2"), not more within-stage migration.
Neither solves yet (edge ~23, need 0). **Next: crank scale (point 3).**

## Other directions (held)
- **Population + crossover (GP)**: combine a "has spine" with a "has framing"
  individual. The per-stage elitism above is a step toward this.
- **DTW (banded)**: partly realized — `Metric::Edge` is a banded edit distance
  over transition events (a DTW cousin). Full DTW (continuous time-warp) still in
  reserve if edge alignment proves too rigid for *global* drift.
- **Structural reward shaping**: removable synthesis scaffolding; lower priority.

## The endgame: hybrid

Synthesis (macro moves) discovers novel *structure* on small sub-problems →
optimization-mode shrinks/refines it → novel sub-solutions become new building
blocks. Keeps the novelty engine running but tractable.

## Recurring hazards
- **Overfitting** — thin corpus is trivially gamed (counter-replay faked one
  byte). Multi-input corpora mandatory; the UART specs push 4 distinct bytes.
- **Oracle exploits (Thompson)** — every thin oracle gets gamed (`OUT PINDIRS`→
  added OE). Tightening is perpetual and protocol-specific. Champions must be
  HW-validated; verify emulator features vs datasheet before searching them.
- **Polish doesn't scale** — 2-opt is O(slots²·ops²); needs localization to
  survive a 32-slot window.

## Throughput (user's ideas — TBD)

A **multiplier on the right algorithm, not a substitute**. Fuel for the engine;
the engine (structural levers) comes first. [Capture specifics here as they come.]

## Open questions
- Macro-move vocabulary: which PIO idioms beyond the counted loop to first-class?
- Best decomposition boundary for DME (per-SM given; per-phase = preamble /
  framing / bit-loop?) — but note shared-state caveat from the UART spine result.
- Tolerance-band scoring for DME without opening exploit surface (smooth for
  search vs tight for validation — no free lunch).
