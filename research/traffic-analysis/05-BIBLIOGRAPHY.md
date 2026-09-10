
# 05 — Annotated Bibliography

Grouped by role in the argument rather than chronologically. Annotations state
what each work contributes and why it is worth reading rather than summarizing
abstracts.

---

## Foundational traffic classification

**Karagiannis, Broido, Faloutsos, claffy.** *Transport Layer Identification of
P2P Traffic.* IMC 2004.
The paper that moved P2P identification off ports and onto behavior. Introduces
the source-destination IP/port pair heuristics still visible in modern
classifiers. Read for the framing of the problem.

**Karagiannis, Papagiannaki, Faloutsos.** *BLINC: Multilevel Traffic
Classification in the Dark.* SIGCOMM 2005.
Classification by social patterns of host communication — who talks to whom, at
what fan-out — rather than by content. The direct ancestor of the address
dispersion features in [04-METHODOLOGY.md](04-METHODOLOGY.md) § 4, and still the
clearest statement of why host-level analysis beats flow-level for P2P.

**Bernaille, Teixeira, Salamatian.** *Early Application Identification.* CoNEXT
2006.
Establishes that the first four to five packets of a flow suffice for
identification. The theoretical basis for every SPLT-style representation since.

**Gringoli, Salgarelli, Dusi, Cascarano, Risso, claffy.** *GT: Picking Up the
Truth from the Ground for Internet Traffic.* SIGCOMM CCR 2009.
Kernel-assisted per-flow process labeling. The reference treatment of ground
truth, and the paper to cite when explaining why port- or DPI-derived labels are
circular.

---

## Encrypted and obfuscated traffic

**Hjelmvik, John.** *Breaking and Improving Protocol Obfuscation.* Chalmers
Technical Report 2010.
**The key citation for this corpus.** Demonstrates statistical identification of
MSE-obfuscated BitTorrent, along with Skype and obfuscated eDonkey, using the
SPID algorithm. Establishes empirically that encryption defeats signature
matching and nothing else.

**Anderson, McGrew.** *Machine Learning for Encrypted Malware Traffic
Classification.* KDD 2017.
The SPLT representation — sequence of packet lengths and times — plus byte
distribution and TLS metadata, at production scale. Underpins Cisco's Encrypted
Traffic Analytics and is the best evidence that encrypted-flow classification is
deployed rather than academic.

**Draper-Gil, Lashkari, Mamun, Ghorbani.** *Characterization of Encrypted and
VPN Traffic Using Time-Related Features.* ICISSP 2016.
Introduces ISCXVPN2016. Valuable as a comparison baseline; note the dataset age
against current client behavior.

**Wang, Zhu, Wang, Zeng, Yang.** *End-to-End Encrypted Traffic Classification
with 1D-CNN.* ISI 2017. And **Lotfollahi et al.**, *Deep Packet*, Soft Computing
2020.
Representative of the shift from engineered features to learned representations
over raw packet bytes and sequences.

**Shapira, Shavitt.** *FlowPic: Encrypted Internet Traffic Classification is as
Easy as Image Recognition.* INFOCOM Workshops 2019.
Renders a flow as a 2D histogram over packet size and time, then applies vision
models. An elegant reuse of mature tooling and a good illustration of how much
signal lives in size-time structure alone.

**Lin, Xu, Gao, Li, Wang.** *ET-BERT.* WWW 2022.
Masked-token pretraining over datagram representations. Current frontier, and
notable for strong performance with limited labeled data — which matters given
how hard labeling is.

---

## Obfuscation theory and attacks

**Houmansadr, Brubaker, Shmatikov.** *The Parrot Is Dead: Observing Unobservable
Network Channels.* IEEE S&P 2013.
**The most important negative result in the field.** Establishes that protocol
mimicry requires full replication of state machine, error handling, and side
channels, and that the attacker needs only one discrepancy. Breaks SkypeMorph,
StegoTorus, and CensorSpoofer. The reason modern systems tunnel rather than
imitate.

**Wang, Dyer, Akella, Ristenpart, Shrimpton.** *Seeing Through Network-Protocol
Obfuscation.* CCS 2015.
Practical attacks on randomizing and format-transforming obfuscation using
entropy and timing. The complement to *The Parrot Is Dead*: mimicry fails, and
randomization is also detectable.

**Dixon, Ristenpart, Shrimpton.** *Network Traffic Obfuscation and Automated
Internet Censorship.* IEEE S&P Magazine 2016.
The randomize / mimic / tunnel taxonomy used in
[03-OBFUSCATION.md](03-OBFUSCATION.md) § 1.

**Alice, Bob, Carol, Beznazwy, Houmansadr.** *How China Detects and Blocks
Shadowsocks.* IMC 2020.
Active probing against a deployed encrypted protocol, in the field. Explains why
obfs4's out-of-band shared secret is essential rather than incidental.

**Wu, Cho, Bock, Levin, Learned-Miller, Ensafi, et al.** *How the Great Firewall
of China Detects and Blocks Fully Encrypted Traffic.* USENIX Security 2023.
**Directly applicable to MSE.** Documents entropy and printable-byte exemption
heuristics used to identify traffic that looks uniformly random, and to block it
on that basis alone. The empirical foundation for the claim that
indistinguishability from random is not indistinguishability from normal.

**Frolov, Wustrow.** *The Use of TLS in Censorship Circumvention.* NDSS 2019.
Measures TLS fingerprint distinguishability and introduces uTLS. The practical
answer to JA3-style fingerprinting of circumvention clients.

---

## Fingerprinting defenses and their limits

**Cai, Nithyanand, Wang, Johnson, Goldberg.** *A Systematic Approach to
Developing and Evaluating Website Fingerprinting Defenses.* CCS 2014.
Introduces Tamaraw and a framework for reasoning about defense guarantees rather
than measuring them empirically against one attack.

**Juarez, Imani, Perry, Diaz, Wright.** *Toward an Efficient Website
Fingerprinting Defense* (WTF-PAD). ESORICS 2016.
Adaptive padding driven by gap distributions. Low overhead, later broken —
which is itself the lesson.

**Wang, Goldberg.** *Walkie-Talkie.* USENIX Security 2017.
Half-duplex communication and burst molding to make distinct activities produce
identical observables. Strong idea, requires application cooperation.

**Sirinam, Imani, Juarez, Wright.** *Deep Fingerprinting.* CCS 2018. And
**Rimmer, Preuveneers, Juarez, Van Goethem, Joosen.** *Automated Website
Fingerprinting through Deep Learning.* NDSS 2018.
Deep models defeating defenses previously considered adequate, including
WTF-PAD. Together these establish the trilemma described in
[03-OBFUSCATION.md](03-OBFUSCATION.md) § 7.

---

## BitTorrent-specific

**Le Blond, Manils, Chaabane, Ali Kaafar, Castelluccia, Legout, Dabbous.** *One
Bad Apple Spoils the Bunch: Exploiting P2P Applications to Trace and Profile Tor
Users.* LEET 2011.
Real address leakage from BitTorrent over Tor via tracker and DHT announces and
non-proxied UDP. The empirical basis for the claim in
[03-OBFUSCATION.md](03-OBFUSCATION.md) § 5 that tunneling a protocol which
self-reports its address does not conceal that address.

**Cohen.** *Incentives Build Robustness in BitTorrent.* P2PECON 2003.
The original design rationale. Worth reading for why the protocol discloses
addresses by construction — the property that makes L5 observation unavoidable.

**BitTorrent Enhancement Proposals.** BEP 3, 5, 6, 9, 10, 11, 14, 15, 20, 29,
52. Primary sources for [01-PROTOCOL.md](01-PROTOCOL.md).

**Message Stream Encryption specification.** The MSE/PE design document. Note it
predates modern statistical classification entirely, which explains the scope of
what it defends against.

---

## Source worth reading as literature

**nDPI**, `src/lib/protocols/bittorrent.c`. Production detection logic combining
signatures with µTP header validation and DHT structure checks. The
consistency-checking approach to µTP is the technique described in
[01-PROTOCOL.md](01-PROTOCOL.md) § 4.

**libprotoident.** Classification from the first four payload bytes per
direction plus flow sizes. Instructive because of the constraint: it shows how
little payload is required.

**obfs4proxy**, `obfs4/obfs4.go`. Reference randomizing transport — ntor
handshake, length obfuscation, IAT modes.

**Zeek**, `scripts/base/frameworks/dpd` and signature files. Framing plus a
signature engine, appropriate when defining your own detection logic.

**libtorrent**, `src/utp_stream.cpp` and `src/pe_crypto.cpp`. The client side:
how µTP framing and MSE are actually implemented in the dominant library, which
is the ground truth for what appears on the wire.
