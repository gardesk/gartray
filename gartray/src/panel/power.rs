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

    /// Shutdown the system via D-Bus (with systemctl fallback)
    pub fn shutdown(&self) -> Result<()> {
        info!("Initiating system shutdown");
        self.call_login1("PowerOff", true)
            .or_else(|e| {
                warn!("D-Bus PowerOff failed: {}, trying systemctl", e);
                Self::run_systemctl("poweroff")
            })
    }

    /// Reboot the system via D-Bus (with systemctl fallback)
    pub fn reboot(&self) -> Result<()> {
        info!("Initiating system reboot");
        self.call_login1("Reboot", true)
            .or_else(|e| {
                warn!("D-Bus Reboot failed: {}, trying systemctl", e);
                Self::run_systemctl("reboot")
            })
    }

    /// Suspend the system via D-Bus (with systemctl fallback)
    pub fn suspend(&self) -> Result<()> {
        info!("Initiating system suspend");
        self.call_login1("Suspend", true)
            .or_else(|e| {
                warn!("D-Bus Suspend failed: {}, trying systemctl", e);
                Self::run_systemctl("suspend")
            })
    }

    /// Hibernate the system via D-Bus (with systemctl fallback)
    pub fn hibernate(&self) -> Result<()> {
        info!("Initiating system hibernate");
        self.call_login1("Hibernate", true)
            .or_else(|e| {
                warn!("D-Bus Hibernate failed: {}, trying suspend", e);
                self.suspend()
            })
    }

    /// Logout current session
    pub fn logout(&self) -> Result<()> {
        info!("Initiating session logout");

        // Try D-Bus TerminateSession first
        if let Some(ref conn) = self.conn {
            // Get session ID from environment or use "self"
            let session_id = std::env::var("XDG_SESSION_ID")
                .unwrap_or_else(|_| "self".to_string());

            let result: Result<(), zbus::Error> = conn.call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                "TerminateSession",
                &(session_id.as_str(),),
            ).map(|_: zbus::Message| ());

            if result.is_ok() {
                return Ok(());
            }
            warn!("D-Bus TerminateSession failed: {:?}", result);
        }

        // Fallback: kill the window manager
        use std::process::Command;
        if let Ok(status) = Command::new("pkill").args(["-TERM", "-x", "gar"]).status() {
            if status.success() {
                return Ok(());
            }
        }

        anyhow::bail!("Failed to logout")
    }

    /// Run systemctl command as fallback
    fn run_systemctl(action: &str) -> Result<()> {
        use std::process::Command;
        let status = Command::new("systemctl")
            .arg(action)
            .status()
            .context(format!("Failed to run systemctl {}", action))?;

        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("systemctl {} failed", action)
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
