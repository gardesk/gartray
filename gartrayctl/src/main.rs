//! gartrayctl - Control CLI for gartray daemon

use anyhow::Result;
use clap::{Parser, Subcommand};

mod ipc;

/// gartrayctl - Control the gartray daemon
#[derive(Parser)]
#[command(name = "gartrayctl")]
#[command(about = "Control the gartray daemon", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show the quick settings panel
    Show,
    /// Hide the quick settings panel
    Hide,
    /// Toggle quick settings panel visibility
    Toggle,
    /// Reload configuration
    Reload,
    /// Get daemon status
    Status,
    /// Stop the daemon
    Quit,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let command = match cli.command {
        Commands::Show => "show",
        Commands::Hide => "hide",
        Commands::Toggle => "toggle",
        Commands::Reload => "reload",
        Commands::Status => "status",
        Commands::Quit => "quit",
    };

    ipc::send_command(command)
}
