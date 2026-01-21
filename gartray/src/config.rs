//! Configuration loading and parsing

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub tray: TrayConfig,
    pub panel: PanelConfig,
    pub theme: ThemeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tray: TrayConfig::default(),
            panel: PanelConfig::default(),
            theme: ThemeConfig::default(),
        }
    }
}

/// Tray configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TrayConfig {
    /// Whether the XEmbed system tray is enabled (for legacy tray icons)
    pub enabled: bool,
    /// Tray position: top-left, top-right, bottom-left, bottom-right
    pub position: String,
    /// Icon size in pixels
    pub icon_size: u32,
    /// Spacing between icons
    pub spacing: f64,
    /// Background color
    pub background: String,
    /// Window opacity
    pub opacity: f64,
    /// X offset from edge (positive = inward)
    pub offset_x: i32,
    /// Y offset from edge (positive = inward)
    pub offset_y: i32,
    /// Monitor to show on: "primary", "all", or monitor name (e.g., "HDMI-0")
    pub monitor: String,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            position: "top-right".to_string(),
            icon_size: 22,
            spacing: 8.0,
            background: "#1a1a1a".to_string(),
            opacity: 0.95,
            offset_x: 8,
            offset_y: 8,
            monitor: "primary".to_string(),
        }
    }
}

/// Quick settings panel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelConfig {
    /// Whether panel is enabled
    pub enabled: bool,
    /// Panel width
    pub width: u32,
    /// Enabled modules
    pub modules: Vec<String>,
    /// Volume module settings
    pub volume: VolumeConfig,
    /// Network module settings
    pub network: NetworkConfig,
    /// Bluetooth module settings
    pub bluetooth: BluetoothConfig,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            width: 360,
            modules: vec![
                "volume".to_string(),
                "brightness".to_string(),
                "network".to_string(),
                "bluetooth".to_string(),
                "battery".to_string(),
                "power".to_string(),
            ],
            volume: VolumeConfig::default(),
            network: NetworkConfig::default(),
            bluetooth: BluetoothConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VolumeConfig {
    pub show_per_app: bool,
    pub show_input: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct NetworkConfig {
    pub show_vpn: bool,
    pub show_ethernet: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BluetoothConfig {
    pub show_battery: bool,
}

/// Theme configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Theme preset: dark, light, or custom
    pub preset: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            preset: "dark".to_string(),
        }
    }
}

/// Get the default config file path
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("gartray")
        .join("config.toml")
}

/// Load configuration from file
pub fn load(path: Option<&str>) -> Result<Config> {
    let config_path = path
        .map(PathBuf::from)
        .unwrap_or_else(config_path);

    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config: {}", config_path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse config: {}", config_path.display()))
    } else {
        tracing::info!("No config file found, using defaults");
        Ok(Config::default())
    }
}
