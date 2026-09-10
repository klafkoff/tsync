//! Move a torrent library between machines without re-downloading it.

use clap::{Parser, Subcommand};
use std::process::ExitCode;

/// tsync CLI
#[derive(Parser)]
#[command(name = "tsync", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check local environment and prerequisites.
    Doctor,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Commands::Doctor => {
            eprintln!("doctor: not yet implemented");
            ExitCode::from(1)
        }
    }
}
