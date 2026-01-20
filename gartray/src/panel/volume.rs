//! Volume control module using wpctl (WirePlumber/PipeWire)
//!
//! Provides volume slider and mute toggle for audio output.
//! Uses wpctl commands which is available in the system PATH.

use anyhow::{Context, Result};
use std::process::Command;
use tracing::{debug, info, warn};

/// Volume state
#[derive(Debug, Clone, Default)]
pub struct VolumeState {
    /// Volume level (0.0 - 1.0)
    pub volume: f64,
    /// Whether muted
    pub muted: bool,
}

/// Volume module for controlling audio via wpctl
pub struct VolumeModule {
    state: VolumeState,
    available: bool,
}

impl VolumeModule {
    /// Create a new volume module
    pub fn new() -> Self {
        Self {
            state: VolumeState::default(),
            available: false,
        }
    }

    /// Connect and check if wpctl is available
    pub fn connect(&mut self) -> Result<()> {
        // Check if wpctl is available by getting current volume
        let output = Command::new("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .output()
            .context("wpctl not found")?;

        if !output.status.success() {
            anyhow::bail!("wpctl not working or no audio sink");
        }

        self.available = true;
        info!("Connected to PipeWire via wpctl");

        // Parse initial state from the output we already have
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(vol_str) = stdout.strip_prefix("Volume: ") {
            let parts: Vec<&str> = vol_str.trim().split_whitespace().collect();
            if let Some(vol) = parts.first() {
                if let Ok(v) = vol.parse::<f64>() {
                    self.state.volume = v;
                }
            }
            self.state.muted = stdout.contains("[MUTED]");
        }

        debug!("Initial volume: {:.0}%, muted: {}", self.state.volume * 100.0, self.state.muted);
        Ok(())
    }

    /// Refresh volume state from wpctl
    pub fn refresh(&mut self) -> Result<()> {
        if !self.available {
            return Ok(());
        }

        // Get volume: wpctl get-volume @DEFAULT_AUDIO_SINK@
        // Output: "Volume: 0.50" or "Volume: 0.50 [MUTED]"
        let output = Command::new("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .output()
            .context("Failed to get volume")?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse "Volume: 0.50" or "Volume: 0.50 [MUTED]"
            if let Some(vol_str) = stdout.strip_prefix("Volume: ") {
                let parts: Vec<&str> = vol_str.trim().split_whitespace().collect();
                if let Some(vol) = parts.first() {
                    if let Ok(v) = vol.parse::<f64>() {
                        self.state.volume = v;
                    }
                }
                self.state.muted = stdout.contains("[MUTED]");
            }
        }

        debug!("Volume: {:.0}%, muted: {}", self.state.volume * 100.0, self.state.muted);
        Ok(())
    }

    /// Set volume (0.0 - 1.0)
    pub fn set_volume(&mut self, volume: f64) -> Result<()> {
        if !self.available {
            return Ok(());
        }

        // wpctl set-volume @DEFAULT_AUDIO_SINK@ 0.50
        let vol_str = format!("{:.2}", volume.clamp(0.0, 1.5));

        let status = Command::new("wpctl")
            .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &vol_str])
            .status()
            .context("Failed to set volume")?;

        if status.success() {
            self.state.volume = volume;
            debug!("Set volume to {:.0}%", volume * 100.0);
        } else {
            warn!("wpctl set-volume failed");
        }

        Ok(())
    }

    /// Toggle mute
    pub fn toggle_mute(&mut self) -> Result<()> {
        if !self.available {
            return Ok(());
        }

        let status = Command::new("wpctl")
            .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
            .status()
            .context("Failed to toggle mute")?;

        if status.success() {
            self.state.muted = !self.state.muted;
            debug!("Mute toggled to {}", self.state.muted);
        } else {
            warn!("wpctl set-mute failed");
        }

        Ok(())
    }

    /// Set mute state
    pub fn set_mute(&mut self, muted: bool) -> Result<()> {
        if !self.available {
            return Ok(());
        }

        let mute_str = if muted { "1" } else { "0" };

        let status = Command::new("wpctl")
            .args(["set-mute", "@DEFAULT_AUDIO_SINK@", mute_str])
            .status()
            .context("Failed to set mute")?;

        if status.success() {
            self.state.muted = muted;
        }

        Ok(())
    }

    /// Get current state
    pub fn state(&self) -> VolumeState {
        self.state.clone()
    }

    /// Check if available
    pub fn is_available(&self) -> bool {
        self.available
    }
}

impl Default for VolumeModule {
    fn default() -> Self {
        Self::new()
    }
}
