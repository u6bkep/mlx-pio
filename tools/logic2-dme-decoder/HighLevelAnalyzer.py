"""Logic 2 High Level Analyzer for the rs485-eth DME / 4b5b / scrambled frame.

ARCHITECTURE
------------
Logic 2 Python extensions (HLAs) cannot read raw digital samples -- they only
consume frames emitted by another analyzer. So this HLA stacks on top of the
built-in **Manchester** analyzer configured as Bi-Phase Mark / Differential
Manchester at 12.5 Mbit/s with **1 bit per transfer**. Biphase codes carry a
transition at every bit boundary, so the built-in decoder re-synchronises each
bit and tolerates the transmitter's clock skew.

Each input frame therefore delivers one line bit. This HLA reassembles the bit
stream into 5-bit LSB-first line-code symbols, finds the J J H H preamble,
descrambles the payload, strips/verifies the IEEE-802.3 FCS, and annotates:
symbols (J/K/H/T/R/Idle and data nibbles), the frame span, the descrambled
payload bytes (hex), and FCS pass/fail.

All protocol logic lives in dme_core.py (pure Python, shared with the offline
validator test_dme_decode.py), so the live decode is exactly what the offline
cross-check verifies against real captures.

If no valid preamble/FCS ever appears, try switching the Manchester analyzer to
"Bi-Phase Space" (inverts the 1/0 sense) -- see README.
"""

from saleae.analyzers import HighLevelAnalyzer, AnalyzerFrame, ChoicesSetting, NumberSetting

import dme_core as core
import eth_parse


class DmeAnalyzer(HighLevelAnalyzer):
    # How many line bits each input (Manchester) frame carries. Set the
    # Manchester analyzer's "Bits per Transfer" to match (1 is recommended).
    bits_per_word = NumberSetting(label="Bits per input word", min_value=1, max_value=32)
    bit_order = ChoicesSetting(label="Input word bit order", choices=("LSB first", "MSB first"))

    result_types = {
        "delimiter": {"format": "{{data.name}}"},
        "nibble": {"format": "{{data.sym}}"},
        "payload": {"format": "payload {{data.len}}B: {{data.summary}}"},
        "fcs": {"format": "FCS {{data.status}} ({{data.detail}})"},
        "error": {"format": "{{data.msg}}"},
    }

    def __init__(self):
        self._bits = []            # buffered line bits
        self._times = []           # (start_time, end_time) per buffered bit
        self._last_end = None      # end_time of the most recent bit
        # gap (in whole bit periods) after which a stalled buffer is flushed
        self._gap_bits = 6

    # -- input handling -----------------------------------------------------

    def _unpack(self, value):
        """Yield individual bits from one input-frame value."""
        width = int(self.bits_per_word)
        if isinstance(value, (bytes, bytearray)):
            for byte in value:
                yield from self._unpack_int(byte, 8)
            return
        yield from self._unpack_int(int(value), width)

    def _unpack_int(self, value, width):
        if self.bit_order == "MSB first":
            rng = range(width - 1, -1, -1)
        else:
            rng = range(width)
        for shift in rng:
            yield (value >> shift) & 1

    def decode(self, frame: AnalyzerFrame):
        raw = frame.data.get("data") if isinstance(frame.data, dict) else None
        if raw is None:
            return None

        out = []

        # Flush a stalled buffer if there is a long gap before this frame.
        if self._last_end is not None and self._bits:
            gap = float(frame.start_time - self._last_end)
            # crude bit-period estimate from the running buffer
            bp = self._est_bit_period()
            if bp and gap > self._gap_bits * bp:
                out += self._flush(force=True)

        for bit in self._unpack(raw):
            self._bits.append(bit)
            self._times.append((frame.start_time, frame.end_time))
        self._last_end = frame.end_time

        out += self._flush(force=False)
        return out or None

    def _est_bit_period(self):
        if len(self._times) < 2:
            return None
        a = self._times[-2][0]
        b = self._times[-1][0]
        try:
            return float(b - a)
        except Exception:
            return None

    # -- frame emission -----------------------------------------------------

    def _flush(self, force):
        """Try to decode a complete frame from the buffer. On success (or on a
        forced flush of a preamble-bearing buffer) emit annotations and drop the
        consumed bits."""
        align = core.find_frame_alignment(self._bits)
        if align is None:
            # No preamble yet; cap buffer growth during idle noise.
            if len(self._bits) > 20000:
                self._bits = self._bits[-4000:]
                self._times = self._times[-4000:]
            return []

        _phase, start_sym, syms = align
        # find the 'J' at/after start_sym
        j = start_sym
        while j < len(syms) and syms[j].sym != "J":
            j += 1
        if j >= len(syms):
            return []

        fr = core.parse_frame(syms, j)
        if not fr.terminated and not force:
            return []                       # wait for the terminating Idle

        frames = self._emit(fr)

        # Drop bits up to the end of this frame's last symbol.
        if fr.symbols:
            last = fr.symbols[-1]
            consumed_bits = last.bit_index + 5
            self._bits = self._bits[consumed_bits:]
            self._times = self._times[consumed_bits:]
        else:
            self._bits, self._times = [], []
        return frames

    def _sym_span(self, sym):
        i0 = sym.bit_index
        i1 = min(sym.bit_index + 4, len(self._times) - 1)
        return self._times[i0][0], self._times[i1][1]

    def _emit(self, fr: core.Frame):
        out = []
        for s in fr.symbols:
            if s.bit_index + 4 >= len(self._times):
                break
            t0, t1 = self._sym_span(s)
            if s.sym in core.SYM_TO_NIBBLE:
                out.append(AnalyzerFrame("nibble", t0, t1, {"sym": s.sym}))
            else:
                out.append(AnalyzerFrame("delimiter", t0, t1,
                                         {"name": core.sym_name(s.sym)}))

        # Whole-frame payload + FCS annotations span the data region.
        if fr.symbols:
            t0, _ = self._sym_span(fr.symbols[0])
            _, t1 = self._sym_span(fr.symbols[-1])
            if fr.payload:
                info = eth_parse.parse_eth(fr.payload)
                out.append(AnalyzerFrame("payload", t0, t1, {
                    "len": len(fr.payload),
                    "summary": info.summary,
                    "hex": fr.payload.hex(),
                }))
            if fr.fcs_rx is not None:
                out.append(AnalyzerFrame("fcs", t0, t1, {
                    "status": "OK" if fr.fcs_ok else "FAIL",
                    "detail": f"rx={fr.fcs_rx:08x} calc={fr.fcs_calc:08x}",
                }))
            else:
                out.append(AnalyzerFrame("error", t0, t1,
                                         {"msg": fr.note or "no FCS"}))
        return out
