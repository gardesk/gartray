//! IPC for gartrayctl communication

use anyhow::Result;

/// Send a command to the running daemon
pub async fn send_command(command: &str) -> Result<()> {
    // TODO: Implement Unix socket IPC
    tracing::info!("Would send IPC command: {}", command);
    Ok(())
}
