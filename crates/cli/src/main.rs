//! Move a torrent library between machines without re-downloading it.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Commands::Doctor { list } => doctor(list),
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

    // Only failures gate, so warnings never break automation.
    if report.is_blocking() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
