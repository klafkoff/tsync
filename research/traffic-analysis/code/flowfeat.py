#!/usr/bin/env python3
"""
flowfeat.py -- statistical flow and host feature extraction.

Computes the features catalogued in ../04-METHODOLOGY.md section 4. Every
feature here is derived from packet headers, sizes, directions, and
timestamps only, so all of them survive payload encryption. That is the whole
point: these are what remains observable after MSE, and what a classifier
operating on encrypted traffic actually uses.

    python3 flowfeat.py capture.pcap                 # host summary
    python3 flowfeat.py capture.pcap --flows         # per-flow records (JSON)
    python3 flowfeat.py capture.pcap --host 10.0.0.5 # restrict to one host

Host-level features are reported separately from flow-level ones because, as
argued in 04-METHODOLOGY.md section 4, per-flow classification is the wrong
unit of analysis for this protocol. The distinguishing behaviour is a property
of a host's aggregate communication pattern.

Requires dpkt:  pip install dpkt
"""

from __future__ import annotations

import argparse
import collections
import json
import math
import socket
import statistics
import struct
import sys

SPLT_LEN = 20          # packets retained per flow for the size/time sequence
FIRST_PAYLOAD = 96     # bytes of first payload kept for entropy analysis


# --------------------------------------------------------------------------
# helpers shared with btsig.py (duplicated to keep each script standalone)
# --------------------------------------------------------------------------


def shannon_entropy(data: bytes) -> float:
    if not data:
        return 0.0
    counts = collections.Counter(data)
    n = len(data)
    return -sum((c / n) * math.log2(c / n) for c in counts.values())


def printable_stats(data: bytes) -> tuple[float, int]:
    if not data:
        return 0.0, 0
    count = run = best = 0
    for b in data:
        if 0x20 <= b <= 0x7E:
            count += 1
            run += 1
            best = max(best, run)
        else:
            run = 0
    return count / len(data), best


def prefix24(ip: str) -> str:
    return ip.rsplit(".", 1)[0] if ":" not in ip else ip.split(":")[0]


# --------------------------------------------------------------------------
# flow state
# --------------------------------------------------------------------------


class Flow:
    __slots__ = ("proto", "a", "b", "first_ts", "last_ts", "splt",
                 "bytes_ab", "bytes_ba", "pkts_ab", "pkts_ba",
                 "sizes", "first_payload", "saw_syn", "saw_synack", "saw_rst")

    def __init__(self, proto: str, a: tuple, b: tuple, ts: float):
        self.proto = proto
        self.a, self.b = a, b            # a is the initiator
        self.first_ts = self.last_ts = ts
        self.splt: list[tuple[int, float]] = []   # (signed size, delta t)
        self.bytes_ab = self.bytes_ba = 0
        self.pkts_ab = self.pkts_ba = 0
        self.sizes: list[int] = []
        self.first_payload = b""
        self.saw_syn = self.saw_synack = self.saw_rst = False

    def add(self, ts: float, size: int, forward: bool, payload: bytes) -> None:
        if len(self.splt) < SPLT_LEN:
            self.splt.append((size if forward else -size, round(ts - self.last_ts, 6)))
        self.last_ts = ts
        self.sizes.append(size)
        if forward:
            self.bytes_ab += size
            self.pkts_ab += 1
        else:
            self.bytes_ba += size
            self.pkts_ba += 1
        if payload and not self.first_payload:
            self.first_payload = payload[:FIRST_PAYLOAD]

    def features(self) -> dict:
        total = self.bytes_ab + self.bytes_ba
        duration = max(self.last_ts - self.first_ts, 1e-6)
        frac_print, run = printable_stats(self.first_payload)

        # Bimodality: BitTorrent interleaves 5-17 byte control messages with
        # ~16.4 KiB piece messages, which few other protocols do per-flow.
        small = sum(1 for s in self.sizes if s <= 100)
        large = sum(1 for s in self.sizes if s >= 1000)
        n = len(self.sizes) or 1

        return {
            "proto": self.proto,
            "src": f"{self.a[0]}:{self.a[1]}", "dst": f"{self.b[0]}:{self.b[1]}",
            "duration_s": round(duration, 3),
            "bytes_up": self.bytes_ab, "bytes_down": self.bytes_ba,
            "pkts_up": self.pkts_ab, "pkts_down": self.pkts_ba,
            "up_down_ratio": round(self.bytes_ab / max(self.bytes_ba, 1), 3),
            "throughput_bps": round(total * 8 / duration),
            "size_mean": round(statistics.fmean(self.sizes), 1) if self.sizes else 0,
            "size_stdev": round(statistics.pstdev(self.sizes), 1) if len(self.sizes) > 1 else 0,
            "frac_small": round(small / n, 3),
            "frac_large": round(large / n, 3),
            "bimodal": round((small / n) * (large / n) * 4, 3),   # peaks at 1.0 when evenly split
            "first_payload_entropy": round(shannon_entropy(self.first_payload), 3),
            "first_payload_printable": round(frac_print, 3),
            "first_payload_run": run,
            "handshake_completed": self.saw_synack,
            "reset": self.saw_rst,
            "splt": self.splt,
        }


# --------------------------------------------------------------------------
# extraction
# --------------------------------------------------------------------------


def addr(raw: bytes) -> str:
    return socket.inet_ntop(socket.AF_INET if len(raw) == 4 else socket.AF_INET6, raw)


def iter_ip(path: str):
    try:
        import dpkt
    except ImportError:
        sys.exit("flowfeat: dpkt is required (pip install dpkt)")

    with open(path, "rb") as fh:
        magic = fh.read(4)
        fh.seek(0)
        reader = (dpkt.pcapng.Reader(fh) if magic == b"\x0a\x0d\x0d\x0a"
                  else dpkt.pcap.Reader(fh))
        link = reader.datalink()
        for ts, buf in reader:
            try:
                if link == dpkt.pcap.DLT_EN10MB:
                    pkt = dpkt.ethernet.Ethernet(buf).data
                elif link in (dpkt.pcap.DLT_NULL, dpkt.pcap.DLT_LOOP):
                    fam = struct.unpack("I", buf[:4])[0]
                    pkt = (dpkt.ip.IP if fam == 2 else dpkt.ip6.IP6)(buf[4:])
                else:
                    pkt = (dpkt.ip6.IP6 if (buf[0] >> 4) == 6 else dpkt.ip.IP)(buf)
            except Exception:
                continue
            if isinstance(pkt, (dpkt.ip.IP, dpkt.ip6.IP6)):
                yield ts, pkt


def extract(path: str, host_filter: str | None = None):
    import dpkt

    flows: dict[tuple, Flow] = {}
    # host-level accumulators
    peers: dict[str, set] = collections.defaultdict(set)
    prefixes: dict[str, set] = collections.defaultdict(set)
    syn_sent: dict[str, int] = collections.Counter()
    syn_acked: dict[str, int] = collections.Counter()

    for ts, pkt in iter_ip(path):
        src, dst = addr(pkt.src), addr(pkt.dst)
        t = pkt.data

        if isinstance(t, dpkt.tcp.TCP):
            proto, payload = "tcp", bytes(t.data)
            is_syn = bool(t.flags & dpkt.tcp.TH_SYN)
            is_ack = bool(t.flags & dpkt.tcp.TH_ACK)
            is_rst = bool(t.flags & dpkt.tcp.TH_RST)
        elif isinstance(t, dpkt.udp.UDP):
            proto, payload = "udp", bytes(t.data)
            is_syn = is_ack = is_rst = False
        else:
            continue

        if host_filter and host_filter not in (src, dst):
            continue

        a, b = (src, t.sport), (dst, t.dport)
        key = (proto,) + tuple(sorted([a, b]))
        flow = flows.get(key)
        if flow is None:
            flow = flows[key] = Flow(proto, a, b, ts)
        forward = (a == flow.a)

        size = len(pkt.data) if not hasattr(pkt, "len") else max(pkt.len - 20, len(t))
        flow.add(ts, len(t), forward, payload)

        if proto == "tcp":
            if is_syn and not is_ack:
                flow.saw_syn = True
                syn_sent[src] += 1
            elif is_syn and is_ack:
                flow.saw_synack = True
                syn_acked[dst] += 1
            if is_rst:
                flow.saw_rst = True

        peers[src].add(dst)
        prefixes[src].add(prefix24(dst))

    return flows, peers, prefixes, syn_sent, syn_acked


def host_features(flows, peers, prefixes, syn_sent, syn_acked) -> list[dict]:
    """
    Host-level aggregates. These are consistently the strongest discriminators
    for BitTorrent, per 04-METHODOLOGY.md section 4.
    """
    out = []
    by_host: dict[str, list[Flow]] = collections.defaultdict(list)
    for f in flows.values():
        by_host[f.a[0]].append(f)

    for host, hf in sorted(by_host.items(), key=lambda kv: -len(kv[1])):
        distinct_prefixes = prefixes.get(host, set())
        # Shannon entropy over the /24 distribution: concentration vs dispersion.
        counts = collections.Counter(prefix24(f.b[0]) for f in hf)
        n = sum(counts.values()) or 1
        prefix_entropy = -sum((c / n) * math.log2(c / n) for c in counts.values())

        sent, acked = syn_sent.get(host, 0), syn_acked.get(host, 0)
        up = sum(f.bytes_ab for f in hf)
        down = sum(f.bytes_ba for f in hf)

        out.append({
            "host": host,
            "flows": len(hf),
            "distinct_peers": len(peers.get(host, set())),
            "distinct_24s": len(distinct_prefixes),
            "prefix_entropy_bits": round(prefix_entropy, 3),
            "syn_sent": sent,
            "syn_unanswered": max(sent - acked, 0),
            "failure_ratio": round(max(sent - acked, 0) / sent, 3) if sent else 0.0,
            "bytes_up": up, "bytes_down": down,
            "up_down_ratio": round(up / max(down, 1), 3),
            "mean_flow_duration_s": round(
                statistics.fmean([f.last_ts - f.first_ts for f in hf]), 2) if hf else 0,
        })
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("pcap")
    ap.add_argument("--flows", action="store_true", help="emit per-flow JSON records")
    ap.add_argument("--host", help="restrict to flows involving this address")
    ap.add_argument("--min-packets", type=int, default=2)
    args = ap.parse_args()

    flows, peers, prefixes, syn_sent, syn_acked = extract(args.pcap, args.host)

    if args.flows:
        for f in flows.values():
            if f.pkts_ab + f.pkts_ba >= args.min_packets:
                print(json.dumps(f.features()))
        return 0

    rows = host_features(flows, peers, prefixes, syn_sent, syn_acked)
    if not rows:
        print("no flows extracted")
        return 0

    print(f"{'host':<22}{'flows':>7}{'/24s':>7}{'H(pfx)':>8}"
          f"{'fail%':>7}{'up:dn':>8}{'dur_s':>8}")
    print("-" * 67)
    for r in rows[:40]:
        print(f"{r['host']:<22}{r['flows']:>7}{r['distinct_24s']:>7}"
              f"{r['prefix_entropy_bits']:>8.2f}{r['failure_ratio'] * 100:>6.0f}%"
              f"{r['up_down_ratio']:>8.2f}{r['mean_flow_duration_s']:>8.1f}")

    print("\nInterpretation (see ../02-DETECTION.md section L2):")
    print("  high /24 count + high prefix entropy  -> address dispersion, characteristic of swarms")
    print("  high failure ratio                    -> stale peer lists; rare in client-server traffic")
    print("  up:dn well above 1 on many flows      -> seeding")
    return 0


if __name__ == "__main__":
    sys.exit(main())
