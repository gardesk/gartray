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
    Show {
        /// X position for panel
        #[arg(default_value = "0")]
        x: i32,
        /// Y position for panel
        #[arg(default_value = "0")]
        y: i32,
    },
    /// Hide the quick settings panel
    Hide,
    /// Toggle quick settings panel visibility
    Toggle {
        /// X position for panel
        #[arg(default_value = "0")]
        x: i32,
        /// Y position for panel
        #[arg(default_value = "0")]
        y: i32,
    },
    /// Reload configuration
    Reload,
    /// Get daemon status
    Status,
    /// Stop the daemon
    Quit,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let cmd = match cli.command {
        Commands::Show { x, y } => ipc::Command::Show { x, y },
        Commands::Hide => ipc::Command::Hide,
        Commands::Toggle { x, y } => ipc::Command::Toggle { x, y },
        Commands::Reload => ipc::Command::Reload,
        Commands::Status => ipc::Command::Status,
        Commands::Quit => ipc::Command::Quit,
    };

    ipc::send_command(cmd)
}
