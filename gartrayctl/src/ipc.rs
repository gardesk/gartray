//! IPC client for communicating with gartray daemon

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

/// Get the path to the IPC socket
fn socket_path() -> PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("gartray.sock")
}

/// IPC commands (must match daemon)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Show,
    Hide,
    Toggle,
    Reload,
    Status,
    Quit,
}

/// IPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub success: bool,
    pub message: Option<String>,
    pub data: Option<serde_json::Value>,
}

/// Send a command to the running daemon
pub fn send_command(command: &str) -> Result<()> {
    let path = socket_path();

    if !path.exists() {
        anyhow::bail!("gartray daemon not running (socket not found)");
    }

    let mut stream = UnixStream::connect(&path)
        .with_context(|| "Failed to connect to gartray daemon")?;

    let cmd = match command {
        "show" => Command::Show,
        "hide" => Command::Hide,
        "toggle" => Command::Toggle,
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
        } else {
            println!("OK");
        }
    } else {
        eprintln!("Error: {}", response.message.unwrap_or_default());
        std::process::exit(1);
    }

    Ok(())
}
