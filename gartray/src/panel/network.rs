//! Network module - WiFi control via NetworkManager D-Bus
//!
//! Uses org.freedesktop.NetworkManager interface on system bus.

use anyhow::{Context, Result};
use tracing::{info, warn, debug};
use zbus::blocking::Connection;
use zbus::zvariant::Value;

/// Network state
#[derive(Debug, Clone, Default)]
pub struct NetworkState {
    pub wifi_enabled: bool,
    pub wifi_available: bool,
}

/// Network module via NetworkManager D-Bus
pub struct NetworkModule {
    conn: Option<Connection>,
    state: NetworkState,
}

impl NetworkModule {
    /// Create a new network module
    pub fn new() -> Self {
        Self {
            conn: None,
            state: NetworkState::default(),
        }
    }

    /// Connect to D-Bus and query initial state
    pub fn connect(&mut self) -> Result<()> {
        let conn = Connection::system().context("Failed to connect to system D-Bus")?;

        // Check if NetworkManager is available
        let nm_running = self.check_nm_available(&conn);
        if !nm_running {
            warn!("NetworkManager not available");
            self.state.wifi_available = false;
            self.conn = Some(conn);
            return Ok(());
        }

        self.state.wifi_available = true;
        self.conn = Some(conn);

        // Query initial WiFi state
        self.update_state()?;

        info!("Network module connected, WiFi enabled: {}", self.state.wifi_enabled);
        Ok(())
    }

    /// Check if NetworkManager is running
    fn check_nm_available(&self, conn: &Connection) -> bool {
        let result: Result<String, zbus::Error> = conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &("org.freedesktop.NetworkManager",),
        ).and_then(|m: zbus::Message| m.body().deserialize());

        result.is_ok()
    }

    /// Update state from NetworkManager
    pub fn update_state(&mut self) -> Result<()> {
        if let Some(ref conn) = self.conn {
            if !self.state.wifi_available {
                return Ok(());
            }

            // Get WirelessEnabled property
            match self.get_bool_property(conn, "WirelessEnabled") {
                Ok(enabled) => {
                    self.state.wifi_enabled = enabled;
                }
                Err(e) => {
                    debug!("Failed to get WirelessEnabled: {}", e);
                }
            }
        }
        Ok(())
    }

    /// Get a boolean property from NetworkManager
    fn get_bool_property(&self, conn: &Connection, property: &str) -> Result<bool> {
        let reply: zbus::Message = conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            "/org/freedesktop/NetworkManager",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.NetworkManager", property),
        )?;

        // Properties.Get returns a Variant<T>
        let body = reply.body();
        let value: Value = body.deserialize()?;
        match value {
            Value::Value(inner) => {
                if let Value::Bool(b) = *inner {
                    Ok(b)
                } else {
                    anyhow::bail!("Expected bool, got {:?}", inner)
                }
            }
            Value::Bool(b) => Ok(b),
            _ => anyhow::bail!("Expected bool variant, got {:?}", value),
        }
    }

    /// Set a property on NetworkManager
    fn set_property(&self, conn: &Connection, property: &str, value: bool) -> Result<()> {
        conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            "/org/freedesktop/NetworkManager",
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &("org.freedesktop.NetworkManager", property, Value::Bool(value)),
        )?;
        Ok(())
    }

    /// Toggle WiFi on/off
    pub fn toggle_wifi(&mut self) -> Result<()> {
        if let Some(ref conn) = self.conn {
            if !self.state.wifi_available {
                warn!("WiFi not available");
                return Ok(());
            }

            let new_state = !self.state.wifi_enabled;
            info!("Toggling WiFi: {} -> {}", self.state.wifi_enabled, new_state);

            self.set_property(conn, "WirelessEnabled", new_state)?;
            self.state.wifi_enabled = new_state;
        }
        Ok(())
    }

    /// Enable WiFi
    pub fn enable_wifi(&mut self) -> Result<()> {
        if let Some(ref conn) = self.conn {
            if !self.state.wifi_available {
                return Ok(());
            }
            self.set_property(conn, "WirelessEnabled", true)?;
            self.state.wifi_enabled = true;
            info!("WiFi enabled");
        }
        Ok(())
    }

    /// Disable WiFi
    pub fn disable_wifi(&mut self) -> Result<()> {
        if let Some(ref conn) = self.conn {
            if !self.state.wifi_available {
                return Ok(());
            }
            self.set_property(conn, "WirelessEnabled", false)?;
            self.state.wifi_enabled = false;
            info!("WiFi disabled");
        }
        Ok(())
    }

    /// Get current state
    pub fn state(&self) -> &NetworkState {
        &self.state
    }

    /// Check if WiFi is enabled
    pub fn is_wifi_enabled(&self) -> bool {
        self.state.wifi_enabled
    }

    /// Check if WiFi hardware is available
    pub fn is_wifi_available(&self) -> bool {
        self.state.wifi_available
    }
}

impl Default for NetworkModule {
    fn default() -> Self {
        Self::new()
    }
}
