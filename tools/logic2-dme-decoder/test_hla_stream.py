"""Validate the HLA's streaming reassembly/emission without the Saleae SDK.

We inject a stub `saleae.analyzers` module so HighLevelAnalyzer.py imports, then
feed the analyzer one fake per-bit input frame at a time (bits taken from the
real capture) and assert it emits a payload frame and an FCS-OK frame. This
exercises the code path unique to the live extension -- bit reassembly, frame
alignment, symbol spans, gap flush -- against real data.
"""

import os
import sys
import types

import dme_core as core
import test_dme_decode as tv


# ---- stub the Saleae SDK before importing the HLA -------------------------

def _install_saleae_stub():
    mod = types.ModuleType("saleae")
    ana = types.ModuleType("saleae.analyzers")

    class AnalyzerFrame:
        def __init__(self, type, start_time, end_time, data=None):
            self.type = type
            self.start_time = start_time
            self.end_time = end_time
            self.data = data or {}

    class HighLevelAnalyzer:
        pass

    class _Setting:
        def __init__(self, *a, **k):
            self.default = k.get("choices", (None,))[0] if "choices" in k else k.get("min_value", 1)

    ana.HighLevelAnalyzer = HighLevelAnalyzer
    ana.AnalyzerFrame = AnalyzerFrame
    ana.ChoicesSetting = _Setting
    ana.NumberSetting = _Setting
    ana.StringSetting = _Setting
    mod.analyzers = ana
    sys.modules["saleae"] = mod
    sys.modules["saleae.analyzers"] = ana


_install_saleae_stub()
import HighLevelAnalyzer as hla_mod  # noqa: E402


def _bits_from_capture():
    times = tv._read_edges(tv._cap_path())
    a, b = core.split_bursts(times)[0]
    bd = core.decode_biphase(times[a:b])
    # per-bit time ranges as plain floats (stub AnalyzerFrame accepts anything)
    return bd.bits, bd.bit_times


def _make_hla():
    h = hla_mod.DmeAnalyzer.__new__(hla_mod.DmeAnalyzer)
    # settings are class-level stubs; set the instance values we rely on
    h.bits_per_word = 1
    h.bit_order = "LSB first"
    h._bits = []
    h._times = []
    h._last_end = None
    h._gap_bits = 6
    return h


def test_hla_streams_one_valid_frame():
    bits, bit_times = _bits_from_capture()
    h = _make_hla()
    Frame = sys.modules["saleae.analyzers"].AnalyzerFrame

    emitted = []
    for bit, (t0, t1) in zip(bits, bit_times):
        f = Frame("data", t0, t1, {"data": bit})
        res = h.decode(f)
        if res:
            emitted.extend(res)
    # trailing flush (capture may lack a final gap)
    res = h._flush(force=True)
    if res:
        emitted.extend(res)

    payloads = [f for f in emitted if f.type == "payload"]
    fcs = [f for f in emitted if f.type == "fcs"]
    assert payloads, "HLA emitted no payload frame"
    assert any(f.data["status"] == "OK" for f in fcs), \
        f"no FCS-OK frame; got {[f.data for f in fcs]}"
    assert payloads[0].data["len"] == 86, payloads[0].data
    assert "Neighbor Solicitation" in payloads[0].data["summary"]

    # delimiter + nibble annotations should also be present
    assert any(f.type == "delimiter" and f.data["name"] == "Start-J" for f in emitted)
    assert any(f.type == "nibble" for f in emitted)


if __name__ == "__main__":
    test_hla_streams_one_valid_frame()
    print("HLA stream test OK")
