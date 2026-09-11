//! Move a torrent library between machines without re-downloading it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tsync_audit::Options;
use tsync_doctor::{Host, Report, checks};
use tsync_plan::DEFAULT_BUDGET;

/// tsync CLI
#[derive(Parser)]
#[command(name = "tsync", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check that this machine has what tsync needs.
    Doctor {
        /// List the checks without running them.
        #[arg(long)]
        list: bool,
    },
    /// Inventory a local client: torrents, files, completion state.
    Audit {
        /// qBittorrent `BT_backup` directory. Blank = per-OS default.
        #[arg(long)]
        bt_backup: Option<PathBuf>,
        /// Resolve save paths relative to this directory (tests / relocated data).
        #[arg(long)]
        data_root: Option<PathBuf>,
    },
    /// Derive the path mapping and batch plan. Dry-run: writes nothing.
    Plan {
        /// Destination data root the save paths will be rewritten to.
        #[arg(long)]
        to: PathBuf,
        /// qBittorrent `BT_backup` directory. Blank = per-OS default.
        #[arg(long)]
        bt_backup: Option<PathBuf>,
        /// Resolve save paths relative to this directory (tests / relocated data).
        #[arg(long)]
        data_root: Option<PathBuf>,
        /// Soft batch budget in GiB. A larger torrent becomes its own batch.
        #[arg(long, default_value_t = 4)]
        budget_gib: u64,
    },
    /// Write rewritten resume data to a staging directory. Never touches originals.
    Rewrite {
        /// Destination data root the save paths will be rewritten to.
        #[arg(long)]
        to: PathBuf,
        /// Directory that receives copied `.torrent` files and new resumes.
        #[arg(long)]
        staging: PathBuf,
        /// qBittorrent `BT_backup` directory. Blank = per-OS default.
        #[arg(long)]
        bt_backup: Option<PathBuf>,
        /// Resolve save paths relative to this directory (tests / relocated data).
        #[arg(long)]
        data_root: Option<PathBuf>,
        /// Soft batch budget in GiB, same as `plan`.
        #[arg(long, default_value_t = 4)]
        budget_gib: u64,
    },
    /// Copy torrent content with rsync. Source files are never modified.
    Transfer {
        /// Destination save-path root the client will use (same as `plan` / `rewrite`).
        #[arg(long)]
        to: PathBuf,
        /// Where the bytes go. Local path, or `host:/abs/path` over SSH.
        /// Defaults to `--to`. Use this when that path is not the host path
        /// (Docker `/data` vs `/opt/seedbox/data`).
        #[arg(long)]
        rsync_to: Option<String>,
        /// qBittorrent `BT_backup` directory. Blank = per-OS default.
        #[arg(long)]
        bt_backup: Option<PathBuf>,
        /// Resolve save paths relative to this directory (tests / relocated data).
        #[arg(long)]
        data_root: Option<PathBuf>,
        /// Soft batch budget in GiB, same as `plan`.
        #[arg(long, default_value_t = 4)]
        budget_gib: u64,
        /// Copy only the smallest torrents that fit in this many MiB.
        #[arg(long)]
        max_mib: Option<u64>,
        /// Copy at most this many torrents.
        #[arg(long)]
        max_torrents: Option<usize>,
        /// Select and report, but do not copy.
        #[arg(long)]
        dry_run: bool,
    },
    /// Add staged torrents to a destination client, paused, then recheck.
    Import {
        /// Destination `WebUI` base URL, e.g. `<http://127.0.0.1:8080>`
        #[arg(long)]
        url: String,
        /// Staging directory from `tsync rewrite`.
        #[arg(long)]
        staging: PathBuf,
        /// Destination `WebUI` username.
        #[arg(long)]
        username: String,
        /// Env var that holds the destination password. Never pass the password here.
        #[arg(long, default_value = "QBT_PASSWORD")]
        password_env: String,
        /// Source `WebUI`, for the dual-seed guard.
        #[arg(long)]
        source_url: Option<String>,
        /// Source username. Defaults to `--username`.
        #[arg(long)]
        source_username: Option<String>,
        /// Env var for the source password. Defaults to `--password-env`.
        #[arg(long)]
        source_password_env: Option<String>,
        /// Skip the dual-seed check. Dual-announce on a private tracker is on you.
        #[arg(long)]
        allow_unverified_source: bool,
        /// Select and refuse, but do not add.
        #[arg(long)]
        dry_run: bool,
        /// Import only the smallest N staged torrents.
        #[arg(long)]
        max_torrents: Option<usize>,
    },
    /// Prove destination torrents are complete and still paused.
    Verify {
        /// Destination `WebUI` base URL, e.g. `<http://127.0.0.1:8080>`
        #[arg(long)]
        url: String,
        /// Staging directory from `tsync rewrite`. Expected hashes come from here.
        #[arg(long)]
        staging: PathBuf,
        /// Destination `WebUI` username.
        #[arg(long)]
        username: String,
        /// Env var that holds the destination password. Never pass the password here.
        #[arg(long, default_value = "QBT_PASSWORD")]
        password_env: String,
        /// Accept a complete torrent that is already seeding. Default is to
        /// fail it — handoff is what starts the destination.
        #[arg(long)]
        allow_seeding: bool,
        /// Verify only the smallest N staged torrents.
        #[arg(long)]
        max_torrents: Option<usize>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Commands::Doctor { list } => doctor(list),
        Commands::Audit {
            bt_backup,
            data_root,
        } => audit(bt_backup, data_root),
        Commands::Plan {
            to,
            bt_backup,
            data_root,
            budget_gib,
        } => plan(&to, bt_backup, data_root, budget_gib),
        Commands::Rewrite {
            to,
            staging,
            bt_backup,
            data_root,
            budget_gib,
        } => rewrite(&to, &staging, bt_backup, data_root, budget_gib),
        Commands::Transfer {
            to,
            rsync_to,
            bt_backup,
            data_root,
            budget_gib,
            max_mib,
            max_torrents,
            dry_run,
        } => transfer(
            &to,
            rsync_to,
            bt_backup,
            data_root,
            budget_gib,
            max_mib,
            max_torrents,
            dry_run,
        ),
        Commands::Import {
            url,
            staging,
            username,
            password_env,
            source_url,
            source_username,
            source_password_env,
            allow_unverified_source,
            dry_run,
            max_torrents,
        } => import(
            WebLogin {
                url: &url,
                username: &username,
                password_env: &password_env,
            },
            &staging,
            source_url.as_deref().map(|source_url| WebLogin {
                url: source_url,
                username: source_username.as_deref().unwrap_or(&username),
                password_env: source_password_env.as_deref().unwrap_or(&password_env),
            }),
            allow_unverified_source,
            dry_run,
            max_torrents,
        ),
        Commands::Verify {
            url,
            staging,
            username,
            password_env,
            allow_seeding,
            max_torrents,
        } => verify(
            WebLogin {
                url: &url,
                username: &username,
                password_env: &password_env,
            },
            &staging,
            allow_seeding,
            max_torrents,
        ),
    }
}

fn doctor(list: bool) -> ExitCode {
    if list {
        for check in checks::LOCAL {
            println!("{:<15} {}", check.id, check.description);
        }
        return ExitCode::SUCCESS;
    }

    let report = Report::run("local", checks::LOCAL, &Host);
    print!("{}", report.render());

    if report.is_blocking() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn audit(bt_backup: Option<PathBuf>, data_root: Option<PathBuf>) -> ExitCode {
    match run_audit(bt_backup, data_root) {
        Ok(manifest) => {
            print!("{}", manifest.render());
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

fn plan(
    to: &Path,
    bt_backup: Option<PathBuf>,
    data_root: Option<PathBuf>,
    budget_gib: u64,
) -> ExitCode {
    let dest = to.to_string_lossy();
    if dest.is_empty() || dest == "/" {
        eprintln!("plan: --to must be a real destination root, not /");
        return ExitCode::FAILURE;
    }

    let manifest = match run_audit(bt_backup, data_root) {
        Ok(manifest) => manifest,
        Err(code) => return code,
    };

    let budget = budget_gib.saturating_mul(1 << 30);
    let budget = if budget == 0 { DEFAULT_BUDGET } else { budget };

    match tsync_plan::build(&manifest, dest.as_ref(), budget) {
        Ok(plan) => {
            print!("{}", plan.render());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("plan: {error}");
            ExitCode::FAILURE
        }
    }
}

fn rewrite(
    to: &Path,
    staging: &Path,
    bt_backup: Option<PathBuf>,
    data_root: Option<PathBuf>,
    budget_gib: u64,
) -> ExitCode {
    let dest = to.to_string_lossy();
    if dest.is_empty() || dest == "/" {
        eprintln!("rewrite: --to must be a real destination root, not /");
        return ExitCode::FAILURE;
    }

    let Some(bt_backup) = bt_backup.or_else(default_bt_backup) else {
        eprintln!("cannot locate BT_backup (home directory unknown)");
        eprintln!("pass --bt-backup PATH");
        return ExitCode::FAILURE;
    };

    let budget = budget_gib.saturating_mul(1 << 30);
    let budget = if budget == 0 { DEFAULT_BUDGET } else { budget };

    match tsync_rewrite::run(&tsync_rewrite::Options {
        bt_backup,
        staging: staging.to_path_buf(),
        dest: dest.into_owned(),
        budget,
        data_root,
    }) {
        Ok(report) => {
            print!("{}", report.render());
            if report.is_complete() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("rewrite: {error}");
            ExitCode::FAILURE
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn transfer(
    to: &Path,
    rsync_to: Option<String>,
    bt_backup: Option<PathBuf>,
    data_root: Option<PathBuf>,
    budget_gib: u64,
    max_mib: Option<u64>,
    max_torrents: Option<usize>,
    dry_run: bool,
) -> ExitCode {
    let dest = to.to_string_lossy();
    if dest.is_empty() || dest == "/" {
        eprintln!("transfer: --to must be a real destination root, not /");
        return ExitCode::FAILURE;
    }

    let Some(bt_backup) = bt_backup.or_else(default_bt_backup) else {
        eprintln!("cannot locate BT_backup (home directory unknown)");
        eprintln!("pass --bt-backup PATH");
        return ExitCode::FAILURE;
    };

    let budget = budget_gib.saturating_mul(1 << 30);
    let budget = if budget == 0 { DEFAULT_BUDGET } else { budget };
    let max_bytes = max_mib.map(|mib| mib.saturating_mul(1024 * 1024));

    match tsync_transfer::run(&tsync_transfer::Options {
        bt_backup,
        dest: dest.into_owned(),
        rsync_to,
        budget,
        data_root,
        max_bytes,
        max_torrents,
        dry_run,
    }) {
        Ok(report) => {
            print!("{}", report.render());
            if report.is_complete() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("transfer: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Clone, Copy)]
struct WebLogin<'a> {
    url: &'a str,
    username: &'a str,
    password_env: &'a str,
}

fn password_from_env(name: &str) -> Result<String, ExitCode> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => {
            eprintln!("import: set {name} to the WebUI password");
            Err(ExitCode::FAILURE)
        }
    }
}

fn session(label: &str, login: WebLogin<'_>) -> Result<tsync_qbt::Session, ExitCode> {
    let password = password_from_env(login.password_env)?;
    tsync_qbt::Session::login(login.url, login.username, &password).map_err(|error| {
        if label.is_empty() {
            eprintln!("import: {error}");
        } else {
            eprintln!("import: {label}: {error}");
        }
        ExitCode::FAILURE
    })
}

fn verify(
    dest: WebLogin<'_>,
    staging: &Path,
    allow_seeding: bool,
    max_torrents: Option<usize>,
) -> ExitCode {
    let dest = match session("", dest) {
        Ok(session) => session,
        Err(code) => return code,
    };

    match tsync_verify::run(&tsync_verify::Options {
        staging: staging.to_path_buf(),
        dest: &dest,
        allow_seeding,
        max_torrents,
    }) {
        Ok(report) => {
            print!("{}", report.render());
            if report.is_complete() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("verify: {error}");
            ExitCode::FAILURE
        }
    }
}

fn import(
    dest: WebLogin<'_>,
    staging: &Path,
    source: Option<WebLogin<'_>>,
    allow_unverified_source: bool,
    dry_run: bool,
    max_torrents: Option<usize>,
) -> ExitCode {
    let dest = match session("", dest) {
        Ok(session) => session,
        Err(code) => return code,
    };

    let source = match source {
        Some(login) => match session("source", login) {
            Ok(session) => Some(session),
            Err(code) => return code,
        },
        None => None,
    };

    match tsync_import::run(&tsync_import::Options {
        staging: staging.to_path_buf(),
        dest: &dest,
        source: source
            .as_ref()
            .map(|session| session as &dyn tsync_qbt::Client),
        allow_unverified_source,
        dry_run,
        max_torrents,
    }) {
        Ok(report) => {
            print!("{}", report.render());
            if report.is_complete() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("import: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_audit(
    bt_backup: Option<PathBuf>,
    data_root: Option<PathBuf>,
) -> Result<tsync_audit::Manifest, ExitCode> {
    let Some(bt_backup) = bt_backup.or_else(default_bt_backup) else {
        eprintln!("cannot locate BT_backup (home directory unknown)");
        eprintln!("pass --bt-backup PATH");
        return Err(ExitCode::FAILURE);
    };

    tsync_audit::run(&Options {
        bt_backup,
        data_root,
    })
    .map_err(|error| {
        eprintln!("audit: {error}");
        if error.is_permission_denied() {
            eprintln!(
                "       on macOS this is usually Full Disk Access: \
                 System Settings > Privacy & Security, then restart the terminal"
            );
        }
        ExitCode::FAILURE
    })
}

fn default_bt_backup() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        Some(home.join("Library/Application Support/qBittorrent/BT_backup"))
    } else {
        Some(home.join(".local/share/qBittorrent/BT_backup"))
    }
}
