//! XEMBED system tray protocol implementation
//!
//! Implements the freedesktop.org System Tray Protocol using gartk-x11.

use std::collections::HashMap;
use anyhow::{Context, Result};
use gartk_x11::{Connection, Window, WindowConfig};
use x11rb::connection::Connection as X11Connection;
use x11rb::protocol::xproto::{
    self, Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux,
    ConnectionExt, CreateWindowAux, EventMask, PropMode, WindowClass,
};
use x11rb::protocol::Event;
use tracing::{debug, info, warn};

use crate::config::TrayConfig;

/// System tray opcodes from the spec
const SYSTEM_TRAY_REQUEST_DOCK: u32 = 0;

/// XEMBED message types
const XEMBED_EMBEDDED_NOTIFY: u32 = 0;
const XEMBED_PROTOCOL_VERSION: u32 = 0;

/// Atoms needed for system tray protocol
#[derive(Debug, Clone)]
struct TrayAtoms {
    net_system_tray_s: Atom,
    net_system_tray_opcode: Atom,
    net_system_tray_orientation: Atom,
    manager: Atom,
    xembed: Atom,
}

impl TrayAtoms {
    fn new(conn: &Connection) -> Result<Self> {
        let screen = conn.screen_num();
        let selection_name = format!("_NET_SYSTEM_TRAY_S{}", screen);

        Ok(Self {
            net_system_tray_s: conn.intern_atom(&selection_name, false)?,
            net_system_tray_opcode: conn.intern_atom("_NET_SYSTEM_TRAY_OPCODE", false)?,
            net_system_tray_orientation: conn.intern_atom("_NET_SYSTEM_TRAY_ORIENTATION", false)?,
            manager: conn.intern_atom("MANAGER", false)?,
            xembed: conn.intern_atom("_XEMBED", false)?,
        })
    }
}

/// A single tray icon
#[derive(Debug, Clone)]
pub struct TrayIcon {
    pub window: xproto::Window,
    pub width: u16,
    pub height: u16,
    pub mapped: bool,
}

/// XEMBED tray manager using gartk-x11
pub struct XEmbedManager {
    conn: Connection,
    atoms: TrayAtoms,
    /// Hidden selection owner window
    selection_window: xproto::Window,
    /// Tray container window (where icons are embedded)
    tray_window: Window,
    /// Tracked icons
    icons: HashMap<xproto::Window, TrayIcon>,
    /// Config
    config: TrayConfig,
    /// Whether we own the selection
    is_owner: bool,
}

impl XEmbedManager {
    /// Create a new XEMBED manager
    pub fn new(config: &TrayConfig) -> Result<Self> {
        let conn = Connection::connect(None).context("Failed to connect to X11")?;
        let atoms = TrayAtoms::new(&conn)?;

        // Create hidden selection owner window
        let selection_window = conn.generate_id()?;
        let values = CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE);

        conn.inner().create_window(
            0,
            selection_window,
            conn.root(),
            -1, -1,
            1, 1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &values,
        )?;

        debug!("Created tray selection window: {}", selection_window);

        // Create visible tray container window using gartk
        let tray_window = Window::create(
            conn.clone(),
            WindowConfig::new()
                .title("gartray")
                .class("gartray")
                .size(200, config.icon_size)
                .position(100, 100)
                .background(0xFF1a1a1a)
                .map_on_create(true),
        )?;

        info!("Created tray window: {}", tray_window.id());

        Ok(Self {
            conn,
            atoms,
            selection_window,
            tray_window,
            icons: HashMap::new(),
            config: config.clone(),
            is_owner: false,
        })
    }

    /// Acquire the system tray selection
    pub fn acquire_selection(&mut self) -> bool {
        let inner = self.conn.inner();

        if inner.set_selection_owner(
            self.selection_window,
            self.atoms.net_system_tray_s,
            x11rb::CURRENT_TIME,
        ).is_err() {
            warn!("Failed to set selection owner");
            return false;
        }

        // Verify we got the selection
        let owner = inner
            .get_selection_owner(self.atoms.net_system_tray_s)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.owner);

        if owner != Some(self.selection_window) {
            warn!("Failed to acquire tray selection");
            return false;
        }

        // Set tray orientation (horizontal = 0)
        let _ = inner.change_property32(
            PropMode::REPLACE,
            self.selection_window,
            self.atoms.net_system_tray_orientation,
            AtomEnum::CARDINAL,
            &[0],
        );

        // Broadcast MANAGER message to root
        let event = ClientMessageEvent::new(
            32,
            self.conn.root(),
            self.atoms.manager,
            [
                x11rb::CURRENT_TIME,
                self.atoms.net_system_tray_s,
                self.selection_window,
                0,
                0,
            ],
        );

        if inner.send_event(false, self.conn.root(), EventMask::STRUCTURE_NOTIFY, event).is_err() {
            warn!("Failed to broadcast MANAGER message");
            return false;
        }

        let _ = self.conn.flush();
        self.is_owner = true;
        info!("Acquired system tray selection, broadcasting MANAGER");
        true
    }

    /// Process pending X11 events
    pub fn process_events(&mut self) -> Result<()> {
        while let Some(event) = self.conn.poll_event()? {
            self.handle_event(event)?;
        }
        Ok(())
    }

    /// Handle a single X11 event
    fn handle_event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::ClientMessage(e) => {
                if e.type_ == self.atoms.net_system_tray_opcode {
                    let opcode = e.data.as_data32()[1];
                    let icon_window = e.data.as_data32()[2] as xproto::Window;

                    if opcode == SYSTEM_TRAY_REQUEST_DOCK {
                        info!("Dock request from window {}", icon_window);
                        self.dock_icon(icon_window)?;
                    }
                }
            }
            Event::DestroyNotify(e) => {
                if self.icons.remove(&e.window).is_some() {
                    info!("Tray icon {} destroyed", e.window);
                    self.reposition_icons()?;
                }
            }
            Event::UnmapNotify(e) => {
                if let Some(icon) = self.icons.get_mut(&e.window) {
                    icon.mapped = false;
                    debug!("Tray icon {} unmapped", e.window);
                    self.reposition_icons()?;
                }
            }
            Event::Expose(e) if e.window == self.tray_window.id() => {
                // Redraw tray background if needed
            }
            _ => {}
        }
        Ok(())
    }

    /// Dock a tray icon
    fn dock_icon(&mut self, icon_window: xproto::Window) -> Result<()> {
        let icon_size = self.config.icon_size as u16;
        let inner = self.conn.inner();

        // Subscribe to events on the icon
        let values = xproto::ChangeWindowAttributesAux::new()
            .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE);

        inner.change_window_attributes(icon_window, &values)?;

        // Calculate position
        let x = {
            let mapped_count = self.icons.values().filter(|i| i.mapped).count() as i16;
            mapped_count * (icon_size as i16 + self.config.spacing as i16)
        };

        // Reparent to tray window
        inner.reparent_window(icon_window, self.tray_window.id(), x, 0)?;

        // Resize to our icon size
        inner.configure_window(
            icon_window,
            &ConfigureWindowAux::new()
                .width(icon_size as u32)
                .height(icon_size as u32),
        )?;

        // Map the icon
        inner.map_window(icon_window)?;

        // Send XEMBED_EMBEDDED_NOTIFY
        let event = ClientMessageEvent::new(
            32,
            icon_window,
            self.atoms.xembed,
            [
                x11rb::CURRENT_TIME,
                XEMBED_EMBEDDED_NOTIFY,
                0,
                self.tray_window.id(),
                XEMBED_PROTOCOL_VERSION,
            ],
        );
        inner.send_event(false, icon_window, EventMask::NO_EVENT, event)?;

        // Track the icon
        self.icons.insert(icon_window, TrayIcon {
            window: icon_window,
            width: icon_size,
            height: icon_size,
            mapped: true,
        });

        self.update_tray_size()?;
        self.conn.flush()?;

        info!("Docked icon {} at x={}", icon_window, x);
        Ok(())
    }

    /// Reposition all icons after add/remove
    fn reposition_icons(&mut self) -> Result<()> {
        let icon_size = self.config.icon_size as i16;
        let spacing = self.config.spacing as i16;
        let inner = self.conn.inner();

        let mut x: i16 = 0;
        for icon in self.icons.values() {
            if icon.mapped {
                inner.configure_window(
                    icon.window,
                    &ConfigureWindowAux::new().x(x as i32).y(0),
                )?;
                x += icon_size + spacing;
            }
        }

        self.update_tray_size()?;
        self.conn.flush()?;
        Ok(())
    }

    /// Update tray window size based on icon count
    fn update_tray_size(&mut self) -> Result<()> {
        let mapped_count = self.icons.values().filter(|i| i.mapped).count();
        if mapped_count == 0 {
            return Ok(());
        }

        let icon_size = self.config.icon_size;
        let spacing = self.config.spacing as u32;
        let width = (mapped_count as u32 * icon_size) + ((mapped_count - 1) as u32 * spacing);

        self.conn.inner().configure_window(
            self.tray_window.id(),
            &ConfigureWindowAux::new().width(width).height(icon_size),
        )?;

        Ok(())
    }

    /// Get the number of docked icons
    pub fn icon_count(&self) -> usize {
        self.icons.len()
    }

    /// Check if we're the selection owner
    pub fn is_owner(&self) -> bool {
        self.is_owner
    }

    /// Release the selection on shutdown
    pub fn release(&mut self) {
        if !self.is_owner {
            return;
        }

        let inner = self.conn.inner();

        // Reparent icons back to root
        for icon in self.icons.values() {
            let _ = inner.unmap_window(icon.window);
            let _ = inner.reparent_window(icon.window, self.conn.root(), 0, 0);
        }

        // Release selection
        let _ = inner.set_selection_owner(
            x11rb::NONE,
            self.atoms.net_system_tray_s,
            x11rb::CURRENT_TIME,
        );

        // Destroy selection window
        let _ = inner.destroy_window(self.selection_window);
        let _ = self.conn.flush();

        self.is_owner = false;
        info!("Released system tray selection");
    }
}

impl Drop for XEmbedManager {
    fn drop(&mut self) {
        self.release();
    }
}
