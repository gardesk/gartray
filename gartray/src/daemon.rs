//! Daemon state machine and main event loop

use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{debug, error, info, warn};

use crate::config::{self, Config};
use crate::ipc::{Command, IpcServer};
use crate::panel::PopupPanel;
use crate::tray::renderer::TrayRenderer;
use crate::tray::sni::{watcher, StatusNotifierHost};
use crate::tray::sni::watcher::WatcherState;
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
    /// D-Bus connection for SNI
    dbus_conn: Option<zbus::Connection>,
    /// SNI watcher state
    sni_watcher_state: Option<Arc<Mutex<WatcherState>>>,
    /// SNI host
    sni_host: Option<StatusNotifierHost>,
    /// Tray renderer for SNI icons
    tray_renderer: TrayRenderer,
    /// Quick settings popup panel
    panel: Option<PopupPanel>,
    running: bool,
}

impl Daemon {
    /// Create a new daemon
    pub fn new(config: Config) -> Result<Self> {
        let (ipc_server, ipc_rx) = IpcServer::new();
        let tray_renderer = TrayRenderer::new(&config.tray);

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
            xembed: None,
            ipc_server,
            ipc_rx,
            dbus_conn: None,
            sni_watcher_state: None,
            sni_host: None,
            tray_renderer,
            panel,
            running: true,
        })
    }

    /// Initialize IPC server
    pub fn init_ipc(&mut self) -> Result<()> {
        self.ipc_server.start().context("Failed to start IPC server")?;
        info!("IPC server started");
        Ok(())
    }

    /// Initialize D-Bus and SNI support
    pub async fn init_dbus(&mut self) -> Result<()> {
        info!("Initializing D-Bus connection...");

        // Connect to session bus
        let conn = zbus::Connection::session()
            .await
            .context("Failed to connect to session D-Bus")?;

        info!("Connected to D-Bus session bus");

        // Start the StatusNotifierWatcher service
        match watcher::start_watcher(&conn).await {
            Ok(state) => {
                self.sni_watcher_state = Some(state.clone());
                info!("StatusNotifierWatcher service started");

                // Create and register the host
                let mut host = StatusNotifierHost::new(conn.clone(), state)
                    .await
                    .context("Failed to create SNI host")?;

                host.register().await?;

                // Query existing items
                if let Err(e) = host.query_existing_items().await {
                    warn!("Failed to query existing SNI items: {}", e);
                }

                self.sni_host = Some(host);
            }
            Err(e) => {
                // Another watcher might be running (e.g., KDE's)
                warn!("Failed to start StatusNotifierWatcher: {} (another tray may be running)", e);
            }
        }

        self.dbus_conn = Some(conn);
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
            Command::Show { x, y } => {
                info!("Showing panel at ({}, {})", x, y);
                if let Some(ref mut panel) = self.panel {
                    let _ = panel.show(x, y);
                    let _ = panel.render();
                }
            }
            Command::Hide => {
                info!("Hiding panel");
                if let Some(ref mut panel) = self.panel {
                    let _ = panel.hide();
                }
            }
            Command::Toggle { x, y } => {
                if let Some(ref mut panel) = self.panel {
                    let visible = panel.is_visible();
                    info!("Panel toggle at ({}, {}): {} -> {}", x, y, visible, !visible);
                    if visible {
                        let _ = panel.hide();
                    } else {
                        let _ = panel.show(x, y);
                        let _ = panel.render();
                    }
                }
            }
            Command::Reload => {
                info!("Reloading config via IPC");
                let _ = self.handle_reload();
            }
            Command::Status => {
                let xembed_count = self.xembed.as_ref().map(|x| x.icon_count()).unwrap_or(0);
                let sni_count = self.sni_host.as_ref().map(|h| h.item_count()).unwrap_or(0);
                let panel_visible = self.panel.as_ref().map(|p| p.is_visible()).unwrap_or(false);
                info!(
                    "Status: running, {} XEMBED icons, {} SNI items, panel {}",
                    xembed_count,
                    sni_count,
                    if panel_visible { "visible" } else { "hidden" }
                );
            }
            Command::Quit => {
                info!("Quit requested via IPC");
                self.running = false;
            }
        }
    }

    /// Poll X11 events and render tray
    async fn poll_x11_events(&mut self) -> Result<()> {
        if let Some(ref mut xembed) = self.xembed {
            xembed.process_events()?;

            // Render SNI icons if we have any
            if let Some(ref sni_host) = self.sni_host {
                let sni_items: Vec<_> = sni_host.items().collect();
                if !sni_items.is_empty() {
                    // Get XEMBED icon count for offset
                    let xembed_offset = xembed.icon_count() as i32
                        * (self.config.tray.icon_size as i32 + self.config.tray.spacing as i32);

                    if let Err(e) = self.tray_renderer.render_sni_icons(
                        xembed.tray_window(),
                        &sni_items,
                        xembed_offset,
                    ) {
                        warn!("Failed to render SNI icons: {}", e);
                    }
                }
            }
        }

        // Process panel events
        if let Some(ref mut panel) = self.panel {
            if let Err(e) = panel.process_events() {
                warn!("Panel event error: {}", e);
            }
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
    daemon.init_dbus().await.context("Failed to initialize D-Bus")?;
    daemon.init_x11().context("Failed to initialize X11")?;
    daemon.run().await
}
