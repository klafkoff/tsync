# Analysis tools

Two read-only tools implementing the techniques described in the surrounding
documents. Both parse a capture file and print what they find. Neither
transmits, modifies, or blocks anything.

```
pip install dpkt
```

## btsig.py — signature detection

Implements the wire-level signatures from [../01-PROTOCOL.md](../01-PROTOCOL.md):
peer wire handshake, DHT KRPC, UDP tracker, local service discovery, µTP header
validation, and MSE candidate detection via the entropy heuristics in section 6.

```
python3 btsig.py capture.pcap
python3 btsig.py capture.pcap --json | jq -r '.proto' | sort | uniq -c
```

The classification functions take plain bytes and have no dependency on the
capture layer, so they are directly unit testable:

```python
from btsig import classify_tcp, gfw_fully_encrypted, parse_utp

classify_tcp(b"\x13BitTorrent protocol" + b"\x00" * 48)   # -> {'proto': 'bt-handshake', ...}
gfw_fully_encrypted(os.urandom(96))                        # -> (True, 'no exemption ...')
parse_utp(bytes([0x41, 0x00]) + b"\x00" * 18)              # -> {'type': 'ST_SYN', ...}
```

## flowfeat.py — statistical features

Computes the feature set from [../04-METHODOLOGY.md](../04-METHODOLOGY.md)
section 4. Every feature derives from headers, sizes, directions, and timestamps
only, so all of them survive payload encryption — which is the point.

```
python3 flowfeat.py capture.pcap                  # host-level summary
python3 flowfeat.py capture.pcap --flows          # per-flow JSON records
python3 flowfeat.py capture.pcap --host 10.0.0.5
```

Host-level output is the default because per-flow classification is the wrong
unit of analysis for this protocol; the discriminating behaviour is a property
of a host's aggregate communication pattern rather than of any single
connection.

## Producing a capture

```
# Linux
dumpcap -i eth0 -b filesize:512000 -b files:64 -w capture.pcapng

# macOS, with process attribution attached to each packet
tcpdump -i pktap -k NP -w capture.pcap
```

The macOS `pktap` form is worth knowing: it records the owning process per
packet, which solves the ground truth labeling problem described in
[../04-METHODOLOGY.md](../04-METHODOLOGY.md) section 2.

Always check capture drop counters. A capture with a few percent loss produces
flow features that are wrong in ways nothing downstream detects.

## Scale

These are teaching implementations, written so the mechanism is visible. For
volume, use [nfstream](https://www.nfstream.org/), which wraps nDPI and produces
comparable records far faster, and keep these for validating that you understand
what it is computing.
