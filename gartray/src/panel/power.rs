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
        // Use loginctl which handles polkit properly
        self.run_command("loginctl", &["poweroff"])
    }

    /// Reboot the system
    pub fn reboot(&self) -> Result<()> {
        info!("Initiating system reboot");
        self.run_command("loginctl", &["reboot"])
    }

    /// Suspend the system
    pub fn suspend(&self) -> Result<()> {
        info!("Initiating system suspend");
        self.run_command("loginctl", &["suspend"])
    }

    /// Hibernate the system (uses suspend as fallback)
    pub fn hibernate(&self) -> Result<()> {
        info!("Initiating system suspend");
        self.run_command("loginctl", &["suspend"])
    }

    /// Logout current session
    pub fn logout(&self) -> Result<()> {
        info!("Initiating session logout");
        // Try multiple approaches for logout
        use std::process::Command;

        // First try: loginctl terminate-session self
        if let Ok(status) = Command::new("loginctl")
            .args(["terminate-session", "self"])
            .status()
        {
            if status.success() {
                return Ok(());
            }
        }

        // Second try: kill the window manager (gar)
        if let Ok(status) = Command::new("pkill")
            .args(["-TERM", "-x", "gar"])
            .status()
        {
            if status.success() {
                return Ok(());
            }
        }

        // Third try: send SIGTERM to the X session leader
        if let Ok(output) = Command::new("loginctl")
            .args(["show-session", "self", "-p", "Leader", "--value"])
            .output()
        {
            if output.status.success() {
                let pid = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !pid.is_empty() {
                    let _ = Command::new("kill").args(["-TERM", &pid]).status();
                    return Ok(());
                }
            }
        }

        anyhow::bail!("Failed to logout")
    }

    /// Run a command and return result
    fn run_command(&self, cmd: &str, args: &[&str]) -> Result<()> {
        use std::process::Command;
        let status = Command::new(cmd)
            .args(args)
            .status()
            .context(format!("Failed to run {} {:?}", cmd, args))?;

        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("{} {:?} failed with exit code {:?}", cmd, args, status.code())
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
