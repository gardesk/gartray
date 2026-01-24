//! IPC for gartray daemon <-> gartrayctl communication
//!
//! Uses Unix domain sockets with JSON protocol.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{debug, error, info};

/// Get the path to the IPC socket
pub fn socket_path() -> PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("gartray.sock")
}

/// IPC commands
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// Show the quick settings panel at position
    Show {
        #[serde(default)]
        x: i32,
        #[serde(default)]
        y: i32,
    },
    /// Hide the quick settings panel
    Hide,
    /// Toggle panel visibility at position
    Toggle {
        #[serde(default)]
        x: i32,
        #[serde(default)]
        y: i32,
    },
    /// Reload configuration
    Reload,
    /// Get daemon status
    Status,
    /// Quit the daemon
    Quit,
}

/// IPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Panel visibility state (for sync with garbar)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
}

impl Response {
    pub fn ok() -> Self {
        Self {
            success: true,
            message: None,
            data: None,
            visible: None,
        }
    }

    pub fn ok_with_visible(visible: bool) -> Self {
        Self {
            success: true,
            message: None,
            data: None,
            visible: Some(visible),
        }
    }

    pub fn ok_with_message(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(msg.into()),
            data: None,
            visible: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(msg.into()),
            data: None,
            visible: None,
        }
    }
}

/// Shared panel visibility state
pub type PanelVisibility = Arc<AtomicBool>;

/// Create a new shared visibility state
pub fn new_visibility() -> PanelVisibility {
    Arc::new(AtomicBool::new(false))
}

/// IPC server for the daemon
pub struct IpcServer {
    socket_path: PathBuf,
    listener: Option<UnixListener>,
    tx: Sender<Command>,
    rx: Option<Receiver<Command>>,
    visibility: PanelVisibility,
}

impl IpcServer {
    /// Create a new IPC server with shared visibility state
    pub fn new(visibility: PanelVisibility) -> (Self, Receiver<Command>) {
        let (tx, rx) = mpsc::channel();
        let server = Self {
            socket_path: socket_path(),
            listener: None,
            tx,
            rx: None,
            visibility,
        };
        (server, rx)
    }

    /// Start listening for connections
    pub fn start(&mut self) -> Result<()> {
        // Remove stale socket
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        let listener = UnixListener::bind(&self.socket_path)
            .with_context(|| format!("Failed to bind socket: {}", self.socket_path.display()))?;

        info!("IPC server listening on {}", self.socket_path.display());

        let tx = self.tx.clone();
        let visibility = self.visibility.clone();
        self.listener = Some(listener.try_clone()?);

        // Spawn listener thread
        thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let tx = tx.clone();
                        let vis = visibility.clone();
                        thread::spawn(move || {
                            if let Err(e) = handle_client(stream, tx, vis) {
                                error!("Client error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Accept error: {}", e);
                    }
                }
            }
        });

        Ok(())
    }

    /// Stop the IPC server
    pub fn stop(&mut self) {
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
        debug!("IPC server stopped");
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Handle a client connection
fn handle_client(mut stream: UnixStream, tx: Sender<Command>, visibility: PanelVisibility) -> Result<()> {
    let reader = BufReader::new(stream.try_clone()?);

    for line in reader.lines() {
        let line = line?;
        debug!("Received: {}", line);

        let response = match serde_json::from_str::<Command>(&line) {
            Ok(cmd) => {
                let needs_visibility = matches!(cmd, Command::Toggle { .. } | Command::Show { .. } | Command::Hide | Command::Status);

                // Forward command to daemon
                if tx.send(cmd).is_err() {
                    Response::error("Daemon not responding")
                } else if needs_visibility {
                    // Wait for daemon to process command and update visibility
                    // Poll visibility until it changes or timeout (max ~150ms)
                    // Longer timeout needed because daemon may be sleeping in poll_panel_events
                    let initial = visibility.load(Ordering::SeqCst);
                    let mut final_visible = initial;
                    for _ in 0..30 {
                        thread::sleep(Duration::from_millis(5));
                        final_visible = visibility.load(Ordering::SeqCst);
                        if final_visible != initial {
                            break;
                        }
                    }
                    Response::ok_with_visible(final_visible)
                } else {
                    Response::ok()
                }
            }
            Err(e) => Response::error(format!("Invalid command: {}", e)),
        };

        let response_json = serde_json::to_string(&response)?;
        writeln!(stream, "{}", response_json)?;
        stream.flush()?;
    }

    Ok(())
}

/// Send a command to the running daemon (client side)
pub async fn send_command(command: &str) -> Result<()> {
    send_command_with_pos(command, 0, 0).await
}

/// Send a command with position to the running daemon
pub async fn send_command_with_pos(command: &str, x: i32, y: i32) -> Result<()> {
    let path = socket_path();

    if !path.exists() {
        anyhow::bail!("gartray daemon not running (socket not found)");
    }

    let mut stream = UnixStream::connect(&path)
        .with_context(|| "Failed to connect to gartray daemon")?;

    let cmd = match command {
        "show" => Command::Show { x, y },
        "hide" => Command::Hide,
        "toggle" | "panel" => Command::Toggle { x, y },
        "reload" => Command::Reload,
        "status" => Command::Status,
        "quit" => Command::Quit,
        _ => anyhow::bail!("Unknown command: {}", command),
    };

    let cmd_json = serde_json::to_string(&cmd)?;
    writeln!(stream, "{}", cmd_json)?;
    stream.flush()?;

    // Read response
    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader.read_line(&mut response_line)?;

    let response: Response = serde_json::from_str(&response_line)?;
    if response.success {
        if let Some(msg) = response.message {
            println!("{}", msg);
        }
    } else {
        eprintln!("Error: {}", response.message.unwrap_or_default());
    }

    Ok(())
}
