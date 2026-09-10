//! Move a torrent library between machines without re-downloading it.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tsync_audit::Options;
use tsync_doctor::{Host, Report, checks};

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
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Commands::Doctor { list } => doctor(list),
        Commands::Audit {
            bt_backup,
            data_root,
        } => audit(bt_backup, data_root),
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
    let Some(bt_backup) = bt_backup.or_else(default_bt_backup) else {
        eprintln!("audit: cannot locate BT_backup (home directory unknown)");
        eprintln!("       pass --bt-backup PATH");
        return ExitCode::FAILURE;
    };

    match tsync_audit::run(&Options {
        bt_backup,
        data_root,
    }) {
        Ok(manifest) => {
            print!("{}", manifest.render());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("audit: {error}");
            if error.is_permission_denied() {
                eprintln!(
                    "       on macOS this is usually Full Disk Access: \
                     System Settings > Privacy & Security, then restart the terminal"
                );
            }
            ExitCode::FAILURE
        }
    }
}

fn default_bt_backup() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        Some(home.join("Library/Application Support/qBittorrent/BT_backup"))
    } else {
        Some(home.join(".local/share/qBittorrent/BT_backup"))
    }
}
