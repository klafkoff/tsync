
# 03 — Obfuscation and Its Limits

The censorship-circumvention literature is the serious body of work on this
problem. It is adversarial against nation-state classifiers with active probing
capability, which makes its negative results unusually trustworthy: techniques
that fail there fail everywhere.

---

## 1. A taxonomy

Dixon, Ristenpart and Shrimpton organize obfuscation into three strategies, and
the distinction predicts which approaches survive contact with a real adversary.

**Randomizing.** Make traffic look like nothing — uniformly random bytes with no
structure. *obfs4, ScrambleSuit, MSE.* Simple, low overhead, and defeats
signature matching completely. Vulnerable to the entropy heuristics in
[01-PROTOCOL.md](01-PROTOCOL.md) § 6, because structurelessness is itself
structure when most traffic is structured.

**Mimicking.** Imitate a different protocol. *SkypeMorph, StegoTorus, FTE.*
Intuitively appealing and, as § 2 establishes, fundamentally unsound.

**Tunneling.** Actually run inside a real protocol using a real implementation.
*meek, Snowflake, Trojan, VLESS+TLS.* Higher overhead, but the cover traffic is
genuine rather than simulated, which removes the entire class of attack that
kills mimicry.

---

## 2. The Parrot Is Dead

Houmansadr, Brubaker and Shmatikov's 2013 result is the most important negative
result in the field, and it generalizes well beyond its original targets.

To imitate a protocol convincingly you must reproduce not only its message
formats but its complete behavior: state machine transitions, error handling,
version negotiation, timing characteristics, retransmission behavior, the
side-effects of its control plane, and its responses to malformed or unexpected
input. Real implementations have decades of accumulated behavior, much of it
undocumented and some of it accidental.

**The asymmetry is fatal.** The imitator must match every observable behavior;
the adversary needs one discrepancy. And the adversary can *probe* — send
unexpected input and compare the response against a genuine implementation. The
authors broke every system they examined, generally in several independent ways.

The corollary drives everything since: **if you want to look like protocol X,
use a real implementation of protocol X as the carrier.** Do not simulate it.
This is why modern circumvention tunnels inside genuine TLS stacks rather than
generating TLS-shaped bytes, and why uTLS exists to make the TLS fingerprint
match a real browser exactly rather than approximately.

---

## 3. Randomizing transports

**obfs4** is the mature representative. An ntor-based authenticated handshake
requiring an out-of-band shared secret (node ID and public key), producing a
byte stream that is uniformly random with no fixed structure at any offset. It
adds length obfuscation via padding and an optional IAT mode that randomizes
inter-arrival times to frustrate timing analysis.

The out-of-band secret matters: it defeats **active probing**, where an
adversary connects to a suspected endpoint to see how it responds. Without the
secret, an obfs4 bridge is indistinguishable from a host that does not answer.
This is precisely the attack that broke naive Shadowsocks deployments, as
documented in the IMC 2020 analysis of the GFW.

The limitation is the entropy trap. obfs4 is not distinguishable *as obfs4*, but
it is distinguishable *as something fully encrypted*, and the 2023 USENIX
analysis shows that being in that category is itself sufficient grounds for
blocking. Randomization defeats identification but not categorization.

IAT mode is also a genuine bandwidth-latency tradeoff rather than a free win,
and its throughput cost is severe enough that most users disable it — a recurring
pattern where the strongest available setting is the one least used.

---

## 4. VPN tunnels

The most common approach and the most commonly misunderstood.

**What it does.** Your ISP sees one long-lived encrypted flow to a single
address. Peer diversity, flow counts, failure ratios, DHT, and tracker announces
are all hidden from the on-path observer. Against L0–L2 this is very effective.

**What it does not do.**

The tunnel itself is identifiable as a tunnel, and usually as a specific one.
WireGuard's handshake initiation is a fixed 148 bytes beginning with a message
type of `0x01` followed by three zero bytes; the response is 92 bytes. Both are
trivially matched. OpenVPN's opcode byte in the high five bits of the first byte
is similarly diagnostic. Neither was designed for indistinguishability, and
neither claims it.

Traffic inside the tunnel is not padded. Volumes, burst structure, and timing
pass through with MTU clamping and a constant overhead. A sustained multi-hour
high-volume flow with the directional asymmetry of seeding remains visible as a
pattern even when its content does not.

The trust boundary moves rather than disappearing. The VPN operator occupies
exactly the position the ISP did, with the same visibility. Logging policy, not
cryptography, is the actual security property, and it is unverifiable from
outside.

**Leak vectors** are where VPN deployments usually fail in practice: IPv6
traffic bypassing an IPv4-only tunnel, DNS queries escaping to the local
resolver, connections established before the tunnel comes up or persisting after
it drops without a kill switch, and — most reliably overlooked — **local service
discovery, which is link-local multicast and never enters the tunnel at all.**

For seeding specifically there is an operational constraint: inbound
connectivity requires port forwarding through the tunnel, and providers offering
it are a small subset. Without it the client is unreachable, which is worth
treating as an explicit health check because it degrades silently: outbound
connections still work and the client looks entirely normal.

---

## 5. Tunneling inside common protocols

The current state of the art, following directly from § 2.

**TLS-carried transports** (Trojan, VLESS with XTLS-Vision, Shadowsocks 2022
behind TLS) use genuine TLS implementations, present real certificates, and fall
back to serving actual web content when probed. There is nothing to distinguish
because the cover is real. uTLS makes the ClientHello fingerprint match a chosen
browser exactly, closing the JA3 gap.

**Snowflake** carries traffic over WebRTC data channels, so it resembles a video
call — a protocol with high natural volume, unpredictable timing, and peer-to-peer
addressing, which makes it an unusually good cover.

**SSH tunnels** are worth noting as a common but weak choice: SSH announces
itself in cleartext with a version banner such as `SSH-2.0-OpenSSH_9.6` as the
first bytes on the wire. An SSH tunnel is not obfuscated; it is merely encrypted.

**Tor should not be used for BitTorrent**, on two independent grounds. It is
antisocial, since bulk transfer over volunteer-operated relays degrades the
network for users with acute needs. And it does not work: Le Blond et al.
demonstrated real address leakage through tracker and DHT announces that carry
the client's self-reported address, plus UDP paths that bypass the proxy
entirely.

---

## 6. Architecture as the dominant strategy

Every technique above concerns making present traffic hard to classify. There is
a categorically different approach, and it is stronger.

**Relocate swarm participation to a different host.** All peer connections, DHT
traffic, tracker announces, and µTP flows originate from a server. The
residential link carries only administrative traffic: an SSH or rsync session to
one fixed address, and HTTPS to a management interface.

Consider what an observer on the residential link now sees. Not obfuscated
BitTorrent — no BitTorrent. One long-lived encrypted bulk transfer to a stable
endpoint, with the shape of a backup or file sync, which is indistinguishable
from those things because **it is literally the same operation**. This is
tunneling in the § 1 sense taken to its conclusion: the cover traffic is not
merely genuine, it is the entire traffic.

Critically, and unlike everything else in this document, **it also addresses L5**.
The address disclosed to swarm peers, including any monitoring peer, is the
server's. That is the layer where consequences originate and the layer no
obfuscation technique reaches.

The residual exposures are worth enumerating honestly, since they are the real
attack surface of this design:

- The server's provider sees everything, occupying the position the VPN operator
  would. Provider choice and acceptable-use policy are the actual controls.
- The link between home and server is a distinctive high-volume flow to one
  address. Its *existence* is unconcealed; only its content is protected.
- Management interfaces are internet-reachable, which is a conventional web
  security problem. It argues for an unbranded authentication gate in front of
  everything, wildcard certificates obtained by DNS-01 challenge so the hostname
  stays out of certificate transparency logs, and services bound to loopback
  behind a reverse proxy.
- Correlation remains possible in principle: an adversary observing both the
  residential link and the server can match volume and timing. This requires a
  much stronger adversary than the one obfuscation targets.

---

## 7. Padding and the trilemma

If the goal were to hide the *shape* of traffic rather than relocate it, the
website fingerprinting defense literature establishes what that costs.

**Constant-rate padding** — transmit at a fixed rate regardless of demand —
provides strong guarantees, since the observable is independent of the secret.
The overhead is prohibitive: bandwidth is paid continuously whether used or not,
and any rate below peak demand adds latency.

**Tamaraw** and **CS-BuFLO** approach this systematically, achieving strong
protection at substantial cost. **WTF-PAD** uses adaptive padding driven by gap
distributions to fill only statistically unusual silences, achieving much lower
overhead — and correspondingly weaker guarantees, as *Deep Fingerprinting* later
demonstrated by breaking it. **Walkie-Talkie** uses half-duplex communication
plus burst molding to make different activities produce identical observables,
which is elegant but requires application cooperation.

The recurring finding across all of it is a **bandwidth-latency-indistinguish-
ability trilemma**: you may have any two. Low-overhead defenses are consistently
broken by better classifiers within a few years, and defenses that survive
demand overhead multiples that nobody accepts in practice.

For bulk transfer the tradeoff is unusually unfavorable. Latency padding is cheap
here because the traffic is not interactive, but the volume signal is the entire
signal, and hiding volume means transmitting cover bytes at the rate of the real
transfer — doubling cost to conceal something the architectural approach removes
for free.

---

## 8. Assessment

Ordered by effectiveness against the full stack rather than by sophistication:

**Ineffective alone.** Port randomization; MSE/PE. Both defeat one layer of a
six-layer stack and neither touches swarm exposure. Enable them — they are free —
but do not model them as protection.

**Effective against on-path observers only.** VPN tunnels, obfs4, TLS-carried
transports. Genuinely good at what they do, and useful. All share the property
that they address the observer on the wire and not the observer in the swarm.

**Effective against the full stack.** Relocating swarm participation to a
different host. The only approach that removes the traffic rather than
concealing it, and the only one that changes what is disclosed at L5.

**Counterproductive.** Tor, which fails on both technical and ethical grounds.
Protocol mimicry, which the theory says cannot be made to work.

The general lesson transfers well beyond this protocol: **when an observable is
load-bearing, changing the architecture that produces it dominates any attempt
to disguise it.**
