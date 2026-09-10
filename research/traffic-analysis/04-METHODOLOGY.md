
# 04 — Empirical Methodology

Traffic classification results are notoriously difficult to reproduce. The two
dominant causes are that ground truth labeling is much harder than papers
acknowledge, and that reported accuracy is almost never adjusted for deployment
prevalence. Both are addressed here, along with the practical tooling.

---

## 1. Capture

**Linux.** `dumpcap` rather than `tcpdump` for long captures, since it handles
ring buffers and file rotation properly:

```
dumpcap -i eth0 -b filesize:512000 -b files:64 -w capture.pcapng
```

Above a few Gbps, kernel capture drops packets silently. AF_XDP, PF_RING ZC, or
DPDK become necessary. **Always check the drop counters** — a capture with 8%
loss produces flow features that are wrong in ways no downstream analysis
detects.

**macOS.** `pktap` is the notable capability, because it attaches process
metadata to captured packets:

```
tcpdump -i pktap -k NP -w capture.pcap
```

This yields per-packet process name and PID, which solves the ground truth
problem directly on this platform and makes macOS an unusually good environment
for building labeled datasets.

**Isolation.** For controlled experiments, capture on a router or bridge rather
than the host under study, so the measurement apparatus does not perturb the
subject. A Linux bridge with a mirrored port, or a physical tap, is the clean
arrangement.

---

## 2. Ground truth

This is the hardest part of any classification study and the most common source
of results that do not replicate.

**Process attribution is the gold standard.** Gringoli et al.'s `gt` established
the approach: a kernel-assisted agent on the endpoint records which process owns
each socket, producing per-flow labels that are correct by construction rather
than inferred.

Modern equivalents:

- **Linux, eBPF.** Attach to `security_socket_connect` or the `sock:inet_sock_set_state`
  tracepoint and emit `(4-tuple, pid, comm)` tuples. `bcc`'s `tcpconnect` and
  `tcpaccept` are usable directly; `bpftrace` suffices for one-off work. This
  correctly captures short-lived flows that polling misses.
- **macOS.** `pktap` as above, or `lsof -i -n -P` sampling for a coarser view.
- **Polling `ss -tanp`** is the fallback, and it is genuinely bad — it misses any
  flow shorter than the poll interval, which is a large fraction of BitTorrent's
  failed connection attempts, and those are exactly the flows carrying the most
  discriminating feature.

**Labeling by port or by DPI is circular** when the object of study is a
classifier. If labels come from nDPI, the model learns to imitate nDPI including
its errors, and reported accuracy measures agreement rather than correctness.
This is common in published work and is worth checking before trusting any
result.

**Controlled generation** is the practical complement: run known clients with
known content in an isolated network, capture with process attribution, and vary
one factor at a time — encryption on and off, µTP versus TCP, seeding versus
leeching, swarm size, VPN on and off. Synthetic traffic lacks the diversity of
real traffic, so it should be used to establish mechanism and validate feature
extraction, then confirmed against labeled real captures.

---

## 3. Feature extraction

**nfstream** is the most practical starting point for research: a Python library
built on nDPI that produces flow records with both statistical features and
protocol labels, so the nDPI label can serve as a comparison baseline while the
statistical features feed a model.

```python
from nfstream import NFStreamer

for flow in NFStreamer(source="capture.pcap",
                       statistical_analysis=True,
                       splt_analysis=10):
    ...
```

**CICFlowMeter** produces the 80-plus feature set used across the ISCX datasets,
which matters mainly for comparability with published results. Its features are
mostly aggregates and it discards packet ordering, so it underperforms SPLT
representations on encrypted traffic.

**Joy** (Cisco) extracts SPLT and byte distributions and is the tooling behind
the Encrypted Traffic Analytics work.

**Zeek** for anything requiring protocol context or custom logic, since its
scripting layer lets you express detection rules directly rather than
post-processing flow records.

[`code/flowfeat.py`](code/flowfeat.py) in this directory implements the core
features from scratch — SPLT, size histograms, entropy, failure ratios, address
dispersion — because the pedagogical value of computing them yourself is high
and the implementations above hide the details that matter.

---

## 4. Feature reference

The features worth computing, with the mechanism each exploits:

| feature | mechanism |
|---|---|
| SPLT: first 10–20 packet sizes and inter-arrival times, signed by direction | protocol negotiation is stereotyped and happens up front |
| packet size histogram, 32 or 64 bins | 16 KiB block invariant produces a fixed sub-MSS mode |
| bimodality coefficient of size distribution | control and data planes interleave in one flow |
| Shannon entropy of first payload packet | separates fully-encrypted from structured |
| printable-byte fraction and longest printable run | GFW-style exemption heuristics |
| flow duration, byte and packet counts per direction | bulk versus interactive |
| up:down byte ratio | seeding inverts the residential norm |
| concurrent flow count per host per window | swarm concurrency |
| distinct remote /24s and ASNs per window | address dispersion |
| Shannon entropy over remote prefix distribution | concentration versus dispersion |
| SYN-without-SYN-ACK ratio | stale peer lists produce failed attempts |
| inter-arrival autocorrelation at 120 s and 1800 s | keep-alive and announce timers |

The last four are host-level rather than per-flow, and they are consistently the
strongest features for this protocol. **Per-flow classification is the wrong unit
of analysis for BitTorrent**, because the distinguishing behavior is a property
of the host's aggregate communication pattern rather than of any individual
connection. Papers that evaluate per-flow understate what a real classifier sees.

---

## 5. Evaluation, and the base-rate problem

The dominant methodological failure in this literature.

Consider a classifier with 99% precision and 99% recall, evaluated on a balanced
dataset — a strong result by published standards. Deploy it where BitTorrent is
0.1% of flows, and among 1,000,000 flows:

```
1,000 positive        →  990 true positives
999,000 negative      →  9,990 false positives (at 1% FPR)

precision in deployment = 990 / 10,980 ≈ 9%
```

**Over ninety percent of alerts are wrong**, from a classifier reported as 99%
precise. The number that survives class rebalancing is the false positive rate,
not precision, and it is the number to demand.

Accordingly:

- Report FPR and TPR, and present ROC or precision-recall curves rather than
  single operating points.
- State the prevalence assumed, and recompute precision at realistic prevalence.
- Prefer precision-recall curves to ROC under heavy imbalance, since ROC
  flatters classifiers when negatives dominate.

**Splitting correctly** matters as much. Split by host and by time, never
randomly by flow — flows from one host in one session are highly correlated, so
random splits leak and inflate results, sometimes enormously. Evaluate on
captures from a different day, and ideally a different network, than training.

**Concept drift** is real here. Client defaults shift, µTP displaced TCP,
encryption became default-on, and DHT usage patterns changed. A classifier
trained on ISCXVPN2016 does not describe 2026 traffic, which is a substantial
caveat on any result derived from the standard public datasets.

---

## 6. Datasets

| dataset | notes |
|---|---|
| ISCXVPN2016 | VPN and non-VPN, includes P2P; the standard comparison baseline, and dated |
| ISCXTor2016 | Tor and non-Tor, same lineage |
| CIC-Darknet2020 | merges the two above with darknet labels |
| UNIBS-2009 | ground truth via `gt`, methodologically strong, very old |
| MAWI | continuous transit captures, payload-stripped, unlabeled |
| CAIDA | large scale, restricted access, anonymized and header-only |

Every public dataset here predates current client behavior. **Generating your own
labeled captures is usually necessary**, with public sets reserved for
comparability against published numbers rather than treated as ground truth
about present-day traffic.

---

## 7. Experiments worth running

Ordered so each builds on the last.

**1. Signature baseline.** Run [`code/btsig.py`](code/btsig.py) over unencrypted
client traffic; confirm handshake, DHT, tracker, LSD, and µTP detection.
Establishes that extraction is correct before anything statistical.

**2. Encryption ablation.** Same client, forced encryption. Signature detection
should collapse to DHT, µTP headers, and LSD only. **The residue is the finding**
— quantify what fraction of identifying observations survives MSE.

**3. Entropy separation.** Compute first-packet entropy and printable-byte
fractions across MSE, TLS, SSH, HTTP, and WireGuard flows. Reproduce the GFW
exemption heuristics from [01-PROTOCOL.md](01-PROTOCOL.md) § 6 and measure their
separation on your own capture.

**4. Size distribution.** Histogram packet lengths for BitTorrent bulk transfer
versus HTTP download of the same file. Locate the `16397 mod MSS` mode and
measure its stability across path MTU changes.

**5. Host-level features.** Compute address dispersion, failure ratio, and
concurrency for a seeding host, a browsing host, and a video streaming host.
Expect these to separate more cleanly than any per-flow feature — this is the
experiment that demonstrates § 4's claim about unit of analysis.

**6. Periodicity.** Autocorrelate packet arrival times over a multi-hour idle
seeding session; recover the 120 s keep-alive and 1800 s announce periods.

**7. VPN residue.** Capture the same seeding session inside a WireGuard tunnel.
Confirm the tunnel is identifiable by handshake, then test how much of the volume
and timing signal survives encapsulation.

**8. Architectural comparison.** Capture the residential link while seeding
locally, then while a remote host seeds and only rsync traffic crosses the link.
Compare against a genuine backup transfer of equivalent volume. This is the
empirical form of [03-OBFUSCATION.md](03-OBFUSCATION.md) § 6, and the expected
result is that no feature separates them because no meaningful difference exists.

---

## 8. Reproducibility

Record for every capture: date and duration, client and version, encryption
setting, transport, swarm characteristics, path MTU, capture point, and drop
counters. Version the extraction code alongside results, since feature
definitions drift silently and a histogram binning change can move accuracy by
several points.

Publish extraction code and feature matrices even when raw captures cannot be
shared for privacy reasons. Most irreproducibility in this field comes from
undocumented preprocessing rather than from unavailable data.
