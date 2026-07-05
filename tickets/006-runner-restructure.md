status: mostly-done (engine-split dropped by agreement; sweep/meta stay as tests)
priority: high

# Runner / problem / engine restructure

The crate grew as one experiment log: 44 `#[ignore]` tests double as
experiments, probes, A/Bs and production runs; the two engine files are ~3k
lines each with problem fixtures duplicated inside test modules; run
artifacts are loose files. Agreed direction (2026-07-04 discussion + the
resumable-runner slice that landed as commit c97055d):

## Target shape

1. **Runner binary** (`src/bin/superopt.rs`): DONE — `spec-ladder`,
   `wave-ladder` (both resumable over the shared Problem-generic
   run_ladder), `diagnose` (last-snapshot inspection: position, champion
   class, gates; works mid-run). One-shot experiments (densify sweep,
   schedule meta-tune) deliberately stay as #[ignore] tests — they are
   experiments with recorded verdicts, not production runs.
2. **`Problem` trait**: DONE for the ladder problems (`problems.rs`:
   DmeSpec, DmeWave; template/space/dataset/default-hp/gates). UART/SPI use
   different engines and were left as test-local fixtures — abstract them
   only when an engine actually runs them again.
3. **Engine module split**: DROPPED (agreed 2026-07-05). Phase-1 deletion
   of the superseded engines (flat_pt, rainbow, novelty, ramp/stage,
   portfolio, meta_anneal) shrank search.rs 3347->2223 and gene_search.rs
   3009->1838; everything left is live, so a further split is churn.
4. **Run artifacts**: DONE — resumable JSONL trace + `<trace>.result.json`
   on completion (run identity, champion words/class, gate verdicts, git
   rev).

## Ignored-test triage (the 44)

**Keep as fast-ish validation canaries** (capability regressions, cheap):
rediscovers_spi_optimum_fixed_wrap, rediscovers_spi_free_wrap_and_genes,
polish_grinds_out_data_residual, serializer_autopull_synthesizes,
gene_portfolio (UART synthesis capability), dme_spec_ladder_crack_l2
(spec-gradient searchability + documented toggler finding).

**Convert to runner subcommands**: dme_curriculum_gated (→ wave-ladder),
dme_diagnose (→ diagnose), dme_curriculum_meta_tune (→ meta subcommand,
only if schedule re-tuning under the spec oracle is wanted),
dme_spec_densify_sweep (→ sweep, or keep as-is short-term; results table
lives in its doc comment).

**Delete — concluded, verdict recorded in docs/journal.md and/or tickets**:
- UART era: uart_tx_optimization_vs_synthesis, uart_k1_base_solvable,
  uart_curriculum_bit_ramp, uart_masked_curriculum,
  uart_data_loop_synthesizes, uart_macro_moves, data_loop_macro_moves,
  diagnose_near_misses, gene_synthesis, gene_param_sweep, gene_reliability.
- Superseded DME engines/probes: dme_probe, dme_headroom, dme_compute,
  dme_metric_ab, dme_flat, dme_flat_pt, dme_flat_scale, dme_breed,
  dme_breed_ab, dme_breed_scale, dme_breed_scale_10x, dme_breed_random,
  dme_rainbow, dme_novelty, dme_curriculum_warmstart, dme_curriculum_length,
  dme_curriculum_multilength, dme_inspect, diagnose_residual, migration_ab.
- Ticket-resolved: dme_meta_tune, dme_tend_sweep (002), migration_ab (001).
- Replaced by the runner: dme_spec_curriculum_gated.

Deleting these orphans the superseded engines (synthesize_flat_pt,
synthesize_rainbow, synthesize_novelty, synthesize_curriculum_ramp/stage,
synthesize_flat_breed's meta harness BreedHp/meta_anneal if nothing else
uses them) — delete those too rather than carrying dead pub fns.

## Constraints

- **Search behavior must not change**: the deterministic tests
  (flat_breed_independent_is_deterministic, dme_spec_ladder_deterministic,
  dme_spec_ladder_resume_is_byte_identical) must stay green, so an
  interrupted long run can be resumed with a rebuilt binary.
- The long spec-ladder run (started 2026-07-04) runs from an already-built
  binary; restructure freely, but do not change snapshot/trace formats
  incompatibly while it is live (or bump the header so resume refuses
  loudly).
