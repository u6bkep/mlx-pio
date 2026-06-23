# Superoptimizer — scratch / thinking doc

> Uncommitted working notes. Where we are, what's broken, directions to try.
> Not a spec; a place to think. Last updated 2026-06-21.

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

## Other directions (held)
- **Population + crossover (GP)**: combine a "has spine" with a "has framing"
  individual. The per-stage elitism above is a step toward this.
- **DTW (banded)**: held in reserve behind the tolerance band — only needed if
  early-synthesis *global* drift exceeds a fixed window (band can't realign
  unbounded drift; DTW can). Don't build until the band demonstrably stalls.
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
