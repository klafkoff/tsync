//! What a check concluded.

use std::fmt;

/// How much a finding matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Working as required.
    Pass,
    /// Usable, but degraded or missing something optional.
    Warn,
    /// Blocks the operations that depend on this check.
    Fail,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        })
    }
}

/// A check's verdict.
///
/// A non-passing outcome that does not say how to fix the problem is only
/// marginally more useful than no check at all. `expected` and `remediation`
/// exist so that writing the useful form is the path of least resistance.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// How much this finding matters.
    pub status: Status,
    /// What was actually found.
    pub found: String,
    /// What was required, when that differs from what was found.
    pub expected: Option<String>,
    /// The concrete action that resolves the finding.
    pub remediation: Option<String>,
}

impl Outcome {
    /// A passing outcome describing what was found.
    #[must_use]
    pub fn pass(found: impl Into<String>) -> Self {
        Self::new(Status::Pass, found)
    }

    /// A warning: usable, but worth knowing about.
    #[must_use]
    pub fn warn(found: impl Into<String>) -> Self {
        Self::new(Status::Warn, found)
    }

    /// A failure that blocks dependent operations.
    #[must_use]
    pub fn fail(found: impl Into<String>) -> Self {
        Self::new(Status::Fail, found)
    }

    fn new(status: Status, found: impl Into<String>) -> Self {
        Self {
            status,
            found: found.into(),
            expected: None,
            remediation: None,
        }
    }

    /// Records what was required.
    #[must_use]
    pub fn expected(mut self, expected: impl Into<String>) -> Self {
        self.expected = Some(expected.into());
        self
    }

    /// Records the exact action that resolves the finding.
    #[must_use]
    pub fn fix(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }
}
