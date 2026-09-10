//! Turn per-file observations into a torrent-level class.

/// What `stat` said about one expected file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observed {
    /// The file is present and this long.
    Present {
        /// Size from `stat`.
        size: u64,
    },
    /// The path does not exist.
    Absent,
    /// The path exists but this process cannot read its metadata.
    Denied,
}

/// One file after it has been looked up on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileView {
    /// Display name relative to the torrent, for the report.
    pub relative: String,
    /// Length declared in the torrent.
    pub declared: u64,
    /// libtorrent priority. `0` means the user deselected the file.
    pub priority: i64,
    /// What the filesystem reported.
    pub observed: Observed,
}

/// How a torrent relates to the files on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Class {
    /// Every wanted file is present at its declared size.
    Intact,
    /// Wanted files are missing or the wrong size.
    Partial {
        /// Wanted files that are not on disk.
        missing: usize,
        /// Wanted files whose size disagrees with the torrent.
        truncated: usize,
        /// The resume bitfield claims complete, but the disk does not.
        disagreement: bool,
    },
    /// The only absences are files the user deselected. Healthy.
    Skipped,
    /// At least one path could not be inspected (permissions / TCC).
    Unverifiable,
}

/// Classify a torrent from its file observations and optional completeness claim.
#[must_use]
pub fn classify(files: &[FileView], claims_complete: Option<bool>) -> Class {
    if files
        .iter()
        .any(|file| matches!(file.observed, Observed::Denied))
    {
        return Class::Unverifiable;
    }

    let mut missing = 0;
    let mut truncated = 0;
    let mut skipped_absent = 0;

    for file in files {
        let skipped = file.priority == 0;
        match file.observed {
            Observed::Absent if skipped => skipped_absent += 1,
            Observed::Absent => missing += 1,
            Observed::Present { size } if size != file.declared && !skipped => {
                truncated += 1;
            }
            Observed::Denied | Observed::Present { .. } => {}
        }
    }

    if missing == 0 && truncated == 0 {
        return if skipped_absent > 0 {
            Class::Skipped
        } else {
            Class::Intact
        };
    }

    Class::Partial {
        missing,
        truncated,
        disagreement: claims_complete == Some(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(declared: u64, priority: i64, observed: Observed) -> FileView {
        FileView {
            relative: "a.flac".to_owned(),
            declared,
            priority,
            observed,
        }
    }

    #[test]
    fn intact_when_every_wanted_file_matches() {
        let files = [file(100, 1, Observed::Present { size: 100 })];
        assert_eq!(classify(&files, Some(true)), Class::Intact);
    }

    #[test]
    fn truncated_is_partial_even_though_the_file_exists() {
        let files = [file(100, 1, Observed::Present { size: 40 })];
        assert_eq!(
            classify(&files, Some(true)),
            Class::Partial {
                missing: 0,
                truncated: 1,
                disagreement: true,
            }
        );
    }

    #[test]
    fn missing_wanted_file_with_a_complete_claim_is_disagreement() {
        let files = [file(100, 1, Observed::Absent)];
        assert_eq!(
            classify(&files, Some(true)),
            Class::Partial {
                missing: 1,
                truncated: 0,
                disagreement: true,
            }
        );
    }

    #[test]
    fn deselected_absent_files_are_skipped_not_partial() {
        let files = [
            file(100, 1, Observed::Present { size: 100 }),
            file(50, 0, Observed::Absent),
        ];
        assert_eq!(classify(&files, Some(true)), Class::Skipped);
    }

    #[test]
    fn denied_stat_is_unverifiable() {
        let files = [file(100, 1, Observed::Denied)];
        assert_eq!(classify(&files, None), Class::Unverifiable);
    }
}
