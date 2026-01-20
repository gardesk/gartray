//! Bluetooth module - control via BlueZ D-Bus
//!
//! Uses org.bluez interface on system bus.

use anyhow::{Context, Result};
use tracing::{info, warn, debug};
use zbus::blocking::Connection;
use zbus::zvariant::Value;

/// Bluetooth state
#[derive(Debug, Clone, Default)]
pub struct BluetoothState {
    pub powered: bool,
    pub available: bool,
    pub adapter_path: Option<String>,
}

/// Bluetooth module via BlueZ D-Bus
pub struct BluetoothModule {
    conn: Option<Connection>,
    state: BluetoothState,
}

impl BluetoothModule {
    /// Create a new bluetooth module
    pub fn new() -> Self {
        Self {
            conn: None,
            state: BluetoothState::default(),
        }
    }

    /// Connect to D-Bus and query initial state
    pub fn connect(&mut self) -> Result<()> {
        let conn = Connection::system().context("Failed to connect to system D-Bus")?;

        // Check if BlueZ is available
        let bluez_running = self.check_bluez_available(&conn);
        if !bluez_running {
            warn!("BlueZ not available");
            self.state.available = false;
            self.conn = Some(conn);
            return Ok(());
        }

        self.conn = Some(conn);

        // Find the default adapter
        self.find_adapter()?;

        if self.state.adapter_path.is_some() {
            self.state.available = true;
            self.update_state()?;
            info!("Bluetooth module connected, powered: {}", self.state.powered);
        } else {
            warn!("No Bluetooth adapter found");
            self.state.available = false;
        }

        Ok(())
    }

    /// Check if BlueZ is running
    fn check_bluez_available(&self, conn: &Connection) -> bool {
        let result: Result<String, zbus::Error> = conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &("org.bluez",),
        ).and_then(|m: zbus::Message| m.body().deserialize());

        result.is_ok()
    }

    /// Find the default Bluetooth adapter
    fn find_adapter(&mut self) -> Result<()> {
        if let Some(ref conn) = self.conn {
            // Use ObjectManager to find adapters
            let reply: Result<zbus::Message, _> = conn.call_method(
                Some("org.bluez"),
                "/",
                Some("org.freedesktop.DBus.ObjectManager"),
                "GetManagedObjects",
                &(),
            );

            if let Ok(msg) = reply {
                // Parse the response to find adapter paths
                // The response is a Dict<ObjectPath, Dict<String, Dict<String, Variant>>>
                if let Ok(objects) = msg.body().deserialize::<std::collections::HashMap<
                    zbus::zvariant::OwnedObjectPath,
                    std::collections::HashMap<String, std::collections::HashMap<String, Value>>
                >>() {
                    for (path, interfaces) in objects {
                        if interfaces.contains_key("org.bluez.Adapter1") {
                            debug!("Found Bluetooth adapter: {}", path);
                            self.state.adapter_path = Some(path.to_string());
                            return Ok(());
                        }
                    }
                }
            }

            // Fallback: try common adapter path
            let common_path = "/org/bluez/hci0";
            if self.adapter_exists(conn, common_path) {
                self.state.adapter_path = Some(common_path.to_string());
            }
        }
        Ok(())
    }

    /// Check if an adapter exists at the given path
    fn adapter_exists(&self, conn: &Connection, path: &str) -> bool {
        let result: Result<zbus::Message, zbus::Error> = conn.call_method(
            Some("org.bluez"),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.bluez.Adapter1", "Powered"),
        );

        result.is_ok()
    }

    /// Update state from BlueZ
    pub fn update_state(&mut self) -> Result<()> {
        let adapter_path = self.state.adapter_path.clone();
        if let (Some(conn), Some(path)) = (&self.conn, adapter_path) {
            match self.get_adapter_bool_property(conn, &path, "Powered") {
                Ok(powered) => {
                    self.state.powered = powered;
                }
                Err(e) => {
                    debug!("Failed to get Powered: {}", e);
                }
            }
        }
        Ok(())
    }

    /// Get a boolean property from the adapter
    fn get_adapter_bool_property(&self, conn: &Connection, adapter_path: &str, property: &str) -> Result<bool> {
        let reply: zbus::Message = conn.call_method(
            Some("org.bluez"),
            adapter_path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.bluez.Adapter1", property),
        )?;

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

    /// Set a property on the adapter
    fn set_adapter_property(&self, conn: &Connection, adapter_path: &str, property: &str, value: bool) -> Result<()> {
        conn.call_method(
            Some("org.bluez"),
            adapter_path,
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &("org.bluez.Adapter1", property, Value::Bool(value)),
        )?;
        Ok(())
    }

    /// Toggle Bluetooth on/off
    pub fn toggle(&mut self) -> Result<()> {
        let adapter_path = self.state.adapter_path.clone();
        if let (Some(conn), Some(path)) = (&self.conn, adapter_path) {
            if !self.state.available {
                warn!("Bluetooth not available");
                return Ok(());
            }

            let new_state = !self.state.powered;
            info!("Toggling Bluetooth: {} -> {}", self.state.powered, new_state);

            self.set_adapter_property(conn, &path, "Powered", new_state)?;
            self.state.powered = new_state;
        }
        Ok(())
    }

    /// Power on Bluetooth
    pub fn power_on(&mut self) -> Result<()> {
        let adapter_path = self.state.adapter_path.clone();
        if let (Some(conn), Some(path)) = (&self.conn, adapter_path) {
            if !self.state.available {
                return Ok(());
            }
            self.set_adapter_property(conn, &path, "Powered", true)?;
            self.state.powered = true;
            info!("Bluetooth powered on");
        }
        Ok(())
    }

    /// Power off Bluetooth
    pub fn power_off(&mut self) -> Result<()> {
        let adapter_path = self.state.adapter_path.clone();
        if let (Some(conn), Some(path)) = (&self.conn, adapter_path) {
            if !self.state.available {
                return Ok(());
            }
            self.set_adapter_property(conn, &path, "Powered", false)?;
            self.state.powered = false;
            info!("Bluetooth powered off");
        }
        Ok(())
    }

    /// Get current state
    pub fn state(&self) -> &BluetoothState {
        &self.state
    }

    /// Check if Bluetooth is powered
    pub fn is_powered(&self) -> bool {
        self.state.powered
    }

    /// Check if Bluetooth hardware is available
    pub fn is_available(&self) -> bool {
        self.state.available
    }
}

impl Default for BluetoothModule {
    fn default() -> Self {
        Self::new()
    }
}
