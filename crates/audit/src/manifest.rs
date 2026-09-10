//! The audit result: a redacted inventory.

use std::fmt::Write as _;

use crate::classify::Class;

/// One torrent (or unpaired backup file) in the inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// 40-character hex infohash, or the backup file stem when parsing failed.
    pub id: String,
    /// Torrent name, lossy-decoded. Empty for unpaired resume files.
    pub name: String,
    /// Save path as recorded in resume data, lossy-decoded.
    pub save_path: String,
    /// Declared total size.
    pub bytes: u64,
    /// Observed classification.
    pub class: Class,
}

/// A complete inventory of one `BT_backup`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// Directory that was scanned.
    pub bt_backup: String,
    /// Paired torrents, in directory order.
    pub torrents: Vec<Entry>,
    /// `.torrent` files with no matching `.fastresume`.
    pub unpaired_torrent: usize,
    /// `.fastresume` files with no matching `.torrent`.
    pub unpaired_resume: usize,
}

impl Manifest {
    /// How many paired torrents have this class.
    #[must_use]
    pub fn count_class(&self, predicate: impl Fn(&Class) -> bool) -> usize {
        self.torrents
            .iter()
            .filter(|entry| predicate(&entry.class))
            .count()
    }

    /// Render the human summary printed by `tsync audit`.
    #[must_use]
    pub fn render(&self) -> String {
        let intact = self.count_class(|class| matches!(class, Class::Intact));
        let skipped = self.count_class(|class| matches!(class, Class::Skipped));
        let partial = self.count_class(|class| matches!(class, Class::Partial { .. }));
        let unverifiable = self.count_class(|class| matches!(class, Class::Unverifiable));
        let disagreements = self.count_class(|class| {
            matches!(
                class,
                Class::Partial {
                    disagreement: true,
                    ..
                }
            )
        });

        let mut out = format!("tsync audit — {n} torrents\n\n", n = self.torrents.len());
        let _ = writeln!(out, "  intact        {intact}");
        if skipped > 0 {
            let _ = writeln!(out, "  skipped       {skipped}   (deselected files only)");
        }
        if partial > 0 {
            let _ = writeln!(
                out,
                "  partial       {partial}{}",
                if disagreements > 0 {
                    format!("   ({disagreements} claimed complete)")
                } else {
                    String::new()
                }
            );
        }
        if unverifiable > 0 {
            let _ = writeln!(out, "  unverifiable  {unverifiable}");
        }
        if self.unpaired_torrent > 0 || self.unpaired_resume > 0 {
            let _ = writeln!(
                out,
                "  unpaired      {} torrent-only, {} resume-only",
                self.unpaired_torrent, self.unpaired_resume
            );
        }

        let notables: Vec<&Entry> = self
            .torrents
            .iter()
            .filter(|entry| !matches!(entry.class, Class::Intact | Class::Skipped))
            .collect();

        if !notables.is_empty() {
            out.push('\n');
            for entry in notables {
                let _ = writeln!(out, "  {}  {}", label(&entry.class), short_id(&entry.id));
                if !entry.name.is_empty() {
                    let _ = writeln!(out, "           {}", entry.name);
                }
                if !entry.save_path.is_empty() {
                    let _ = writeln!(out, "           {}", entry.save_path);
                }
                if let Class::Partial {
                    missing,
                    truncated,
                    disagreement,
                } = entry.class
                {
                    let _ = writeln!(
                        out,
                        "           missing {missing}, truncated {truncated}{}",
                        if disagreement {
                            ", resume claims complete"
                        } else {
                            ""
                        }
                    );
                }
            }
        }

        out.push('\n');
        out
    }
}

fn label(class: &Class) -> &'static str {
    match class {
        Class::Intact => "INTACT",
        Class::Partial {
            disagreement: true, ..
        } => "PARTIAL*",
        Class::Partial { .. } => "PARTIAL",
        Class::Skipped => "SKIPPED",
        Class::Unverifiable => "UNVERIFIABLE",
    }
}

fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…", &id[..12])
    } else {
        id.to_owned()
    }
}
