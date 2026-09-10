# BitTorrent Traffic Analysis and Protocol Obfuscation

A research corpus on how peer-to-peer traffic is identified on a network, why
encryption alone does not prevent identification, and what the censorship-
circumvention literature establishes about the limits of obfuscation.

---

## Isolation

**This directory is independent of the `tsync` tool.** Nothing here is imported,
built, or executed by the migration tooling, and nothing in the tool depends on
conclusions drawn here. It exists as a standalone research track that happens to
share a repository.

The one place the two touch is analytical rather than operational, and it is
covered in [03-OBFUSCATION.md](03-OBFUSCATION.md): the seedbox architecture the
tool builds turns out to be a stronger countermeasure than any of the
obfuscation techniques studied here, for reasons worth understanding precisely.

---

## Scope and framing

This is network measurement and traffic classification research. The material is
drawn from published academic work — IMC, SIGCOMM, IEEE S&P, USENIX Security,
NDSS, CCS — and from the source of open-source classifiers whose detection logic
is publicly readable.

The subject matter is dual-use in the ordinary way that all traffic analysis is.
Classification research is what network operators use for capacity planning and
what security teams use to find command-and-control channels; obfuscation
research is what keeps circumvention tools working under national firewalls. The
two halves are the same field read in opposite directions, and neither is
comprehensible without the other.

Two framings are deliberately avoided. This is not a guide to evading
enforcement, and as [02-DETECTION.md](02-DETECTION.md) argues at length, it would
be a poor one anyway — the mechanism that actually produces consequences for
BitTorrent users is not traffic inspection, and treating it as though it were is
the central analytical error in most popular writing on the subject.

---

## Contents

| document | subject |
|---|---|
| [01-PROTOCOL.md](01-PROTOCOL.md) | Wire-level anatomy: handshake, message framing, DHT, µTP, tracker protocols, MSE/PE cryptography |
| [02-DETECTION.md](02-DETECTION.md) | Detection taxonomy from port matching through transformer classifiers; what each layer sees and what defeats it |
| [03-OBFUSCATION.md](03-OBFUSCATION.md) | Countermeasures, their theoretical limits, and why mimicry fails |
| [04-METHODOLOGY.md](04-METHODOLOGY.md) | Empirical practice: capture, ground truth labeling, feature extraction, evaluation, and the base-rate problem |
| [05-BIBLIOGRAPHY.md](05-BIBLIOGRAPHY.md) | Annotated literature |
| [code/](code/) | Working signature detector and flow feature extractor |

---

## Central claims

The corpus argues six things. Each is developed with evidence in the documents
above; they are stated here so the through-line is visible up front.

**1. Detection is a layered stack, and encryption removes exactly one layer.**
Message Stream Encryption was designed in 2006 against pattern-matching deep
packet inspection, and it defeats that completely. It does nothing about flow
counts, packet size distributions, connection failure ratios, peer address
entropy, or announce periodicity — all of which survive encryption intact
because they are properties of the communication pattern rather than its
content.

**2. High entropy is itself a signature.** A flow whose first packet is
indistinguishable from random bytes is unusual, and treating that as suspicious
is a deployed technique, not a theoretical one. Wu et al. documented the Great
Firewall blocking fully-encrypted protocols using a small set of entropy and
printable-byte heuristics. Perfect encryption produces a perfectly recognizable
absence of structure.

**3. Mimicry is fundamentally harder than randomization or tunneling.**
Houmansadr et al. established in *The Parrot Is Dead* that imitating a protocol
requires replicating its full state machine, error behavior, and side channels —
an unbounded obligation where the attacker need find only one discrepancy. The
practical corollary is that systems which *are* the cover protocol beat systems
that merely resemble it.

**4. The enforcement mechanism is swarm surveillance, not packet inspection.**
Copyright monitoring firms participate in swarms as ordinary peers and record
the addresses that connect to them. Your address is disclosed to them by the
protocol working correctly. No amount of link-layer obfuscation addresses an
observer that you connect to directly and hand your address to voluntarily.

**5. Architecture dominates obfuscation.** Relocating swarm participation to a
different host does not hide the traffic; it means the traffic is not there.
This is categorically stronger than making present traffic hard to classify, and
it is the only approach in this corpus that is robust against both the network
observer and the swarm observer simultaneously.

**6. There is no low-overhead strong indistinguishability.** The website
fingerprinting defense literature — Tamaraw, WTF-PAD, Walkie-Talkie — repeatedly
demonstrates a bandwidth-latency-indistinguishability trilemma. Defenses with
tolerable overhead provide partial protection; defenses with strong guarantees
cost multiples of the original traffic.

---

## How to use this

The documents are ordered as a dependency chain: protocol structure determines
what signatures exist, signatures determine what obfuscation must hide, and
methodology determines whether any claim about either can be verified.

For empirical work, [04-METHODOLOGY.md](04-METHODOLOGY.md) is the load-bearing
document. Traffic classification results are notoriously hard to reproduce,
overwhelmingly because ground truth labeling is difficult and because published
accuracy figures are reported without reference to class prevalence. Both
problems are addressed there.
