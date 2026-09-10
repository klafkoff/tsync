# Set up a machine to build and run tsync

Copy each block as written. The last command in every section is a check:
if it prints what the comment says, you can move on.

`tsync doctor` is the same idea once the binary exists — it probes for the
capability, not a version number. On macOS that matters, because the system
`rsync` *looks* like an old GNU rsync and is not.

---

## 1. Tools

### macOS

Homebrew first, then GNU rsync and a Rust toolchain. Apple's `/usr/bin/rsync`
is `openrsync` and cannot report transfer progress.

```bash
# Homebrew, if you do not already have it
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"

# Apple silicon: put Homebrew on PATH for this shell and future ones
eval "$(/opt/homebrew/bin/brew shellenv)"
echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> ~/.zprofile

# Intel Macs use this instead of the two lines above
# eval "$(/usr/local/bin/brew shellenv)"
# echo 'eval "$(/usr/local/bin/brew shellenv)"' >> ~/.zprofile

brew install rsync rustup
rustup default stable
```

Confirm GNU rsync is the one `PATH` finds, and that it is new enough
(`--info=help` exists only on GNU rsync ≥ 3.1.0):

```bash
command -v rsync
# expect: /opt/homebrew/bin/rsync   (Apple silicon)
#     or: /usr/local/bin/rsync      (Intel)

rsync --version | head -1
# expect: rsync  version 3.x.x  protocol version 3x
# reject: openrsync: protocol version 29

rsync --info=help >/dev/null && echo "rsync capability: ok"
# expect: rsync capability: ok
```

If `command -v rsync` still prints `/usr/bin/rsync`, Homebrew is installed
but not ahead of `/usr/bin` on `PATH`. Re-run the `brew shellenv` lines
above, open a new terminal, and check again.

### Linux

```bash
# Debian / Ubuntu
sudo apt-get update
sudo apt-get install -y rsync openssh-client curl build-essential pkg-config

# Fedora
# sudo dnf install -y rsync openssh-clients curl gcc pkg-config

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
# reload the shell so cargo is on PATH
. "$HOME/.cargo/env"
```

```bash
rsync --version | head -1
# expect: rsync  version 3.x.x

rsync --info=help >/dev/null && echo "rsync capability: ok"
# expect: rsync capability: ok

rustc --version
# expect: rustc 1.98.x or newer (1.98 is what this repo pins)
```

---

## 2. Build tsync from this repository

```bash
git clone https://github.com/klafkoff/tsync.git
cd tsync

cargo test --workspace
# expect: all tests passed

cargo run -p tsync -- doctor
# expect: a report ending in "N passed", exit status 0
#         any FAIL line prints the exact command that fixes it

cargo run -p tsync -- audit
# expect: a count of intact / partial torrents; changes nothing on disk

cargo run -p tsync -- plan --to /opt/seedbox/data
# expect: a mapping, batch table, and excluded list; writes nothing

mkdir -p /tmp/tsync-staging
cargo run -p tsync -- rewrite --to /opt/seedbox/data --staging /tmp/tsync-staging
# expect: copied .torrent files and new .fastresume files under staging
#         originals in BT_backup are unchanged
#         fails if qBittorrent is running (lockfile)
```

`rust-toolchain.toml` pins the compiler. The first `cargo` invocation
downloads that toolchain; later ones reuse it.

Useful checks while hacking:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo run -p tsync -- doctor --list
```

---

## 3. What `doctor` expects on this machine

| check | pass looks like | if it fails |
|---|---|---|
| `ssh` | `OpenSSH_…` | install the platform's OpenSSH client |
| `rsync` | `GNU rsync 3.x.x` | macOS: `brew install rsync` and fix `PATH` as above |
| `bt_backup` | `readable, N torrents` | grant Full Disk Access to the terminal (macOS), or pass a custom state directory later |

qBittorrent does not have to be running for `doctor`. The backup directory
only has to be listable. On macOS, `stat` succeeding and `ls` failing is
TCC, not a Unix permission — System Settings → Privacy & Security → Full
Disk Access, then restart the terminal.

---

## 4. Remote host

Buying a VPS and attaching disk is out of band. Once you have SSH key
access to a Linux box, the stack (qBittorrent, qui, Caddy) is a directory
of scripts that run *without* installing tsync. That lives in `deploy/`
once it ships; until then `tsync doctor` only inspects the machine you
are on.

Minimum on the far side, when transfers start:

```bash
# on the remote host
rsync --version | head -1
# expect: rsync  version 3.x.x

rsync --info=help >/dev/null && echo "rsync capability: ok"
```
