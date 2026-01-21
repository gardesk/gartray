//! Network module - WiFi control via NetworkManager D-Bus
//!
//! Uses org.freedesktop.NetworkManager interface on system bus.

use anyhow::{Context, Result};
use tracing::{info, warn, debug};
use zbus::blocking::Connection;
use zbus::zvariant::Value;

/// WiFi access point info
#[derive(Debug, Clone)]
pub struct AccessPoint {
    /// SSID (network name)
    pub ssid: String,
    /// Signal strength (0-100)
    pub strength: u8,
    /// Whether currently connected
    pub connected: bool,
    /// Security type (none, wpa, wpa2, etc.)
    pub security: String,
    /// D-Bus object path for this AP
    pub path: String,
}

/// Network state
#[derive(Debug, Clone, Default)]
pub struct NetworkState {
    pub wifi_enabled: bool,
    pub wifi_available: bool,
    /// Currently connected network SSID
    pub connected_ssid: Option<String>,
    /// Available access points
    pub access_points: Vec<AccessPoint>,
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

    /// Get connected network SSID
    pub fn connected_ssid(&self) -> Option<&str> {
        self.state.connected_ssid.as_deref()
    }

    /// Get available access points
    pub fn access_points(&self) -> &[AccessPoint] {
        &self.state.access_points
    }

    /// Scan for WiFi networks
    pub fn scan_networks(&mut self) -> Result<()> {
        // Refresh wifi_enabled state before scanning (may have changed since startup)
        self.update_state()?;

        let conn = match &self.conn {
            Some(c) => c,
            None => {
                debug!("No D-Bus connection for WiFi scan");
                return Ok(());
            }
        };

        if !self.state.wifi_available || !self.state.wifi_enabled {
            debug!("WiFi not available or not enabled, skipping scan");
            self.state.access_points.clear();
            return Ok(());
        }

        info!("Scanning for WiFi networks...");

        // Get wireless device
        let wifi_device = match self.get_wifi_device(conn) {
            Some(d) => {
                info!("Found WiFi device: {}", d);
                d
            }
            None => {
                warn!("No WiFi device found");
                return Ok(());
            }
        };

        // Request scan (async, results come later)
        let _ = self.request_scan(conn, &wifi_device);

        // Get access points
        let aps = match self.get_access_points(conn, &wifi_device) {
            Ok(a) => a,
            Err(e) => {
                warn!("Failed to get access points: {}", e);
                return Err(e);
            }
        };

        // Get active connection to find connected SSID
        self.state.connected_ssid = self.get_active_ssid(conn, &wifi_device);

        // Mark connected AP
        let connected_ssid = self.state.connected_ssid.clone();
        self.state.access_points = aps.into_iter()
            .map(|mut ap| {
                ap.connected = Some(&ap.ssid) == connected_ssid.as_ref();
                ap
            })
            .collect();

        // Sort by signal strength (strongest first), connected first
        self.state.access_points.sort_by(|a, b| {
            match (a.connected, b.connected) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => b.strength.cmp(&a.strength),
            }
        });

        info!("Found {} access points, connected: {:?}",
              self.state.access_points.len(),
              self.state.connected_ssid);
        Ok(())
    }

    /// Get the first WiFi device path
    fn get_wifi_device(&self, conn: &Connection) -> Option<String> {
        // GetDevices returns array of object paths
        let reply: zbus::Message = conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            "/org/freedesktop/NetworkManager",
            Some("org.freedesktop.NetworkManager"),
            "GetDevices",
            &(),
        ).ok()?;

        let devices: Vec<zbus::zvariant::OwnedObjectPath> = reply.body().deserialize().ok()?;

        for device in devices {
            let device_path = device.to_string();
            // Check device type (2 = WiFi)
            if let Ok(device_type) = self.get_device_type(conn, &device_path) {
                if device_type == 2 {
                    return Some(device_path);
                }
            }
        }
        None
    }

    /// Get device type
    fn get_device_type(&self, conn: &Connection, device_path: &str) -> Result<u32> {
        let reply: zbus::Message = conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            device_path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.NetworkManager.Device", "DeviceType"),
        )?;

        let body = reply.body();
        let value: Value = body.deserialize()?;
        match value {
            Value::Value(inner) => {
                if let Value::U32(t) = *inner {
                    Ok(t)
                } else {
                    anyhow::bail!("Expected u32")
                }
            }
            Value::U32(t) => Ok(t),
            _ => anyhow::bail!("Expected u32 variant"),
        }
    }

    /// Request WiFi scan (non-blocking)
    fn request_scan(&self, conn: &Connection, device_path: &str) -> Result<()> {
        let options: std::collections::HashMap<&str, Value> = std::collections::HashMap::new();
        conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            device_path,
            Some("org.freedesktop.NetworkManager.Device.Wireless"),
            "RequestScan",
            &(options,),
        )?;
        Ok(())
    }

    /// Get access points from wireless device
    fn get_access_points(&self, conn: &Connection, device_path: &str) -> Result<Vec<AccessPoint>> {
        let reply: zbus::Message = conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            device_path,
            Some("org.freedesktop.NetworkManager.Device.Wireless"),
            "GetAccessPoints",
            &(),
        )?;

        let body = reply.body();
        let ap_paths: Vec<zbus::zvariant::OwnedObjectPath> = body.deserialize()?;
        debug!("Got {} AP paths from D-Bus", ap_paths.len());
        let mut aps = Vec::new();

        for ap_path in ap_paths {
            let path_str = ap_path.to_string();
            debug!("Getting info for AP: {}", path_str);
            match self.get_ap_info(conn, &path_str) {
                Ok(ap) => {
                    debug!("  SSID: '{}', strength: {}", ap.ssid, ap.strength);
                    // Skip hidden networks (empty SSID)
                    if !ap.ssid.is_empty() {
                        aps.push(ap);
                    }
                }
                Err(e) => {
                    debug!("  Failed to get AP info: {}", e);
                }
            }
        }

        // Deduplicate by SSID (keep strongest signal)
        let mut seen: std::collections::HashMap<String, AccessPoint> = std::collections::HashMap::new();
        for ap in aps {
            seen.entry(ap.ssid.clone())
                .and_modify(|existing| {
                    if ap.strength > existing.strength {
                        *existing = ap.clone();
                    }
                })
                .or_insert(ap);
        }

        Ok(seen.into_values().collect())
    }

    /// Get access point info
    fn get_ap_info(&self, conn: &Connection, ap_path: &str) -> Result<AccessPoint> {
        // Get SSID (byte array)
        let ssid = self.get_ap_ssid(conn, ap_path)?;

        // Get Strength (u8, 0-100)
        let strength = self.get_ap_strength(conn, ap_path).unwrap_or(0);

        // Get security flags
        let security = self.get_ap_security(conn, ap_path).unwrap_or_else(|| "none".to_string());

        Ok(AccessPoint {
            ssid,
            strength,
            connected: false, // Will be set later
            security,
            path: ap_path.to_string(),
        })
    }

    /// Get AP SSID
    fn get_ap_ssid(&self, conn: &Connection, ap_path: &str) -> Result<String> {
        let reply: zbus::Message = conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            ap_path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.NetworkManager.AccessPoint", "Ssid"),
        )?;

        let body = reply.body();
        // Properties.Get returns variant<ay> - try to deserialize directly
        let variant: zbus::zvariant::OwnedValue = body.deserialize()?;

        // Debug: print the variant structure
        let json = serde_json::to_value(&variant).unwrap_or(serde_json::Value::Null);
        debug!("SSID json: {}", json);

        // The variant should contain an array of bytes
        // Try different parsing strategies
        if let serde_json::Value::Array(arr) = &json {
            let bytes: Vec<u8> = arr.iter()
                .filter_map(|v| v.as_u64().map(|n| n as u8))
                .collect();
            if !bytes.is_empty() {
                return Ok(String::from_utf8_lossy(&bytes).to_string());
            }
        }

        // Try as zvariant object with "zvariant::Value::Value" key
        if let serde_json::Value::Object(obj) = &json {
            if let Some(serde_json::Value::Array(arr)) = obj.get("zvariant::Value::Value") {
                let bytes: Vec<u8> = arr.iter()
                    .filter_map(|v| v.as_u64().map(|n| n as u8))
                    .collect();
                if !bytes.is_empty() {
                    return Ok(String::from_utf8_lossy(&bytes).to_string());
                }
            }
        }

        Err(anyhow::anyhow!("Could not parse SSID from: {}", json))
    }

    /// Get AP signal strength
    fn get_ap_strength(&self, conn: &Connection, ap_path: &str) -> Option<u8> {
        let reply: zbus::Message = conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            ap_path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.NetworkManager.AccessPoint", "Strength"),
        ).ok()?;

        let body = reply.body();
        let value: Value = body.deserialize().ok()?;
        match value {
            Value::Value(inner) => {
                if let Value::U8(s) = *inner { Some(s) } else { None }
            }
            Value::U8(s) => Some(s),
            _ => None,
        }
    }

    /// Get AP security type
    fn get_ap_security(&self, conn: &Connection, ap_path: &str) -> Option<String> {
        // WpaFlags property
        let reply: zbus::Message = conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            ap_path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.NetworkManager.AccessPoint", "WpaFlags"),
        ).ok()?;

        let body = reply.body();
        let value: Value = body.deserialize().ok()?;
        let wpa_flags = match value {
            Value::Value(inner) => {
                if let Value::U32(f) = *inner { f } else { 0 }
            }
            Value::U32(f) => f,
            _ => 0,
        };

        // RsnFlags (WPA2)
        let rsn_flags = self.get_ap_rsn_flags(conn, ap_path).unwrap_or(0);

        if rsn_flags > 0 {
            Some("WPA2".to_string())
        } else if wpa_flags > 0 {
            Some("WPA".to_string())
        } else {
            Some("Open".to_string())
        }
    }

    /// Get AP RSN (WPA2) flags
    fn get_ap_rsn_flags(&self, conn: &Connection, ap_path: &str) -> Option<u32> {
        let reply: zbus::Message = conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            ap_path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.NetworkManager.AccessPoint", "RsnFlags"),
        ).ok()?;

        let body = reply.body();
        let value: Value = body.deserialize().ok()?;
        match value {
            Value::Value(inner) => {
                if let Value::U32(f) = *inner { Some(f) } else { None }
            }
            Value::U32(f) => Some(f),
            _ => None,
        }
    }

    /// Connect to a WiFi network
    /// If password is None and network is secured, this will try to use saved credentials
    pub fn connect_to_network(&mut self, ssid: &str) -> Result<()> {
        let conn = match &self.conn {
            Some(c) => c,
            None => anyhow::bail!("No D-Bus connection"),
        };

        if !self.state.wifi_available || !self.state.wifi_enabled {
            anyhow::bail!("WiFi not available or not enabled");
        }

        info!("Connecting to WiFi network: {}", ssid);

        // Get the WiFi device
        let wifi_device = match self.get_wifi_device(conn) {
            Some(d) => d,
            None => anyhow::bail!("No WiFi device found"),
        };

        // Find the access point path for this SSID
        let ap_path = self.state.access_points.iter()
            .find(|ap| ap.ssid == ssid)
            .map(|ap| ap.path.clone())
            .ok_or_else(|| anyhow::anyhow!("Access point not found: {}", ssid))?;

        // Try to find existing connection settings for this SSID
        if let Some(connection_path) = self.find_connection_for_ssid(conn, ssid) {
            info!("Found existing connection profile, activating: {}", connection_path);
            return self.activate_connection(conn, &connection_path, &wifi_device, &ap_path);
        }

        // No existing connection - try to connect (NM will create one for open networks)
        // For secured networks without saved credentials, this will fail
        info!("No saved connection, attempting AddAndActivateConnection");
        self.add_and_activate_connection(conn, ssid, &wifi_device, &ap_path)
    }

    /// Disconnect from current WiFi network
    pub fn disconnect(&mut self) -> Result<()> {
        let conn = match &self.conn {
            Some(c) => c,
            None => anyhow::bail!("No D-Bus connection"),
        };

        let wifi_device = match self.get_wifi_device(conn) {
            Some(d) => d,
            None => anyhow::bail!("No WiFi device found"),
        };

        info!("Disconnecting WiFi device");
        conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            wifi_device.as_str(),
            Some("org.freedesktop.NetworkManager.Device"),
            "Disconnect",
            &(),
        )?;

        self.state.connected_ssid = None;
        Ok(())
    }

    /// Find existing connection settings for an SSID
    fn find_connection_for_ssid(&self, conn: &Connection, ssid: &str) -> Option<String> {
        // Get all connection settings from Settings service
        let reply: zbus::Message = conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            "/org/freedesktop/NetworkManager/Settings",
            Some("org.freedesktop.NetworkManager.Settings"),
            "ListConnections",
            &(),
        ).ok()?;

        let body = reply.body();
        let connections: Vec<zbus::zvariant::OwnedObjectPath> = body.deserialize().ok()?;

        for conn_path in connections {
            let path_str = conn_path.to_string();
            if let Some(conn_ssid) = self.get_connection_ssid(conn, &path_str) {
                if conn_ssid == ssid {
                    return Some(path_str);
                }
            }
        }
        None
    }

    /// Get SSID from a connection settings object
    fn get_connection_ssid(&self, conn: &Connection, conn_path: &str) -> Option<String> {
        // GetSettings returns a{sa{sv}} - dict of setting names to dict of properties
        let reply: zbus::Message = conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            conn_path,
            Some("org.freedesktop.NetworkManager.Settings.Connection"),
            "GetSettings",
            &(),
        ).ok()?;

        let body = reply.body();
        let settings: std::collections::HashMap<String, std::collections::HashMap<String, Value>> =
            body.deserialize().ok()?;

        // Check connection type is 802-11-wireless
        if let Some(connection) = settings.get("connection") {
            if let Some(Value::Str(conn_type)) = connection.get("type") {
                if conn_type.as_str() != "802-11-wireless" {
                    return None;
                }
            }
        }

        // Get SSID from 802-11-wireless settings
        let wifi_settings = settings.get("802-11-wireless")?;
        let ssid_value = wifi_settings.get("ssid")?;

        // SSID is stored as ay (array of bytes)
        match ssid_value {
            Value::Array(arr) => {
                let bytes: Vec<u8> = arr.iter()
                    .filter_map(|v| {
                        if let Value::U8(b) = v { Some(*b) } else { None }
                    })
                    .collect();
                if !bytes.is_empty() {
                    Some(String::from_utf8_lossy(&bytes).to_string())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Activate an existing connection
    fn activate_connection(
        &self,
        conn: &Connection,
        connection_path: &str,
        device_path: &str,
        ap_path: &str,
    ) -> Result<()> {
        use zbus::zvariant::ObjectPath;
        conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            "/org/freedesktop/NetworkManager",
            Some("org.freedesktop.NetworkManager"),
            "ActivateConnection",
            &(
                ObjectPath::from_str_unchecked(connection_path),
                ObjectPath::from_str_unchecked(device_path),
                ObjectPath::from_str_unchecked(ap_path),
            ),
        )?;
        info!("Connection activation requested");
        Ok(())
    }

    /// Add and activate a new connection (for open networks or when NM has secrets)
    fn add_and_activate_connection(
        &self,
        conn: &Connection,
        ssid: &str,
        device_path: &str,
        ap_path: &str,
    ) -> Result<()> {
        use std::collections::HashMap;
        use zbus::zvariant::ObjectPath;

        // Build minimal connection settings
        let mut connection: HashMap<&str, Value> = HashMap::new();
        connection.insert("type", Value::Str("802-11-wireless".into()));
        connection.insert("id", Value::Str(ssid.into()));

        let mut wireless: HashMap<&str, Value> = HashMap::new();
        // SSID as byte array
        let ssid_bytes: Vec<Value> = ssid.bytes().map(Value::U8).collect();
        wireless.insert("ssid", Value::Array(ssid_bytes.into()));
        wireless.insert("mode", Value::Str("infrastructure".into()));

        let mut settings: HashMap<&str, HashMap<&str, Value>> = HashMap::new();
        settings.insert("connection", connection);
        settings.insert("802-11-wireless", wireless);

        conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            "/org/freedesktop/NetworkManager",
            Some("org.freedesktop.NetworkManager"),
            "AddAndActivateConnection",
            &(
                settings,
                ObjectPath::from_str_unchecked(device_path),
                ObjectPath::from_str_unchecked(ap_path),
            ),
        )?;

        info!("AddAndActivateConnection requested for: {}", ssid);
        Ok(())
    }

    /// Get active connection SSID for a device
    fn get_active_ssid(&self, conn: &Connection, device_path: &str) -> Option<String> {
        // Get ActiveAccessPoint from the wireless device
        let reply: zbus::Message = conn.call_method(
            Some("org.freedesktop.NetworkManager"),
            device_path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.NetworkManager.Device.Wireless", "ActiveAccessPoint"),
        ).ok()?;

        let body = reply.body();
        let value: Value = body.deserialize().ok()?;
        let ap_path = match value {
            Value::Value(inner) => {
                if let Value::ObjectPath(p) = &*inner {
                    p.to_string()
                } else {
                    return None;
                }
            }
            Value::ObjectPath(p) => p.to_string(),
            _ => return None,
        };

        // "/" means no active connection
        if ap_path == "/" {
            return None;
        }

        self.get_ap_ssid(conn, &ap_path).ok()
    }
}

impl Default for NetworkModule {
    fn default() -> Self {
        Self::new()
    }
}
