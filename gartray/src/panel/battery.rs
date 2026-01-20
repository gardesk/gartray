//! Battery status module using UPower D-Bus
//!
//! Shows battery percentage, charging status, and time remaining.

use anyhow::{Context, Result};
use zbus::{Connection, proxy};
use tracing::{debug, info, warn};

/// UPower device proxy
#[proxy(
    interface = "org.freedesktop.UPower.Device",
    default_path = "/org/freedesktop/UPower/devices/DisplayDevice"
)]
trait UPowerDevice {
    /// Battery percentage
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;

    /// State (charging, discharging, etc.)
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;

    /// Time to empty in seconds
    #[zbus(property)]
    fn time_to_empty(&self) -> zbus::Result<i64>;

    /// Time to full in seconds
    #[zbus(property)]
    fn time_to_full(&self) -> zbus::Result<i64>;

    /// Whether the device is present
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;

    /// Device type (battery = 2)
    #[zbus(property, name = "Type")]
    fn device_type(&self) -> zbus::Result<u32>;
}

/// Battery state constants
pub mod state {
    pub const UNKNOWN: u32 = 0;
    pub const CHARGING: u32 = 1;
    pub const DISCHARGING: u32 = 2;
    pub const EMPTY: u32 = 3;
    pub const FULLY_CHARGED: u32 = 4;
    pub const PENDING_CHARGE: u32 = 5;
    pub const PENDING_DISCHARGE: u32 = 6;
}

/// Battery status
#[derive(Debug, Clone, Default)]
pub struct BatteryState {
    /// Battery percentage (0-100)
    pub percentage: f64,
    /// Whether charging
    pub charging: bool,
    /// Whether fully charged
    pub fully_charged: bool,
    /// Time remaining (formatted string)
    pub time_remaining: Option<String>,
    /// Whether battery is present
    pub present: bool,
}

/// Battery status module
pub struct BatteryModule {
    proxy: Option<UPowerDeviceProxy<'static>>,
    state: BatteryState,
    available: bool,
}

impl BatteryModule {
    /// Create a new battery module
    pub fn new() -> Self {
        Self {
            proxy: None,
            state: BatteryState::default(),
            available: false,
        }
    }

    /// Connect to UPower
    pub async fn connect(&mut self, conn: &Connection) -> Result<()> {
        let proxy = UPowerDeviceProxy::builder(conn)
            .destination("org.freedesktop.UPower")?
            .build()
            .await
            .context("Failed to create UPower proxy")?;

        // Check if this is actually a battery
        match proxy.device_type().await {
            Ok(2) => {
                // Type 2 is Battery
                self.proxy = Some(proxy);
                self.available = true;
                info!("Connected to UPower battery device");
                self.refresh().await?;
            }
            Ok(t) => {
                debug!("Display device is not a battery (type={})", t);
                self.available = false;
            }
            Err(e) => {
                warn!("Failed to get device type: {}", e);
                self.available = false;
            }
        }

        Ok(())
    }

    /// Refresh battery state
    pub async fn refresh(&mut self) -> Result<()> {
        let proxy = match &self.proxy {
            Some(p) => p,
            None => return Ok(()),
        };

        // Get percentage
        if let Ok(pct) = proxy.percentage().await {
            self.state.percentage = pct;
        }

        // Get state
        if let Ok(state_val) = proxy.state().await {
            self.state.charging = state_val == state::CHARGING;
            self.state.fully_charged = state_val == state::FULLY_CHARGED;

            // Get time remaining
            self.state.time_remaining = match state_val {
                state::CHARGING => {
                    proxy.time_to_full().await.ok().and_then(|secs| {
                        if secs > 0 {
                            Some(format_duration(secs))
                        } else {
                            None
                        }
                    })
                }
                state::DISCHARGING => {
                    proxy.time_to_empty().await.ok().and_then(|secs| {
                        if secs > 0 {
                            Some(format_duration(secs))
                        } else {
                            None
                        }
                    })
                }
                _ => None,
            };
        }

        // Check presence
        if let Ok(present) = proxy.is_present().await {
            self.state.present = present;
        }

        debug!(
            "Battery: {:.0}%, charging={}, time={:?}",
            self.state.percentage,
            self.state.charging,
            self.state.time_remaining
        );

        Ok(())
    }

    /// Get current state
    pub fn state(&self) -> &BatteryState {
        &self.state
    }

    /// Check if battery is available
    pub fn is_available(&self) -> bool {
        self.available && self.state.present
    }

    /// Get appropriate icon name for current state
    pub fn icon_name(&self) -> &'static str {
        if !self.available || !self.state.present {
            return "battery-missing";
        }

        let pct = self.state.percentage;
        let charging = self.state.charging;

        if self.state.fully_charged {
            "battery-full-charged"
        } else if charging {
            if pct >= 80.0 { "battery-full-charging" }
            else if pct >= 50.0 { "battery-good-charging" }
            else if pct >= 20.0 { "battery-low-charging" }
            else { "battery-caution-charging" }
        } else {
            if pct >= 80.0 { "battery-full" }
            else if pct >= 50.0 { "battery-good" }
            else if pct >= 20.0 { "battery-low" }
            else if pct >= 5.0 { "battery-caution" }
            else { "battery-empty" }
        }
    }
}

/// Format duration in seconds to human-readable string
fn format_duration(secs: i64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;

    if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}
