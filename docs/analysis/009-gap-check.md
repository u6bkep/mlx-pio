# 009 gap check: why `set pins,1` and `mov pins,!null` didn't fold

**Question.** Under the champion-census config (SET window == OUT window ==
pin 0, width 1), `set pins,1` (0xE001) and `mov pins,!null` (0xA00B) are
behaviorally AND state-identical, yet coexisted as distinct champions. If
the ticket-009 word quotient were complete, shouldn't the fork-sibling
dedup have pruned one?

## Root cause (two separate findings)

**1. The lemma set had no cross-opcode schema — by construction, not by
failure.** `word_canon` is a static, hand-audited rewrite whitelist; there
is no runtime digest/prover that proposes classes (the batteries only
*verify* the whitelist). Nothing relates SET to MOV, so both words were
their own canon (`canon(E001)=e001`, `canon(A00B)=a00b`): a missing lemma
schema, exactly. The semantics are identical line-by-line in `exec_op`
(narrow/mod.rs): `SET PINS,d` → `write_pin_field(out_latch, d, set_base,
set_count)`; `MOV PINS,!NULL` → `write_pin_field(out_latch, !0, out_base,
out_count)`. With coincident windows and d all-ones, the writes are the
same; neither touches any other state, both are 1 cycle, neither sets PC.

**2. Even a complete word quotient cannot prune this pair — the finding
the census actually exposes is structural.** The 009 dedup only compares
*siblings of the same fork* (`sib_ids` is cleared per fork, engine.rs
~3270-3330), and the opcode field (0xE000) is forked first (`demand`).
Two full words in different opcodes are never siblings of any fork, and at
the opcode fork itself the partial-word class ids require EVERY completion
to pair up — never true across opcodes. So cross-opcode equivalences are
invisible to fork-sibling dedup no matter how complete `word_canon` is.
Folding these champions needs a *champion-level* (or superposition-level)
canonicalization pass — the "static canonicalization program" thread.

**The state-dependent control (`mov pins,!x`, 0xA009).** Writes `~x`; ≡
`set pins,1` only when x is even. Correctly kept separate — but note the
machinery keeps it separate for the same mechanical reason (no lemma), not
because a prover rejected it. The safety net is `word_canon_battery_sound`:
any proposed A009 fold dies on the battery's odd-x states. The gap census
below confirms 0xA009's fingerprint differs from 0xE001/0xA00B.

## Generalization (measured)

New ignored test `word_quotient_gap_census` (tests/narrow_engine.rs):
one-cycle behavioral fingerprint of all 65,536 words over the shared state
battery x 4 gpio words under the census config; groups spanning >1 canon
class = unfolded equivalences (upper bound; exact re-verified, not hashed).
"ENUMERABLE" restricts to words `values_into` can construct — the search
never emits reserved encodings, so only those matter for search behavior.

| | groups | excess classes | words involved |
|---|---|---|---|
| before fix, all words | 3,048 | 11,960 | 27,328 |
| before fix, enumerable | 680 | 1,080 | 5,056 |
| after fix, all words | 2,792 | 11,000 | 19,072 |
| **after fix, enumerable** | **8** | **248** | **512** |

The enumerable gap decomposed into six schema families: (a) SET
PINS/PINDIRS all-ones/all-zeros ≡ MOV PINS/PINDIRS !NULL/NULL when the SET
window is the OUT window; (b) SET X/Y,0 ≡ MOV X/Y,NULL; (c) MOV dst,::NULL
≡ MOV dst,NULL (reverse of 0); (d) MOV PC,const ≡ JMP-always (pc_set path);
(e) IRQ wait bit dead when clear is set; (f) IRQ set+wait: delay dead
behind a never-waking self-set stall. The bulk of the non-enumerable count
is reserved encodings (IN src 4/5, MOV op 3 / src 4, SET dst 3/5-7) that
alias NULL/no-op behavior.

## Fix (landed, lemmas 7-11 in `word_canon`)

Schemas (a)-(e) implemented; each is a few lines, config-parameterized
where needed, and battery-gated. Representative rule learned the hard way:
**the class canon must stay inside the enumerated space** — mapping
`mov pc,!null` → `jmp 31` broke `nop_l1_census_exact` because JMP targets
outside the footprint are never enumerated; the MOV spelling is the rep and
`jmp 31` folds into it. Family (f) is deliberately NOT folded: the delay
deadness holds only because engine semantics never externally clear IRQ
flags; hardware (other SMs / CPU) can, and ticket 010 would break it.

Search impact: cross-opcode folds change nothing (see finding 2); the only
behavior delta is (e) — at the IrqBits fork, `clear` and `clear+wait`
completion profiles now coincide and the wait spelling is sibling-pruned.
A/B on 120 s of `tx_a_l3_22_mine`: quo/item 4.18e-2 → 4.25e-2 at the first
heartbeat, +0.4% cumulative at 47 M items; throughput unchanged; no crash;
`census_l1` still proves exact coverage in both directions.

## Battery hardening (side finding)

The shared state battery had blind spots that also weakened the soundness
gate: `isr` stride `(i*3+1)%9` only produced {1, 31, 0xFFFFFFFF} (every
probed ISR had bit0 = 1 — `mov pindirs,isr` fingerprint-matched `!null`);
no state had a full RX FIFO (PUSH's block bit never consulted); and 24
states couldn't cover joint cases like isr_count==32 AND RX full (needs
i≡6 mod 7 ∧ i≡2 mod 5). Fixed: stride 4, RX fill `(i+2)%5`, 48 states.

## Residual risk / follow-ups

- The gap census is one-cycle and battery-based: necessary, not
  sufficient. True lemma-grade confidence for the new schemas rests on the
  hand audit against `exec_op` (done, documented in the lemma list) plus
  the 4-config battery gate.
- Champion-level dedup (finding 2) is where the census pair actually gets
  folded; `word_canon` is now complete enough to canonicalize per-slot
  spellings for that pass.
- The z3 mirror could re-prove lemmas 7-11 mechanically if wanted.
