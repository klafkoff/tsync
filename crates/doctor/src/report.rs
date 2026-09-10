//! Running a set of checks and rendering the result.

use std::fmt::Write as _;

use crate::checks::Check;
use crate::env::Environment;
use crate::outcome::{Outcome, Status};

/// Column width for the check identifier, with room to spare so output stays
/// aligned as checks are added.
const ID_WIDTH: usize = 15;

/// Indentation for the detail lines under a finding.
const DETAIL_INDENT: &str = "                        ";

/// One check and what it concluded.
pub struct Entry {
    /// The check's identifier.
    pub id: &'static str,
    /// The verdict.
    pub outcome: Outcome,
}

/// The result of running a set of checks.
pub struct Report {
    /// Which machine was inspected, for the heading.
    pub scope: String,
    /// One entry per check, in the order they ran.
    pub entries: Vec<Entry>,
}

impl Report {
    /// Runs every check in `checks` against `env`.
    #[must_use]
    pub fn run(scope: impl Into<String>, checks: &'static [Check], env: &dyn Environment) -> Self {
        Self {
            scope: scope.into(),
            entries: checks
                .iter()
                .map(|check| Entry {
                    id: check.id,
                    outcome: (check.probe)(env),
                })
                .collect(),
        }
    }

    /// How many checks reached a given status.
    #[must_use]
    pub fn count(&self, status: Status) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.outcome.status == status)
            .count()
    }

    /// Whether anything failed.
    ///
    /// Warnings deliberately do not block. They describe degraded but usable
    /// setups, and a preflight that halts on those stops being run at all.
    #[must_use]
    pub fn is_blocking(&self) -> bool {
        self.count(Status::Fail) > 0
    }

    /// Renders the report as the text shown to the user.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = format!("tsync doctor — {}\n\n", self.scope);

        for entry in &self.entries {
            let outcome = &entry.outcome;
            let _ = writeln!(
                out,
                "  {status:<4}  {id:<ID_WIDTH$} {found}",
                status = outcome.status,
                id = entry.id,
                found = outcome.found,
            );
            if let Some(expected) = &outcome.expected {
                let _ = writeln!(out, "{DETAIL_INDENT}need {expected}");
            }
            if let Some(remediation) = &outcome.remediation {
                let _ = writeln!(out, "{DETAIL_INDENT}fix: {remediation}");
            }
        }

        let _ = write!(out, "\n  {}\n", self.summary());
        out
    }

    fn summary(&self) -> String {
        let counts = [
            (self.count(Status::Fail), "failed"),
            (self.count(Status::Warn), "warning"),
            (self.count(Status::Pass), "passed"),
        ];

        let parts: Vec<String> = counts
            .iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, label)| {
                // "warning" is the only one of the three that needs a plural.
                if *label == "warning" && *count != 1 {
                    format!("{count} warnings")
                } else {
                    format!("{count} {label}")
                }
            })
            .collect();

        if parts.is_empty() {
            return "no checks ran".to_owned();
        }
        parts.join(" · ")
    }
}
