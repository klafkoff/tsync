#!/usr/bin/env python3
"""
btsig.py -- BitTorrent protocol signature detection over a packet capture.

Implements the wire-level signatures documented in ../01-PROTOCOL.md:

    peer wire handshake             BEP 3    section 1
    DHT KRPC query/response         BEP 5    section 3
    UDP tracker connect/announce    BEP 15   section 5
    local service discovery         BEP 14   section 5
    uTP header validation           BEP 29   section 4
    MSE/PE candidates via entropy            section 6

This is a read-only research tool. It parses a capture file and reports what
it finds. It does not transmit, modify, or block anything.

    python3 btsig.py capture.pcap
    python3 btsig.py capture.pcap --json

The classification functions operate on plain bytes and have no dependency on
the capture layer, so they can be unit tested directly:

    >>> classify_tcp(b"\\x13BitTorrent protocol" + b"\\x00" * 48)["proto"]
    'bt-handshake'

Capture parsing requires dpkt:  pip install dpkt
"""

from __future__ import annotations

import argparse
import collections
import json
import math
import socket
import struct
import sys

# --------------------------------------------------------------------------
# signatures
# --------------------------------------------------------------------------

BT_HANDSHAKE = b"\x13BitTorrent protocol"
UDP_TRACKER_MAGIC = b"\x00\x00\x04\x17\x27\x10\x19\x80"  # 0x41727101980
LSD_PREFIX = b"BT-SEARCH * HTTP/1.1"
DHT_QUERY = b"d1:ad2:id20:"
DHT_RESPONSE = b"d1:rd2:id20:"
DHT_ERROR = b"d1:eli"

LSD_GROUPS = {"239.192.152.143", "ff15::efc0:988f"}

# BEP 20 Azureus-style client prefixes
PEER_ID_CLIENTS = {
    b"qB": "qBittorrent", b"lt": "libtorrent", b"LT": "libtorrent",
    b"TR": "Transmission", b"UT": "uTorrent", b"DE": "Deluge",
    b"AZ": "Vuze", b"BT": "BitTorrent", b"KT": "KTorrent",
}

# uTP message types (BEP 29)
UTP_TYPES = {0: "ST_DATA", 1: "ST_FIN", 2: "ST_STATE", 3: "ST_RESET", 4: "ST_SYN"}

# --------------------------------------------------------------------------
# primitives
# --------------------------------------------------------------------------


def shannon_entropy(data: bytes) -> float:
    """Shannon entropy in bits per byte. Uniform random approaches 8.0."""
    if not data:
        return 0.0
    counts = collections.Counter(data)
    n = len(data)
    return -sum((c / n) * math.log2(c / n) for c in counts.values())


def printable_stats(data: bytes) -> tuple[float, int]:
    """Return (fraction printable ASCII, longest contiguous printable run)."""
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


def gfw_fully_encrypted(data: bytes) -> tuple[bool, str]:
    """
    Reproduce the exemption heuristics described in Wu et al., USENIX Security
    2023, and summarised in ../01-PROTOCOL.md section 6.

    Returns (looks_fully_encrypted, reason). A flow exempted by any rule is
    NOT treated as fully encrypted; one exempted by none would be blocked by
    the described system.
    """
    if not data:
        return False, "empty"

    set_bits = sum(bin(b).count("1") for b in data)
    frac_set = set_bits / (8 * len(data))
    if not (0.425 <= frac_set <= 0.575):
        return False, f"ex1 popcount {frac_set:.3f} outside random range"

    if len(data) >= 6 and all(0x20 <= b <= 0x7E for b in data[:6]):
        return False, "ex2 first six bytes printable"

    frac_print, longest_run = printable_stats(data)
    if frac_print > 0.5:
        return False, f"ex3 {frac_print:.0%} printable"
    if longest_run >= 20:
        return False, f"ex4 printable run of {longest_run}"

    return True, f"no exemption (popcount {frac_set:.3f}, printable {frac_print:.0%})"


def parse_utp(data: bytes) -> dict | None:
    """
    Validate a candidate uTP header (BEP 29, 20 bytes).

    Detection uses structural consistency rather than a constant pattern,
    which is the approach nDPI takes and the reason its false positive rate
    on this path is low. See ../01-PROTOCOL.md section 4.
    """
    if len(data) < 20:
        return None
    b0 = data[0]
    msg_type, version = b0 >> 4, b0 & 0x0F
    if version != 1 or msg_type not in UTP_TYPES:
        return None
    extension = data[1]
    if extension > 3:
        return None
    conn_id, ts_us, ts_diff, wnd, seq, ack = struct.unpack(">HIIIHH", data[2:20])
    # A plausible window is non-absurd; ST_SYN carries seq_nr 1 in most clients.
    if wnd > (1 << 26):
        return None
    return {
        "type": UTP_TYPES[msg_type], "conn_id": conn_id, "wnd": wnd,
        "seq": seq, "ack": ack, "ts_us": ts_us, "ts_diff": ts_diff,
    }


def parse_peer_id(peer_id: bytes) -> str | None:
    """Decode a BEP 20 Azureus-style peer id. Self-reported and forgeable."""
    if len(peer_id) >= 8 and peer_id[0:1] == b"-" and peer_id[7:8] == b"-":
        client = PEER_ID_CLIENTS.get(peer_id[1:3], peer_id[1:3].decode("latin1"))
        return f"{client} {peer_id[3:7].decode('latin1')}"
    return None


# --------------------------------------------------------------------------
# classification
# --------------------------------------------------------------------------


def classify_tcp(payload: bytes) -> dict | None:
    """Classify the first payload bytes of a TCP flow."""
    if not payload:
        return None

    if payload.startswith(BT_HANDSHAKE):
        out = {"proto": "bt-handshake", "layer": "L1"}
        if len(payload) >= 68:
            reserved = payload[20:28]
            out["info_hash"] = payload[28:48].hex()
            out["reserved"] = reserved.hex()
            out["ext"] = [
                name for name, (i, m) in {
                    "ltep": (5, 0x10), "dht": (7, 0x01),
                    "pex": (7, 0x02), "fast": (7, 0x04), "v2": (7, 0x10),
                }.items() if reserved[i] & m
            ]
            if (client := parse_peer_id(payload[48:68])):
                out["client"] = client
        return out

    if payload.startswith(b"GET /announce?") or b"info_hash=" in payload[:256]:
        return {"proto": "http-tracker", "layer": "L1"}

    encrypted, reason = gfw_fully_encrypted(payload[:96])
    if encrypted and len(payload) >= 96:
        return {
            "proto": "mse-candidate", "layer": "L2",
            "entropy": round(shannon_entropy(payload[:96]), 3), "note": reason,
        }
    return None


def classify_udp(payload: bytes, dst: str, dport: int) -> dict | None:
    """Classify a UDP datagram payload."""
    if not payload:
        return None

    if payload.startswith(UDP_TRACKER_MAGIC):
        action = struct.unpack(">I", payload[8:12])[0] if len(payload) >= 12 else -1
        return {"proto": "udp-tracker", "layer": "L1",
                "action": {0: "connect", 1: "announce", 2: "scrape"}.get(action, action)}

    if payload.startswith(LSD_PREFIX) or (dport == 6771 and dst in LSD_GROUPS):
        info = None
        for line in payload.split(b"\r\n"):
            if line.lower().startswith(b"infohash:"):
                info = line.split(b":", 1)[1].strip().decode("latin1")
        return {"proto": "lsd", "layer": "L1", "info_hash": info}

    if payload.startswith(DHT_QUERY) or payload.startswith(DHT_RESPONSE) \
            or payload.startswith(DHT_ERROR):
        method = None
        if b"1:q" in payload:
            try:
                seg = payload.split(b"1:q", 1)[1]
                ln, rest = seg.split(b":", 1)
                method = rest[: int(ln)].decode("latin1")
            except (ValueError, IndexError):
                pass
        return {"proto": "dht-krpc", "layer": "L1", "method": method}

    if (utp := parse_utp(payload)):
        return {"proto": "utp", "layer": "L1", **utp}

    return None


# --------------------------------------------------------------------------
# capture layer
# --------------------------------------------------------------------------


def iter_ip(path: str):
    """Yield (timestamp, dpkt IP/IP6 packet) from a pcap or pcapng file."""
    try:
        import dpkt
    except ImportError:
        sys.exit("btsig: dpkt is required for capture parsing (pip install dpkt)")

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
                else:  # DLT_RAW and friends
                    pkt = (dpkt.ip6.IP6 if (buf[0] >> 4) == 6 else dpkt.ip.IP)(buf)
            except Exception:
                continue
            if isinstance(pkt, (dpkt.ip.IP, dpkt.ip6.IP6)):
                yield ts, pkt


def addr(raw: bytes) -> str:
    fam = socket.AF_INET if len(raw) == 4 else socket.AF_INET6
    return socket.inet_ntop(fam, raw)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("pcap")
    ap.add_argument("--json", action="store_true", help="emit one JSON object per hit")
    ap.add_argument("--limit", type=int, default=0, help="stop after N hits")
    args = ap.parse_args()

    try:
        import dpkt
    except ImportError:
        return 1  # iter_ip reports

    counts: collections.Counter = collections.Counter()
    clients: collections.Counter = collections.Counter()
    infohashes: set[str] = set()
    seen_tcp: set[tuple] = set()
    hits = 0

    for ts, pkt in iter_ip(args.pcap):
        src, dst = addr(pkt.src), addr(pkt.dst)
        transport = pkt.data

        if isinstance(transport, dpkt.tcp.TCP):
            key = (src, transport.sport, dst, transport.dport)
            if key in seen_tcp or not transport.data:
                continue
            seen_tcp.add(key)  # classify only the first payload of each flow
            result = classify_tcp(bytes(transport.data))
            sport, dport = transport.sport, transport.dport
        elif isinstance(transport, dpkt.udp.UDP):
            result = classify_udp(bytes(transport.data), dst, transport.dport)
            sport, dport = transport.sport, transport.dport
        else:
            continue

        if not result:
            continue

        counts[result["proto"]] += 1
        if (c := result.get("client")):
            clients[c] += 1
        if (h := result.get("info_hash")):
            infohashes.add(h)

        record = {"ts": round(ts, 6), "src": f"{src}:{sport}",
                  "dst": f"{dst}:{dport}", **result}
        if args.json:
            print(json.dumps(record))
        else:
            extra = " ".join(f"{k}={v}" for k, v in result.items()
                             if k not in ("proto", "layer") and v)
            print(f"{ts:.3f}  {result['proto']:<16} {src}:{sport} -> {dst}:{dport}  {extra}")

        hits += 1
        if args.limit and hits >= args.limit:
            break

    if not args.json:
        print("\n--- summary ---")
        for proto, n in counts.most_common():
            print(f"  {proto:<16} {n}")
        if clients:
            print("  clients observed:", ", ".join(f"{c} ({n})" for c, n in clients.most_common()))
        if infohashes:
            print(f"  distinct infohashes: {len(infohashes)}")
        if not counts:
            print("  no BitTorrent signatures found")

    return 0


if __name__ == "__main__":
    sys.exit(main())
