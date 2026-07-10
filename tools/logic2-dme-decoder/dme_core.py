"""Pure-Python decode core for the rs485-eth DME / 4b5b / scrambled protocol.

This module has NO dependency on the Saleae SDK so it can be exercised
offline (see test_dme_decode.py / decode_csv.py) and reused verbatim by the
Logic 2 High Level Analyzer (HighLevelAnalyzer.py).

Pipeline (matches reference/rs485-eth/src/{lib.rs,line_code.rs}):

    line edges --(biphase-mark)--> bits --(5b LSB-first)--> line-code symbols
      --> frame parse (J J H H, discard 12 preamble nibbles, collect payload)
      --> 17-bit self-synchronising descrambler --> payload bytes
      --> strip/verify IEEE-802.3 FCS (CRC-32) --> Ethernet frame

The wire is Biphase-Mark Code (a.k.a. FM1 / one flavour of "differential
Manchester"): a transition at EVERY bit boundary, plus an extra mid-bit
transition for a '1' and none for a '0'. Decoding is done from transition
*timing*, so absolute polarity is irrelevant.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional


# ---------------------------------------------------------------------------
# 4b5b line-code table (canonical 5-bit values, from line_code.rs)
# ---------------------------------------------------------------------------

# canonical 5-bit value -> display symbol
CODE_TO_SYM = {
    0x1E: "0", 0x09: "1", 0x14: "2", 0x15: "3",
    0x0A: "4", 0x0B: "5", 0x0E: "6", 0x0F: "7",
    0x12: "8", 0x13: "9", 0x16: "A", 0x17: "B",
    0x1A: "C", 0x1B: "D", 0x1C: "E", 0x1D: "F",
    0x00: "Q",  # Quiet
    0x1F: "I",  # Idle
    0x04: "H",  # Halt
    0x18: "J",  # Start-of-stream delimiter 1
    0x11: "K",  # Start-of-stream delimiter 2
    0x0D: "T",  # End delimiter
    0x07: "R",  # Reset
    0x01: "V",  # Invalid
}

# symbol char -> data nibble (only for the 16 data codes)
SYM_TO_NIBBLE = {
    "0": 0x0, "1": 0x1, "2": 0x2, "3": 0x3, "4": 0x4, "5": 0x5,
    "6": 0x6, "7": 0x7, "8": 0x8, "9": 0x9, "A": 0xA, "B": 0xB,
    "C": 0xC, "D": 0xD, "E": 0xE, "F": 0xF,
}


def sym_name(sym: str) -> str:
    """Human label for a symbol char."""
    return {
        "J": "Start-J", "K": "Start-K", "H": "Halt", "T": "End-T",
        "R": "Reset-R", "I": "Idle", "Q": "Quiet", "V": "Invalid",
    }.get(sym, sym)


# ---------------------------------------------------------------------------
# IEEE 802.3 FCS (CRC-32/reflected) -- exact port of ethernet_crc32()
# ---------------------------------------------------------------------------

def ethernet_crc32(data: bytes) -> int:
    crc = 0xFFFF_FFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            mask = -(crc & 1) & 0xFFFF_FFFF
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask)
    return (~crc) & 0xFFFF_FFFF


# ---------------------------------------------------------------------------
# 17-bit self-synchronising descrambler -- exact port of descramble_nibble()
# ---------------------------------------------------------------------------

_REV4 = [0x0, 0x8, 0x4, 0xC, 0x2, 0xA, 0x6, 0xE,
         0x1, 0x9, 0x5, 0xD, 0x3, 0xB, 0x7, 0xF]

SCRAMBLER_INIT = 0x0001_2345


class Descrambler:
    """Nibble-at-a-time descrambler; state re-syncs after ~5 input nibbles."""

    def __init__(self, state: int = SCRAMBLER_INIT):
        self.state = state & 0x1FFFF

    def nibble(self, scrambled: int) -> int:
        s = self.state & 0x1FFFF
        tap13 = _REV4[(s >> 10) & 0xF]
        tap16 = _REV4[(s >> 13) & 0xF]
        out = (scrambled ^ tap13 ^ tap16) & 0xF
        ins = _REV4[scrambled & 0xF]           # input scrambled bits shift in
        self.state = ((s << 4) & 0x1FFFF) | ins
        return out


# ---------------------------------------------------------------------------
# Biphase-Mark decode: transition edge times -> bits
# ---------------------------------------------------------------------------

def split_bursts(times, gap_ns: float = 300.0):
    """Split a monotonically increasing edge-time list (seconds) into bursts,
    breaking wherever the inter-edge gap exceeds `gap_ns`. Returns a list of
    (start_index, end_index_exclusive) into `times`."""
    if not times:
        return []
    bursts = []
    start = 0
    for i in range(1, len(times)):
        if (times[i] - times[i - 1]) * 1e9 > gap_ns:
            if i - start >= 10:
                bursts.append((start, i))
            start = i
    if len(times) - start >= 10:
        bursts.append((start, len(times)))
    return bursts


@dataclass
class BitDecode:
    bits: list                    # 0/1 values in transmission order
    bit_times: list               # (start_s, end_s) per bit
    errors: int                   # runs that could not be classified
    half_ns: float                # estimated half-bit period


def decode_biphase(times, half_ns: Optional[float] = None) -> BitDecode:
    """Decode one burst of transition times (seconds) into a bit stream.

    A full-period run (no mid-bit transition) is a '0'; two consecutive
    half-period runs (a mid-bit transition) is a '1'. The half-bit period is
    estimated from the data when not supplied, so moderate clock skew is fine.
    """
    if len(times) < 2:
        return BitDecode([], [], 0, half_ns or 40.0)

    runs = [(times[i + 1] - times[i]) * 1e9 for i in range(len(times) - 1)]

    if half_ns is None:
        # Runs cluster at the half-bit period and at ~2x it. For scrambled
        # (near-random) data a '1' contributes two half runs and a '0' one
        # full run, so half-runs outnumber full-runs ~2:1 and the median of
        # all runs sits inside the half cluster. Estimate the half period as
        # the median of the runs at or below 1.5x that median.
        srt = sorted(runs)
        m = srt[len(srt) // 2]
        halves = sorted(r for r in runs if r <= 1.5 * m)
        half_ns = halves[len(halves) // 2] if halves else m

    thresh = 1.5 * half_ns          # < thresh => half, else => full
    max_full = 2.6 * half_ns        # beyond this the run is unclassifiable

    bits = []
    bit_times = []
    errors = 0
    i = 0
    n = len(runs)
    while i < n:
        r = runs[i]
        if r > max_full:
            errors += 1
            i += 1
            continue
        if r >= thresh:
            # full period, no mid transition -> '0'
            bits.append(0)
            bit_times.append((times[i], times[i + 1]))
            i += 1
        else:
            # half period; expect a second half to complete a '1'
            if i + 1 < n and runs[i + 1] < thresh:
                bits.append(1)
                bit_times.append((times[i], times[i + 2]))
                i += 2
            else:
                errors += 1
                i += 1
    return BitDecode(bits, bit_times, errors, half_ns)


# ---------------------------------------------------------------------------
# bits -> line-code symbols (5 bits, LSB-first) with frame alignment
# ---------------------------------------------------------------------------

@dataclass
class Symbol:
    sym: str            # display char (J/K/H/T/R/I/Q/V or 0-F)
    value: int          # canonical 5-bit value
    bit_index: int      # index of first bit of this symbol in the bit stream


def bits_to_symbols(bits, phase: int = 0):
    """Group bits into 5-bit LSB-first symbols starting at bit offset `phase`."""
    syms = []
    j = phase
    while j + 5 <= len(bits):
        v = 0
        for k in range(5):
            v |= bits[j + k] << k
        syms.append(Symbol(CODE_TO_SYM.get(v, "?"), v, j))
        j += 5
    return syms


def find_frame_alignment(bits):
    """Find (phase, symbol_start_index) of a 'J J H H' preamble, trying all
    five bit phases. Returns None if no preamble is found."""
    for phase in range(5):
        syms = bits_to_symbols(bits, phase)
        chars = [s.sym for s in syms]
        for start in range(len(chars) - 3):
            if chars[start:start + 4] == ["J", "J", "H", "H"]:
                return phase, start, syms
    return None


# ---------------------------------------------------------------------------
# Frame state machine + descramble -- mirrors RXProcessor in lib.rs
# ---------------------------------------------------------------------------

@dataclass
class Frame:
    symbols: list = field(default_factory=list)   # list[Symbol] incl preamble/trailer
    preamble_ok: bool = False
    payload: bytes = b""            # descrambled bytes with FCS stripped
    fcs_rx: Optional[int] = None
    fcs_calc: Optional[int] = None
    fcs_ok: bool = False
    complete: bool = False          # ran the state machine to a stop
    terminated: bool = False        # actually consumed a terminating Idle
    note: str = ""


def parse_frame(symbols, start: int) -> Frame:
    """Run the RX state machine over `symbols` starting at index `start`
    (which should point at the first 'J'). Consumes up to the terminating
    Idle. Returns a decoded Frame."""
    fr = Frame()
    desc = Descrambler()
    discard = 0
    nibbles = []          # descrambled payload nibbles, in order
    saw_halt = False

    i = start
    while i < len(symbols):
        s = symbols[i]
        fr.symbols.append(s)
        c = s.sym
        if c in SYM_TO_NIBBLE:
            nib = SYM_TO_NIBBLE[c]
            if discard > 0:
                desc.nibble(nib)      # advance state over preamble, drop
                discard -= 1
            else:
                nibbles.append(desc.nibble(nib))
        elif c == "H":
            discard = 12              # 11 scrambled 0x5 + 1 SFD nibble
            saw_halt = True
        elif c == "I":               # Idle terminates the frame
            fr.terminated = True
            i += 1
            break
        elif c in ("J", "K", "T", "R", "Q"):
            pass                      # delimiters: no data
        else:                         # V / '?' invalid line code
            pass
        i += 1

    fr.preamble_ok = saw_halt
    fr.complete = True

    # Pair nibbles (low, high) into bytes.
    data = bytearray()
    for k in range(0, len(nibbles) - 1, 2):
        data.append(nibbles[k] | (nibbles[k + 1] << 4))
    data = bytes(data)

    if len(data) >= 4:
        fr.fcs_rx = int.from_bytes(data[-4:], "little")
        fr.fcs_calc = ethernet_crc32(data[:-4])
        fr.fcs_ok = fr.fcs_rx == fr.fcs_calc
        fr.payload = data[:-4]
    else:
        fr.payload = data
        fr.note = "runt (<4 bytes)"
    return fr


# ---------------------------------------------------------------------------
# Top-level convenience: bit stream -> list[Frame]
# ---------------------------------------------------------------------------

def decode_bits_to_frames(bits):
    """Locate every 'J J H H' preamble in a bit stream and decode each frame.
    Returns (frames, symbols, phase) where symbols/phase are for the first
    aligned decode (useful for annotation)."""
    align = find_frame_alignment(bits)
    if align is None:
        return [], [], 0
    phase, first_start, syms = align
    frames = []
    idx = first_start
    while idx < len(syms):
        # advance to the next 'J' at or after idx
        while idx < len(syms) and syms[idx].sym != "J":
            idx += 1
        if idx >= len(syms):
            break
        fr = parse_frame(syms, idx)
        frames.append(fr)
        # jump past the symbols this frame consumed
        idx += len(fr.symbols)
    return frames, syms, phase
