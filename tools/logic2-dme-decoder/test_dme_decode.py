"""Offline validation of the DME decode pipeline against a real capture.

Run with:  pytest test_dme_decode.py -v

The capture is one Neighbor Solicitation frame from a working transmitter,
exported from Logic 2 at 500 MS/s on channel 0. Success criteria (from the
task): the descrambled payload parses as an Ethernet/ICMPv6 NS from MAC
36:b5:26:05:09:27 AND the FCS validates.

The capture path can be overridden with the DME_CAP env var; it defaults to
the scratchpad capture and falls back to a copy checked in next to this file.
"""

import os

import dme_core as core
import eth_parse

EXPECTED_SRC_MAC = "36:b5:26:05:09:27"

_DEFAULT_CAPS = [
    os.environ.get("DME_CAP", ""),
    "/tmp/claude-1000/-home-ben-Documents-programmingSync-pio-optimization/"
    "4fd80004-f72c-468a-ac63-f3fe8eb412d0/scratchpad/cap/digital.csv",
    os.path.join(os.path.dirname(__file__), "sample_capture.csv"),
]


def _cap_path():
    for p in _DEFAULT_CAPS:
        if p and os.path.exists(p):
            return p
    raise FileNotFoundError("no DME capture CSV found; set DME_CAP")


def _read_edges(path):
    import csv
    times = []
    with open(path, newline="") as f:
        rdr = csv.reader(f)
        next(rdr, None)
        for row in rdr:
            if row:
                times.append(float(row[0]))
    return times


def _decode_all(path):
    times = _read_edges(path)
    frames = []
    for a, b in core.split_bursts(times):
        bd = core.decode_biphase(times[a:b])
        fr, _syms, _phase = core.decode_bits_to_frames(bd.bits)
        frames.extend(fr)
    return frames


def test_capture_decodes_to_one_valid_frame():
    frames = _decode_all(_cap_path())
    valid = [f for f in frames if f.fcs_ok]
    assert len(valid) >= 1, f"expected >=1 FCS-valid frame, got {len(valid)}"


def test_fcs_validates():
    frames = _decode_all(_cap_path())
    fr = next(f for f in frames if f.fcs_ok)
    assert fr.fcs_rx == fr.fcs_calc
    # independently recompute over the payload
    assert core.ethernet_crc32(fr.payload) == fr.fcs_rx


def test_frame_is_icmpv6_ns_from_expected_mac():
    frames = _decode_all(_cap_path())
    fr = next(f for f in frames if f.fcs_ok)
    info = eth_parse.parse_eth(fr.payload)
    assert info.src == EXPECTED_SRC_MAC, info.src
    assert info.ethertype == 0x86DD, hex(info.ethertype)
    assert info.fields.get("icmp6_type") == 135, info.fields
    assert info.fields.get("icmp6_name") == "Neighbor Solicitation"


def test_crc32_known_vector():
    # IEEE 802.3 check value, matches lib.rs crc32_known_vector_123456789
    assert core.ethernet_crc32(b"123456789") == 0xCBF4_3926
    assert core.ethernet_crc32(b"") == 0x0000_0000


def test_descrambler_matches_reference_init():
    # State constant mirrors RXProcessor::new() in lib.rs.
    assert core.SCRAMBLER_INIT == 0x0001_2345


if __name__ == "__main__":
    frames = _decode_all(_cap_path())
    for i, fr in enumerate(frames):
        info = eth_parse.parse_eth(fr.payload)
        print(f"frame {i}: fcs_ok={fr.fcs_ok} {info.summary}")
    print("OK" if any(f.fcs_ok for f in frames) else "FAIL")
