"""Minimal Ethernet / IPv6 / ICMPv6 dissection, just enough to label a frame
in a decoder annotation. Not a full parser -- returns a short human string
plus a few structured fields."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional


def mac(b: bytes) -> str:
    return ":".join(f"{x:02x}" for x in b)


ICMP6_TYPES = {
    128: "Echo Request", 129: "Echo Reply",
    133: "Router Solicitation", 134: "Router Advertisement",
    135: "Neighbor Solicitation", 136: "Neighbor Advertisement",
    137: "Redirect",
}


@dataclass
class EthInfo:
    dst: str = ""
    src: str = ""
    ethertype: int = 0
    summary: str = ""
    fields: dict = field(default_factory=dict)


def parse_eth(payload: bytes) -> EthInfo:
    info = EthInfo()
    if len(payload) < 14:
        info.summary = f"short ({len(payload)}B)"
        return info
    info.dst = mac(payload[0:6])
    info.src = mac(payload[6:12])
    info.ethertype = int.from_bytes(payload[12:14], "big")
    et = info.ethertype
    body = payload[14:]

    if et == 0x0806:
        info.summary = f"ARP  {info.src} -> {info.dst}"
    elif et == 0x0800:
        info.summary = f"IPv4 {info.src} -> {info.dst}"
    elif et == 0x86DD:
        info.summary = _parse_ipv6(info, body)
    else:
        info.summary = f"ethertype 0x{et:04x}  {info.src} -> {info.dst}"
    return info


def _parse_ipv6(info: EthInfo, body: bytes) -> str:
    if len(body) < 40:
        return f"IPv6 (truncated) {info.src} -> {info.dst}"
    next_hdr = body[6]
    src6 = _ipv6(body[8:24])
    dst6 = _ipv6(body[24:40])
    info.fields.update(ip_src=src6, ip_dst=dst6, next_header=next_hdr)
    if next_hdr == 58 and len(body) >= 41:      # ICMPv6
        icmp_type = body[40]
        name = ICMP6_TYPES.get(icmp_type, f"type {icmp_type}")
        info.fields["icmp6_type"] = icmp_type
        info.fields["icmp6_name"] = name
        return f"IPv6/ICMPv6 {name}  {src6} -> {dst6}"
    return f"IPv6 next_hdr={next_hdr}  {src6} -> {dst6}"


def _ipv6(b: bytes) -> str:
    parts = [f"{int.from_bytes(b[i:i+2],'big'):x}" for i in range(0, 16, 2)]
    return ":".join(parts)
