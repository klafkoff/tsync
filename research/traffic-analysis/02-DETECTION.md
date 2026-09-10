
# 02 — Detection

Detection is best understood as a stack of independent layers. Each operates on
different evidence, each is defeated by different countermeasures, and — the
point most often missed — defeating one leaves the others fully operational.

```
L5  swarm participation      an observer inside the swarm
L4  behavioral / host        DNS, SNI, IP reputation, endpoint state
L3  machine learning         learned representations over L2 features
L2  flow / statistical       sizes, timing, counts, entropy
L1  signature / DPI          payload byte patterns
L0  port                     transport port numbers
```

Encryption addresses L1. Tunneling addresses L0–L3 by relocating the observation
point. **Nothing at the network layer addresses L5**, which is where practical
consequences originate.

---

## L0 — Port-based

Historically 6881–6889 TCP and UDP. Obsolete as a primary method since clients
began randomizing ports by default around 2005, but still deployed because it is
free and still catches unconfigured installs.

Worth retaining as a *feature* rather than a classifier. Port 6881 remains
disproportionately represented in real traffic, so it contributes signal to a
model even though it decides nothing alone.

---

## L1 — Signature matching

Byte patterns at known offsets. The canonical set is developed in
[01-PROTOCOL.md](01-PROTOCOL.md) and implemented in
[`code/btsig.py`](code/btsig.py):

```
\x13BitTorrent protocol           peer wire handshake, offset 0
d1:ad2:id20:                      DHT query
d1:rd2:id20:                      DHT response
\x00\x00\x04\x17\x27\x10\x19\x80  UDP tracker connect magic, offset 0
BT-SEARCH * HTTP/1.1              local service discovery
GET /announce?info_hash=          HTTP tracker
```

Production implementations worth reading rather than merely citing:

**nDPI** (`src/lib/protocols/bittorrent.c`) is the most instructive open-source
reference, because it combines literal signatures with heuristics for µTP header
validation and DHT structure checking. Its approach to reducing false positives
on the µTP path — validating header field consistency rather than matching a
constant — is the technique described in 01 § 4.

**libprotoident** takes a deliberately minimal approach: it classifies using
only the first four payload bytes in each direction plus flow sizes. This is
valuable precisely because of the constraint, since it demonstrates how little
payload is needed and therefore how little truncation protects.

**Zeek** provides framing and a signature engine (`dpd.sig`) rather than a
finished classifier, which makes it the right tool for research where you want
to define and evaluate your own detection logic.

Signature matching is defeated completely by MSE, by any tunnel, and by any
transport encryption. Its persistence in deployed products reflects that a large
share of real BitTorrent traffic is still unobfuscated.

---

## L2 — Flow and statistical analysis

The layer that matters, because it operates unchanged on encrypted payloads. All
features below are computed from packet headers, sizes, directions, and
timestamps only.

### Packet size distribution

Discussed structurally in 01 § 2. The 16 KiB block invariant produces a
distinctive length histogram: a dominant mode at path MSS plus a secondary spike
at the fixed remainder of `16397 mod MSS`, interleaved with a dense cluster of
5–17 byte control messages.

The remainder spike is the discriminating feature. Bulk transfer over HTTP
produces the MSS mode but no fixed sub-MSS companion, because HTTP response
bodies are not chunked at a constant application-layer boundary.

### Concurrency and address dispersion

A torrent client maintains tens to hundreds of simultaneous peer connections
spread across many autonomous systems and geographic regions. Almost nothing
else on a residential connection behaves this way. Browsers open many
connections, but overwhelmingly to a handful of CDN-hosted destinations on 443.

The strongest single feature is **address entropy per unit time**: the Shannon
entropy of the distribution of remote /24 prefixes or origin ASNs contacted in a
window. Ordinary traffic concentrates heavily; swarm traffic disperses widely.
Karagiannis et al. built much of BLINC on exactly this class of social-graph
observation, classifying by the shape of who-talks-to-whom rather than by
content.

### Connection failure ratio

Peer lists from trackers, DHT, and PEX contain many stale entries — peers behind
NAT, offline, or firewalled. A client attempts them all, so **the ratio of
initiated connections that never complete a handshake is high**, with a large
population of SYNs receiving no SYN-ACK, plus RSTs and timeouts.

This is one of the most robust features available. It is invisible to payload
inspection, unaffected by encryption, difficult to suppress without degrading
performance, and rare in other applications — client-server protocols connect to
addresses that are, by construction, usually listening.

### Directional asymmetry

Seeding inverts the residential norm. Sustained upload substantially exceeding
download is unusual outside backup and video conferencing, and the per-flow
*distribution* of that ratio distinguishes further: a seeding host shows many
flows that are almost purely outbound, whereas a backup shows one.

### Periodicity

Three timers run continuously:

- tracker re-announce, typically every 1800 s
- DHT bucket refresh, roughly every 15 minutes
- peer keep-alives, every 120 s

Autocorrelation or spectral analysis of packet arrival times exposes these
directly. Keep-alives are especially useful because they persist through idle
periods, so **a long-lived flow that is otherwise silent but ticks every two
minutes is nearly diagnostic**, and they are the reason a mostly-idle seeding
host remains classifiable.

### Entropy of early payload

For encrypted flows, the Shannon entropy of the first N payload bytes separates
MSE and other fully-random protocols from structured ones. The GFW heuristics in
01 § 6 are the deployed form of this. The measurement is cheap — a byte-frequency
histogram over the first packet — and it is implemented in
[`code/flowfeat.py`](code/flowfeat.py).

### Early-packet sequences

Bernaille et al. established that the sizes and directions of the first four to
five packets of a flow suffice to identify most applications, since protocol
negotiation happens up front and is highly stereotyped. This underpins the
SPLT representation — Sequence of Packet Lengths and Times — used in Cisco's
Encrypted Traffic Analytics and described by Anderson and McGrew.

SPLT is the feature representation to reach for. It is compact, requires no
payload, is computable in real time, and feeds both classical and neural models.

---

## L3 — Machine learning

Three generations, all still deployed.

**Feature-based ensembles.** Random forests and gradient boosting over
hand-engineered flow statistics of the kind above. Fast, interpretable, robust
to modest distribution shift, and what most commercial appliances actually run.
Feature importance is directly readable, which matters for research — it tells
you which protocol property is doing the work.

**Sequence models.** 1D CNNs and LSTMs over packet size/direction sequences.
Wang et al. and Lotfollahi et al.'s *Deep Packet* established the approach;
Shapira and Shavitt's *FlowPic* reframes a flow as a 2D histogram image and
applies standard vision architectures, which is an elegant way to reuse mature
tooling.

**Pretrained transformers.** ET-BERT applies masked-token pretraining over
datagram representations, then fine-tunes for classification, reaching strong
results with limited labeled data. This mirrors the trajectory of NLP and is the
current frontier.

The website fingerprinting literature — Rimmer et al., Sirinam et al.'s *Deep
Fingerprinting* — is directly transferable methodology even though the target
differs, because it solves the same problem of classifying encrypted flows from
size and timing sequences alone, against defended traffic.

**A caution that recurs in [04-METHODOLOGY.md](04-METHODOLOGY.md):** reported
accuracy in this literature is frequently inflated by evaluation on balanced
datasets that do not resemble deployment prevalence, and by temporal or
site-level leakage between train and test splits.

---

## L4 — Behavioral and host-level

Evidence outside the traffic itself, often stronger than anything in it.

**DNS.** Resolution of DHT bootstrap names or tracker hostnames precedes and
predicts BitTorrent activity. Encrypted DNS relocates this evidence to the
resolver operator rather than eliminating it.

**TLS SNI and JA3/JA4.** Tracker hostnames in SNI resolve the question directly.
Client fingerprints identify the TLS implementation, which can distinguish a
torrent client's HTTPS announce from a browser's — an implementation mismatch
against the claimed user agent is itself an anomaly. Frolov and Wustrow's uTLS
work exists precisely because circumvention tools needed to fix this leak.

**IP reputation.** Sets of addresses observed participating in swarms are
harvested continuously and are commercially available. Correlating a subscriber's
destinations against such a set requires no protocol analysis at all.

**Endpoint state.** Where host visibility exists, this dominates everything:
process socket tables, open file handles, listening ports. Also the basis of the
most reliable ground truth labeling, discussed in
[04-METHODOLOGY.md](04-METHODOLOGY.md) § 2.

---

## L5 — Swarm participation

**This is the layer that produces consequences, and it is not traffic analysis
at all.**

BitTorrent works by peers announcing their address to a tracker or DHT and
accepting connections from other peers. Participation *requires* address
disclosure — it is not a flaw but the mechanism. Any party can join a swarm as
an ordinary peer and record every address it encounters.

This is how copyright enforcement actually operates. Monitoring firms run clients
that join swarms for targeted content and log observed addresses with timestamps
and infohashes. Notices follow from that log, correlated against subscriber
records. **No packet inspection occurs anywhere in this process.**

The consequences for threat modeling are stark and mostly unappreciated:

- Encrypting peer traffic accomplishes nothing here. The monitoring peer is a
  legitimate endpoint of the connection and decrypts by construction.
- Randomizing ports accomplishes nothing. You are announcing your port.
- Defeating your ISP's classifier accomplishes nothing. The observer is not your
  ISP.
- A VPN *does* help, but only because it substitutes a different address — the
  disclosure still happens, to the same observer, in the same way.

Le Blond et al. sharpened the point in a different direction, showing that
BitTorrent over Tor leaks the real address anyway, because the tracker announce
and DHT carry addresses the client believes to be its own, and clients commonly
bypass the proxy for UDP. Tunneling a protocol that self-reports its address
does not conceal the address.

**The correct conclusion is that network obfuscation and swarm exposure are
orthogonal threat models.** Obfuscation defends against an observer on the path.
It offers nothing against an observer at the endpoint. Only changing which
address participates addresses the second, which is the argument developed in
[03-OBFUSCATION.md](03-OBFUSCATION.md) § 6.

---

## What survives what

| countermeasure | L0 | L1 | L2 | L3 | L4 | L5 |
|---|---|---|---|---|---|---|
| random ports | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| MSE / PE | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| VPN | ✅ | ✅ | ~ | ~ | ~ | ~ |
| obfs4 | ✅ | ✅ | ~ | ~ | ❌ | ❌ |
| tunnel over TLS | ✅ | ✅ | ~ | ~ | ~ | ❌ |
| remote host (seedbox) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |

✅ defeated · ~ degraded but not defeated · ❌ unaffected

The final row is qualitatively different from the others. Every other
countermeasure makes traffic harder to classify; relocating swarm participation
means the traffic does not exist on the observed link and the disclosed address
is not yours. That difference — removal versus concealment — is the most
important practical finding in this corpus.
