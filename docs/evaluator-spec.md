# PIO evaluator — state/step contract (the "twin spec")

This document pins the state layout and per-cycle algorithm of the
narrowing engine's own evaluator (`pio_superopt/src/narrow/`). It is
written to serve **two implementations**: the Rust evaluator (landed)
and a future shard/lean formalization (planned — the TX-equality proof
arc). Both must implement exactly this contract; the Rust one is
additionally differential-fuzzed against the vendored emulator
(`vendor/picoem-common/src/pio/sm.rs`), which remains the soundness
authority. Provenance for every rule below is that file plus the
RP2350 datasheet §11.

Scope: one PIO block, one state machine, GPIO low bank (pins 0..31).
Multi-SM composition (IRQ handshake, shared instruction memory) is a
later layer; the state already carries `irq_flags` so a joint-state
product machine can be built from two copies.

## 1. State (forkable, memcpy-able)

All evaluator state that varies per cycle lives in one flat `Copy`
struct — this is what the narrowing engine snapshots at a fork point.
Everything else (the program words, decoded config) is immutable per
candidate and lives in a shared config.

    NState:
      pc          : u5           # instruction address
      x, y        : u32          # scratch registers
      isr, osr    : u32          # shift registers
      isr_count   : 0..=32       # bits shifted IN since last clear
      osr_count   : 0..=32       # bits shifted OUT since last refill
      delay_count : u5           # remaining delay cycles
      stall       : Stall        # None | WaitGpio | WaitPin | WaitIrq
                                 #      | Pull | Push | IrqWait
      pending_exec: Option<u16>  # forced instruction (EXEC family)
      irq_flags   : u8           # block-level IRQ flags 0..7
      out_latch   : u32          # shared pin VALUE latch (pad_out)
      dir_latch   : u32          # shared pin DIRECTION latch (pad_oe)
      clk_acc     : u32          # clock-divider accumulator (×256)
      tx, rx      : Fifo         # buf[8], head, count, depth

Notes:
- The pin latches are block-level on hardware; with one SM they are
  state. Side-set, OUT/SET/MOV PINS and PINDIRS all write these same
  latches (HOLD semantics — a value persists until rewritten).
- **`out_latch` initializes to ALL-ONES** (`dir_latch` to 0): a pin
  made an output before anything writes its value drives HIGH. This is
  the vendored emulator's reset state (`shared_pin_values: u32::MAX`)
  and the DME fixtures/certifications encode against it.
- **`osr_count` initializes to 32** ("OSR empty at reset" — all bits
  already shifted out, matching real RP2350): autopull refills on the
  very first OUT, and `jmp !OSRE` is FALSE at reset. `isr_count` is 0.
- `irq_flags` is block-level on hardware; carried here for the IRQ
  instruction and the future 2-SM product machine.
- Deterministic-schedule exclusion: the clkdiv firing pattern and the
  external stimulus depend only on cycle index and config, never on
  program content — a narrowing driver MAY precompute them and share
  across all forks; `clk_acc` stays in state so a plain interpreter
  needs no schedule.

## 2. Config (immutable per candidate)

    NCfg:
      code[32]      : u16        # slot index == instruction address
      wrap_bottom, wrap_top : u5
      side_count    : 0..=5      # PINCTRL_SIDESET_COUNT (incl. enable bit)
      side_en       : bool       # EXECCTRL_SIDE_EN (opt side-set)
      side_pindir   : bool       # side-set drives PINDIRS not PINS
      jmp_pin       : u5
      in_base, out_base, set_base, sideset_base : u5
      out_count     : u6, set_count : u3
      in_count      : u6         # SHIFTCTRL IN_COUNT (0 = no mask); 0 here
      autopush, autopull : bool
      push_threshold, pull_threshold : 1..=32   # (encoded 0 means 32)
      in_shift_right, out_shift_right : bool
      clkdiv_int : u16 (0 ⇒ 65536), clkdiv_frac : u8
      status_sel : bool, status_n : u4          # MOV STATUS; 0/false here
      sm_id      : u2            # for IRQ rel-index resolution
      tx_depth, rx_depth : {0,4,8}   # FJOIN: tx→(8,0), rx→(0,8), else (4,4)

## 3. The system-clock cycle

Per system clock, in order:

    1. gpio_in := compose(state, external stimulus)      # §4
    2. if clock_tick(): execute_cycle(gpio_in)           # §5
    3. observe: levels := compose(state, stimulus); oe := dir_latch

`clock_tick`: `clk_acc += 256; fire iff clk_acc >= T` (then subtract),
where `T = clkdiv_int*256 + clkdiv_frac` (int 0 ⇒ 65536*256). A
disabled SM never ticks; the evaluator is "enabled" from cycle 0.

The SM reads the gpio word composed at the START of the cycle (its own
just-written outputs become visible the NEXT cycle — the emulator's
one-cycle loopback; the hardware input synchronizer's extra delay is a
uniform shift and provably cancels for edge-relative programs).

## 4. GPIO compose

    driven  := out_latch & dir_latch
    gpio_in := (driven & !ext_mask) | (ext_value & ext_mask)

External stimulus OVERRIDES PIO drive on stimulated pins (harness
`set_pin` semantics — a latched output must be asserted via an exec'd
MOV, not stimulus). Pins neither driven nor stimulated read 0.

## 5. execute_cycle(gpio_in)

    if delay_count > 0:  delay_count -= 1; return        # delay burns whole cycles
    if stall != None:
        if still_stalled(gpio_in): return                # §7
        stall := None                                    # fall through: re-execute
    (insn, forced) := pending_exec.take() or (code[pc], false)
    (op, delay, sideset) := decode(insn)                 # §6
    pc_set := exec(op, gpio_in)                          # §8; may set stall
    apply_sideset(sideset)                               # ALWAYS — even when stalled
    if stall == None:
        delay_count := delay
        if !forced and !pc_set: advance_pc()

`advance_pc`: `pc == wrap_top ? wrap_bottom : (pc+1) & 31`.

Key orderings (the classic traps):
- **Side-set asserts on the FIRST cycle even under stall**, and is
  applied AFTER the op's own pin writes (asserted side-set wins on a
  shared pin that cycle).
- Delay is latched only when the instruction COMPLETES (a stalled
  instruction's delay waits until the stall resolves).
- A resolved stall RE-EXECUTES `code[pc]` (fetch is repeated; PC has
  not advanced). A forced instruction that stalls therefore resumes as
  `code[pc]`, NOT the forced word — vendored-emulator behavior, kept.

`force_exec(insn)` (setup path, e.g. `SET PINDIRS`): clear stall and
delay, set `pending_exec := insn`, run one `execute_cycle` outside the
clock divider.

## 6. Decode

Fields: `opcode = insn[15:13]`, `field = insn[12:8]`, `operand = insn[7:0]`.
The 5-bit `field` splits by config: top `side_count` bits are side-set,
bottom `5 - side_count` are delay. With `side_en`, the top side-set bit
is a per-instruction enable — enable clear ⇒ side-set None (opts out,
pins HOLD). Decode is TOTAL: reserved codes execute as no-ops / zero
sources exactly as §8 states — `pending_exec` can carry arbitrary words.

## 7. Stall re-check (`still_stalled`)

    WaitGpio {pol, idx}: gpio_in[idx & 31] != pol
    WaitPin  {pol, idx}: gpio_in[(in_base + idx) & 31] != pol
    WaitIrq  {pol, idx}: irq_flags[idx & 7] != pol      # clear happens on re-execute
    Pull:  tx.is_empty()
    Push:  rx.is_full()
    IrqWait {idx}: irq_flags[idx & 7] still set

## 8. Instruction semantics

`bit_count` fields: raw 0 ⇒ 32. `write_pin_field(latch, value, base,
count)`: no-op when count = 0, else replace `count` bits (mod-32
rotate) starting at `base`.

- **JMP cond target** — conditions: Always; !X (x==0); X-- (taken iff
  x≠0, decrement ONLY when taken); !Y; Y--; X≠Y; PIN (gpio_in[jmp_pin]);
  !OSRE (osr_count < pull_threshold). Taken ⇒ pc := target (pc_set).
- **WAIT pol src idx** — GPIO: absolute pin; PIN: in_base-relative;
  IRQ: idx resolved (§rel), met ⇒ AUTO-CLEAR the flag, unmet ⇒ stall —
  the stall record stores the RESOLVED index (rel already applied), and
  §7's re-check uses it as-is; src 3 (JMPPIN): no-op (vendored stub).
- **IN src count** — pre-shift: if autopush ∧ isr_count ≥ push_threshold:
  rx full ⇒ stall(Push) and return; else push isr, isr := 0, count := 0.
  Then shift `count` bits of src into ISR (right: into MSBs; left: into
  LSBs; src ∈ {PINS (in_base-rotated), X, Y, NULL, —, —, ISR, OSR};
  reserved codes read 0). isr_count := min(32, isr_count + count).
  Post-shift: if autopush ∧ threshold reached ∧ rx has room: push now.
- **OUT dst count** — pre: if autopull ∧ osr_count ≥ pull_threshold:
  refill from tx or stall(Pull) and return. Shift `count` bits out of
  OSR (right: from LSBs; left: from MSBs); osr_count := min(32, +count).
  dst: PINS (out_base/out_count-clipped: count_eff = min(out_count,
  count)); X; Y; NULL; PINDIRS (same clip); PC (:= data & 0x1F, pc_set);
  ISR (:= data; count NOT set — vendored); EXEC (pending_exec :=
  data & 0xFFFF — the shifted-out word truncates to 16 bits).
- **PUSH iffull block** — rx full: iffull ⇒ no-op; block ⇒ stall(Push);
  else DROP (push discarded). Then push isr; isr := 0; isr_count := 0.
- **PULL ifempty block** — tx empty: ifempty ⇒ osr := x, count := 0;
  block ⇒ stall(Pull); else osr := x (nonblocking empty-pull reads X).
  Else osr := pop; osr_count := 0.
- **MOV dst op src** — src: PINS (in_base-rotated, masked by in_count
  when 1..=31), X, Y, NULL, — , STATUS (level(sel ? rx : tx) < status_n
  ? all-ones : 0), ISR, OSR. op: none | invert | bit-reverse. dst:
  PINS (out_base/out_count via write_pin_field — full out_count, no
  clip by a bit count), X, Y, PINDIRS (out range), EXEC (pending_exec
  := val & 0xFFFF), PC (:= val & 0x1F, pc_set), ISR (:= val,
  isr_count := 0? — NO: vendored sets value only, counts untouched),
  OSR (same).
- **IRQ clear wait idx** — idx resolved: bit4 set ⇒ rel: `(((idx&3)+
  sm_id)%4) | (idx&4)`, else idx&7. clear ⇒ clear flag. Else set flag;
  wait ⇒ stall(IrqWait) until someone clears it.
- **SET dst data** — PINS/PINDIRS via set_base/set_count; X/Y :=
  zero-extended data. Reserved dst codes: no-op.

MOV ISR/OSR **do not touch the shift counters**, and OUT ISR does not
set isr_count — these mirror the vendored emulator (fuzz-pinned) and
are flagged for datasheet re-audit if a hardware divergence ever shows.

## 9. The driver/harness contract

The layer between a test vector and the cycle loop, previously implicit
in `run_with_stim` / `narrow_diff.rs` (surfaced by the shard twin):

- **Autopull pad**: when the config has autopull ON, `autopull_pad`
  zero words are appended to `inputs` before anything else.
- **Preload vs streaming**: if the PADDED input list has <= 4 words, it
  is pushed into the TX FIFO once before cycle 0; otherwise it streams —
  before EACH cycle, refill the TX FIFO to full from the remaining
  words, in order. (The threshold is the unjoined hardware FIFO depth,
  applied regardless of the candidate's actual join config.)
- **Output pins**: each listed pin's `dir_latch` bit is set before
  cycle 0 (the register path's exec'd `SET PINDIRS, 1` reduces to this).
- **Stimulus latching**: the external value at cycle i is
  `stim_values[min(i, len-1)]` (empty list = 0) — the last value holds.
- **Capture word**: after the cycle, bit j = level of
  `capture_pins[j]` from §4's compose, bit 16+j = its `dir_latch` bit.

## 10. Trust chain

    shard twin  ==spec==  Rust evaluator  ==diff-fuzz==  vendored emulator
                                                          ==certified==  hardware (bench)

The Rust evaluator is gated by `pio_superopt/tests/narrow_diff.rs`:
byte-identical `trace_pads`-format output vs `run::run` (vendored path)
on the DME reference, random-program fleets across side-set configs and
config genes, and stimulus-driven input programs. Any semantic change
must keep that suite green or explicitly amend this spec (and the shard
twin) in the same change.

Known emulator-vs-hardware deltas to carry into oracle design, not into
this contract: input-synchronizer delay (uniform, cancels), duty-skew
and parked-phase distortion (spec/oracle layer — quantified there, see
STATUS "phase-invariant RX").
