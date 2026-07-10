# RS485-ETH DME protocol decoder for Saleae Logic 2

An independent cross-check decoder for the `rs485-eth` custom 10BASE-T1S-style
line protocol: 12.5 MBd biphase-mark line coding, 4b5b line codes, a 17-bit
self-synchronising scrambler, and IEEE-802.3 framed Ethernet with FCS.

It exists to verify the embedded RX/TX path from the outside: decode a scope
capture with completely separate code and confirm the payload, MAC, and FCS.

There are two deliverables here that share one decode core:

1. **`decode_csv.py`** — a standalone, fully self-contained offline decoder that
   reads a Logic 2 digital-CSV export and does the *entire* decode itself (edge
   timing → bits → symbols → frame → descramble → payload → FCS). This is the
   fully-validated cross-check tool; it depends on nothing from Saleae.
2. **`HighLevelAnalyzer.py`** — a Logic 2 High Level Analyzer that annotates the
   same decode live in the Logic 2 UI, stacked on the built-in Manchester
   analyzer.

Both use the same pure-Python core (`dme_core.py`, `eth_parse.py`), so the live
annotation is exactly what the offline validator checks against real captures.

## Protocol, in one paragraph

The wire is **Biphase-Mark Code** (FM1): a transition at every 80 ns bit
boundary, plus an extra transition at the 40 ns half-bit for a `1` and none for
a `0`. Bits group into **5-bit 4b5b line codes, LSB-first**. A frame is
`J J H H`, then 11 scrambled `0x5` nibbles and one scrambled `0xD` SFD nibble
(12 preamble nibbles the receiver discards while it re-syncs the scrambler),
then the scrambled payload (2 nibbles per byte, low nibble first) including the
4-byte Ethernet FCS, then `T R` and Idle. Decoding is polarity-insensitive
because it works from transition *timing*.

## Architecture: why this shape

Logic 2 Python HLAs **cannot read raw digital samples** — they only consume
frames produced by another analyzer. So the live extension stacks on the
built-in **Manchester** analyzer (set to Bi-Phase Mark / Differential
Manchester, 1 bit per transfer). Biphase codes carry a guaranteed transition at
every bit boundary, so the built-in decoder re-synchronises on each bit and
tolerates the transmitter's clock skew (this capture runs ~30–40 ns half-bits
against a 40 ns nominal, and still decodes cleanly). The HLA then reassembles
the bit stream into 5-bit symbols and runs all protocol logic itself.

The alternative — a full custom low-level analyzer reading raw edges — requires
the C++ Analyzer SDK (compiled per-platform), which is much heavier for a
cross-check tool. The offline `decode_csv.py` already fills the "decode raw
edges with our own code" role and is the fully-validated artifact, so the HLA is
kept as the lightweight live-annotation convenience.

### Honest validation boundary

The offline pipeline (`decode_csv.py` / the pytest suite) is validated
end-to-end against a real capture (see below). The HLA's *reassembly and
emission* logic is also unit-tested against the same real bits (via a stubbed
SDK in `test_hla_stream.py`). The one seam **not** verified here is the built-in
Manchester analyzer's exact bit output on live hardware — that can only be
confirmed inside the Logic 2 GUI. If the live decode looks wrong, see
"Troubleshooting" below.

## Offline decoder / validator

Uses a `uv`-managed venv (system pip is off-limits on this machine):

```bash
cd tools/logic2-dme-decoder
uv venv
uv pip install -r requirements-dev.txt

# decode a capture
.venv/bin/python decode_csv.py sample_capture.csv          # add --symbols for the full symbol stream

# run the validation suite
.venv/bin/python -m pytest -v
```

`sample_capture.csv` is the reference capture (one ICMPv6 Neighbor Solicitation
from a working transmitter, 500 MS/s, channel 0). Expected result:

```
frame 0: preamble=ok, payload=86B, FCS OK
  IPv6/ICMPv6 Neighbor Solicitation  fe80::34b5:26ff:fe05:927 -> ff02::1:ff7f:c5d0
```

Source MAC `36:b5:26:05:09:27`, FCS validates. The pytest suite asserts exactly
this (FCS-valid frame, ICMPv6 NS, expected MAC) plus the CRC-32 check vector.

## Loading the HLA in Logic 2

1. Capture the single-ended DME signal on **channel 0 at 500 MS/s** (≥80 MS/s is
   the analyzer minimum for 12.5 MBd biphase; 500 MS/s gives clean edges).
2. Add the built-in **Manchester** analyzer on channel 0:
   - Mode: **Bi-Phase Mark** (a.k.a. Differential Manchester — mid-bit
     transition = `1`).
   - Bit Rate: **12500000** (12.5 Mbit/s).
   - Bits per Transfer: **1**.
   - Bit order / idle level: leave default; decoding is polarity-insensitive.
3. Open the **Extensions** panel → three-dots menu → **Load Existing
   Extension…** and pick this folder's `extension.json`.
4. Add the **RS485-ETH DME** analyzer and set its input to the Manchester
   analyzer above. Leave "Bits per input word" = 1 and "LSB first".

Annotations produced: each line-code symbol (`Start-J`, `Halt`, data nibbles,
`End-T`, `Reset-R`, `Idle`, …), a payload bubble with byte count and an
Ethernet/ICMPv6 one-line summary (full hex in `data.hex`), and an FCS `OK`/`FAIL`
bubble showing received vs recomputed CRC.

## Troubleshooting

- **No frames / all-garbage symbols:** the 1/0 sense may be inverted for your
  transmitter. Switch the Manchester analyzer mode to **Bi-Phase Space** and
  re-check. If symbols look shifted, confirm Bits per Transfer = 1.
- **FCS FAIL but symbols look sane:** likely a genuine RX-side bug worth
  chasing, or a capture with a truncated/late frame. Cross-check by exporting
  the same capture to CSV and running `decode_csv.py` on it.
- **Nothing decodes at 12.5 MBd:** verify sample rate ≥ 80 MS/s and that the
  probe is on the single-ended output of the RS485 receiver, not the
  differential pair.

## Files

| File | Purpose |
|------|---------|
| `dme_core.py` | Pure-Python decode core: biphase decode, 4b5b, framer, descrambler, FCS. No SDK deps. |
| `eth_parse.py` | Minimal Ethernet / IPv6 / ICMPv6 dissection for the summary line. |
| `decode_csv.py` | Standalone offline decoder for Logic 2 CSV exports. |
| `HighLevelAnalyzer.py` | The Logic 2 HLA (stacks on the Manchester analyzer). |
| `extension.json` | Logic 2 extension manifest. |
| `test_dme_decode.py` | Offline validation of the full pipeline against `sample_capture.csv`. |
| `test_hla_stream.py` | Validates the HLA's streaming reassembly against the same real bits (stubbed SDK). |
| `sample_capture.csv` | Reference capture (ICMPv6 NS from `36:b5:26:05:09:27`). |

## Provenance

Ported exactly from `reference/rs485-eth/src/lib.rs` and `line_code.rs`:
the `ethernet_crc32` CRC, the `descramble_nibble` scrambler (17-bit, init
`0x12345`, taps at state bits [13:10] and [16:13] with 4-bit `rev4` reversal,
shifting in the received scrambled nibble), the 4b5b table, and the
`J J H H` + discard-12 preamble handling. The scrambler is self-synchronising,
so the descrambler locks within the 12 discarded preamble nibbles regardless of
initial state.
