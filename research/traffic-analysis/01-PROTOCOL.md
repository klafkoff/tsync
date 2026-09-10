
# 01 — Protocol Anatomy

Everything a classifier can key on originates in protocol structure. This
document establishes that structure at the byte level, because the difference
between a robust detector and a fragile one is usually precision about offsets
and invariants rather than sophistication of method.

Specifications referenced: BEP 3 (core), BEP 5 (DHT), BEP 6 (fast extension),
BEP 9 (metadata exchange), BEP 10 (extension protocol), BEP 11 (PEX), BEP 14
(local discovery), BEP 15 (UDP tracker), BEP 20 (peer ID conventions), BEP 29
(µTP), BEP 52 (v2), and the Message Stream Encryption specification.

---

## 1. The peer wire handshake

The handshake is fixed at 68 bytes and is the most-matched signature in
networking.

```
offset  len  field
------  ---  -----------------------------------------------
     0    1  pstrlen         = 0x13 (19)
     1   19  pstr            = "BitTorrent protocol"
    20    8  reserved        extension flags
    28   20  info_hash       SHA-1 of the bencoded info dict
    48   20  peer_id         client identifier
```

The first twenty bytes are constant across every implementation, which is why
`\x13BitTorrent protocol` appears verbatim in nDPI, Snort, Suricata, Zeek, and
essentially every commercial DPI product. Matching it costs nothing and yields
no false positives.

### Reserved bits as a client fingerprint

The eight reserved bytes advertise extension support. Commonly observed
assignments:

```
reserved[5] & 0x10    extension protocol (LTEP, BEP 10)
reserved[7] & 0x01    DHT (BEP 5)
reserved[7] & 0x02    peer exchange
reserved[7] & 0x04    fast extension (BEP 6)
reserved[7] & 0x10    v2 / hybrid (BEP 52)
reserved[0] & 0x80    Azureus messaging protocol
```

The value here is not any individual bit but the **pattern**. Clients advertise
characteristic combinations, so the reserved field narrows implementation and
often version even when the peer ID is absent or forged. This is the same
reasoning behind TLS fingerprinting: the set and order of advertised
capabilities is more stable and more revealing than any self-reported
identifier.

### Peer ID conventions

BEP 20 describes two conventions. Azureus-style dominates:

```
-qB5050-<12 random bytes>
 ^^^     ^^^^
 client  version
```

Common prefixes: `-qB` qBittorrent, `-lt`/`-LT` libtorrent, `-TR` Transmission,
`-UT` µTorrent, `-DE` Deluge, `-AZ` Vuze. The peer ID is self-reported and
trivially forged, which makes it useful for population measurement and useless
for adversarial identification — the reserved-bit pattern and the behavioral
fingerprints in [02-DETECTION.md](02-DETECTION.md) are far harder to fake
because they are emergent rather than declared.

---

## 2. Message framing

After the handshake, every message is length-prefixed:

```
<length: uint32 BE><id: uint8><payload>
```

The length prefix counts the ID byte plus payload. A zero-length message is a
keep-alive, sent roughly every two minutes.

```
id  message           wire size
--  ----------------  -----------------------
--  keep-alive        4
 0  choke             5
 1  unchoke           5
 2  interested        5
 3  not interested    5
 4  have              9
 5  bitfield          5 + ceil(pieces/8)
 6  request           17
 7  piece             4 + 9 + block length
 8  cancel            17
 9  port              7
13  suggest piece     9      (fast extension)
14  have all          5      (fast extension)
15  have none         5      (fast extension)
16  reject request    17     (fast extension)
17  allowed fast      9      (fast extension)
20  extended          variable (LTEP)
```

### The 16 KiB invariant

This is the single most important structural fact for statistical detection.

Block requests are conventionally 2^14 = 16384 bytes. Nearly every client
requests this size, and many refuse larger. A `piece` message carrying one block
is therefore:

```
4 (length prefix) + 1 (id) + 4 (index) + 4 (begin) + 16384 = 16397 bytes
```

Over TCP with a 1460-byte MSS, 16397 bytes segments into eleven full-size
segments and a 137-byte remainder. **That remainder is a fingerprint.** Bulk
transfer in BitTorrent produces a packet length distribution with an enormous
mode at MSS and a secondary spike at a specific sub-MSS value determined by
block size and path MTU — a shape that HTTP downloads, video streaming, and file
sync do not produce, because none of them chunk application data at a fixed
16 KiB boundary with an 9-byte header.

Encryption does not change this. MSE encrypts payload bytes; it does not
re-frame them. The size distribution passes through intact.

### Bimodality

Control messages are 5 to 17 bytes; data messages are ~16.4 KiB. A BitTorrent
flow is therefore strongly **bimodal** in packet size, with a dense cluster of
tiny packets interleaved into bulk transfer. Interactive protocols are small-
packet dominated, bulk protocols are large-packet dominated, and few things
sustain both simultaneously per-flow. The interleaving pattern is itself
informative: `have` announcements arrive at a rate proportional to swarm
activity, giving a control-plane cadence unrelated to the data plane.

---

## 3. DHT — Kademlia over UDP

BEP 5 specifies a Kademlia DHT using bencoded KRPC messages over UDP.

```
query:     d1:ad2:id20:<20-byte node id>...e1:q<len>:<method>1:t2:<txn>1:y1:qe
response:  d1:rd2:id20:<20-byte node id>...e1:t2:<txn>1:y1:re
error:     d1:eli<code>e<len>:<message>e1:t2:<txn>1:y1:ee
```

Methods: `ping`, `find_node`, `get_peers`, `announce_peer`, plus `sample_infohashes`
from BEP 51.

Detection is trivial and does not require reassembly, because KRPC messages fit
in single datagrams. The prefixes `d1:ad2:id20:` and `d1:rd2:id20:` are
effectively unique, and the `1:y1:q` / `1:y1:r` / `1:y1:e` discriminator confirms.

**DHT is the loudest component of the protocol suite.** It is unencrypted, it is
high-volume, it runs continuously regardless of transfer activity, and it
contacts a large and constantly changing set of addresses. A host running DHT is
identifiable from UDP payloads alone with no flow analysis whatsoever.

### Bootstrap as a pre-traffic signal

Clients bootstrap against well-known nodes:

```
router.bittorrent.com:6881
dht.transmissionbt.com:6881
router.utorrent.com:6881
dht.libtorrent.org:25401
```

The DNS resolution of these names precedes any BitTorrent traffic and is
conclusive on its own. Encrypted DNS hides the query content from a passive
observer but not from the resolver operator, and the subsequent connection to
the resolved address remains visible. **This is the earliest detectable moment
in a client's lifecycle**, and it occurs before a single peer connection exists.

---

## 4. µTP — LEDBAT over UDP

BEP 29 defines a userspace transport with delay-based congestion control,
designed to yield to interactive traffic. It now carries the majority of
BitTorrent data.

```
offset  len  field
------  ---  ---------------------------------------
     0    1  type (high nibble) | version (low nibble)
     1    1  extension
     2    2  connection_id
     4    4  timestamp_microseconds
     8    4  timestamp_difference_microseconds
    12    4  wnd_size
    16    2  seq_nr
    18    2  ack_nr
```

Types: `ST_DATA` 0, `ST_FIN` 1, `ST_STATE` 2, `ST_RESET` 3, `ST_SYN` 4. Version
is 1, so the first byte is one of `0x01 0x11 0x21 0x31 0x41`, with `0x41`
(ST_SYN) opening every connection.

**The µTP header is not encrypted even when the payload is.** MSE operates
inside the µTP stream, so the twenty-byte header remains in cleartext. This is a
significant and under-appreciated exposure: a 20-byte header with a
version-1 nibble, a monotonically incrementing `seq_nr`, and a
`timestamp_difference_microseconds` field that behaves like a one-way delay
estimate is highly structured and easily validated across consecutive datagrams.

Detection can therefore proceed by *consistency checking* rather than pattern
matching — parse the candidate header, verify that `seq_nr` advances plausibly
and that timestamps are monotonic and microsecond-scaled, and the false positive
rate collapses. The delay-based congestion control also produces a
characteristic sending pattern, since LEDBAT deliberately backs off on queueing
delay in a way loss-based TCP does not.

---

## 5. Tracker protocols

### HTTP

```
GET /announce?info_hash=%12%34...&peer_id=-qB5050-...&port=6881
    &uploaded=0&downloaded=0&left=0&compact=1&event=started
```

Over cleartext HTTP this exposes the infohash, the client, the listening port,
and transfer volumes. It is the highest-value single observation available to a
passive observer: it identifies not merely that BitTorrent is in use but exactly
which content, and `left=0` distinguishes seeding from downloading.

Over HTTPS the URL is protected, but **the TLS SNI still carries the tracker
hostname** unless Encrypted Client Hello is in use. Tracker hostnames are
enumerable, so SNI alone often resolves the question. Certificate transparency
logs make even private tracker hostnames discoverable, which is why the
deployment design elsewhere in this repository uses wildcard certificates
obtained by DNS-01 challenge.

### UDP (BEP 15)

```
connect request (16 bytes):
  offset 0   8  protocol_id = 0x41727101980
  offset 8   4  action = 0
  offset 12  4  transaction_id
```

The magic constant appears on the wire as `00 00 04 17 27 10 19 80`. An
eight-byte fixed value at offset zero in a sixteen-byte datagram is an
unambiguous signature with no plausible collision.

Announce (action 1) is 98 bytes and carries the infohash at offset 16 and peer
ID at offset 36. Because both are at fixed offsets in a fixed-size datagram,
extraction requires no parsing.

### Local Service Discovery (BEP 14)

Multicast to `239.192.152.143:6771` and `[ff15::efc0:988f]:6771`:

```
BT-SEARCH * HTTP/1.1\r\n
Host: 239.192.152.143:6771\r\n
Port: 6881\r\n
Infohash: <40 hex chars>\r\n
\r\n\r\n
```

Cleartext, multicast, and containing the infohash. On any network with a
monitoring host in the broadcast domain, LSD alone defeats every other
precaution. It is enabled by default in most clients and is the most commonly
overlooked exposure in this entire document.

---

## 6. Message Stream Encryption

MSE (also "protocol encryption", PE) was introduced in 2006 specifically to
defeat ISP traffic shaping. Understanding its construction explains precisely
what it does and does not accomplish.

### Construction

Diffie-Hellman over a fixed 768-bit prime with generator 2. Private keys are
160-bit.

```
A → B   Ya (96 bytes) || PadA (0–512 random bytes)
B → A   Yb (96 bytes) || PadB (0–512 random bytes)

        S = shared secret
        SKEY = info_hash

A → B   HASH('req1', S)                             (20 bytes)
        HASH('req2', SKEY) xor HASH('req3', S)      (20 bytes)
        ENCRYPT(VC, crypto_provide, len(PadC), PadC, len(IA), IA)

B → A   ENCRYPT(VC, crypto_select, len(PadD), PadD)
```

`VC` is eight zero bytes serving as a verification constant. `crypto_provide`
and `crypto_select` are bitfields: `0x01` plaintext, `0x02` RC4.

RC4 keys are `HASH('keyA', S, SKEY)` for A→B and `HASH('keyB', S, SKEY)` for
B→A, with the first 1024 bytes of keystream discarded to avoid the known biases
in early RC4 output.

### What it achieves

The infohash never appears in cleartext. `HASH('req2', SKEY) xor HASH('req3', S)`
requires knowledge of the shared secret to invert, so an observer cannot recover
which content is being exchanged. All post-handshake bytes are encrypted, so
message framing, IDs, and payloads are hidden from pattern matching.

Against 2006-era signature DPI this is complete: there is no constant string to
match.

### What it does not achieve

**It is unauthenticated.** No identity is verified in either direction, so an
active adversary can complete the handshake with either party. The infohash
obfuscation assumes the observer does not already know which infohash to test —
but the construction is a *known-plaintext oracle*: given a candidate `SKEY`, an
observer who has recorded the handshake and derived `S` can confirm or reject it.
An adversary monitoring a specific set of torrents can therefore test membership
directly.

**It does not conceal structure.** Packet sizes, timing, direction, flow counts,
connection durations, and the 16 KiB block granularity are all untouched. MSE
protects content and leaves the pattern fully exposed.

**Its own shape is distinctive.** The first flight from each side is a 96-byte
DH public key followed by random padding. What an observer sees is a TCP flow
whose opening bytes are high-entropy with no recognizable protocol structure, in
a length range of 96 to 608 bytes, bidirectionally. That is not the absence of a
signature; it is a different signature.

Hjelmvik and John demonstrated exactly this in 2010, identifying MSE-obfuscated
BitTorrent with high accuracy using statistical protocol identification. The
technique has only improved since.

### The entropy trap

Wu et al. (USENIX Security 2023) documented the Great Firewall of China
detecting and blocking "fully encrypted" protocols using a small set of
heuristics applied to the first packet of a flow. The exemption rules — a flow
is *not* treated as fully-encrypted if any hold — are approximately:

```
ex1  fraction of set bits outside [0.425, 0.575]
ex2  first six bytes are all printable ASCII
ex3  more than half of bytes are printable ASCII
ex4  twenty or more contiguous printable bytes
ex5  matches a known protocol (TLS, HTTP, ...)
```

A flow exempted by none is treated as an unrecognized encrypted protocol and
blocked. **MSE's opening DH exchange trips this directly**: a 96-byte uniformly
random public key has a set-bit fraction near 0.5 and essentially no printable
runs.

The generalizable result is that indistinguishability from random is not
indistinguishability from normal. Most traffic on a network is *structured*, so
being structureless is conspicuous. This is the core insight that drove
circumvention research away from pure randomization and toward tunneling inside
genuinely common protocols, which [03-OBFUSCATION.md](03-OBFUSCATION.md) takes
up.

---

## 7. Summary of observables

| observable | encrypted by MSE | inside a VPN | notes |
|---|---|---|---|
| handshake signature | yes | yes | trivially matched otherwise |
| infohash in tracker announce | HTTPS only | yes | SNI still leaks hostname |
| DHT KRPC payloads | no | yes | DHT is never encrypted |
| µTP header | no | yes | 20 bytes cleartext inside MSE |
| LSD multicast | no | no | link-local, escapes tunnels |
| packet size distribution | no | partially | MTU clamping alters but does not erase |
| flow count and peer entropy | no | yes | hidden from ISP, visible to VPN operator |
| connection failure ratio | no | yes | |
| announce periodicity | no | partially | volume timing survives |
| your address, to swarm peers | no | no† | †changed, not hidden |

The final row is the one that matters most and is developed in
[02-DETECTION.md](02-DETECTION.md) § 5.
