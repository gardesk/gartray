//! Daemon state machine and main event loop

use anyhow::Result;
use tracing::info;

use crate::config;

/// Run the tray daemon
pub async fn run(config_path: Option<String>, _foreground: bool) -> Result<()> {
    let config = config::load(config_path.as_deref())?;
    info!("Loaded configuration: {:?}", config.tray.position);

    // TODO: Initialize X11 connection via gartk-x11
    // TODO: Create tray window
    // TODO: Initialize XEMBED manager
    // TODO: Initialize SNI watcher (D-Bus)
    // TODO: Start event loop

    info!("gartray daemon starting...");
    info!("Tray position: {}", config.tray.position);
    info!("Icon size: {}px", config.tray.icon_size);
    info!("Panel modules: {:?}", config.panel.modules);

    // Placeholder: wait forever
    tokio::signal::ctrl_c().await?;
    info!("Shutting down...");

    Ok(())
}
