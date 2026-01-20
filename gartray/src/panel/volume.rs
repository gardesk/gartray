//! Volume control module using pactl
//!
//! Provides volume slider and mute toggle for audio output.
//! Uses pactl commands - works with both PulseAudio and PipeWire.

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
    /// Sink name
    pub sink_name: String,
}

/// Volume module for controlling audio via pactl
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

    /// Connect and check if pactl is available
    pub fn connect(&mut self) -> Result<()> {
        // Check if pactl is available
        let output = Command::new("pactl")
            .arg("--version")
            .output()
            .context("pactl not found")?;

        if !output.status.success() {
            anyhow::bail!("pactl not working");
        }

        self.available = true;
        info!("Connected to PulseAudio");

        // Get initial state
        self.refresh()?;

        Ok(())
    }

    /// Refresh volume state from pactl
    pub fn refresh(&mut self) -> Result<()> {
        if !self.available {
            return Ok(());
        }

        // Get default sink
        if let Ok(sink) = self.get_default_sink() {
            self.state.sink_name = sink;
        }

        // Get volume and mute state
        if let Ok((volume, muted)) = self.get_sink_volume(&self.state.sink_name) {
            self.state.volume = volume;
            self.state.muted = muted;
        }

        debug!("Volume: {:.0}%, muted: {}", self.state.volume * 100.0, self.state.muted);
        Ok(())
    }

    /// Get the default sink name
    fn get_default_sink(&self) -> Result<String> {
        let output = Command::new("pactl")
            .args(["get-default-sink"])
            .output()
            .context("Failed to get default sink")?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            anyhow::bail!("pactl get-default-sink failed")
        }
    }

    /// Get volume and mute state for a sink
    fn get_sink_volume(&self, sink: &str) -> Result<(f64, bool)> {
        // Get volume
        let output = Command::new("pactl")
            .args(["get-sink-volume", sink])
            .output()
            .context("Failed to get sink volume")?;

        let mut volume = 0.5;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse output like "Volume: front-left: 65536 / 100% / 0.00 dB, ..."
            if let Some(pct) = stdout.split('/').nth(1) {
                if let Some(num) = pct.trim().strip_suffix('%') {
                    if let Ok(v) = num.trim().parse::<f64>() {
                        volume = v / 100.0;
                    }
                }
            }
        }

        // Get mute state
        let output = Command::new("pactl")
            .args(["get-sink-mute", sink])
            .output()
            .context("Failed to get sink mute")?;

        let mut muted = false;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            muted = stdout.contains("yes");
        }

        Ok((volume, muted))
    }

    /// Set volume (0.0 - 1.0)
    pub fn set_volume(&mut self, volume: f64) -> Result<()> {
        if !self.available {
            return Ok(());
        }

        let sink = &self.state.sink_name;
        if sink.is_empty() {
            return Ok(());
        }

        // Convert to percentage
        let pct = (volume.clamp(0.0, 1.5) * 100.0) as u32;
        let volume_str = format!("{}%", pct);

        let status = Command::new("pactl")
            .args(["set-sink-volume", sink, &volume_str])
            .status()
            .context("Failed to set volume")?;

        if status.success() {
            self.state.volume = volume;
            debug!("Set volume to {:.0}%", volume * 100.0);
        } else {
            warn!("pactl set-sink-volume failed");
        }

        Ok(())
    }

    /// Toggle mute
    pub fn toggle_mute(&mut self) -> Result<()> {
        if !self.available {
            return Ok(());
        }

        let sink = &self.state.sink_name;
        if sink.is_empty() {
            return Ok(());
        }

        let status = Command::new("pactl")
            .args(["set-sink-mute", sink, "toggle"])
            .status()
            .context("Failed to toggle mute")?;

        if status.success() {
            self.state.muted = !self.state.muted;
            debug!("Mute toggled to {}", self.state.muted);
        } else {
            warn!("pactl set-sink-mute failed");
        }

        Ok(())
    }

    /// Set mute state
    pub fn set_mute(&mut self, muted: bool) -> Result<()> {
        if !self.available {
            return Ok(());
        }

        let sink = &self.state.sink_name;
        if sink.is_empty() {
            return Ok(());
        }

        let mute_str = if muted { "1" } else { "0" };

        let status = Command::new("pactl")
            .args(["set-sink-mute", sink, mute_str])
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
