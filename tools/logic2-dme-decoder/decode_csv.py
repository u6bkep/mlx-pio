#!/usr/bin/env python3
"""Standalone offline DME decoder for Saleae Logic 2 digital CSV exports.

This is the fully self-contained cross-check tool: it does the ENTIRE decode
(edge timing -> bits -> symbols -> frame -> descramble -> payload -> FCS) in
pure Python, depending on nothing from the Saleae SDK. Export a single digital
channel from Logic 2 (Time [s], value transitions) and run:

    python decode_csv.py path/to/digital.csv [--channel 0] [--half-ns 40]

Use it to sanity-check the live HLA annotation, or on its own from CI.
"""

from __future__ import annotations

import argparse
import csv
import sys

import dme_core as core
import eth_parse


def read_csv_edges(path: str, channel: int = 0):
    """Read a Saleae digital-CSV export -> list of transition times (seconds).

    Saleae exports one header row then `time,value` rows at each transition.
    We only need the timestamps: biphase decoding is polarity-insensitive, so
    the value column (and its inversion vs the receiver) does not matter."""
    times = []
    with open(path, newline="") as f:
        rdr = csv.reader(f)
        next(rdr, None)  # header
        for row in rdr:
            if not row:
                continue
            times.append(float(row[0]))
    return times


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("csv", help="Saleae digital CSV export")
    ap.add_argument("--channel", type=int, default=0)
    ap.add_argument("--half-ns", type=float, default=None,
                    help="half-bit period in ns (default: auto-estimate)")
    ap.add_argument("--gap-ns", type=float, default=300.0,
                    help="inter-frame gap threshold in ns")
    ap.add_argument("--symbols", action="store_true",
                    help="print the full line-code symbol stream per frame")
    args = ap.parse_args(argv)

    times = read_csv_edges(args.csv, args.channel)
    if not times:
        print("no transitions in capture", file=sys.stderr)
        return 2
    span = times[-1] - times[0]
    print(f"{len(times)} transitions over {span:.6f}s")

    bursts = core.split_bursts(times, gap_ns=args.gap_ns)
    print(f"{len(bursts)} burst(s) (frame candidates)\n")

    any_valid = False
    for bi, (a, b) in enumerate(bursts):
        seg = times[a:b]
        bd = core.decode_biphase(seg, half_ns=args.half_ns)
        frames, syms, phase = core.decode_bits_to_frames(bd.bits)
        print(f"burst {bi}: {b - a} edges, half~{bd.half_ns:.0f}ns, "
              f"{len(bd.bits)} bits, {bd.errors} edge errs, "
              f"phase={phase}, {len(frames)} frame(s)")

        if args.symbols and syms:
            print("  symbols:", " ".join(s.sym for s in syms))

        for fi, fr in enumerate(frames):
            _report_frame(fi, fr)
            if fr.fcs_ok:
                any_valid = True
        print()

    return 0 if any_valid else 1


def _report_frame(fi: int, fr: core.Frame) -> None:
    status = "FCS OK" if fr.fcs_ok else "FCS FAIL"
    if fr.fcs_rx is None:
        status = f"no FCS ({fr.note})"
    print(f"  frame {fi}: preamble={'ok' if fr.preamble_ok else 'MISSING'}, "
          f"payload={len(fr.payload)}B, {status}")
    if fr.fcs_rx is not None:
        print(f"    fcs rx=0x{fr.fcs_rx:08x} calc=0x{fr.fcs_calc:08x}")
    if fr.payload:
        info = eth_parse.parse_eth(fr.payload)
        print(f"    {info.summary}")
        hexs = fr.payload.hex()
        shown = " ".join(hexs[i:i + 2] for i in range(0, min(len(hexs), 64), 2))
        more = " ..." if len(fr.payload) > 32 else ""
        print(f"    bytes: {shown}{more}")


if __name__ == "__main__":
    raise SystemExit(main())
