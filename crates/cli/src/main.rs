//! Move a torrent library between machines without re-downloading it.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

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
        /// Destination save-path root the dest client uses for new torrents too
        /// (same as `plan` / `rewrite`). After copy, that tree must be writable
        /// by the dest client user — rsync keeps the source OS uid.
        #[arg(long)]
        to: PathBuf,
        /// Where the bytes go. Local path, or `host:/abs/path` over SSH.
        /// Defaults to `--to`. Use this when the client save path is not
        /// the path on the other machine (`/data` vs `/opt/seedbox/data`).
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
        /// Force-recheck incomplete stopped torrents. Does not stop afterwards
        /// (a stop cancels the hash on qBittorrent 5.x). Skip if dest is
        /// already hashing.
        #[arg(long)]
        recheck: bool,
        /// Poll dest and redraw a progress bar until hashing finishes.
        #[arg(long)]
        wait: bool,
    },
    /// Stop the source, then start the destination. Never the reverse.
    Handoff {
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
        /// Source `WebUI`. When set, tsync stops those hashes before dest starts.
        #[arg(long)]
        source_url: Option<String>,
        /// Source username. Defaults to `--username`.
        #[arg(long)]
        source_username: Option<String>,
        /// Env var for the source password. Defaults to `--password-env`.
        #[arg(long)]
        source_password_env: Option<String>,
        /// Start dest after you have stopped the source yourself.
        /// Prefer `--source-url` so tsync can confirm the source is silent.
        #[arg(long)]
        confirm: bool,
        /// Classify only; do not stop or start anything.
        #[arg(long)]
        dry_run: bool,
        /// Handoff only the smallest N staged torrents.
        #[arg(long)]
        max_torrents: Option<usize>,
    },
    /// Pull torrent data back. Never adds it to a local client.
    Fetch {
        /// Where bytes live now. Local path or `host:/abs/path`.
        #[arg(long)]
        from: String,
        /// Local directory that receives the files.
        #[arg(long)]
        to: PathBuf,
        /// Client-visible save root on the remote (`/data`).
        #[arg(long)]
        save_root: PathBuf,
        /// Remote `WebUI`, used only to skip incomplete torrents.
        #[arg(long)]
        url: String,
        /// Staging directory from `tsync rewrite`.
        #[arg(long)]
        staging: PathBuf,
        /// Remote `WebUI` username.
        #[arg(long)]
        username: String,
        /// Env var that holds the remote password.
        #[arg(long, default_value = "QBT_PASSWORD")]
        password_env: String,
        /// `skip` (default), `complete-files`, or `all`.
        #[arg(long, default_value = "skip")]
        partial: String,
        /// Classify only; do not copy.
        #[arg(long)]
        dry_run: bool,
        /// Fetch only the smallest N staged torrents.
        #[arg(long)]
        max_torrents: Option<usize>,
    },
    /// Rewrite, transfer, import, verify; handoff only with `--handoff`.
    Migrate {
        /// Destination save-path root (same as `plan` / `rewrite`).
        #[arg(long)]
        to: PathBuf,
        /// Where the bytes go. Local path or `host:/abs/path`.
        #[arg(long)]
        rsync_to: Option<String>,
        /// Staging directory for rewritten resumes.
        #[arg(long)]
        staging: PathBuf,
        /// Destination `WebUI`.
        #[arg(long)]
        url: String,
        /// Destination username.
        #[arg(long)]
        username: String,
        /// Env var for the destination password.
        #[arg(long, default_value = "QBT_PASSWORD")]
        password_env: String,
        /// Source `WebUI` (import guard and handoff).
        #[arg(long)]
        source_url: Option<String>,
        /// Source username. Defaults to `--username`.
        #[arg(long)]
        source_username: Option<String>,
        /// Env var for the source password. Defaults to `--password-env`.
        #[arg(long)]
        source_password_env: Option<String>,
        /// qBittorrent `BT_backup`. Blank = per-OS default.
        #[arg(long)]
        bt_backup: Option<PathBuf>,
        /// Resolve save paths relative to this directory.
        #[arg(long)]
        data_root: Option<PathBuf>,
        /// Soft batch budget in GiB.
        #[arg(long, default_value_t = 4)]
        budget_gib: u64,
        /// After verify, stop the source and start dest.
        #[arg(long)]
        handoff: bool,
        /// Handoff without a source API (you already stopped the source).
        #[arg(long)]
        confirm: bool,
        /// Resume from this step: rewrite, transfer, import, verify, handoff.
        #[arg(long, default_value = "rewrite")]
        from_step: String,
        /// Dry-run transfer, import, and handoff.
        #[arg(long)]
        dry_run: bool,
        /// Cap every step to the smallest N torrents.
        #[arg(long)]
        max_torrents: Option<usize>,
        /// Import without a source `WebUI`.
        #[arg(long)]
        allow_unverified_source: bool,
    },
}

fn main() -> ExitCode {
    dispatch(Cli::parse())
}

#[allow(clippy::too_many_lines)]
fn dispatch(cli: Cli) -> ExitCode {
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
            recheck,
            wait,
        } => verify(
            WebLogin {
                url: &url,
                username: &username,
                password_env: &password_env,
            },
            &staging,
            allow_seeding,
            max_torrents,
            recheck,
            wait,
        ),
        Commands::Handoff {
            url,
            staging,
            username,
            password_env,
            source_url,
            source_username,
            source_password_env,
            confirm,
            dry_run,
            max_torrents,
        } => handoff(
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
            confirm,
            dry_run,
            max_torrents,
        ),
        Commands::Fetch {
            from,
            to,
            save_root,
            url,
            staging,
            username,
            password_env,
            partial,
            dry_run,
            max_torrents,
        } => fetch(
            WebLogin {
                url: &url,
                username: &username,
                password_env: &password_env,
            },
            &from,
            &to,
            &save_root,
            &staging,
            &partial,
            dry_run,
            max_torrents,
        ),
        Commands::Migrate {
            to,
            rsync_to,
            staging,
            url,
            username,
            password_env,
            source_url,
            source_username,
            source_password_env,
            bt_backup,
            data_root,
            budget_gib,
            handoff,
            confirm,
            from_step,
            dry_run,
            max_torrents,
            allow_unverified_source,
        } => migrate(
            &to,
            rsync_to,
            &staging,
            WebLogin {
                url: &url,
                username: &username,
                password_env: &password_env,
            },
            source_url.as_deref().map(|source_url| WebLogin {
                url: source_url,
                username: source_username.as_deref().unwrap_or(&username),
                password_env: source_password_env.as_deref().unwrap_or(&password_env),
            }),
            bt_backup,
            data_root,
            budget_gib,
            handoff,
            confirm,
            &from_step,
            dry_run,
            max_torrents,
            allow_unverified_source,
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
            eprintln!("tsync: set {name} to the WebUI password");
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
    recheck: bool,
    wait: bool,
) -> ExitCode {
    let dest = match session("", dest) {
        Ok(session) => session,
        Err(code) => return code,
    };

    if !wait {
        return match tsync_verify::run(&tsync_verify::Options {
            staging: staging.to_path_buf(),
            dest: &dest,
            allow_seeding,
            max_torrents,
            recheck,
        }) {
            Ok(report) => finish_verify(&report),
            Err(error) => {
                eprintln!("verify: {error}");
                ExitCode::FAILURE
            }
        };
    }

    let tty = io::stdout().is_terminal();
    let mut kick = recheck;
    let mut prev_lines = 0;
    let mut prev_hashed = 0;
    let mut prev_at = Instant::now();
    let mut seen = false;
    let mut rate = None;
    loop {
        match tsync_verify::watch(&tsync_verify::Options {
            staging: staging.to_path_buf(),
            dest: &dest,
            allow_seeding,
            max_torrents,
            recheck: kick,
        }) {
            Ok(snap) => {
                if seen {
                    let dt_ms = u64::try_from(prev_at.elapsed().as_millis())
                        .unwrap_or(1)
                        .max(1);
                    let delta = snap.hashed_bytes.saturating_sub(prev_hashed);
                    rate = Some(delta.saturating_mul(1000) / dt_ms);
                }
                seen = true;
                prev_hashed = snap.hashed_bytes;
                prev_at = Instant::now();
                kick = false;

                let hashing = snap.report.checking > 0 && !snap.report.is_complete();
                if hashing {
                    let frame = snap.render_rate(rate);
                    if tty {
                        reprint_frame(prev_lines, &frame);
                        prev_lines = frame.lines().count();
                    } else {
                        eprintln!(
                            "verify: ready {} checking {} hashed {} / {}",
                            snap.report.ready,
                            snap.report.checking,
                            snap.hashed_bytes,
                            snap.total_bytes
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    continue;
                }

                if tty && prev_lines > 0 {
                    reprint_frame(prev_lines, &snap.render_rate(rate));
                    println!();
                }
                return finish_verify(&snap.report);
            }
            Err(error) => {
                eprintln!("verify: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
}

fn finish_verify(report: &tsync_verify::Report) -> ExitCode {
    print!("{}", report.render());
    if report.is_complete() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn reprint_frame(prev_lines: usize, frame: &str) {
    if prev_lines > 0 {
        print!("\x1b[{prev_lines}A\x1b[J");
    }
    print!("{frame}");
    let _ = io::stdout().flush();
}

#[allow(clippy::too_many_arguments)]
fn fetch(
    remote: WebLogin<'_>,
    from: &str,
    to: &Path,
    save_root: &Path,
    staging: &Path,
    partial: &str,
    dry_run: bool,
    max_torrents: Option<usize>,
) -> ExitCode {
    let dest = to.to_string_lossy();
    if dest.is_empty() || dest == "/" {
        eprintln!("fetch: --to must be a real directory, not /");
        return ExitCode::FAILURE;
    }
    let partial = match partial.parse() {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("fetch: {error}");
            return ExitCode::FAILURE;
        }
    };
    let remote = match session("fetch", remote) {
        Ok(session) => session,
        Err(code) => return code,
    };

    match tsync_fetch::run(&tsync_fetch::Options {
        staging: staging.to_path_buf(),
        from: from.to_owned(),
        to: dest.into_owned(),
        save_root: save_root.to_string_lossy().into_owned(),
        remote: &remote,
        partial,
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
            eprintln!("fetch: {error}");
            ExitCode::FAILURE
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn migrate(
    to: &Path,
    rsync_to: Option<String>,
    staging: &Path,
    dest: WebLogin<'_>,
    source: Option<WebLogin<'_>>,
    bt_backup: Option<PathBuf>,
    data_root: Option<PathBuf>,
    budget_gib: u64,
    do_handoff: bool,
    confirm: bool,
    from_step: &str,
    dry_run: bool,
    max_torrents: Option<usize>,
    allow_unverified_source: bool,
) -> ExitCode {
    let dest_root = to.to_string_lossy();
    if dest_root.is_empty() || dest_root == "/" {
        eprintln!("migrate: --to must be a real destination root, not /");
        return ExitCode::FAILURE;
    }
    let from_step = match from_step.parse() {
        Ok(step) => step,
        Err(error) => {
            eprintln!("migrate: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(bt_backup) = bt_backup.or_else(default_bt_backup) else {
        eprintln!("cannot locate BT_backup (home directory unknown)");
        eprintln!("pass --bt-backup PATH");
        return ExitCode::FAILURE;
    };
    let dest = match session("migrate", dest) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let source = match source {
        Some(login) => match session("migrate source", login) {
            Ok(session) => Some(session),
            Err(code) => return code,
        },
        None => None,
    };
    let budget = budget_gib.saturating_mul(1 << 30);
    let budget = if budget == 0 { DEFAULT_BUDGET } else { budget };

    match tsync_migrate::run(&tsync_migrate::Options {
        bt_backup,
        staging: staging.to_path_buf(),
        dest: dest_root.into_owned(),
        rsync_to,
        data_root,
        budget,
        dest_client: &dest,
        source_client: source
            .as_ref()
            .map(|session| session as &dyn tsync_qbt::Client),
        allow_unverified_source,
        handoff: do_handoff,
        confirm,
        from_step,
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
            eprintln!("migrate: {error}");
            ExitCode::FAILURE
        }
    }
}

fn handoff(
    dest: WebLogin<'_>,
    staging: &Path,
    source: Option<WebLogin<'_>>,
    confirm: bool,
    dry_run: bool,
    max_torrents: Option<usize>,
) -> ExitCode {
    let dest = match session("handoff", dest) {
        Ok(session) => session,
        Err(code) => return code,
    };

    let source = match source {
        Some(login) => match session("handoff source", login) {
            Ok(session) => Some(session),
            Err(code) => return code,
        },
        None => None,
    };

    match tsync_handoff::run_with_progress(
        &tsync_handoff::Options {
            staging: staging.to_path_buf(),
            dest: &dest,
            source: source
                .as_ref()
                .map(|session| session as &dyn tsync_qbt::Client),
            confirm,
            dry_run,
            max_torrents,
        },
        |tick| {
            let verb = if tick.started { "started" } else { "skip" };
            live_line("handoff", tick.done, tick.total, &tick.id, verb);
        },
    ) {
        Ok(report) => {
            finish_live_line();
            print!("{}", report.render());
            if report.is_complete() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            finish_live_line();
            eprintln!("handoff: {error}");
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

    match tsync_import::run_with_progress(
        &tsync_import::Options {
            staging: staging.to_path_buf(),
            dest: &dest,
            source: source
                .as_ref()
                .map(|session| session as &dyn tsync_qbt::Client),
            allow_unverified_source,
            dry_run,
            max_torrents,
        },
        |tick| live_line("import", tick.done, tick.total, &tick.id, "dest"),
    ) {
        Ok(report) => {
            finish_live_line();
            print!("{}", report.render());
            if report.is_complete() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            finish_live_line();
            eprintln!("import: {error}");
            ExitCode::FAILURE
        }
    }
}

fn live_line(command: &str, done: usize, total: usize, id: &str, verb: &str) {
    let short = if id.len() > 12 {
        format!("{}…", &id[..12])
    } else {
        id.to_owned()
    };
    if io::stderr().is_terminal() {
        eprint!("\r{command}: {done:>3}/{total}  {verb}  {short}    ");
        let _ = io::stderr().flush();
    } else if done == 1 || done == total || done.is_multiple_of(10) {
        eprintln!("{command}: {done}/{total}  {verb}  {short}");
    }
}

fn finish_live_line() {
    if io::stderr().is_terminal() {
        eprintln!();
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
