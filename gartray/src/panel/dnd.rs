//! Do Not Disturb module
//!
//! Tracks DND state and optionally integrates with notification daemons.
//! Supports dunst via dunstctl if available.

use anyhow::Result;
use tracing::{info, debug};
use std::process::Command;

/// DND state
#[derive(Debug, Clone, Default)]
pub struct DndState {
    pub enabled: bool,
    pub dunst_available: bool,
}

/// Do Not Disturb module
pub struct DndModule {
    state: DndState,
}

impl DndModule {
    /// Create a new DND module
    pub fn new() -> Self {
        Self {
            state: DndState::default(),
        }
    }

    /// Initialize and check for notification daemon support
    pub fn init(&mut self) -> Result<()> {
        // Check if dunstctl is available
        self.state.dunst_available = self.check_dunst_available();

        if self.state.dunst_available {
            // Query current dunst state
            self.update_from_dunst();
            info!("DND module initialized with dunst support, enabled: {}", self.state.enabled);
        } else {
            info!("DND module initialized (local state only, no dunst)");
        }

        Ok(())
    }

    /// Check if dunstctl is available
    fn check_dunst_available(&self) -> bool {
        Command::new("dunstctl")
            .arg("is-paused")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Update state from dunst
    fn update_from_dunst(&mut self) {
        if let Ok(output) = Command::new("dunstctl").arg("is-paused").output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                self.state.enabled = stdout.trim() == "true";
            }
        }
    }

    /// Toggle DND on/off
    pub fn toggle(&mut self) -> Result<()> {
        let new_state = !self.state.enabled;
        info!("Toggling DND: {} -> {}", self.state.enabled, new_state);

        if self.state.dunst_available {
            // Use dunstctl to toggle
            let cmd = if new_state { "set-paused" } else { "set-paused" };
            let arg = if new_state { "true" } else { "false" };

            match Command::new("dunstctl").args([cmd, arg]).status() {
                Ok(status) if status.success() => {
                    self.state.enabled = new_state;
                    debug!("dunstctl {} {} succeeded", cmd, arg);
                }
                Ok(status) => {
                    debug!("dunstctl failed with status: {}", status);
                    // Fall back to local state
                    self.state.enabled = new_state;
                }
                Err(e) => {
                    debug!("dunstctl error: {}", e);
                    self.state.enabled = new_state;
                }
            }
        } else {
            // Just track local state
            self.state.enabled = new_state;
        }

        Ok(())
    }

    /// Enable DND
    pub fn enable(&mut self) -> Result<()> {
        if !self.state.enabled {
            self.state.enabled = true;
            if self.state.dunst_available {
                let _ = Command::new("dunstctl").args(["set-paused", "true"]).status();
            }
            info!("DND enabled");
        }
        Ok(())
    }

    /// Disable DND
    pub fn disable(&mut self) -> Result<()> {
        if self.state.enabled {
            self.state.enabled = false;
            if self.state.dunst_available {
                let _ = Command::new("dunstctl").args(["set-paused", "false"]).status();
            }
            info!("DND disabled");
        }
        Ok(())
    }

    /// Get current state
    pub fn state(&self) -> &DndState {
        &self.state
    }

    /// Check if DND is enabled
    pub fn is_enabled(&self) -> bool {
        self.state.enabled
    }
}

impl Default for DndModule {
    fn default() -> Self {
        Self::new()
    }
}
