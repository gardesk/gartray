//! XEMBED system tray protocol implementation
//!
//! Implements the freedesktop.org System Tray Protocol:
//! https://specifications.freedesktop.org/systemtray-spec/systemtray-spec-latest.html
//!
//! Key concepts:
//! - Selection owner: gartray claims _NET_SYSTEM_TRAY_S{screen} to become the tray manager
//! - XEMBED: Tray icons are embedded windows reparented to a container
//! - Client messages: Apps request docking via SYSTEM_TRAY_REQUEST_DOCK

// TODO: Port from garbar/garbar/src/modules/tray.rs with improvements:
// - Standalone window (not embedded in bar)
// - Click event forwarding to icons
// - Hover state tracking
// - Visual feedback

use std::collections::HashMap;
use x11rb::protocol::xproto::Window;

/// A single tray icon
#[derive(Debug, Clone)]
pub struct TrayIcon {
    pub window: Window,
    pub width: u16,
    pub height: u16,
    pub mapped: bool,
}

/// XEMBED tray manager
pub struct XEmbedManager {
    icons: HashMap<Window, TrayIcon>,
    // TODO: Add X11 connection, atoms, selection window
}

impl XEmbedManager {
    /// Create a new XEMBED manager
    pub fn new() -> Self {
        Self {
            icons: HashMap::new(),
        }
    }

    /// Acquire the system tray selection
    pub fn acquire_selection(&mut self) -> bool {
        // TODO: Implement selection acquisition
        false
    }

    /// Handle a dock request from an application
    pub fn dock_icon(&mut self, _window: Window) {
        // TODO: Implement icon docking
    }

    /// Handle icon destruction
    pub fn handle_destroy(&mut self, window: Window) -> bool {
        self.icons.remove(&window).is_some()
    }
}

impl Default for XEmbedManager {
    fn default() -> Self {
        Self::new()
    }
}
