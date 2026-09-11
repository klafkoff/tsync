//! Drive GNU rsync. One invocation per mapping rule, files listed explicitly.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tsync_audit::paths::{content_path, locate_save};
use tsync_core::metainfo;
use tsync_core::pathmap::Rule;
use tsync_plan::{Plan, Planned};

use crate::Options;

/// Local path or `host:/path` over SSH.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// Destination is on this machine.
    Local(PathBuf),
    /// Destination is `ssh` + a Unix path.
    Remote {
        /// `Host` alias or `user@host`.
        host: String,
        /// Absolute path on that host.
        path: String,
    },
}

impl Target {
    pub(crate) fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("rsync destination is empty".to_owned());
        }
        if let Some((host, path)) = split_remote(spec) {
            if path.is_empty() {
                return Err(format!("rsync destination {spec} has no path after ':'"));
            }
            return Ok(Self::Remote {
                host: host.to_owned(),
                path: path.to_owned(),
            });
        }
        Ok(Self::Local(PathBuf::from(spec)))
    }

    fn rsync_url(&self) -> String {
        match self {
            Self::Local(path) => path.to_string_lossy().into_owned(),
            Self::Remote { host, path } => format!("{host}:{path}"),
        }
    }
}

/// `host:/abs` or `user@host:rel`. Local paths (`/…`, `./…`) stay local.
fn split_remote(spec: &str) -> Option<(&str, &str)> {
    if spec.starts_with('/') || spec.starts_with('.') {
        return None;
    }
    let idx = spec.find(':')?;
    let host = &spec[..idx];
    if host.is_empty() || host.contains('/') {
        return None;
    }
    Some((host, &spec[idx + 1..]))
}

pub(crate) fn resolve_binary() -> Result<PathBuf, String> {
    const CANDIDATES: &[&str] = &["rsync", "/opt/homebrew/bin/rsync", "/usr/local/bin/rsync"];
    for candidate in CANDIDATES {
        if is_gnu(Path::new(candidate)) {
            return Ok(PathBuf::from(candidate));
        }
    }
    Err("need GNU rsync >= 3.1.0 (for --info=progress2); run `tsync doctor`".to_owned())
}

fn is_gnu(path: &Path) -> bool {
    Command::new(path)
        .arg("--info=help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(crate) fn ensure_dest(target: &Target) -> Result<(), String> {
    match target {
        Target::Local(path) => fs::create_dir_all(path)
            .map_err(|error| format!("cannot create destination {}: {error}", path.display())),
        Target::Remote { host, path } => {
            let status = Command::new("ssh")
                .args(["-o", "BatchMode=yes", host, "mkdir", "-p", "--", path])
                .status()
                .map_err(|error| format!("ssh {host}: {error}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("ssh {host} mkdir -p {path} failed ({status})"))
            }
        }
    }
}

struct Group {
    source_root: PathBuf,
    files: Vec<PathBuf>,
    torrents: Vec<String>,
}

/// Copy planned torrents. Per-torrent "no files" stays a torrent failure;
/// the rsync itself is one process per mapping rule.
pub(crate) fn copy_plan(
    opts: &Options,
    plan: &Plan,
    target: &Target,
    rsync: &Path,
) -> (usize, Vec<(String, String)>) {
    let mut groups: Vec<Group> = Vec::new();
    let mut failed = Vec::new();

    for item in plan.batches.iter().flat_map(|batch| batch.torrents.iter()) {
        let Some(rule) = plan.mapping.rule_for(&item.entry.save_path) else {
            failed.push((
                item.entry.id.clone(),
                "save path matches no mapping rule".to_owned(),
            ));
            continue;
        };
        match collect_relatives(opts, item, rule) {
            Ok(relatives) if relatives.is_empty() => failed.push((
                item.entry.id.clone(),
                "no source files could be copied".to_owned(),
            )),
            Ok(relatives) => {
                let source_root =
                    locate_save(rule.from_display().as_bytes(), opts.data_root.as_deref());
                if let Some(group) = groups
                    .iter_mut()
                    .find(|group| group.source_root == source_root)
                {
                    group.files.extend(relatives);
                    group.torrents.push(item.entry.id.clone());
                } else {
                    groups.push(Group {
                        source_root,
                        files: relatives,
                        torrents: vec![item.entry.id.clone()],
                    });
                }
            }
            Err(reason) => failed.push((item.entry.id.clone(), reason)),
        }
    }

    let mut copied = 0;
    for group in groups {
        match rsync_files(rsync, target, &group) {
            Ok(()) => copied += group.torrents.len(),
            Err(reason) => {
                for id in group.torrents {
                    failed.push((id, reason.clone()));
                }
            }
        }
    }
    (copied, failed)
}

fn collect_relatives(opts: &Options, item: &Planned, rule: &Rule) -> Result<Vec<PathBuf>, String> {
    let torrent_path = opts.bt_backup.join(format!("{}.torrent", item.entry.id));
    let bytes = fs::read(&torrent_path).map_err(|error| format!("read torrent: {error}"))?;
    let meta = metainfo::parse(&bytes).map_err(|error| error.to_string())?;
    let source_root = locate_save(rule.from_display().as_bytes(), opts.data_root.as_deref());

    let mut relatives = Vec::new();
    for file in &meta.files {
        let source = content_path(
            item.entry.save_path.as_bytes(),
            &meta,
            file,
            opts.data_root.as_deref(),
        );
        if !source.exists() {
            continue;
        }
        let relative = source.strip_prefix(&source_root).map_err(|_| {
            format!(
                "source {} is not under {}",
                source.display(),
                source_root.display()
            )
        })?;
        relatives.push(relative.to_path_buf());
    }
    Ok(relatives)
}

fn rsync_files(rsync: &Path, target: &Target, group: &Group) -> Result<(), String> {
    if group.files.is_empty() {
        return Ok(());
    }
    let list = write_files_from(&group.files).map_err(|error| format!("files-from: {error}"))?;
    let dest = target.rsync_url();
    let mut cmd = Command::new(rsync);
    cmd.arg("-a")
        .arg("--partial")
        .arg("--info=progress2")
        .arg("--outbuf=L")
        .arg("--from0")
        .arg("--files-from")
        .arg(&list);
    if matches!(target, Target::Remote { .. }) {
        cmd.arg("-e").arg("ssh -o BatchMode=yes");
    }
    cmd.arg("--")
        .arg(slash_dir(&group.source_root))
        .arg(slash_dir_str(&dest));
    let status = cmd.status();
    let _ = fs::remove_file(&list);
    let status = status.map_err(|error| format!("rsync: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("rsync exited {status}"))
    }
}

fn slash_dir(path: &Path) -> PathBuf {
    let mut out = path.to_path_buf();
    out.push("");
    out
}

fn slash_dir_str(spec: &str) -> String {
    if spec.ends_with('/') {
        spec.to_owned()
    } else {
        format!("{spec}/")
    }
}

fn write_files_from(files: &[PathBuf]) -> io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "tsync-files-from-{}-{}.from0",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let mut out = fs::File::create(&path)?;
    for file in files {
        write_relative(&mut out, file)?;
        out.write_all(&[0])?;
    }
    out.flush()?;
    Ok(path)
}

fn write_relative(out: &mut fs::File, path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        out.write_all(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        out.write_all(path.to_string_lossy().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_spec_is_host_and_path() {
        assert_eq!(
            Target::parse("seedbox:/opt/seedbox/data").expect("parse"),
            Target::Remote {
                host: "seedbox".into(),
                path: "/opt/seedbox/data".into(),
            }
        );
        assert_eq!(
            Target::parse("root@203.0.113.10:/data").expect("parse"),
            Target::Remote {
                host: "root@203.0.113.10".into(),
                path: "/data".into(),
            }
        );
    }

    #[test]
    fn absolute_and_relative_stay_local() {
        assert_eq!(
            Target::parse("/opt/seedbox/data").expect("parse"),
            Target::Local(PathBuf::from("/opt/seedbox/data"))
        );
        assert_eq!(
            Target::parse("./dest").expect("parse"),
            Target::Local(PathBuf::from("./dest"))
        );
    }
}
