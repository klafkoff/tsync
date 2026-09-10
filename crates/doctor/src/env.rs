//! The boundary between a check and the machine it inspects.
//!
//! Checks never touch the system directly. They are handed an [`Environment`]
//! and may only do what it offers, which buys two things.
//!
//! The first is testability. [`Fake`] replays recorded command output, so the
//! whole suite runs with no installed dependencies, no network, and no real
//! host. Version banners vary in ways nobody predicts, and the only honest way
//! to cover that is a table of strings observed in the wild.
//!
//! The second is that local and remote checks become the same code. An
//! implementation that runs commands over SSH would make every existing check
//! work against a remote machine without any of them being aware it happened.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

/// What a finished command produced.
#[derive(Clone, Debug)]
pub struct CommandOutput {
    /// Exit status, or `None` if the process was killed by a signal.
    pub code: Option<i32>,
    /// Everything written to standard output.
    pub stdout: String,
    /// Everything written to standard error.
    pub stderr: String,
}

impl CommandOutput {
    /// Whether the command exited zero.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }

    /// Both streams concatenated.
    ///
    /// Tools disagree about where to print a version banner — `ssh -V` uses
    /// stderr, `rsync --version` uses stdout — and for identification the
    /// distinction carries no information.
    #[must_use]
    pub fn combined(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// The operating system being inspected.
///
/// Carried explicitly rather than read from `cfg!` because remediation has to
/// describe the machine under test, which is not always the one running this
/// binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// macOS.
    MacOs,
    /// Any Linux distribution.
    Linux,
    /// Anything else.
    Other,
}

/// Everything a check is permitted to observe.
pub trait Environment {
    /// The operating system under inspection.
    fn platform(&self) -> Platform;

    /// The home directory of the account being inspected, if known.
    fn home(&self) -> Option<PathBuf>;

    /// Runs `program` with `args` and waits for it.
    ///
    /// # Errors
    ///
    /// Returns an error only when the program could not be launched at all. A
    /// program that ran and exited non-zero is a success at this level, and an
    /// [`Ok`] carrying a non-zero [`CommandOutput::code`].
    fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutput>;

    /// Lists a directory's entries.
    ///
    /// Deliberately a listing rather than an existence test. On macOS the two
    /// are separate permissions: `stat` on a known path can succeed while
    /// reading its parent directory is denied, which yields a confusing
    /// partial view instead of a clean failure.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be listed, preserving the
    /// [`io::ErrorKind`] so callers can tell absent from forbidden.
    fn list_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
}

/// The real machine this process is running on.
#[derive(Clone, Copy, Debug, Default)]
pub struct Host;

impl Environment for Host {
    fn platform(&self) -> Platform {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "linux") {
            Platform::Linux
        } else {
            Platform::Other
        }
    }

    fn home(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutput> {
        let output = std::process::Command::new(program).args(args).output()?;
        Ok(CommandOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn list_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect()
    }
}

/// A scripted environment for tests.
///
/// Only registered invocations succeed. Anything else that is not marked
/// missing reports a non-zero exit, which is how a real tool answers a flag it
/// does not recognize — so a capability probe fails simply by not being
/// registered, and tests stay short.
#[derive(Clone, Debug)]
pub struct Fake {
    platform: Platform,
    home: Option<PathBuf>,
    commands: HashMap<String, CommandOutput>,
    missing: HashSet<String>,
    dirs: HashMap<PathBuf, Vec<PathBuf>>,
    dir_errors: HashMap<PathBuf, io::ErrorKind>,
}

impl Fake {
    /// A machine with nothing installed and no home directory.
    #[must_use]
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            home: None,
            commands: HashMap::new(),
            missing: HashSet::new(),
            dirs: HashMap::new(),
            dir_errors: HashMap::new(),
        }
    }

    /// Sets the home directory.
    #[must_use]
    pub fn home(mut self, path: impl Into<PathBuf>) -> Self {
        self.home = Some(path.into());
        self
    }

    /// Registers a successful invocation, written as it would be typed.
    #[must_use]
    pub fn command(self, invocation: &str, stdout: &str) -> Self {
        self.command_exit(invocation, 0, stdout, "")
    }

    /// Registers an invocation with an explicit exit code and both streams.
    #[must_use]
    pub fn command_exit(mut self, invocation: &str, code: i32, stdout: &str, stderr: &str) -> Self {
        self.commands.insert(
            invocation.to_owned(),
            CommandOutput {
                code: Some(code),
                stdout: stdout.to_owned(),
                stderr: stderr.to_owned(),
            },
        );
        self
    }

    /// Marks a program as absent, so every invocation of it fails to launch.
    #[must_use]
    pub fn missing(mut self, program: &str) -> Self {
        self.missing.insert(program.to_owned());
        self
    }

    /// Registers a listable directory holding the named entries.
    #[must_use]
    pub fn dir(mut self, path: impl Into<PathBuf>, entries: &[&str]) -> Self {
        let path = path.into();
        let entries = entries.iter().map(|name| path.join(name)).collect();
        self.dirs.insert(path, entries);
        self
    }

    /// Registers a directory that fails to list with the given error kind.
    #[must_use]
    pub fn dir_error(mut self, path: impl Into<PathBuf>, kind: io::ErrorKind) -> Self {
        self.dir_errors.insert(path.into(), kind);
        self
    }
}

impl Environment for Fake {
    fn platform(&self) -> Platform {
        self.platform
    }

    fn home(&self) -> Option<PathBuf> {
        self.home.clone()
    }

    fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutput> {
        if self.missing.contains(program) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{program}: no such file or directory"),
            ));
        }

        let invocation = if args.is_empty() {
            program.to_owned()
        } else {
            format!("{program} {}", args.join(" "))
        };

        Ok(self
            .commands
            .get(&invocation)
            .cloned()
            .unwrap_or_else(|| CommandOutput {
                code: Some(1),
                stdout: String::new(),
                stderr: format!("{program}: unrecognized option\n"),
            }))
    }

    fn list_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        if let Some(kind) = self.dir_errors.get(path) {
            return Err(io::Error::new(*kind, format!("{}", path.display())));
        }
        self.dirs
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{}", path.display())))
    }
}
