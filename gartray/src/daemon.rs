//! Daemon state machine and main event loop

use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{debug, info, warn};

use crate::config::{self, Config};
use crate::ipc::{Command, IpcServer};
use crate::tray::xembed::XEmbedManager;

/// Get the path to the PID file
fn pid_file_path() -> PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("gartray.pid")
}

/// Check if an existing daemon is running
fn check_existing_daemon() -> Result<()> {
    let pid_path = pid_file_path();

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
        }
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
    xembed: Option<XEmbedManager>,
    ipc_server: IpcServer,
    ipc_rx: Receiver<Command>,
    running: bool,
    panel_visible: bool,
}

impl Daemon {
    /// Create a new daemon
    pub fn new(config: Config) -> Result<Self> {
        let (ipc_server, ipc_rx) = IpcServer::new();
        Ok(Self {
            config,
            xembed: None,
            ipc_server,
            ipc_rx,
            running: true,
            panel_visible: false,
        })
    }

    /// Initialize IPC server
    pub fn init_ipc(&mut self) -> Result<()> {
        self.ipc_server.start().context("Failed to start IPC server")?;
        info!("IPC server started");
        Ok(())
    }

    /// Initialize X11 and tray
    pub fn init_x11(&mut self) -> Result<()> {
        info!("Initializing X11 connection...");

        let mut xembed = XEmbedManager::new(&self.config.tray)?;

        if xembed.acquire_selection() {
            info!("Acquired system tray selection");
            self.xembed = Some(xembed);
        } else {
            warn!("Failed to acquire tray selection (another tray running?)");
        }

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
                _ = self.poll_x11_events() => {
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

    /// Handle an IPC command
    fn handle_ipc_command(&mut self, cmd: Command) {
        debug!("Handling IPC command: {:?}", cmd);
        match cmd {
            Command::Show => {
                info!("Showing panel");
                self.panel_visible = true;
                // TODO: Actually show the panel window
            }
            Command::Hide => {
                info!("Hiding panel");
                self.panel_visible = false;
                // TODO: Actually hide the panel window
            }
            Command::Toggle => {
                self.panel_visible = !self.panel_visible;
                info!("Panel visibility toggled: {}", self.panel_visible);
                // TODO: Actually toggle the panel window
            }
            Command::Reload => {
                info!("Reloading config via IPC");
                let _ = self.handle_reload();
            }
            Command::Status => {
                info!(
                    "Status: running, {} icons, panel {}",
                    self.xembed.as_ref().map(|x| x.icon_count()).unwrap_or(0),
                    if self.panel_visible { "visible" } else { "hidden" }
                );
            }
            Command::Quit => {
                info!("Quit requested via IPC");
                self.running = false;
            }
        }
    }

    /// Poll X11 events
    async fn poll_x11_events(&mut self) -> Result<()> {
        if let Some(ref mut xembed) = self.xembed {
            xembed.process_events()?;
        }

        // Small delay to prevent busy loop
        tokio::time::sleep(tokio::time::Duration::from_millis(16)).await;
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

/// Run the tray daemon
pub async fn run(config_path: Option<String>, _foreground: bool) -> Result<()> {
    check_existing_daemon()?;
    write_pid_file()?;
    let _pid_guard = PidGuard;

    let config = config::load(config_path.as_deref())?;
    info!("Loaded configuration");
    info!("Tray position: {}", config.tray.position);
    info!("Icon size: {}px", config.tray.icon_size);

    let mut daemon = Daemon::new(config)?;
    daemon.init_ipc().context("Failed to initialize IPC")?;
    daemon.init_x11().context("Failed to initialize X11")?;
    daemon.run().await
}
