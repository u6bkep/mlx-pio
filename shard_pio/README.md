# shard_pio — the shard twin of the narrowing engine's PIO evaluator

A PIO state-machine emulator written in [shard]
(`~/Documents/programmingSync/computer-whisperer/shard`, a colleague's
proof-language project — read as data, never modified), implementing exactly
the contract in `docs/evaluator-spec.md`, and validated against the certified
vector set `docs/shard_vectors.jsonl` (101 vectors dumped from the Rust
evaluator, whose traces are pinned against the vendored emulator, which is
bench-certified). Trust chain position:

    shard twin ==vectors== Rust evaluator ==diff-fuzz== vendored emulator ==certified== hardware

**Status: 101/101 vectors pass** (`eval direct`, ~0.5 s), and the sources go
through the shard checker clean: type gate green, all recursive fns
admitted via `(measure (struct …))` obligations, zero CANON advisories.
The gate discriminates: a broad mutation (out_latch reset 0 instead of
all-ones) fails 101/101; a subtle one (side-set suppressed under stall)
fails exactly the 4 vectors that exercise it.

## Layout

    emulator.shard        the twin: NState/NCfg, total decode, step,
                          clock_tick, compose, stall re-check, vector driver
    runner.shard          World app: runs every vector, prints PASS/FAIL + totals
    vectors_data.shard    GENERATED — the 101 vectors as shard data
    vectors/shard_vectors.jsonl   the certified vector set (copy of docs/)
    tools/gen_vectors.py  jsonl -> vectors_data.shard generator (stdlib python)

## Running the gate

Build the shard bootstrap once:

    cargo build --release --manifest-path <shard>/rust_bootstrap/Cargo.toml

Regenerate the embedded vectors (only needed when the jsonl changes):

    python3 tools/gen_vectors.py

Run the validation (from this directory; `eval direct` runs a narrow app's
`main` on the bootstrap evaluator — fast path, no repo-root restriction):

    <shard>/rust_bootstrap/target/release/eval direct runner.shard

Expected tail: `101 passed, 0 failed (of 101)`, exit 0 (any FAIL ⇒ exit 1).

## Running the checker (type gate + totality + CANON)

The self-hosted checker requires its target to live under the shard repo
root (module identity is repo-root-relative). Copy — do not touch the
colleague's tree in place — the shard repo somewhere scratch, drop this
directory in as `shard_pio/` (the name is load-bearing: the `(use (::
shard_pio emulator *))` lines encode the module path), and check:

    cp -r <shard> /tmp/shard-check && cp -r <this dir> /tmp/shard-check/shard_pio
    cd /tmp/shard-check
    rust_bootstrap/target/release/eval run kernel/check.shard shard_pio/runner.shard

Expected: `0 passed, 0 failed` (there are no claims yet — the value is the
type/measure/CANON gates all being silent), with a `MEASURED … OK` line per
recursive fn. Checking the full 101-vector `vectors_data.shard` is slow in
the self-hosted tower (tens of minutes — 240 KB of list literals);
substituting a 3-vector file (`python3 tools/gen_vectors.py <(head -3
vectors/shard_vectors.jsonl) …/shard_pio/vectors_data.shard`) checks the
whole closure in ~2.5 min. `emulator.shard` alone (the part with actual
semantics) checks in ~1 min.

## Implementation notes (deviations are representational only)

- **Flags are Int 0/1, not Bool.** The bootstrap's prim comparisons return
  the CORE `Bool`; a self-contained file that declares its own `Bool` twin
  gets `Bool vs Bool` qname clashes at the type gate, and importing the
  kernel stdlib would hardwire a cross-repo relative path into the source.
  So every spec `bool` (config flags, stall polarity, pc_set, fired,
  forced, streaming) is an `Int` 0/1 and every `if` condition is a prim
  comparison. Spec semantics are unchanged.
- **Fifo is a front-first queue list + cached count + depth** rather than
  `buf[8]/head/count/depth` (shard has no arrays). Push drops when full or
  depth 0; pop from the front — observably identical to the vendored ring.
- **32-bit discipline:** every u32-valued expression is masked
  (`band … 4294967295`) after add/shift; complements are `bxor v
  4294967295`; rotates handle k = 0 via the complementary-shift-by-32
  (defined: prim shifts are valid below 64).
- **`force_exec` is implemented** per spec §5 but unexercised by the
  vector set (the driver's pin-direction setup is a direct dir_latch mask,
  mirroring `narrow::run_with_stim`).
- **Driver semantics** (from `narrow_diff.rs::dump_shard_vectors` +
  `narrow::run_with_stim`, not the spec): the generator pre-appends
  `autopull_pad` zeros to `inputs`; streaming (refill-to-full before each
  cycle) triggers when the PADDED length exceeds 4; stim value at cycle i
  is `stim_values[min(i, len-1)]` (empty = 0); observation is
  `compose` + raw `dir_latch` on the capture pins, after the tick.

## Spec gaps found while implementing (candidate amendments)

1. **Driver layer is unspecified.** §3 defines the per-system-clock cycle
   but the harness conventions the vectors depend on (FIFO pre-load vs
   streaming and its threshold-on-padded-length, autopull padding,
   stim-value latching, capture-word encoding) live only in Rust
   (`narrow_diff.rs` / `run_with_stim`). The spec could gain a §10
   "driver/harness contract".
2. **Truncation masks are implicit.** OUT EXEC / MOV EXEC store
   `data as u16` (mask 0xFFFF), OUT PC / MOV PC mask to 0x1F — the spec
   says "pending_exec := data" / "PC (pc_set)" without stating the masks.
3. **WaitIrq carries the RESOLVED index.** §8 resolves the IRQ index at
   execute time and §7 re-checks `idx & 7`; that the stall stores the
   resolved index (not the raw operand) is only implicit.

## Toward the 2-SM product machine (design notes only)

The target theorem is `2-SM-pair ≡ 1-SM-TX` (the certified single-SM TX
transform). What the twin needs:

- **Split block state from SM state.** `irq_flags`, `out_latch`,
  `dir_latch` are block-level on hardware and are carried in NState only
  because one SM owns them (spec §1 note). The product machine lifts them
  into a `Block` record: `(Block irq_flags out_latch dir_latch)` plus two
  per-SM cores (pc/x/y/isr/osr/counts/delay/stall/pending/clk_acc/tx/rx).
  Getter/setter discipline here makes that a mechanical refactor.
- **Pin the inter-SM cycle order.** Per system clock the vendored emulator
  ticks SMs in index order; each SM reads the gpio word composed at the
  START of the cycle, but latch writes and IRQ set/clear are sequential
  within the cycle — whether SM1 sees SM0's same-cycle IRQ write (it does,
  on hardware and in the vendored emulator) vs same-cycle pin write (it
  does not — one-cycle loopback) must be stated in the spec and pinned by
  2-SM vectors before any proof is attempted.
- **Shared instruction memory** is just sharing `code[32]` between two
  NCfgs (wrap/pins/shift config stays per-SM); independent clock dividers
  are already per-SM state (`clk_acc`).
- **Proof shape:** a joint `step2` (SM0 then SM1 over the shared block),
  trace-equality goal against the 1-SM TX program, by induction over the
  cycle list with an invariant relating joint state to single-SM state —
  the same fuel-per-cycle structure `run_cycles` already has. The
  `requirement`/`fulfills` contract layer is where the equality statement
  should live once stated.
