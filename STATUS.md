# STATUS — current frontier

> REWRITTEN each session (not appended). History → `docs/journal.md`.
> Durable design/lessons → `docs/architecture.md`. Last updated 2026-07-05 (pm).

## Strategy pivot (agreed 2026-07-05)

The 6th L=6 stall (autopull-gene run: champion had NO pull at all and still
broke at the word seam, fe=8 vs 11) killed the refill-spine hypothesis and
with it the from-scratch synthesis framing. New tracks:

1. **COMPRESSION (running)** — STOKE-style: seed a CERTIFIED encoder, anneal
   for size, champion moves only through the certifier gate.
   `superopt compress --seed 5EED --trace runs/compress-0x5eed.jsonl`
   (32×200k/cycle, all 13 lengths, resumable; Ctrl-C snapshots).
   Seed = `dme_spec_ref()`: hand-written spec-shaped autopull encoder,
   8 insns, certifies clean. dme_ref can never certify (14-cy cell, +1/word
   slip) — the seed is new, locked by tests.
2. **ENUMERATION (designed, not built)** — structure/timing decomposition:
   ~150-op structural alphabet, delays closed analytically (16-cycle budget
   per cell), fleet-sharded (~100 cores ≈ 3×10¹² screens/day). Decisive to
   body-length 5, borderline 6 with pruning; scaffolds needed at 7+.
   Composes with compression: champion size bounds the sweep.
3. **Real-firmware targets surveyed** (Raven-Firmware dme_pio.rs): TX block
   31/32 (TX_A 4 + TX_B 17 + timestamp 10, cross-SM IRQ toggle handshake),
   RX block 32/32 (unrolled jmp-pin sampler ladder = 11 insns). Flagship
   compression target = RX (observational-equivalence oracle: original
   program as golden). Harness has multi-SM (`from_shared`) and per-cycle
   `set_pin` — input-waveform driving needs RunSpec wiring.

## Today's commits (master)

- 6f0ce2f eval cache (transparent, ~1.2×, 29-30% hits)
- d777208 dme-spec-ap: autopull gene + per-candidate `RunSpec.autopull_pad`
- 1ba9ff8 compression mode: `dme_spec_ref` seed, `synthesize_compress`
  (reuses GatedSnapshot/stop protocol, resume locked by test), `compress`
  subcommand. CERTIFIER FIX: no pad on cert corpora (pad inflated the
  ap-run gates; re-cert FAIL(1)/FAIL(3), verdict unchanged).

## Next actions

1. Watch compression (monitor active; SHRUNK events stream). Read out via
   `superopt diagnose --trace runs/compress-0x5eed.jsonl`.
2. Design + build the enumerator (shard manifest, timing closure, fleet).
3. RX testbed infra: RunSpec pin-stimulus track + RX FIFO capture +
   golden-equivalence battery.
4. Still flagged: `algorithm_word` worktree + `gene-v2-ir` branch.
