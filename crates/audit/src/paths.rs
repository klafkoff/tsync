//! Reconstruct the on-disk path of a torrent file.

use std::path::{Path, PathBuf};

use tsync_core::metainfo::{ContentFile, Metainfo};

/// Builds the absolute path audit should `stat`.
///
/// Multi-file: `save_path / info.name / <components>`.
/// Single-file: `save_path / info.name`.
///
/// When `data_root` is set, `save_path` is treated as relative to that
/// directory (leading `/` stripped). Tests use this so fixtures written under
/// a temp dir still carry realistic `/srv/...` prefixes in resume data.
#[must_use]
pub fn content_path(
    save_path: &[u8],
    meta: &Metainfo,
    file: &ContentFile,
    data_root: Option<&Path>,
) -> PathBuf {
    let mut path = locate_save(save_path, data_root);
    path.push(os_from_bytes(&meta.name));
    if meta.multi_file {
        for component in &file.path {
            path.push(os_from_bytes(component));
        }
    }
    path
}

/// The torrent root rsync should copy: `save_path / name`.
///
/// Multi-file torrents are a directory; single-file torrents are a file of
/// that name. `data_root` relocates the save path the same way
/// [`content_path`] does.
#[must_use]
pub fn torrent_root(save_path: &str, name: &str, data_root: Option<&Path>) -> PathBuf {
    let mut path = locate_save(save_path.as_bytes(), data_root);
    if !name.is_empty() {
        path.push(os_from_bytes(name.as_bytes()));
    }
    path
}

/// A slash-separated relative name for the report, never an absolute path.
#[must_use]
pub fn relative_name(meta: &Metainfo, file: &ContentFile) -> String {
    if meta.multi_file {
        file.path
            .iter()
            .map(|component| String::from_utf8_lossy(component).into_owned())
            .collect::<Vec<_>>()
            .join("/")
    } else {
        String::from_utf8_lossy(&meta.name).into_owned()
    }
}

/// The on-disk directory for a resume `save_path`, honoring `data_root`.
#[must_use]
pub fn locate_save(save_path: &[u8], data_root: Option<&Path>) -> PathBuf {
    let save = os_from_bytes(save_path);
    match data_root {
        None => save,
        Some(root) => match save.strip_prefix("/") {
            Ok(relative) => root.join(relative),
            Err(_) => root.join(save),
        },
    }
}

fn os_from_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).as_ref())
    }
}
