# Superoptimizer — goal, pipeline, durable lessons

> Durable reference for `pio_superopt`. Updated when facts change, not per
> session. Current frontier: `STATUS.md`. History/provenance: `docs/journal.md`.
> The emulator/harness layer is documented in the root `README.md`.

## The goal

Stochastic search (SA / MCMC) to **generate** RP2350 PIO programs, scored in a
cycle-accurate emulator. The payoff is **novel, short** PIO that humans
wouldn't hand-write — oversampled clocks, overlapped loops through
seemingly-unrelated code. North star: a custom 10BASE-T1S (DME) TX/RX,
ultimately multi-SM. Modeled on STOKE; cousin to AlphaDev.

Objective: **minimize instruction slots in flash**, subject to (a) correctness
(per the oracle — see below) and (b) a real-time deadline. Clock divider is a
free axis in principle (out of the genes in oracle v1).

## The working pipeline

Genome → score → search, end to end, fast and deterministic:

- **Emulator**: vendored `picoem-common` + `rp2350-emu`, patched (clkdiv
  `INT==0`→65536; opt-side-set overlay bug; `WAIT IRQ` fix). PIO-only fast
  step (`tick_pio`) + per-eval reuse via `Pio::reset` → ~1µs/eval, ~4.2µs per
  scored candidate. Compute-bound (IPC 4.2, L1-resident); 32 threads ≈ 20×
  aggregate due to SMT contention. Fat LTO across the 4-crate hot path.
- **IR** (`ir.rs`): typed `Op` enums = exactly the legal encodings;
  encoder+decoder round-trip-tested vs the emulator's decoder. `Program` =
  `[Option<Insn>;32]` + wrap + `Config` genome. Size = **occupied span**.
  `validate()` makes illegal states explicit (no silent legalization).
  There is also a structured **Gene IR** (`gene.rs`: atomic `Loop`/`Cond`
  nodes) from the UART era; the flat slot search is the active representation.
- **Waveform capture**: per cycle, per pin, **level AND output-enable (OE)**
  (`trace_pads`). OE killed the `OUT PINDIRS` exploit.
- **Metrics** (`cost.rs`): strict cycle-aligned Hamming + masked variant;
  `Metric::Edge` — transition-event sequences scored by banded edit distance
  (shift = Δ/(W+1)), with **densify** (`edge_cost_w`, spurious weight < 1) to
  defeat the emit-few-edges hiding trap. Edge-cost 0 at W=0 ⟺ exact waveform.
- **Oracles** (two-tier, ticket 005): a **loose search metric** (gradient) and
  a **strict independent certifier** (`certify.rs`: receiver-style DME
  decoder, mutant-tested, zero shared code with the reference/cost). The split
  is mandatory — every softened single oracle to date got gamed. Spec-level
  target shape: nominal cell = 16 cycles, data transition at +8.
- **Search engines** (`search.rs`, `gene_search.rs`): Metropolis + geometric
  cooling + restarts; range-aware moves; cost = `W·correctness + size`
  (STOKE two-phase); greedy polish (1-opt sweep + 2-opt kick);
  `synthesize_flat_breed` (persistent islands, window ladder, optional
  recombination); `synthesize_curriculum_gated` (the **gated ladder** —
  advance on performance, lexicographic rung selection, stall retries with
  reheat, cross-pollinated warm pools, self-mined macro splices);
  `meta_anneal` over hyperparams. Independent mode (`poll_rate=0`) is
  **deterministic**, locked by test.

## What's validated (capability milestones)

- **SPI-TX / autopull serializer: synthesis works** from random (size 2,
  seconds; creative variants like `MOV PINS,BitReverse OSR`).
- **UART-TX: solved** via Gene IR + annealed tolerance + portfolio
  parallelism; champion was novel (size 6, creative framing).
- **DME (cycle-exact oracle): L=2..5 solved** by the gated curriculum ladder
  — the read+branch+toggle conjunction is discoverable under curriculum
  pressure. **L=6 is a hard wall** (word-boundary/OSR-refill; 4 stalls,
  search-side levers exhausted) → drove the spec-oracle pivot.

## Durable lessons (paid for; don't re-learn)

- **Measurement gates catch what correctness gates can't.** The memo's
  conflict-scope bug (every fork-frame record silently poisoned since its
  first commit) survived exact censuses and on/off-equivalence because
  dropped records are SOUND — only a gate asserting the memo's *effect
  size* (items must halve on a constructed sharing space) exposed it.
  Every performance lever needs a gate on its magnitude, not just its
  soundness.
- **Generalization beats eviction, but value-exact patterns still hit a
  wall at depth.** Consulted-state keys + provenance (ticket 007) are the
  playground's free key-minimality rebuilt for the flat evaluator, and
  they multiplied sharing (L=2 1..1: 9.2x) — yet the L=3 bracket
  flatlines under ANY capacity (1M and 8M records both starve). Deep
  regions differ in values the subtree only ever predicate-tests; the
  next rung is predicate-class patterns, not more RAM (~100MB/1M records
  measured).
- **Memo probing is a first-class cost.** Naive per-record scans lost
  50x throughput; the playground's two-level index (mask -> hashed
  projection) is load-bearing, not an optimization. Even fixed, the memo
  loses wall-clock where sharing is thin (L=2 0..1) — hit density decides,
  and it is wrap/space-dependent.
- **X/Y renaming is not a free symmetry, but it IS quotientable.** The
  only asymmetric channels are PULL-on-empty's physical-X read and
  exec'd (data) words; a binding fork whose twin becomes an ordinary
  concrete item (words mirrored, x/y swapped) is sound where champion-
  time mirroring provably is not.
- **The oracle is the strongest human-bias source.** Cycle-exact equality to a
  hand-written reference MANDATES human timing bit-for-bit (and `dme_ref` is
  not even spec-shaped). Next binding constraints, in order: 10-slot window
  (blocks overlapped-loop novelty), single-SM harness, move vocabulary.
- **Thompson hazard is perpetual**: every loose metric gets gamed
  (`OUT PINDIRS` → OE capture; counter-replay → multi-input corpora;
  spec tolerance band → half-cell toggler). Loose-search + strict-independent-
  certify is the standing defense; champions must eventually be HW-validated.
- **Conjunctive structures are gradient-free** (counted-loop spine, DME
  data-conditional mid): no partial credit until all parts are present and
  wired. Scale cannot manufacture a gradient; curriculum/decomposition or
  atomic moves can. Region-masking fails on shared-pin protocols (degenerate
  oscillator trap).
- **Level-Hamming is deceptive for transition codes** (half credit for free);
  score edges, and price spurious edges below missing ones (densify).
- **Keep the move vocabulary machine-discovered.** Hand macros work (cracked
  the UART spine) but inject the priors the project exists to avoid; mined
  macros/idioms are the acceptable form. Moves shape visit density, not
  reachability — with finite compute, density is destiny.
- **Meta-tuning transfers only without budget-coupled knobs** (drop the
  ladder/max_window lever); short inner trials are representative (t_end
  optimum is budget-invariant). ROI is thin at DME complexity.
- **Cross-breeding/migration below its complexity threshold is a wash**;
  independent restarts win at DME scale AND buy determinism for free.
- **Warm-start monoculture is real**: warm pools converge to the exact warm
  champ; diversity must be injected structurally (dedup by op structure), and
  even that can fail against deep basins (the L=6 verdict).

## Standing directives (user)

- **Observability**: long unattended runs must stream per-rung/attempt
  markers to stderr AND write a structured JSONL trace — per-restart
  checkpoints, new-best events with disassembly, per-attempt final minima
  distributions (the minima distribution is analysis data, not just progress).
- **Big dumps go to `/data/pio_optimization/runs/`** (second SSD), not the
  crate's `runs/` dir — the narrowing engine's mining flags can emit tens of
  GB. Engine instrumentation (all env-driven, search-behavior-free, gated by
  `instrumentation_flags_do_not_change_search`): `PIO_NARROW_DUMP` (memo-hit
  pairs), `PIO_NARROW_PROBE_LOG` + `PIO_NARROW_PROBE_BYTES` (per-probe
  outcome census + sampled miss diagnostics: which state component or program
  cond blocks a match; stride-doubles past half budget, default 8 GiB),
  `PIO_NARROW_SNAPSHOT` + `PIO_NARROW_SNAPSHOT_MAX` (full memo-table JSONL
  dump before each purge and at search end).
- **Discuss one-way doors** (correctness redefinitions, oracle changes)
  before implementing.

## Roadmap shape (after the spec oracle lands)

Re-test config genes (clkdiv/autopull) under the spec oracle → 32-slot window
(only pays once the objective rewards overlap; 2-opt polish needs localization
to survive it) → multi-SM harness → the real rs10base-t1s target. Endgame
hybrid: synthesis discovers novel structure on sub-problems → optimization
mode shrinks it → novel sub-solutions become new building blocks.

## Held directions

- Population/crossover GP (combine partial-structure individuals).
- Full DTW alignment if banded edge alignment proves too rigid for global drift.
- Structural reward shaping (removable scaffolding).
- Throughput/scale-out: a multiplier on the right algorithm, not a substitute.
