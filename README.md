# tsync

Move a torrent library between machines without re-downloading it.

If you seed from a laptop and want to move to a server, the data transfer is the
easy part. The hard part is convincing the destination client that the files it
just received are the files it already has, so it resumes seeding instead of
downloading 60 GB you are holding in your hand.

`tsync` does that, and refuses to do anything unsafe along the way.

> **Status: in development.** `tsync doctor`, `audit`, `plan`, `rewrite`,
> `transfer`, `import`, and `verify` run. Handoff and later steps are not
> available yet.

To build from source and confirm the local environment, copy the blocks in
[docs/setup.md](docs/setup.md).

---

## What it does

```
audit      inventory a local client: torrents, files, completion state
plan       derive path mappings and preview every change as a diff
rewrite    rewrite resume data for the destination's paths
transfer   copy data with resumable, progress-reporting rsync
import     add torrents to the destination, paused, and force a recheck
verify     confirm the destination has complete, valid data
handoff    stop the source and start the destination, in that order
fetch      pull data back, with partial-torrent handling and top-up
migrate    move an entire setup to a different host
doctor     verify dependencies and environment on both ends
```

---

## Design principles

These are constraints the tool enforces, not advice it offers.

**Never re-download.** Torrents are imported paused, rechecked while paused, and
resumed only when verified complete. Layered guards — a stop condition, a
download-rate circuit breaker, and a verification gate — mean a bug results in a
stopped torrent rather than a duplicate download.

**Never seed the same infohash twice.** The destination is proven complete
*before* the source stops, and the source stops *before* the destination starts.
There is a deliberate few-second gap where neither is seeding. On a private
tracker brief downtime is free; overlap looks like account sharing.

**Never modify data.** The source library is read-only to `tsync`. Resume files
are rewritten into a staging directory, never in place, and the original local
copy is never deleted as part of a migration.

**Byte-identical serialization.** Resume files are bencode. Decode-then-encode is
asserted byte-identical before any rewrite is accepted, so a parser
disagreement fails loudly instead of silently corrupting a torrent.

**Verify against the swarm, not against the source.** Copies are validated by
hashing against the piece hashes in the torrent metadata — an independent
authority — rather than by comparing source to destination, which only proves
the two agree.

---

## Requirements

- GNU `rsync` 3.1 or newer on both ends. **macOS ships `openrsync`, which will
  not work.** `brew install rsync` puts the real binary on disk; it must also
  come first on `PATH` (Homebrew does not replace `/usr/bin/rsync`).
- SSH **key** access to the destination. A password prompt will hang an
  unattended `rsync` / `tsync` run. Copy-paste key setup is
  [docs/setup.md §4](docs/setup.md#4-ssh-key-access-to-the-destination).
- A supported client. qBittorrent 4.x and 5.x today; the client layer is an
  interface, and adding another is a contained change.

Run `tsync doctor` before anything else. It probes for capabilities rather than
parsing version strings, and every failure it reports comes with the exact
command that fixes it. Copy-paste setup for the local toolchain, SSH keys, and
the PATH trap on macOS is in [docs/setup.md](docs/setup.md).

`tsync transfer` copies only the planned files (not a folder walk) with GNU
rsync. `--to` is the save-path root the destination client will use, same as
`plan` / `rewrite`. When that path is not where the host stores bytes — Docker
`/data` on the container, `/opt/seedbox/data` on the box — pass both:

```bash
tsync transfer --to /data --rsync-to seedbox:/opt/seedbox/data
```

`--rsync-to` is a local path or `host:/abs/path` over SSH (`BatchMode=yes`).
Re-running resumes; rsync skips files that already match.

`tsync import` talks to a destination qBittorrent WebUI. It adds each staged
`.torrent` paused, with a stop-after-check condition, then force-rechecks. A
1 byte/s download limit is applied for the duration and restored afterwards.
The password is read from `QBT_PASSWORD` — never from the command line.

```bash
export QBT_PASSWORD
tsync import --url http://127.0.0.1:8080 --staging /tmp/tsync-staging \
  --username admin --source-url http://127.0.0.1:8080
```

`--source-url` is the dual-seed guard. If the source is still seeding those
hashes, import refuses. Do not point `--url` at a client that is still seeding
the same torrents.

`tsync verify` is the gate after import: every staged hash must be
piece-complete and still stopped. Seeding on the destination fails unless you
pass `--allow-seeding` (after handoff). It does not announce, and it does not
check a public listen port.

```bash
export QBT_PASSWORD
tsync verify --url http://127.0.0.1:8080 --staging /tmp/tsync-staging \
  --username admin
```

---

## Research

[`research/traffic-analysis/`](research/traffic-analysis/) is an independent
corpus on how peer-to-peer traffic is identified on a network, why encryption
alone does not prevent identification, and what the censorship-circumvention
literature establishes about the limits of obfuscation. It includes two working
analysis tools.

It is not used by `tsync` and has no bearing on how the tool works. It is
published because the material is genuinely interesting and hard to find
assembled in one place.

---

## License

GPL-3.0. See [LICENSE](LICENSE).
