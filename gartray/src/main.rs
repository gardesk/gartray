//! gartray - Quick settings panel for gar desktop
//!
//! A quick settings panel for volume, brightness, network,
//! bluetooth, battery, and power controls.
//!
//! Note: System tray functionality has been moved to garbar.

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

mod config;
mod daemon;
mod ipc;
mod panel;
mod ui;

/// gartray - Quick settings panel for gar desktop
#[derive(Parser)]
#[command(name = "gartray")]
#[command(about = "Quick settings panel for gar desktop", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Run in foreground (don't daemonize)
    #[arg(short, long)]
    foreground: bool,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the panel daemon
    Daemon {
        /// Run in foreground
        #[arg(short, long)]
        foreground: bool,
    },
    /// Show the quick settings panel
    Panel,
    /// Toggle quick settings panel visibility
    Toggle,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Daemon { foreground }) => {
            info!("Starting gartray daemon");
            daemon::run(cli.config, foreground || cli.foreground).await
        }
        None => {
            info!("Starting gartray daemon");
            daemon::run(cli.config, cli.foreground).await
        }
        Some(Commands::Panel) => {
            info!("Showing quick settings panel");
            ipc::send_command("panel").await
        }
        Some(Commands::Toggle) => {
            info!("Toggling quick settings panel");
            ipc::send_command("toggle").await
        }
    }
}
