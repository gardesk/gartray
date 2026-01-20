//! Bluetooth module - control via BlueZ D-Bus
//!
//! Uses org.bluez interface on system bus.

use anyhow::{Context, Result};
use tracing::{info, warn, debug};
use zbus::blocking::Connection;
use zbus::zvariant::Value;

/// Bluetooth device info
#[derive(Debug, Clone)]
pub struct BluetoothDevice {
    /// Device name (human-readable)
    pub name: String,
    /// MAC address
    pub address: String,
    /// Whether device is paired
    pub paired: bool,
    /// Whether device is currently connected
    pub connected: bool,
    /// Whether device is trusted (auto-connect)
    pub trusted: bool,
    /// Device type icon name (audio-headphones, input-keyboard, etc.)
    pub icon: String,
    /// D-Bus object path
    pub path: String,
}

/// Bluetooth state
#[derive(Debug, Clone, Default)]
pub struct BluetoothState {
    pub powered: bool,
    pub available: bool,
    pub adapter_path: Option<String>,
    /// Discovered/known devices
    pub devices: Vec<BluetoothDevice>,
    /// Whether discovery is active
    pub discovering: bool,
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

    /// Get known/discovered devices
    pub fn devices(&self) -> &[BluetoothDevice] {
        &self.state.devices
    }

    /// Scan for Bluetooth devices
    /// This gets all known devices from BlueZ (paired + discovered)
    pub fn scan_devices(&mut self) -> Result<()> {
        let conn = match &self.conn {
            Some(c) => c,
            None => {
                debug!("No D-Bus connection for Bluetooth scan");
                return Ok(());
            }
        };

        if !self.state.available || !self.state.powered {
            debug!("Bluetooth not available or not powered, skipping scan");
            self.state.devices.clear();
            return Ok(());
        }

        info!("Scanning for Bluetooth devices...");

        // Use ObjectManager to get all objects
        let reply: zbus::Message = conn.call_method(
            Some("org.bluez"),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )?;

        let body = reply.body();
        let objects: std::collections::HashMap<
            zbus::zvariant::OwnedObjectPath,
            std::collections::HashMap<String, std::collections::HashMap<String, Value>>
        > = body.deserialize()?;

        let mut devices = Vec::new();

        for (path, interfaces) in objects {
            // Look for Device1 interface
            if let Some(device_props) = interfaces.get("org.bluez.Device1") {
                if let Some(device) = self.parse_device(&path.to_string(), device_props) {
                    debug!("Found device: {} ({})", device.name, device.address);
                    devices.push(device);
                }
            }
        }

        // Sort: connected first, then paired, then by name
        devices.sort_by(|a, b| {
            match (a.connected, b.connected) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => match (a.paired, b.paired) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.name.cmp(&b.name),
                }
            }
        });

        info!("Found {} Bluetooth devices", devices.len());
        self.state.devices = devices;
        Ok(())
    }

    /// Parse device properties into BluetoothDevice
    fn parse_device(
        &self,
        path: &str,
        props: &std::collections::HashMap<String, Value>,
    ) -> Option<BluetoothDevice> {
        // Get name (may be missing for unnamed devices)
        let name = self.extract_string(props.get("Name"))
            .or_else(|| self.extract_string(props.get("Alias")))
            .unwrap_or_else(|| "Unknown Device".to_string());

        // Get address (required)
        let address = self.extract_string(props.get("Address"))?;

        // Get boolean properties
        let paired = self.extract_bool(props.get("Paired")).unwrap_or(false);
        let connected = self.extract_bool(props.get("Connected")).unwrap_or(false);
        let trusted = self.extract_bool(props.get("Trusted")).unwrap_or(false);

        // Get icon
        let icon = self.extract_string(props.get("Icon"))
            .unwrap_or_else(|| "bluetooth".to_string());

        Some(BluetoothDevice {
            name,
            address,
            paired,
            connected,
            trusted,
            icon,
            path: path.to_string(),
        })
    }

    /// Extract string from Value
    fn extract_string(&self, value: Option<&Value>) -> Option<String> {
        match value {
            Some(Value::Str(s)) => Some(s.to_string()),
            Some(Value::Value(inner)) => {
                if let Value::Str(s) = &**inner {
                    Some(s.to_string())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Extract bool from Value
    fn extract_bool(&self, value: Option<&Value>) -> Option<bool> {
        match value {
            Some(Value::Bool(b)) => Some(*b),
            Some(Value::Value(inner)) => {
                if let Value::Bool(b) = &**inner {
                    Some(*b)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Start discovery (for finding new devices)
    pub fn start_discovery(&mut self) -> Result<()> {
        let adapter_path = self.state.adapter_path.clone();
        if let (Some(conn), Some(path)) = (&self.conn, adapter_path) {
            if !self.state.available || !self.state.powered {
                return Ok(());
            }

            info!("Starting Bluetooth discovery");
            conn.call_method(
                Some("org.bluez"),
                path.as_str(),
                Some("org.bluez.Adapter1"),
                "StartDiscovery",
                &(),
            )?;
            self.state.discovering = true;
        }
        Ok(())
    }

    /// Stop discovery
    pub fn stop_discovery(&mut self) -> Result<()> {
        let adapter_path = self.state.adapter_path.clone();
        if let (Some(conn), Some(path)) = (&self.conn, adapter_path) {
            if self.state.discovering {
                info!("Stopping Bluetooth discovery");
                let _ = conn.call_method(
                    Some("org.bluez"),
                    path.as_str(),
                    Some("org.bluez.Adapter1"),
                    "StopDiscovery",
                    &(),
                );
                self.state.discovering = false;
            }
        }
        Ok(())
    }

    /// Check if discovery is active
    pub fn is_discovering(&self) -> bool {
        self.state.discovering
    }

    /// Connect to a device
    pub fn connect_device(&self, device_path: &str) -> Result<()> {
        if let Some(conn) = &self.conn {
            info!("Connecting to Bluetooth device: {}", device_path);
            conn.call_method(
                Some("org.bluez"),
                device_path,
                Some("org.bluez.Device1"),
                "Connect",
                &(),
            )?;
        }
        Ok(())
    }

    /// Disconnect from a device
    pub fn disconnect_device(&self, device_path: &str) -> Result<()> {
        if let Some(conn) = &self.conn {
            info!("Disconnecting Bluetooth device: {}", device_path);
            conn.call_method(
                Some("org.bluez"),
                device_path,
                Some("org.bluez.Device1"),
                "Disconnect",
                &(),
            )?;
        }
        Ok(())
    }
}

impl Default for BluetoothModule {
    fn default() -> Self {
        Self::new()
    }
}
