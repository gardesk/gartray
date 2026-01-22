//! Brightness control module
//!
//! Controls screen brightness via /sys/class/backlight/

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Brightness state
#[derive(Debug, Clone, Default)]
pub struct BrightnessState {
    /// Current brightness (0.0 - 1.0)
    pub brightness: f64,
    /// Device name
    pub device: String,
    /// Maximum brightness value
    pub max_brightness: u32,
}

/// Brightness control module
pub struct BrightnessModule {
    /// Path to the backlight device
    device_path: Option<PathBuf>,
    /// Maximum brightness
    max_brightness: u32,
    /// Current state
    state: BrightnessState,
}

impl BrightnessModule {
    /// Create a new brightness module
    pub fn new() -> Self {
        Self {
            device_path: None,
            max_brightness: 100,
            state: BrightnessState::default(),
        }
    }

    /// Initialize and find the backlight device
    pub fn init(&mut self) -> Result<()> {
        let backlight_dir = PathBuf::from("/sys/class/backlight");

        if !backlight_dir.exists() {
            return Err(anyhow::anyhow!("No backlight directory found"));
        }

        // Find first available backlight device
        let entries = fs::read_dir(&backlight_dir)
            .context("Failed to read backlight directory")?;

        for entry in entries.flatten() {
            let path = entry.path();
            let brightness_path = path.join("brightness");
            let max_path = path.join("max_brightness");

            if brightness_path.exists() && max_path.exists() {
                // Read max brightness
                let max_str = fs::read_to_string(&max_path)
                    .context("Failed to read max_brightness")?;
                let max: u32 = max_str.trim().parse()
                    .context("Failed to parse max_brightness")?;

                let device_name = entry.file_name().to_string_lossy().to_string();
                info!("Found backlight device: {} (max={})", device_name, max);

                self.device_path = Some(path);
                self.max_brightness = max;
                self.state.device = device_name;
                self.state.max_brightness = max;

                // Read current brightness
                self.refresh()?;

                return Ok(());
            }
        }

        Err(anyhow::anyhow!("No backlight device found"))
    }

    /// Refresh brightness from device
    pub fn refresh(&mut self) -> Result<()> {
        let device_path = self.device_path.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No device initialized"))?;

        let brightness_path = device_path.join("brightness");
        let brightness_str = fs::read_to_string(&brightness_path)
            .context("Failed to read brightness")?;
        let brightness: u32 = brightness_str.trim().parse()
            .context("Failed to parse brightness")?;

        self.state.brightness = brightness as f64 / self.max_brightness as f64;
        debug!("Current brightness: {:.0}%", self.state.brightness * 100.0);

        Ok(())
    }

    /// Set brightness (0.0 - 1.0)
    pub fn set_brightness(&mut self, brightness: f64) -> Result<()> {
        let device_path = self.device_path.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No device initialized"))?;

        let brightness = brightness.clamp(0.01, 1.0); // Don't allow complete darkness
        let value = (brightness * self.max_brightness as f64) as u32;

        let brightness_path = device_path.join("brightness");

        // Try to write directly (requires permissions)
        match fs::write(&brightness_path, value.to_string()) {
            Ok(_) => {
                self.state.brightness = brightness;
                debug!("Set brightness to {:.0}%", brightness * 100.0);
                Ok(())
            }
            Err(e) => {
                // Try using brightnessctl if available
                warn!("Direct write failed: {}, trying brightnessctl", e);
                self.set_brightness_via_brightnessctl(value)
            }
        }
    }

    /// Set brightness using brightnessctl command
    fn set_brightness_via_brightnessctl(&mut self, value: u32) -> Result<()> {
        use std::process::Command;

        let output = Command::new("brightnessctl")
            .args(["set", &value.to_string()])
            .output()
            .context("Failed to run brightnessctl")?;

        if output.status.success() {
            self.state.brightness = value as f64 / self.max_brightness as f64;
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "brightnessctl failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }

    /// Increase brightness by a step
    pub fn increase(&mut self, step: f64) -> Result<()> {
        let new = (self.state.brightness + step).min(1.0);
        self.set_brightness(new)
    }

    /// Decrease brightness by a step
    pub fn decrease(&mut self, step: f64) -> Result<()> {
        let new = (self.state.brightness - step).max(0.01);
        self.set_brightness(new)
    }

    /// Get current state
    pub fn state(&self) -> &BrightnessState {
        &self.state
    }

    /// Check if initialized
    pub fn is_available(&self) -> bool {
        self.device_path.is_some()
    }

    /// Get max brightness value
    pub fn max_brightness(&self) -> u32 {
        self.max_brightness
    }

    /// Get device path
    pub fn device_path(&self) -> Option<&PathBuf> {
        self.device_path.as_ref()
    }
}
