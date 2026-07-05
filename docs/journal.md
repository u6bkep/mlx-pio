# Journal — session history

> Append-only, newest entry at top. Moved verbatim from the old `SCRATCH.md`
> on 2026-07-04. Not required reading — search it for provenance when needed.
> Current state lives in `STATUS.md`; durable design in `docs/architecture.md`.

## 2026-07-04 — L=6 verdict: STALL (4x), testbed retired; oracle pivot underway

- Overnight 2..=14 run (old engine): L=2..5 solved, L=6 stalled fe=80. Trace
  diagnosis: **warm-start lock-in** — all 64 warm restarts ended at EXACTLY the
  warm champ (0 improvements in ~256M evals, reheat to 0.79); all 32 random
  restarts plateaued 2.6-4x worse. Monoculture in a deep moat.
- Countermeasures built (commits 333747f, 8b3029d): **cross-pollinated
  retries** (restart minima deduped by op structure -> diverse warm pool) +
  **self-mined macro splices** (recurring 2-3 insn runs, jmp targets
  relativized, spliced ~1-in-8). Attempt 0 stays RNG-identical to old engine.
- Rerun (`xpoll-mined`): still 4/13, L=6 fe=80 all 3 attempts. Attempt 2 truly
  engaged (27 distinct minima, pool=8 lib=8); best incursion sel=13322 — ONE
  unit off the champ — nothing crossed. Frequency-mining harvested boilerplate,
  never the rare refill idiom (>=2-programs threshold). Lesson: the wall is
  basin topology at this cell geometry; search-side levers exhausted.
- **Oracle pivot started (ticket 005, all 4 design decisions resolved)**:
  - Real 802.3 Clause 147 numbers extracted (docs/802.3-clause147-dme-timing.md):
    T2=80ns±100ppm, T3=40ns band 38-42, jitter<5ns s2s. All SUB-CYCLE at PIO
    resolutions -> TX freedom = compliant FAMILY (phase/polarity/schedule),
    not edge fuzz; bands become load-bearing for the future RX direction.
  - **FINDING: dme_ref itself is NOT spec-shaped** (14-cycle cell, data at +6
    not centered, +1 slip per 5-bit word = pull latency). Pinned as test
    `dme_ref_is_not_spec_shaped`. Every cycle-exact champion was forced to
    copy these artifacts. Autopull/balanced-refill = the path to compliance.
  - Certifier landed on master (src/certify.rs, commit 4e6b317): independent
    receiver-style decoder, mutant-tested, no shared code with dme_ref/cost.
  - Decisions: clkdiv OUT of genes v1; bare bit-cells v1; in-repo certifier now
    (scope cross-check later); **nominal cell = 16 cycles, data at +8**.
  - Spec search metric + testbed implemented on branch `spec-oracle`
    (worktree ../pio_optimization-spec-oracle): Target plumbing (Wave|SpecBits),
    spec testbed, certifier-gated ladder. Finding: the tolerance band is
    charitable to a data-independent half-cell toggler (Thompson hazard as
    predicted) — L=2 cracks only at full 32x4M budget. densify_w sweep with a
    structural TOGGLER/CONJUNCTION classifier written to price the exploit out.

## 2026-07-03 — status + THE PATH FORWARD (agreed with user)

### What happened this session

- **Padding regression found & fixed** (commit 93900e2): the FIFO-fed padding
  added in c76ed4d ("harmless with autopull off") broke the conjunction crack —
  padded stalls L=2 (0/13, front_err=1.5); unpadded solves L=2..5, same
  seed/budget. Padding is now opt-in (`pad: bool`), required only for autopull
  retries. **The autopull "double-consume race" hypothesis is RETRACTED** —
  its L=2 evidence was the padding, so autopull+fed-FIFO is UNKNOWN, not negative.
- **Word-boundary wall replicated unconfounded**: unpadded extended ladder
  solves L=2..5, stalls L=6 (front_err=63). Crucially the stalled champion
  contains the refill idiom skeleton (`jmp NotOsrEmpty` + a second `pull`) —
  the search FINDS the mechanism idea unaided; it fails to finish wiring it.
- **Ladder engine fixes** (commit a911650): lexicographic rung selection
  (frontier-solvers can no longer lose selection to a lower-metric non-solver),
  stall recovery (2 retries, escalating reheat, half-random pool), deterministic
  early-stop (per-checkpoint barrier) + heartbeat. Determinism re-verified.
- **In flight**: the L=6 verdict run (2..=14, fixed engine), restarted with
  structured trace logging.

### The L=6 verdict decides the macro question

- **If it cracks**: curriculum pressure alone discovers mechanisms — no macro
  injection needed. Climb the ladder, then implement the END config-polish
  (flip autopull on, delete redundant pull/OSR-check, re-validate — still
  unimplemented), then STOP polishing this testbed (no meta-tune size-squeezing
  for its own sake) and pivot to the oracle work below.
- **If it stalls after all retries**: next lever is **self-mined macros**, NOT
  hand-authored ones — harvest recurring instruction subsequences from the
  elite pool / stalled champions (e.g. the `jmp !OSRE / pull` pair it already
  invented) and offer them as insertion moves. Historical note: hand macros
  already had an era here (cracked the UART spine, superseded by the atomic
  Gene-IR loop); the new constraint is the PROJECT GOAL — novel programs a
  human wouldn't write — so human-authored idiom injection is a last resort.
  Moves shape visit density, not reachability; but with finite compute, density
  is destiny, so keep the vocabulary machine-discovered.

### Ranked sources of human bias (why the next fronts are what they are)

User's goal: novel PIO (oversampled clocks, overlapped loops through
seemingly-unrelated code) for the real multi-SM 10BASE-T1S program. The
binding biases, strongest first:

1. **The oracle**: cycle-exact equality to the hand-written reference waveform
   at pinned clkdiv doesn't just bias toward human implementations — it
   MANDATES human timing bit-for-bit. Oversampling/retiming solutions score as
   garbage by construction. (This is why clkdiv-as-gene "failed": at window=0
   any clkdiv change is maximally wrong. The landscape, not the wiring.)
2. **slots=10 window**: overlapped-loop tricks need room + multiple entry
   paths; a 10-slot single-wrap window can't express them. Blocker for 32:
   polish is O(slots²·ops²), search budget vs space size.
3. **Single-SM harness**: the real target is multi-SM (IRQ handshakes, SM→SM
   signal chains); search + oracle plumbing are single-SM throughout.
4. (Distant fourth) macro vocabulary — see above.

### The next major front: SPEC-LEVEL ORACLE (design before code)

Replace "equals the reference waveform" with "satisfies the protocol":
- **Loose metric for search** (gradient): tolerance-band edge scoring —
  `edge_cost`'s timing `window` is already a banded aligner; free clkdiv.
- **Strict independent checker for certification**: a real protocol checker
  (edge positions within spec jitter of ideal cell boundaries, mid-bit
  presence per data bit, data recoverable), NOT the search metric. Split is
  mandatory: every softened oracle to date got gamed (Thompson hazard —
  `OUT PINDIRS`, counter-replay).
- Champions must still be HW-validated eventually.
- This is a correctness REDEFINITION and a one-way-door design — discuss the
  DME tolerance spec + exploit surface with the user before implementing.

Sequencing after the oracle: 32-slot window scaling (only pays once the
objective can reward overlap), then multi-SM harness, then the real
rs10base-t1s target (start with one SM's worth under the spec oracle).

## 2026-06-23 — Performance + determinism + meta-tuning resolved

Picked up the "NEXT" from 06-22 (profile the eval hot path) and it cascaded
into resolving the whole meta-tuning question. All committed.

### Eval perf — 3.4x on the real breed-step (ticket 004)
Profile-first with a Criterion suite (`benches/eval.rs`) + the `fixtures` module.
- **edge_cost 3.9x**: a full all-ones mask scanned all 32 bit-channels (30 empty)
  — intersect `care` with bits present in golden|candidate; rolled the DP onto
  two rows. 12.3→3.2µs.
- **Emulator 3x**: `emu.step()` ran both Cortex-M33 cores + all peripherals every
  PIO cycle. Vendored `rp2350-emu` and added `step_pio_only`/`tick_pio` (PIO
  slice only; skip cores/timers/IRQ, hoist the redundant GPIO refresh, skip reset
  blocks). Gated by `fast_step_matches_full` (DME ref + plateau + 300 random,
  byte-identical to the full step). 16.5→4.2µs.
- **Fat LTO**: the hot path crossed 4 crate boundaries through un-inlined pub fns
  (~15% call overhead). 5.2→4.2µs.
- **Cache hypothesis REFUTED**: perf shows IPC 4.2, L1d miss ~0% (Emulator is
  11.5 KiB, fits L1) — compute-bound, not cache-bound. At 32 threads IPC drops
  4.2→2.6 (SMT exec-unit contention), so 32 islands ≈ 20x aggregate, not 32x.
- Near the single-SM ceiling; more needs single-SM specialization (declined —
  multi-SM is wanted) or a JIT. The 4-SM-loop skip was measured neutral.

### Determinism — free, via the cooperation A/B (tickets 001/002)
- **Cross-breeding does NOT help on DME** (`dme_breed_ab`, n=16): independent
  restarts beat cooperative on best/median/mean. The n=6 look-positive was
  *noise* — same seed gave 17 then 23 (board↔thread timing). Slot crossover is
  destructive; disruption > benefit at DME complexity. (May still pay at higher
  complexity — not refuted, just below threshold here.)
- **Independent mode (`poll_rate=0`) is deterministic** (no board reads → pure
  seeded anneal; locked by `flat_breed_independent_is_deterministic`). So
  trustworthy meta-tuning needed no barrier-sync rewrite — just run independent.

### Meta-tuning — resolved (ticket 002)
- **Transfer trap was the `max_window`/ladder lever**: dropped it from the
  meta-genome → tuning transfers (deterministic: default 23.0 vs tuned 21.3 @800k).
- **`t_end` sweep**: optimum is budget-invariant (0.05 at both 150k and 800k) ⇒
  **short inner trials ARE representative**. The perf work let us *prove* this.
- **New weak link**: the meta-anneal under-optimizes (found t_end=0.54; sweep says
  0.05). 24 SA iters stall on a ~1-D landscape — a grid beats SA for small HP
  spaces. And at 800k the t_end curve is nearly flat: HP tuning's ROI on DME is
  thin; its real value is at higher complexity.

### NEXT (as recorded then)
DME is still unsolved (best edge-cost ~17–22, plateaued). Perf + determinism are
in hand. The open frontier is **why DME plateaus** — the search isn't reaching
edge-cost 0 regardless of scale/HPs. Likely a *structural/representational* gap
(the mids), not a tuning or speed gap. Candidates: richer move set / decomposition
for the data-conditional cell; or accept DME as a stepping stone and move toward
the real target (10BASE-T1S) where cooperation + meta-tuning should finally pay.
*(Resolution: the curriculum/conjunction work of 06-24..07-03, then the oracle
pivot — see later entries.)*

## 2026-06-22 — mlx86 detour → novelty pivot → flat edge engine

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
Neither solves yet (edge ~23, need 0).

### Plateau → diagnosis → the cross-breeding pivot

Flat+edge+PT+scale (the full mlx86 recipe) **plateaus at edge-cost ~22** —
7× iters barely moved it (24→22). **Diagnosis** (`dme_diagnose`, classify golden
edges as *boundary*/clock vs *mid*/data-conditional via an all-zeros reference):
the champion matches some boundaries but **0/11 mids, every seed** — the
data-conditional transition is **never born**. Two findings:
- The mid cell is **expressible** in the flat search (`out x`/`jmp x--` exist)
  but **gradient-free** — no partial credit until the whole timed cell is right,
  so local search never enters it. Scale can't manufacture a gradient.
- The edge metric has its **own trap**: missing = spurious = 1, so the safe move
  is *emit few edges, avoid spurious* (a champion hid at 7 edges / 0 spurious).

**The new path (committed) — staging overstayed its welcome.** Three changes,
all confirmed to matter:
1. **Densify** (`edge_cost_w`, spurious weight < 1): attempting an edge costs
   less than leaving one unmatched → the search densifies instead of hiding.
2. **Continuous cross-breeding islands** (`synthesize_flat_breed`): persistent
   parallel islands on a **fixed window ladder** (no staged re-election), each a
   long anneal, sharing a board and **recombining** (slot-range `crossover`) not
   copying — so a "has clock" and a "has mid fragment" island can yield a child
   with both, crossing the conjunctive gap.
3. **Scale**: 32 islands = 32 cores (was 12 — we were leaving 20 idle).

**BREAKTHROUGH:** the data-conditional mids — never born before — **are born**:
4-5/11, edge-cost trajectory **staged-plateau 22 → small-breed 23 → breed@scale
17** (5/11 mids, 12/20 boundaries, 1 spurious; a creative `mov Pins,~Pins` /
`~Osr` / `Isr` tangle, *not* the human reference). Still not solved (17, need 0)
and **high-variance** (1/3 seeds breaks through).

### Migration verdict
- **Within-stage island migration (copy) is a wash** — UART, flat-DME, and at
  scale. Superseded by **cross-breeding (recombination > copy)**. Ticket 001's
  copy-migration is retired; the useful descendant is the breeding board.

## Earlier (UART era, pre-2026-06-22)

### THE central problem (as it was then): assembling the counted-loop spine

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

Two levers ruled out, one ruled in:
- **Curriculum / bit-count ramp** — dead: relocates the cliff to the base rung.
- **Region-masking decomposition** — dead for shared-pin protocols (oscillator trap).
- **Building-block / macro moves** — INDICATED. The failure is *coordination*
  (4 simultaneous mutations, no gradient); a macro move makes the spine one move.

### Building-block macro moves (slot search, committed 84e1de5)

Slot-search macro `insert_counted_loop` (self-sufficient `pull / set x,N /
out / jmp x--`, wrap-enclosed) + `MutateImmediate` (dial an immediate without
re-rolling the op). **Cracked the spine**: `data_loop` 6/26 → 0/0. Two lessons
paid for: a block must be immediately waveform-correct to survive selection
(the reward gives no credit for structure), and the count needs a tuning move.
But full UART only reached 21/44 — loops survived yet got **mangled and
couldn't integrate**: in a flat array a loop is divisible, so the search keeps
re-deranging it. → motivated the first-class IR.

### First-class IR + annealed tolerance metric (committed 59cc470)

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
