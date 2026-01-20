//! Power actions module
//!
//! Provides shutdown, restart, logout, and hibernate functionality
//! via systemd D-Bus interface (org.freedesktop.login1).

use anyhow::{Context, Result};
use tracing::{info, warn};
use zbus::blocking::Connection;

/// Power actions via systemd-logind D-Bus
pub struct PowerModule {
    conn: Option<Connection>,
}

impl PowerModule {
    /// Create a new power module
    pub fn new() -> Self {
        Self { conn: None }
    }

    /// Connect to D-Bus
    pub fn connect(&mut self) -> Result<()> {
        let conn = Connection::system().context("Failed to connect to system D-Bus")?;
        self.conn = Some(conn);
        info!("Power module connected to D-Bus");
        Ok(())
    }

    /// Shutdown the system
    pub fn shutdown(&self) -> Result<()> {
        info!("Initiating system shutdown");
        self.call_login1("PowerOff", true)
    }

    /// Reboot the system
    pub fn reboot(&self) -> Result<()> {
        info!("Initiating system reboot");
        self.call_login1("Reboot", true)
    }

    /// Suspend the system
    pub fn suspend(&self) -> Result<()> {
        info!("Initiating system suspend");
        self.call_login1("Suspend", true)
    }

    /// Hibernate the system
    pub fn hibernate(&self) -> Result<()> {
        info!("Initiating system hibernate");
        self.call_login1("Hibernate", true)
    }

    /// Logout current session
    pub fn logout(&self) -> Result<()> {
        info!("Initiating session logout");
        // For logout, we terminate the current session
        // This requires getting the current session ID first
        if let Some(ref conn) = self.conn {
            // Get current session
            let session_id = std::env::var("XDG_SESSION_ID")
                .unwrap_or_else(|_| "auto".to_string());

            let proxy = zbus::blocking::fdo::DBusProxy::new(conn)?;

            // Call TerminateSession on login1.Manager
            let result: Result<(), zbus::Error> = conn.call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                "TerminateSession",
                &(session_id,),
            ).map(|_: zbus::Message| ());

            match result {
                Ok(_) => {
                    info!("Logout initiated");
                    Ok(())
                }
                Err(e) => {
                    warn!("Failed to logout via D-Bus: {}", e);
                    // Fallback: try loginctl
                    self.logout_fallback()
                }
            }
        } else {
            self.logout_fallback()
        }
    }

    /// Fallback logout using loginctl command
    fn logout_fallback(&self) -> Result<()> {
        use std::process::Command;
        let status = Command::new("loginctl")
            .args(["terminate-session", ""])
            .status()
            .context("Failed to run loginctl")?;

        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("loginctl terminate-session failed")
        }
    }

    /// Call a method on org.freedesktop.login1.Manager
    fn call_login1(&self, method: &str, interactive: bool) -> Result<()> {
        if let Some(ref conn) = self.conn {
            let result: Result<(), zbus::Error> = conn.call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                method,
                &(interactive,),
            ).map(|_: zbus::Message| ());

            result.context(format!("Failed to call {}", method))
        } else {
            anyhow::bail!("Not connected to D-Bus")
        }
    }

    /// Check if an action is available
    pub fn can_power_off(&self) -> bool {
        self.check_capability("CanPowerOff")
    }

    pub fn can_reboot(&self) -> bool {
        self.check_capability("CanReboot")
    }

    pub fn can_suspend(&self) -> bool {
        self.check_capability("CanSuspend")
    }

    pub fn can_hibernate(&self) -> bool {
        self.check_capability("CanHibernate")
    }

    fn check_capability(&self, method: &str) -> bool {
        if let Some(ref conn) = self.conn {
            let result: Result<String, zbus::Error> = conn.call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                method,
                &(),
            ).and_then(|m: zbus::Message| m.body().deserialize());

            matches!(result, Ok(s) if s == "yes" || s == "challenge")
        } else {
            false
        }
    }
}

impl Default for PowerModule {
    fn default() -> Self {
        Self::new()
    }
}
