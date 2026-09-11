# tsync

Move a qBittorrent library to another machine **without re-downloading**,
then start the destination only after the source is silent.

Install and `PATH` checks: [docs/setup.md](docs/setup.md). Then `tsync doctor`.

```
1  audit     what is on disk
2  plan      path mapping (writes nothing)
3  rewrite   dest resumes → staging dir (never touches originals)
4  transfer  rsync the files (safe to re-run; resumes)
5  import    add to dest **paused**, then recheck
6  verify    dest must be piece-complete and still stopped
7  handoff   stop source, then start dest — never the reverse
```

`fetch` copies data back and never adds it to a client.
`migrate` runs steps 3–6 as one command. Dest stays paused unless you pass `--handoff`.

---

## What the other machine must already have

On the destination, before `transfer` / `import`:

- **SSH key login** that works with no password prompt.
  `ssh -o BatchMode=yes HOST 'echo ok'` must print `ok`.
- **GNU rsync ≥ 3.1** (same requirement as the machine you run `tsync` from).
- **qBittorrent 4.x or 5.x** with WebAPI enabled. A WebUI bound to
  loopback and reached through an SSH tunnel is fine.
- **Disk and a save path** the dest client uses. `--to` is that client
  path. `--rsync-to` is where the bytes land on the host if those two
  paths are not the same.
- **Dest stays paused.** Do not Start torrents there until `handoff`.

How you installed that client is outside this tool. Build and `PATH`
checks on the machine you run `tsync` from: [docs/setup.md](docs/setup.md).

---

## Use it in this order

Passwords come from the environment, never the command line.

```bash
export QBT_PASSWORD          # dest WebUI
export QBT_SOURCE_PASSWORD   # laptop WebUI, if different

tsync doctor

tsync audit
tsync plan --to /data
tsync rewrite --to /data --staging /tmp/tsync-staging

# --to is the save path the dest client uses.
# --rsync-to is where the bytes land on the other machine.
tsync transfer --to /data --rsync-to seedbox:/opt/seedbox/data

# Leave dest paused. Re-run transfer if it stops; rsync resumes.
tsync import --url http://127.0.0.1:8080 --username admin \
  --staging /tmp/tsync-staging --source-url http://127.0.0.1:8081 \
  --source-password-env QBT_SOURCE_PASSWORD

tsync verify --url http://127.0.0.1:8080 --username admin \
  --staging /tmp/tsync-staging
# expect: ready = every torrent, checking = 0, failed = 0
# if checking > 0, wait and run verify again

tsync handoff --url http://127.0.0.1:8080 --source-url http://127.0.0.1:8081 \
  --staging /tmp/tsync-staging --username admin \
  --source-password-env QBT_SOURCE_PASSWORD
```

Same thing as one command, still **paused** at the end:

```bash
tsync migrate --to /data --rsync-to seedbox:/opt/seedbox/data \
  --staging /tmp/tsync-staging --url http://127.0.0.1:8080 \
  --source-url http://127.0.0.1:8081 --username admin \
  --source-password-env QBT_SOURCE_PASSWORD
```

Add `--handoff` to migrate only after you are ready for dest to announce.

`--max-torrents N` on transfer / import / verify / handoff is a smallest-N
pilot. Drop it for the full library.

---

## What tsync will refuse

| You try to… | What happens |
|---|---|
| `handoff` while dest is missing, checking, or incomplete | Dest is **not** started. Transfer or recheck first. |
| `handoff` while dest is downloading | Dest is **not** started. |
| `handoff` while both clients are seeding the same hash | Neither client is touched. |
| `handoff` with `--source-url` and source still seeds after stop | Dest is **not** started. |
| `migrate --handoff` when verify is not all `ready` | Stops at verify. Dest is **not** started. |
| `import` when dest is already seeding a hash the source still announces | Refused (dual-seed). Adding **paused** while the laptop still seeds is allowed. |

`handoff` is the only tsync command that starts dest. It does **not** look at
an rsync progress bar. It asks the dest client: is this hash piece-complete
and stopped? A half-finished `transfer` fails that check.

**tsync cannot stop a Start click in the dest WebUI.** If you press Start
there during a copy, dest can announce while the laptop still seeds. Do not
do that. Leave dest paused until `verify` is clean and `handoff` has run.

---

## Also

- **GNU rsync ≥ 3.1** on both ends. macOS `/usr/bin/rsync` is `openrsync` and
  will not work. `brew install rsync` and put it first on `PATH`.
- **SSH key** to the dest host. A password prompt hangs an unattended copy.
- qBittorrent 4.x and 5.x. Dest WebUI on loopback + an SSH tunnel is fine.
- `rewrite` refuses if the source client lockfile is present. Snapshot
  `BT_backup` and pass `--bt-backup` if qBittorrent is still running.
- `fetch` never imports. The remote keeps seeding.

```bash
tsync fetch --from seedbox:/opt/seedbox/data --to ~/Music \
  --save-root /data --url http://127.0.0.1:8080 \
  --staging /tmp/tsync-staging --username admin
```

Rules the tool enforces: source files are read-only; resumes are rewritten
only into staging; dest is proven complete before the source stops; the
source is silent before dest starts.

Independent traffic-analysis notes (not used by the tool):
[`research/traffic-analysis/`](research/traffic-analysis/).

GPL-3.0. See [LICENSE](LICENSE).
