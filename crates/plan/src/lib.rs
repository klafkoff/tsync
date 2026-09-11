//! Turn an audit into a reviewable, dry-run migration plan.
//!
//! Reads a manifest, derives path-mapping rules, excludes torrents that are
//! not safe to move, and packs the rest into size-budgeted batches. Nothing
//! here writes resume data or copies a file.

use std::fmt::Write as _;

use tsync_audit::{Class, Entry, Manifest};
use tsync_core::pathmap::{self, Mapping};

/// Default batch budget: 4 GiB. Soft — a larger torrent becomes its own batch.
pub const DEFAULT_BUDGET: u64 = 4 * (1 << 30);

/// Why a plan could not be built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Mapping derivation failed.
    #[error(transparent)]
    Mapping(#[from] pathmap::Error),
}

/// A torrent that will not be transferred, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exclusion {
    /// The audit entry.
    pub entry: Entry,
    /// Human reason, suitable for the printed plan.
    pub reason: String,
}

/// One batch of whole torrents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    /// 1-based index in plan order.
    pub number: usize,
    /// Eligible torrents, still smallest-first inside the batch.
    pub torrents: Vec<Planned>,
}

impl Batch {
    /// Sum of declared sizes.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.torrents.iter().map(|item| item.entry.bytes).sum()
    }
}

/// An eligible torrent after its save path has been rewritten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    /// The audit entry.
    pub entry: Entry,
    /// Save path on the destination.
    pub dest_path: String,
}

/// The complete dry-run.
#[derive(Clone, Debug)]
pub struct Plan {
    /// Derived rewrite rules.
    pub mapping: Mapping,
    /// Packed eligible torrents.
    pub batches: Vec<Batch>,
    /// Torrents left out, with reasons.
    pub excluded: Vec<Exclusion>,
    /// Soft size budget used to cut batches.
    pub budget: u64,
}

impl Plan {
    /// Eligible torrent count.
    #[must_use]
    pub fn eligible_count(&self) -> usize {
        self.batches.iter().map(|batch| batch.torrents.len()).sum()
    }

    /// Eligible byte count.
    #[must_use]
    pub fn eligible_bytes(&self) -> u64 {
        self.batches.iter().map(Batch::bytes).sum()
    }

    /// Excluded byte count.
    #[must_use]
    pub fn excluded_bytes(&self) -> u64 {
        self.excluded.iter().map(|item| item.entry.bytes).sum()
    }

    /// Whether any two rules share a destination (possible `Album/` collision).
    #[must_use]
    pub fn has_converging_rules(&self) -> bool {
        !self.mapping.converging().is_empty()
    }

    /// Keep the smallest torrents until a byte or count cap is reached.
    ///
    /// A torrent that would exceed the remaining byte budget is not taken
    /// (later items are larger, so the walk stops). Caps of `None` leave that
    /// dimension unlimited. Re-packs the kept set with the same batch budget.
    #[must_use]
    pub fn take_smallest(self, max_bytes: Option<u64>, max_torrents: Option<usize>) -> Self {
        if max_bytes.is_none() && max_torrents.is_none() {
            return self;
        }

        let mut items: Vec<Planned> = self
            .batches
            .iter()
            .flat_map(|batch| batch.torrents.iter().cloned())
            .collect();
        items.sort_by_key(|item| item.entry.bytes);

        let mut taken = Vec::new();
        let mut used = 0_u64;
        for item in items {
            if max_torrents.is_some_and(|limit| taken.len() >= limit) {
                break;
            }
            if max_bytes.is_some_and(|cap| used.saturating_add(item.entry.bytes) > cap) {
                break;
            }
            used = used.saturating_add(item.entry.bytes);
            taken.push(item);
        }

        Self {
            mapping: self.mapping,
            batches: pack(taken, self.budget),
            excluded: self.excluded,
            budget: self.budget,
        }
    }

    /// Human report. Dry-run: no paths in this text are writes.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::from("tsync plan — dry run\n\n");

        let _ = writeln!(out, "  mapping");
        for rule in self.mapping.rules() {
            let _ = writeln!(out, "    {}  →  {}", rule.from_display(), rule.to_display());
        }
        if self.has_converging_rules() {
            let _ = writeln!(
                out,
                "    warning: two sources map onto one destination; \
                 check for colliding album directories"
            );
        }

        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  eligible   {:>5}   {}",
            self.eligible_count(),
            format_bytes(self.eligible_bytes())
        );
        let _ = writeln!(
            out,
            "  excluded   {:>5}   {}",
            self.excluded.len(),
            format_bytes(self.excluded_bytes())
        );

        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  {} batches · {} budget · smallest-first",
            self.batches.len(),
            format_bytes(self.budget)
        );
        let _ = writeln!(out);
        let _ = writeln!(out, "  batch  torrents         size");
        let _ = writeln!(out, "  -----------------------------");
        for batch in &self.batches {
            let _ = writeln!(
                out,
                "   {:>2}     {:>5}     {:>10}",
                batch.number,
                batch.torrents.len(),
                format_bytes(batch.bytes())
            );
        }

        let rewrites: Vec<&Planned> = self
            .batches
            .iter()
            .flat_map(|batch| batch.torrents.iter())
            .collect();
        if !rewrites.is_empty() {
            let _ = writeln!(out);
            let _ = writeln!(out, "  rewrites");
            for item in rewrites {
                let name = if item.entry.name.is_empty() {
                    short_id(&item.entry.id)
                } else {
                    item.entry.name.clone()
                };
                let _ = writeln!(out, "    {name}");
                let _ = writeln!(out, "      {}  →  {}", item.entry.save_path, item.dest_path);
            }
        }

        if !self.excluded.is_empty() {
            let _ = writeln!(out);
            let _ = writeln!(out, "  excluded");
            for item in &self.excluded {
                let _ = writeln!(
                    out,
                    "    {:<12} {}  {}",
                    class_tag(&item.entry.class),
                    short_id(&item.entry.id),
                    if item.entry.name.is_empty() {
                        "—"
                    } else {
                        &item.entry.name
                    }
                );
                let _ = writeln!(out, "                 {}", item.reason);
            }
        }

        out.push('\n');
        out
    }
}

/// Builds a plan from an audit manifest.
///
/// Intact and skipped torrents are eligible. Everything else is listed under
/// excluded. Destination is required so the printed diffs are real paths.
///
/// # Errors
///
/// Returns [`Error::Mapping`] when no save path exists among the eligible set
/// (or `dest` is empty). If every torrent is excluded, the mapping is derived
/// from excluded save paths so the report still shows what *would* have moved.
pub fn build(manifest: &Manifest, dest: &str, budget: u64) -> Result<Plan, Error> {
    let mut eligible = Vec::new();
    let mut excluded = Vec::new();

    for entry in &manifest.torrents {
        match &entry.class {
            Class::Intact | Class::Skipped => eligible.push(entry.clone()),
            other => excluded.push(Exclusion {
                entry: entry.clone(),
                reason: exclusion_reason(other),
            }),
        }
    }

    let paths_for_map: Vec<String> = if eligible.is_empty() {
        excluded
            .iter()
            .map(|item| item.entry.save_path.clone())
            .collect()
    } else {
        eligible
            .iter()
            .map(|entry| entry.save_path.clone())
            .collect()
    };

    let mapping = Mapping::derive(&paths_for_map, dest)?;

    let mut planned = Vec::new();
    for entry in eligible {
        match mapping.apply(&entry.save_path) {
            Some(dest_path) => planned.push(Planned { entry, dest_path }),
            None => excluded.push(Exclusion {
                entry,
                reason: "save path matches no mapping rule".to_owned(),
            }),
        }
    }

    planned.sort_by_key(|item| item.entry.bytes);
    let batches = pack(planned, budget);

    Ok(Plan {
        mapping,
        batches,
        excluded,
        budget,
    })
}

fn pack(items: Vec<Planned>, budget: u64) -> Vec<Batch> {
    let mut batches = Vec::new();
    let mut current: Vec<Planned> = Vec::new();
    let mut used = 0_u64;

    for item in items {
        let size = item.entry.bytes;
        if !current.is_empty() && used.saturating_add(size) > budget {
            batches.push(take_batch(&mut current, batches.len() + 1));
            used = 0;
        }
        used = used.saturating_add(size);
        current.push(item);
    }

    if !current.is_empty() {
        batches.push(take_batch(&mut current, batches.len() + 1));
    }
    batches
}

fn take_batch(current: &mut Vec<Planned>, number: usize) -> Batch {
    Batch {
        number,
        torrents: std::mem::take(current),
    }
}

fn exclusion_reason(class: &Class) -> String {
    match class {
        Class::Partial {
            missing,
            truncated,
            disagreement,
        } => {
            let mut reason = format!("partial: {missing} missing, {truncated} truncated");
            if *disagreement {
                reason.push_str(", resume claims complete");
            }
            reason
        }
        Class::Unverifiable => "could not inspect files on disk".to_owned(),
        Class::Intact | Class::Skipped => String::new(),
    }
}

fn class_tag(class: &Class) -> &'static str {
    match class {
        Class::Intact => "INTACT",
        Class::Skipped => "SKIPPED",
        Class::Unverifiable => "UNVERIFIABLE",
        Class::Partial {
            disagreement: true, ..
        } => "PARTIAL*",
        Class::Partial { .. } => "PARTIAL",
    }
}

fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…", &id[..12])
    } else {
        id.to_owned()
    }
}

fn format_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if n >= GIB {
        let whole = n / GIB;
        let frac = (n % GIB) * 100 / GIB;
        format!("{whole}.{frac:02} GiB")
    } else if n >= MIB {
        format!("{} MiB", n / MIB)
    } else if n >= KIB {
        format!("{} KiB", n / KIB)
    } else {
        format!("{n} B")
    }
}
