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
| `ssh` | `OpenSSH_…` | install the platform's OpenSSH client, then [§4](#4-ssh-key-access-to-the-destination) |
| `rsync` | `GNU rsync 3.x.x` | macOS: `brew install rsync` and fix `PATH` as above |
| `bt_backup` | `readable, N torrents` | grant Full Disk Access to the terminal (macOS), or pass a custom state directory later |

qBittorrent does not have to be running for `doctor`. The backup directory
only has to be listable. On macOS, `stat` succeeding and `ls` failing is
TCC, not a Unix permission — System Settings → Privacy & Security → Full
Disk Access, then restart the terminal.

---

## 4. SSH key access to the destination

`tsync` and `rsync` both ride SSH. Password login is not enough: a prompt
will hang an unattended transfer. The check at the end of this section is
the requirement — `BatchMode=yes` must succeed.

Replace `USER` and `HOST` with the account and address of the other
machine (`deploy@seedbox.example`, …).

### 4.1 Create a key on this machine

Skip the `ssh-keygen` line if `ls` already prints a path.

```bash
ls ~/.ssh/id_ed25519.pub 2>/dev/null \
  || ssh-keygen -t ed25519 -C "$(whoami)@$(hostname -s)" -f ~/.ssh/id_ed25519 -N ""

chmod 700 ~/.ssh
chmod 600 ~/.ssh/id_ed25519
chmod 644 ~/.ssh/id_ed25519.pub

ssh-keygen -y -f ~/.ssh/id_ed25519 >/dev/null && echo "ssh key: ok"
# expect: ssh key: ok
```

`-N ""` is an empty passphrase so transfers do not stop for a prompt. To
use a passphrase instead, omit `-N ""` and load the key once per login:

```bash
# macOS — store the passphrase in the Keychain
ssh-add --apple-use-keychain ~/.ssh/id_ed25519

# Linux
eval "$(ssh-agent -s)"
ssh-add ~/.ssh/id_ed25519

ssh-add -l | grep -q ED25519 && echo "ssh-agent: ok"
# expect: ssh-agent: ok
```

### 4.2 Install the public key on the remote (pick one)

**A. Password login still works.** From this machine:

```bash
ssh-copy-id -i ~/.ssh/id_ed25519.pub USER@HOST
```

**B. A session is already open on the other machine.** On this
machine, copy the public key:

```bash
cat ~/.ssh/id_ed25519.pub
# expect: ssh-ed25519 AAAA… comment
```

On the remote:

```bash
mkdir -p ~/.ssh
chmod 700 ~/.ssh
# append the one line you copied — do not wrap it
printf '%s\n' 'ssh-ed25519 AAAA… comment' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
```

If you connect as `root` now and will use a different user later, write
that user’s `~/.ssh/authorized_keys`, not root’s.

### 4.3 Optional Host alias

Saves repeating `USER@HOST` and stops OpenSSH offering every key in
`~/.ssh` (some `sshd` configs drop the connection after a few failures).

```bash
# set these to the other machine and the account that owns authorized_keys
remote_host=seedbox.example
remote_user=deploy

mkdir -p ~/.ssh
chmod 700 ~/.ssh
touch ~/.ssh/config
chmod 600 ~/.ssh/config

cat >> ~/.ssh/config <<EOF
Host seedbox
  HostName ${remote_host}
  User ${remote_user}
  IdentityFile ~/.ssh/id_ed25519
  IdentitiesOnly yes
EOF
```

After this, `ssh seedbox` and `rsync … seedbox:` use the same alias.

### 4.4 Check — this is the requirement

`BatchMode=yes` refuses a password prompt. If this fails, transfers will
hang. `accept-new` records the host key on first connect (OpenSSH 7.6+).

```bash
# with the alias from 4.3
ssh -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new \
  -i ~/.ssh/id_ed25519 seedbox 'echo ssh-key-auth: ok'
# expect: ssh-key-auth: ok

# without an alias
# ssh -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new \
#   -i ~/.ssh/id_ed25519 USER@HOST 'echo ssh-key-auth: ok'
```

If it asks for a password, the public key is not in that account’s
`authorized_keys`, or the private key is not the one you think. Re-run
4.2, then this check — do not type the password to “get past it.”

---

## 5. The other machine

After [§4](#4-ssh-key-access-to-the-destination) succeeds, `tsync doctor`
only inspects the machine you are on.

The destination must already have a qBittorrent 4.x or 5.x WebAPI, disk
for the library, and GNU rsync. Dest stays paused until `handoff`. See
the contract in the [README](../README.md#what-the-other-machine-must-already-have).

Two dest settings that are not the copy itself:

- The dest client's default save path for **new** torrents must be the
  same root you pass as `--to` (often `/data`). qBittorrent 5 reads
  `Session\DefaultSavePath`. Image defaults such as `/downloads` are
  a different directory and often are not mounted.
- `transfer` is rsync. It keeps the source machine's file ownership
  (macOS, Linux, or Windows). The dest client may run as another user
  (a container `PUID`). After the copy, `chown` the dest tree to that
  user. Complete torrents can seed without write. A new add cannot.

GNU rsync on the far side, when transfers start:

```bash
# on the remote host (after: ssh seedbox)
rsync --version | head -1
# expect: rsync  version 3.x.x

rsync --info=help >/dev/null && echo "rsync capability: ok"
# expect: rsync capability: ok
```
