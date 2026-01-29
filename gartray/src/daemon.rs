//! Daemon state machine and main event loop for quick settings panel

use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{debug, info, warn};

use crate::config::{self, Config};
use crate::ipc::{Command, IpcServer, PanelVisibility, new_visibility};
use crate::panel::PopupPanel;

/// Get the path to the PID file
fn pid_file_path() -> PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("gartray.pid")
}

/// Check if an existing daemon is running
fn check_existing_daemon() -> Result<()> {
    let pid_path = pid_file_path();
    let socket_path = crate::ipc::socket_path();

    if pid_path.exists() {
        let pid_str = fs::read_to_string(&pid_path)?;
        let pid: i32 = pid_str.trim().parse()?;

        let proc_path = format!("/proc/{}", pid);
        if std::path::Path::new(&proc_path).exists() {
            anyhow::bail!(
                "gartray daemon already running (PID {}). Remove {} if incorrect.",
                pid,
                pid_path.display()
            );
        } else {
            warn!("Removing stale PID file for PID {}", pid);
            fs::remove_file(&pid_path)?;
            // Also clean up stale socket
            if socket_path.exists() {
                warn!("Removing stale socket file");
                let _ = fs::remove_file(&socket_path);
            }
        }
    } else if socket_path.exists() {
        // Socket exists but no PID file - orphaned socket
        warn!("Removing orphaned socket file (no PID file)");
        let _ = fs::remove_file(&socket_path);
    }

    Ok(())
}

/// Write the current process PID to the PID file
fn write_pid_file() -> Result<()> {
    let pid_path = pid_file_path();
    let pid = std::process::id();

    let mut file = fs::File::create(&pid_path)?;
    writeln!(file, "{}", pid)?;

    debug!("Wrote PID {} to {}", pid, pid_path.display());
    Ok(())
}

/// Remove the PID file
fn remove_pid_file() {
    let pid_path = pid_file_path();
    if let Err(e) = fs::remove_file(&pid_path) {
        warn!("Failed to remove PID file: {}", e);
    } else {
        debug!("Removed PID file");
    }
}

/// PID file guard - removes on drop
struct PidGuard;

impl Drop for PidGuard {
    fn drop(&mut self) {
        remove_pid_file();
    }
}

/// Daemon state
pub struct Daemon {
    config: Config,
    ipc_server: IpcServer,
    ipc_rx: Receiver<Command>,
    /// Quick settings popup panel
    panel: Option<PopupPanel>,
    running: bool,
    /// Shared visibility state for IPC sync
    visibility: PanelVisibility,
    /// Time when panel was last hidden (for debouncing toggle commands)
    last_hide_time: Option<std::time::Instant>,
}

impl Daemon {
    /// Create a new daemon
    pub fn new(config: Config) -> Result<Self> {
        let visibility = new_visibility();
        let (ipc_server, ipc_rx) = IpcServer::new(visibility.clone());

        // Create popup panel if enabled
        let panel = if config.panel.enabled {
            match PopupPanel::new(&config.panel) {
                Ok(p) => {
                    info!("Created quick settings panel");
                    Some(p)
                }
                Err(e) => {
                    warn!("Failed to create panel: {}", e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            config,
            ipc_server,
            ipc_rx,
            panel,
            running: true,
            visibility,
            last_hide_time: None,
        })
    }

    /// Initialize IPC server
    pub fn init_ipc(&mut self) -> Result<()> {
        self.ipc_server.start().context("Failed to start IPC server")?;
        info!("IPC server started");
        Ok(())
    }

    /// Run the main event loop
    pub async fn run(&mut self) -> Result<()> {
        info!("Entering main event loop");

        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sighup = signal(SignalKind::hangup())?;

        while self.running {
            // Check for IPC commands (non-blocking)
            self.poll_ipc_commands();

            tokio::select! {
                _ = sigterm.recv() => {
                    info!("Received SIGTERM, shutting down");
                    self.running = false;
                }
                _ = sighup.recv() => {
                    info!("Received SIGHUP, reloading config");
                    self.handle_reload()?;
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("Received Ctrl+C, shutting down");
                    self.running = false;
                }
                _ = self.poll_panel_events() => {
                    // Events processed
                }
            }
        }

        info!("Daemon shutdown complete");
        Ok(())
    }

    /// Poll for IPC commands
    fn poll_ipc_commands(&mut self) {
        while let Ok(cmd) = self.ipc_rx.try_recv() {
            self.handle_ipc_command(cmd);
        }
    }

    /// Update the shared visibility state
    fn update_visibility(&self) {
        let visible = self.panel.as_ref().map(|p| p.is_visible()).unwrap_or(false);
        self.visibility.store(visible, Ordering::SeqCst);
    }

    /// Handle an IPC command
    fn handle_ipc_command(&mut self, cmd: Command) {
        debug!("Handling IPC command: {:?}", cmd);
        match cmd {
            Command::Show { x, y } => {
                if let Some(ref mut panel) = self.panel {
                    if panel.is_visible() {
                        debug!("Panel already visible, ignoring Show command");
                    } else {
                        info!("Showing panel at ({}, {})", x, y);
                        let _ = panel.show(x, y);
                        let _ = panel.render();
                    }
                }
                self.update_visibility();
            }
            Command::Hide => {
                info!("Hiding panel");
                if let Some(ref mut panel) = self.panel {
                    if panel.is_visible() {
                        let _ = panel.hide();
                        self.last_hide_time = Some(std::time::Instant::now());
                    }
                }
                self.update_visibility();
            }
            Command::Toggle { x, y } => {
                if let Some(ref mut panel) = self.panel {
                    let was_visible = panel.is_visible();

                    // Debounce: if panel was just hidden (within 300ms), ignore toggle to show
                    // This prevents "bounce" when click-outside on gear triggers immediate re-toggle
                    if !was_visible {
                        if let Some(hide_time) = self.last_hide_time {
                            let elapsed = hide_time.elapsed();
                            if elapsed < std::time::Duration::from_millis(300) {
                                info!("Panel toggle ignored (debounce {}ms since hide)", elapsed.as_millis());
                                return;
                            }
                        }
                    }

                    info!("Panel toggle at ({}, {}): {} -> {}", x, y, was_visible, !was_visible);
                    if was_visible {
                        let _ = panel.hide();
                        self.last_hide_time = Some(std::time::Instant::now());
                        // Set visibility to false BEFORE any events can change it
                        self.visibility.store(false, Ordering::SeqCst);
                    } else {
                        let _ = panel.show(x, y);
                        let _ = panel.render();
                        // Set visibility to true BEFORE any events can change it
                        self.visibility.store(true, Ordering::SeqCst);
                    }
                }
            }
            Command::Reload => {
                info!("Reloading config via IPC");
                let _ = self.handle_reload();
            }
            Command::Status => {
                self.update_visibility();
                let panel_visible = self.panel.as_ref().map(|p| p.is_visible()).unwrap_or(false);
                info!(
                    "Status: running, panel {}",
                    if panel_visible { "visible" } else { "hidden" }
                );
            }
            Command::Quit => {
                info!("Quit requested via IPC");
                self.running = false;
            }
        }
    }

    /// Poll panel events
    async fn poll_panel_events(&mut self) -> Result<()> {
        // Process panel events
        if let Some(ref mut panel) = self.panel {
            let was_visible = panel.is_visible();
            if let Err(e) = panel.process_events() {
                warn!("Panel event error: {}", e);
            }
            // Only update visibility if panel was closed by events (escape/click-outside)
            let is_visible = panel.is_visible();
            if was_visible && !is_visible {
                self.visibility.store(false, Ordering::SeqCst);
                self.last_hide_time = Some(std::time::Instant::now());
            }
        }

        // Small delay to prevent busy loop (8ms for responsive IPC)
        tokio::time::sleep(tokio::time::Duration::from_millis(8)).await;
        Ok(())
    }

    /// Handle config reload
    fn handle_reload(&mut self) -> Result<()> {
        match config::load(None) {
            Ok(new_config) => {
                info!("Reloaded configuration");
                self.config = new_config;
            }
            Err(e) => {
                warn!("Failed to reload config: {}", e);
            }
        }
        Ok(())
    }
}

/// Run the panel daemon
pub async fn run(config_path: Option<String>, _foreground: bool) -> Result<()> {
    check_existing_daemon()?;
    write_pid_file()?;
    let _pid_guard = PidGuard;

    let config = config::load(config_path.as_deref())?;
    info!("Loaded configuration");
    info!("Panel width: {}px", config.panel.width);
    info!("Enabled modules: {:?}", config.panel.modules);

    let mut daemon = Daemon::new(config)?;
    daemon.init_ipc().context("Failed to initialize IPC")?;
    daemon.run().await
}
