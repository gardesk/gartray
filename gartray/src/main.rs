//! gartray - System tray and quick settings panel
//!
//! A modern system tray supporting both XEMBED and StatusNotifierItem protocols,
//! with an integrated quick settings panel for volume, brightness, network,
//! bluetooth, battery, and power controls.

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

mod config;
mod daemon;
mod ipc;
mod panel;
mod tray;
mod ui;

/// gartray - System tray and quick settings panel for gar desktop
#[derive(Parser)]
#[command(name = "gartray")]
#[command(about = "System tray and quick settings panel", long_about = None)]
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
    /// Start the tray daemon
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
        Some(Commands::Daemon { foreground }) | None => {
            info!("Starting gartray daemon");
            daemon::run(cli.config, foreground || cli.foreground).await
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
