//! Quick settings popup window
//!
//! Creates and manages the popup panel window with all settings modules.

use anyhow::{Context, Result};
use cairo::{Context as CairoContext, Format, ImageSurface};
use gartk_x11::{Connection, Window, WindowConfig};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, EventMask, GrabMode, PropMode};
use x11rb::protocol::randr::ConnectionExt as RandrConnectionExt;
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;
use x11rb::CURRENT_TIME;
use tracing::{debug, info, warn};

use crate::config::PanelConfig;
use crate::panel::volume::VolumeModule;
use crate::panel::brightness::BrightnessModule;
use crate::panel::battery::BatteryModule;
use crate::panel::power::PowerModule;
use crate::panel::network::NetworkModule;
use crate::panel::bluetooth::BluetoothModule;
use crate::panel::dnd::DndModule;

/// Monitor information
#[derive(Debug, Clone)]
struct MonitorInfo {
    x: i32,
    y: i32,
    width: u32,
    _height: u32,
}

/// Slider row layout info (for volume/brightness)
#[derive(Debug, Clone)]
struct SliderRow {
    name: String,
    y: f64,
    height: f64,
    slider_x: f64,
    slider_width: f64,
    /// Icon hit region (for mute toggle)
    icon_x: f64,
    icon_width: f64,
}

/// Toggle button layout info
#[derive(Debug, Clone)]
struct ToggleButton {
    name: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    active: bool,
}

/// List item layout info (for WiFi/Bluetooth lists)
#[derive(Debug, Clone)]
struct ListItem {
    /// Item identifier (SSID or device path)
    id: String,
    y: f64,
    height: f64,
}

/// Expansion state for picker sections
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpandedSection {
    None,
    WiFi,
    Bluetooth,
}

/// Quick settings popup panel
pub struct PopupPanel {
    conn: Connection,
    window: Option<Window>,
    surface: Option<ImageSurface>,
    config: PanelConfig,
    visible: bool,
    width: u32,
    height: u32,
    /// Background color
    bg_color: (f64, f64, f64, f64),
    /// Current position
    pos_x: i32,
    pos_y: i32,
    /// Volume control module
    volume: Option<VolumeModule>,
    /// Brightness control module
    brightness: Option<BrightnessModule>,
    /// Battery status module
    battery: Option<BatteryModule>,
    /// Power actions module
    power: Option<PowerModule>,
    /// Network (WiFi) module
    network: Option<NetworkModule>,
    /// Bluetooth module
    bluetooth: Option<BluetoothModule>,
    /// Do Not Disturb module
    dnd: Option<DndModule>,
    /// Slider rows for hit testing (volume, brightness)
    slider_rows: Vec<SliderRow>,
    /// Toggle buttons for hit testing
    toggle_buttons: Vec<ToggleButton>,
    /// Currently dragging a slider (module name)
    dragging: Option<String>,
    /// Pending value during drag (avoids blocking PulseAudio calls)
    drag_value: f64,
    /// Currently hovered button name (for hover effects)
    hovered_button: Option<String>,
    /// Currently expanded section (WiFi or Bluetooth picker)
    expanded: ExpandedSection,
    /// List items for hit testing in expanded section
    list_items: Vec<ListItem>,
    /// Base panel height (without expansion)
    base_height: u32,
    /// Button press start time (for hold detection)
    press_start: Option<std::time::Instant>,
    /// Which button is being pressed (for hold detection)
    press_button: Option<String>,
    /// WiFi list scroll offset (number of items scrolled)
    wifi_scroll_offset: usize,
    /// Bluetooth list scroll offset (number of items scrolled)
    bluetooth_scroll_offset: usize,
    /// WiFi password entry - SSID we're entering password for
    password_entry_ssid: Option<String>,
    /// WiFi password being typed
    password_text: String,
    /// Last time slider value was applied (for throttling wpctl calls)
    last_slider_apply: Option<std::time::Instant>,
    /// Last time we rendered (for 60fps cap during drag)
    last_render: Option<std::time::Instant>,
    /// Last time WiFi networks were scanned (for periodic refresh)
    last_wifi_scan: Option<std::time::Instant>,
    /// Previously focused window (to restore on hide)
    previous_focus: Option<u32>,
    /// Time when panel was shown (for grace period on FocusOut)
    show_time: Option<std::time::Instant>,
}

impl PopupPanel {
    /// Create a new popup panel
    pub fn new(config: &PanelConfig) -> Result<Self> {
        let conn = Connection::connect(None).context("Failed to connect to X11")?;

        // Calculate height: header + toggle grid (2 rows) + power buttons + sliders
        // Header: 50px, Toggle grid: 2x68=136px, Power row: 55px, Sliders: 2x50px, Padding
        let height = 360;

        // Initialize modules based on config
        let mut volume = None;
        let mut brightness = None;
        let mut battery = None;

        for module_name in &config.modules {
            match module_name.as_str() {
                "volume" => {
                    let mut vol = VolumeModule::new();
                    if let Err(e) = vol.connect() {
                        warn!("Failed to connect to PulseAudio: {}", e);
                    } else {
                        info!("Volume module initialized");
                        volume = Some(vol);
                    }
                }
                "brightness" => {
                    let mut bright = BrightnessModule::new();
                    if let Err(e) = bright.init() {
                        warn!("Failed to init brightness: {}", e);
                    } else {
                        info!("Brightness module initialized");
                        brightness = Some(bright);
                    }
                }
                "battery" => {
                    // Battery uses async D-Bus, so we just create a placeholder
                    // Real async init happens via connect() later if needed
                    info!("Battery module placeholder created (async init pending)");
                    battery = Some(BatteryModule::new());
                }
                _ => {}
            }
        }

        // Initialize power module (always available)
        let power = {
            let mut p = PowerModule::new();
            if let Err(e) = p.connect() {
                warn!("Failed to connect power module: {}", e);
                None
            } else {
                info!("Power module initialized");
                Some(p)
            }
        };

        // Initialize network (WiFi) module
        let network = {
            let mut n = NetworkModule::new();
            if let Err(e) = n.connect() {
                warn!("Failed to connect network module: {}", e);
                None
            } else {
                info!("Network module initialized, WiFi: {}",
                      if n.is_wifi_enabled() { "on" } else { "off" });
                Some(n)
            }
        };

        // Initialize Bluetooth module
        let bluetooth = {
            let mut b = BluetoothModule::new();
            if let Err(e) = b.connect() {
                warn!("Failed to connect bluetooth module: {}", e);
                None
            } else {
                info!("Bluetooth module initialized, powered: {}",
                      if b.is_powered() { "on" } else { "off" });
                Some(b)
            }
        };

        // Initialize DND module
        let dnd = {
            let mut d = DndModule::new();
            if let Err(e) = d.init() {
                warn!("Failed to init DND module: {}", e);
                None
            } else {
                info!("DND module initialized");
                Some(d)
            }
        };

        Ok(Self {
            conn,
            window: None,
            surface: None,
            config: config.clone(),
            visible: false,
            width: config.width,
            height,
            bg_color: (0.12, 0.12, 0.14, 1.0), // Solid dark background
            pos_x: 0,
            pos_y: 0,
            volume,
            brightness,
            battery,
            power,
            network,
            bluetooth,
            dnd,
            slider_rows: Vec::new(),
            toggle_buttons: Vec::new(),
            dragging: None,
            drag_value: 0.0,
            hovered_button: None,
            expanded: ExpandedSection::None,
            list_items: Vec::new(),
            base_height: height,
            press_start: None,
            press_button: None,
            wifi_scroll_offset: 0,
            bluetooth_scroll_offset: 0,
            password_entry_ssid: None,
            password_text: String::new(),
            last_slider_apply: None,
            last_render: None,
            last_wifi_scan: None,
            previous_focus: None,
            show_time: None,
        })
    }

    /// Show the panel near a position (e.g., near the tray)
    pub fn show(&mut self, x: i32, y: i32) -> Result<()> {
        // Always recreate window to ensure fresh state
        if self.window.is_some() {
            // Destroy old window
            self.window = None;
            self.surface = None;
        }

        // Just refresh the enabled state (fast, no scan)
        if let Some(ref mut network) = self.network {
            let _ = network.update_state();
        }

        self.create_window(x, y)?;

        if let Some(ref window) = self.window {
            window.map()?;
            self.conn.flush()?;
            info!("Panel window {} mapped at ({}, {})", window.id(), self.pos_x, self.pos_y);

            // Record show time for FocusOut grace period
            self.show_time = Some(std::time::Instant::now());

            // Save current focus before taking it
            if let Ok(focus_reply) = self.conn.inner().get_input_focus()?.reply() {
                if focus_reply.focus != x11rb::NONE && focus_reply.focus != window.id() {
                    self.previous_focus = Some(focus_reply.focus);
                    debug!("Saved previous focus: {}", focus_reply.focus);
                }
            }

            // Set input focus so we receive keyboard events
            self.conn.inner().set_input_focus(
                x11rb::protocol::xproto::InputFocus::PARENT,
                window.id(),
                CURRENT_TIME,
            )?.check()?;

            // Grab pointer to detect clicks outside the panel
            // Retry a few times in case another app (e.g., garbar) has the implicit button grab
            let mut grab_success = false;
            for attempt in 0..5 {
                if attempt > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                match self.conn.inner().grab_pointer(
                    false,  // owner_events - false to get all events to grab window
                    window.id(),
                    EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                    x11rb::NONE,  // confine_to - don't confine
                    x11rb::NONE,  // cursor - use default
                    CURRENT_TIME,
                ) {
                    Ok(cookie) => {
                        if let Ok(reply) = cookie.reply() {
                            use x11rb::protocol::xproto::GrabStatus;
                            debug!("Pointer grab attempt {}: {:?}", attempt + 1, reply.status);
                            if reply.status == GrabStatus::SUCCESS {
                                grab_success = true;
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to grab pointer: {}", e);
                        break;
                    }
                }
            }
            if !grab_success {
                debug!("Pointer grab failed after retries, click-outside-to-close may not work");
            }
            self.conn.flush()?;
        }

        self.visible = true;
        Ok(())
    }

    /// Hide the panel
    pub fn hide(&mut self) -> Result<()> {
        // Ungrab pointer first
        let _ = self.conn.inner().ungrab_pointer(CURRENT_TIME);

        if let Some(ref window) = self.window {
            window.unmap()?;
            self.conn.flush()?;
        }

        // Restore focus to previous window
        if let Some(prev_focus) = self.previous_focus.take() {
            debug!("Restoring focus to window {}", prev_focus);
            let _ = self.conn.inner().set_input_focus(
                x11rb::protocol::xproto::InputFocus::PARENT,
                prev_focus,
                CURRENT_TIME,
            );
            self.conn.flush()?;
        }

        self.visible = false;
        debug!("Panel hidden");
        Ok(())
    }

    /// Toggle panel visibility
    pub fn toggle(&mut self, x: i32, y: i32) -> Result<()> {
        if self.visible {
            self.hide()
        } else {
            self.show(x, y)
        }
    }

    /// Calculate and update panel height based on expanded section
    fn update_panel_height(&mut self) -> Result<()> {
        const LIST_ITEM_HEIGHT: u32 = 32;
        const WIFI_MAX_VISIBLE: u32 = 3;
        const BLUETOOTH_MAX_VISIBLE: u32 = 5;
        const SCROLL_INDICATOR_HEIGHT: u32 = 16;
        const HINT_HEIGHT: u32 = 20;
        const PADDING: u32 = 8;

        let expansion_height = match self.expanded {
            ExpandedSection::None => 0,
            ExpandedSection::WiFi => {
                let item_count = self.network.as_ref()
                    .map(|n| n.access_points().len())
                    .unwrap_or(0) as u32;
                // At least show space for "No networks" message
                let visible_items = item_count.min(WIFI_MAX_VISIBLE).max(1);
                let mut height = visible_items * LIST_ITEM_HEIGHT;
                // Add space for scroll indicators if list is scrollable
                if item_count > WIFI_MAX_VISIBLE {
                    height += SCROLL_INDICATOR_HEIGHT * 2; // up + down indicators
                }
                height + HINT_HEIGHT + PADDING
            }
            ExpandedSection::Bluetooth => {
                let item_count = self.bluetooth.as_ref()
                    .map(|b| b.devices().len())
                    .unwrap_or(0) as u32;
                let visible_items = item_count.min(BLUETOOTH_MAX_VISIBLE).max(1);
                let mut height = visible_items * LIST_ITEM_HEIGHT;
                if item_count > BLUETOOTH_MAX_VISIBLE {
                    height += SCROLL_INDICATOR_HEIGHT * 2;
                }
                height + PADDING
            }
        };

        let new_height = self.base_height + expansion_height;

        if new_height != self.height {
            self.height = new_height;
            // Resize the window if it exists
            if let Some(ref mut window) = self.window {
                window.resize(self.width, self.height)?;
                self.conn.flush()?;
                debug!("Panel resized to {}x{}", self.width, self.height);
            }
            // Recreate surface at new size
            self.surface = Some(ImageSurface::create(Format::ARgb32, self.width as i32, self.height as i32)?);
            // Re-render
            self.render()?;
        }

        Ok(())
    }

    /// Check if panel is visible
    /// Returns true only if visible flag is set AND window exists
    pub fn is_visible(&self) -> bool {
        self.visible && self.window.is_some()
    }

    /// Query monitors using RandR
    fn query_monitors(&self) -> Vec<MonitorInfo> {
        let mut monitors = Vec::new();

        // Try to get monitor info via RandR
        if let Ok(resources) = self.conn.inner().randr_get_screen_resources(self.conn.screen().root) {
            if let Ok(res) = resources.reply() {
                for &crtc in &res.crtcs {
                    if let Ok(crtc_info) = self.conn.inner().randr_get_crtc_info(crtc, res.config_timestamp) {
                        if let Ok(info) = crtc_info.reply() {
                            if info.width > 0 && info.height > 0 {
                                monitors.push(MonitorInfo {
                                    x: info.x as i32,
                                    y: info.y as i32,
                                    width: info.width as u32,
                                    _height: info.height as u32,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Fallback to full screen if no monitors found
        if monitors.is_empty() {
            let screen = self.conn.screen();
            monitors.push(MonitorInfo {
                x: 0,
                y: 0,
                width: screen.width_in_pixels as u32,
                _height: screen.height_in_pixels as u32,
            });
        }

        monitors
    }

    /// Find which monitor contains a point
    fn find_monitor_at(&self, x: i32, y: i32) -> Option<MonitorInfo> {
        let monitors = self.query_monitors();
        for mon in monitors {
            if x >= mon.x && x < mon.x + mon.width as i32 {
                return Some(mon);
            }
        }
        None
    }

    /// Create the popup window
    fn create_window(&mut self, x: i32, y: i32) -> Result<()> {
        // Get screen dimensions for positioning
        let screen = self.conn.screen();
        let screen_width = screen.width_in_pixels as i32;
        let screen_height = screen.height_in_pixels as i32;

        info!("Screen: {}x{}, click position: ({}, {})", screen_width, screen_height, x, y);

        // Find which monitor the click is on
        let monitor = self.find_monitor_at(x, y);

        // Calculate panel position based on click coordinates
        // If coordinates are 0,0 (default from CLI), use top-right corner
        let (panel_x, panel_y) = if x == 0 && y == 0 {
            // Default: top-right corner with margin, below a typical bar
            let px = screen_width - self.width as i32 - 16;
            let py = 40; // Below top bar
            (px, py)
        } else if let Some(mon) = monitor {
            // Align panel's right edge to monitor's right edge with small margin
            let monitor_right = mon.x + mon.width as i32;
            let px = monitor_right - self.width as i32 - 8;

            // Position below click for top bar, above for bottom bar
            let py = if y < mon.y + 100 {
                // Click near top of monitor - show panel below with small gap
                y + 8
            } else if y > screen_height - 100 {
                // Click near bottom - show panel above
                y - self.height as i32 - 8
            } else {
                // Click in middle - show panel below
                y + 8
            };

            // Ensure panel stays on screen vertically
            let py = py.max(8).min(screen_height - self.height as i32 - 8);

            (px, py)
        } else {
            // Fallback: align to screen right edge
            let px = screen_width - self.width as i32 - 8;
            let py = y + 8;
            (px.max(8), py.max(8).min(screen_height - self.height as i32 - 8))
        };

        self.pos_x = panel_x;
        self.pos_y = panel_y;

        // Use override_redirect to bypass WM positioning entirely
        // This ensures the panel appears exactly where we want it
        let window = Window::create(
            self.conn.clone(),
            WindowConfig::default()
                .title("gartray Quick Settings")
                .class("gartray-panel")
                .size(self.width, self.height)
                .position(panel_x, panel_y)
                .override_redirect(true)
                .background(0xFF1e1e22) // Dark background
                .map_on_create(false),
        )?;

        // Request focus, exposure, button, and motion events for interactivity
        self.conn.inner().change_window_attributes(
            window.id(),
            &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                .event_mask(
                    EventMask::FOCUS_CHANGE
                        | EventMask::KEY_PRESS
                        | EventMask::EXPOSURE
                        | EventMask::BUTTON_PRESS
                        | EventMask::BUTTON_RELEASE
                        | EventMask::POINTER_MOTION  // For hover effects
                        | EventMask::BUTTON1_MOTION  // For slider dragging
                ),
        )?;

        // Set window type to POPUP_MENU to prevent picom from applying inactive transparency
        let wm_type = self.conn.inner().intern_atom(false, b"_NET_WM_WINDOW_TYPE")?.reply()?.atom;
        let popup_type = self.conn.inner().intern_atom(false, b"_NET_WM_WINDOW_TYPE_POPUP_MENU")?.reply()?.atom;
        self.conn.inner().change_property32(
            PropMode::REPLACE,
            window.id(),
            wm_type,
            AtomEnum::ATOM,
            &[popup_type],
        )?;

        info!("Created panel window {} ({}x{}) at ({}, {})",
              window.id(), self.width, self.height, panel_x, panel_y);

        self.window = Some(window);
        self.surface = Some(
            ImageSurface::create(Format::ARgb32, self.width as i32, self.height as i32)
                .map_err(|e| anyhow::anyhow!("Failed to create surface: {}", e))?
        );

        Ok(())
    }

    /// Render the panel content
    pub fn render(&mut self) -> Result<()> {
        // Clear hit test regions
        self.slider_rows.clear();
        self.toggle_buttons.clear();

        // Render to surface in a separate scope so context is dropped before copy_to_window
        {
            let surface = self.surface.as_ref()
                .ok_or_else(|| anyhow::anyhow!("No surface"))?;

            let ctx = CairoContext::new(surface)
                .map_err(|e| anyhow::anyhow!("Failed to create context: {}", e))?;

            // Clear with background
            let (r, g, b, a) = self.bg_color;
            ctx.set_source_rgba(r, g, b, a);
            ctx.paint().map_err(|e| anyhow::anyhow!("Paint failed: {}", e))?;

            // Draw rounded rectangle background
            let radius = 12.0;
            self.draw_rounded_rect(&ctx, 0.0, 0.0, self.width as f64, self.height as f64, radius);
            ctx.set_source_rgba(r, g, b, a);
            ctx.fill().map_err(|e| anyhow::anyhow!("Fill failed: {}", e))?;

            // Draw border
            self.draw_rounded_rect(&ctx, 1.0, 1.0, self.width as f64 - 2.0, self.height as f64 - 2.0, radius - 1.0);
            ctx.set_source_rgba(0.4, 0.4, 0.45, 1.0);
            ctx.set_line_width(1.5);
            ctx.stroke().map_err(|e| anyhow::anyhow!("Stroke failed: {}", e))?;

            // Draw header
            ctx.set_source_rgba(1.0, 1.0, 1.0, 0.95);
            ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
            ctx.set_font_size(16.0);
            ctx.move_to(20.0, 32.0);
            ctx.show_text("Quick Settings").map_err(|e| anyhow::anyhow!("Text failed: {}", e))?;

            // Draw separator line
            ctx.set_source_rgba(0.3, 0.3, 0.35, 1.0);
            ctx.move_to(16.0, 45.0);
            ctx.line_to(self.width as f64 - 16.0, 45.0);
            ctx.stroke().ok();

            // === Toggle Button Grid ===
            let grid_y = 55.0;
            let btn_width = (self.width as f64 - 48.0) / 2.0;  // 2 columns with padding
            let btn_height = 60.0;
            let btn_spacing = 8.0;

            // Get current states from modules
            let wifi_active = self.network.as_ref().map(|n| n.is_wifi_enabled()).unwrap_or(false);
            let bt_active = self.bluetooth.as_ref().map(|b| b.is_powered()).unwrap_or(false);
            let dnd_active = self.dnd.as_ref().map(|d| d.is_enabled()).unwrap_or(false);

            // Helper to check if a button is hovered - clone to avoid borrow conflicts
            let hovered_button = self.hovered_button.clone();
            let is_hovered = |name: &str| hovered_button.as_ref().map(|h| h == name).unwrap_or(false);

            // Row 1: WiFi, Bluetooth
            let wifi_label = if wifi_active {
                self.network.as_ref()
                    .and_then(|n| n.connected_ssid())
                    .map(|s| if s.chars().count() > 10 { format!("{}...", s.chars().take(8).collect::<String>()) } else { s.to_string() })
                    .unwrap_or_else(|| "WiFi".to_string())
            } else {
                "WiFi Off".to_string()
            };
            self.draw_toggle_button(&ctx, &wifi_label, "wifi", wifi_active, is_hovered("wifi"), 16.0, grid_y, btn_width, btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "wifi".to_string(), x: 16.0, y: grid_y, width: btn_width, height: btn_height, active: wifi_active,
            });

            // Bluetooth button label - show connected device name if any
            let bt_label = if bt_active {
                self.bluetooth.as_ref()
                    .and_then(|b| b.devices().iter().find(|d| d.connected))
                    .map(|d| if d.name.chars().count() > 10 { format!("{}...", d.name.chars().take(8).collect::<String>()) } else { d.name.clone() })
                    .unwrap_or_else(|| "Bluetooth".to_string())
            } else {
                "BT Off".to_string()
            };
            self.draw_toggle_button(&ctx, &bt_label, "bluetooth", bt_active, is_hovered("bluetooth"), 16.0 + btn_width + btn_spacing, grid_y, btn_width, btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "bluetooth".to_string(), x: 16.0 + btn_width + btn_spacing, y: grid_y, width: btn_width, height: btn_height, active: bt_active,
            });

            // Row 2: Battery (status), Do Not Disturb
            let row2_y = grid_y + btn_height + btn_spacing;
            let battery_text = if let Some(ref bat) = self.battery {
                let state = bat.state();
                format!("{:.0}%", state.percentage)
            } else {
                "N/A".to_string()
            };
            self.draw_toggle_button(&ctx, &battery_text, "battery", false, is_hovered("battery"), 16.0, row2_y, btn_width, btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "battery".to_string(), x: 16.0, y: row2_y, width: btn_width, height: btn_height, active: false,
            });

            self.draw_toggle_button(&ctx, "DND", "dnd", dnd_active, is_hovered("dnd"), 16.0 + btn_width + btn_spacing, row2_y, btn_width, btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "dnd".to_string(), x: 16.0 + btn_width + btn_spacing, y: row2_y, width: btn_width, height: btn_height, active: dnd_active,
            });

            // === Expanded Section (WiFi or Bluetooth picker) ===
            let mut expansion_height = 0.0;
            let expanded_y = row2_y + btn_height + btn_spacing;

            match self.expanded {
                ExpandedSection::WiFi => {
                    expansion_height = self.render_wifi_list(&ctx, expanded_y)?;
                }
                ExpandedSection::Bluetooth => {
                    expansion_height = self.render_bluetooth_list(&ctx, expanded_y)?;
                }
                ExpandedSection::None => {}
            }

            // === Power Button Row (4 smaller buttons) ===
            let power_y = expanded_y + expansion_height + if expansion_height > 0.0 { 8.0 } else { 0.0 };
            let power_btn_width = (self.width as f64 - 56.0) / 4.0;  // 4 columns
            let power_btn_height = 50.0;
            let power_spacing = 8.0;

            // Shutdown
            let shutdown_x = 16.0;
            self.draw_toggle_button(&ctx, "Shutdown", "power", false, is_hovered("shutdown"), shutdown_x, power_y, power_btn_width, power_btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "shutdown".to_string(), x: shutdown_x, y: power_y, width: power_btn_width, height: power_btn_height, active: false,
            });

            // Restart
            let restart_x = shutdown_x + power_btn_width + power_spacing;
            self.draw_toggle_button(&ctx, "Restart", "restart", false, is_hovered("restart"), restart_x, power_y, power_btn_width, power_btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "restart".to_string(), x: restart_x, y: power_y, width: power_btn_width, height: power_btn_height, active: false,
            });

            // Logout
            let logout_x = restart_x + power_btn_width + power_spacing;
            self.draw_toggle_button(&ctx, "Logout", "logout", false, is_hovered("logout"), logout_x, power_y, power_btn_width, power_btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "logout".to_string(), x: logout_x, y: power_y, width: power_btn_width, height: power_btn_height, active: false,
            });

            // Hibernate/Sleep
            let hibernate_x = logout_x + power_btn_width + power_spacing;
            self.draw_toggle_button(&ctx, "Sleep", "hibernate", false, is_hovered("hibernate"), hibernate_x, power_y, power_btn_width, power_btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "hibernate".to_string(), x: hibernate_x, y: power_y, width: power_btn_width, height: power_btn_height, active: false,
            });

            // === Volume Slider ===
            let slider_y = power_y + power_btn_height + 12.0;
            let slider_x = 70.0;
            let slider_width = self.width as f64 - slider_x - 24.0;
            let slider_height = 42.0;

            // Get volume value (use drag value if dragging)
            let (volume_value, is_muted) = if let Some(ref vol) = self.volume {
                let state = vol.state();
                let v = if self.dragging.as_ref() == Some(&"volume".to_string()) {
                    self.drag_value
                } else {
                    state.volume
                };
                (v, state.muted)
            } else {
                (0.5, false)
            };

            self.draw_volume_slider(&ctx, volume_value, is_muted, 16.0, slider_y, slider_x, slider_width, slider_height)?;
            self.slider_rows.push(SliderRow {
                name: "volume".to_string(),
                y: slider_y,
                height: slider_height,
                slider_x,
                slider_width,
                icon_x: 16.0,
                icon_width: 45.0,
            });

            // === Brightness Slider ===
            let brightness_y = slider_y + slider_height + 8.0;
            let brightness_value = if let Some(ref bright) = self.brightness {
                if self.dragging.as_ref() == Some(&"brightness".to_string()) {
                    self.drag_value
                } else {
                    bright.state().brightness
                }
            } else {
                0.5
            };

            self.draw_brightness_slider(&ctx, brightness_value, 16.0, brightness_y, slider_x, slider_width, slider_height)?;
            self.slider_rows.push(SliderRow {
                name: "brightness".to_string(),
                y: brightness_y,
                height: slider_height,
                slider_x,
                slider_width,
                icon_x: 16.0,
                icon_width: 45.0,
            });

            // ctx is dropped here, releasing the surface lock
        }

        // Copy to window (now surface is free)
        self.copy_to_window()?;

        debug!("Panel rendered successfully");
        Ok(())
    }

    /// Draw a toggle button with Cairo-rendered icon
    fn draw_toggle_button(
        &self,
        ctx: &CairoContext,
        label: &str,
        icon_type: &str,
        active: bool,
        hovered: bool,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<()> {
        // Background
        self.draw_rounded_rect(ctx, x, y, width, height, 8.0);
        if active {
            ctx.set_source_rgba(0.38, 0.68, 0.93, 0.3);  // Cyan tint when active
        } else if hovered {
            ctx.set_source_rgba(0.28, 0.28, 0.32, 1.0);  // Lighter when hovered
        } else {
            ctx.set_source_rgba(0.18, 0.18, 0.2, 1.0);
        }
        ctx.fill().ok();

        // Border when active or hovered
        if active || hovered {
            self.draw_rounded_rect(ctx, x + 1.0, y + 1.0, width - 2.0, height - 2.0, 7.0);
            if active {
                ctx.set_source_rgba(0.38, 0.68, 0.93, 0.8);
            } else {
                ctx.set_source_rgba(0.45, 0.45, 0.5, 0.6);  // Subtle border on hover
            }
            ctx.set_line_width(2.0);
            ctx.stroke().ok();
        }

        // Draw icon using Cairo primitives
        let icon_x = x + width / 2.0;
        let icon_y = y + 25.0;
        let alpha = if active || hovered { 1.0 } else { 0.7 };
        self.draw_icon(ctx, icon_type, icon_x, icon_y, alpha, active);

        // Label
        ctx.set_source_rgba(1.0, 1.0, 1.0, alpha);
        ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        ctx.set_font_size(11.0);
        ctx.move_to(x + width / 2.0 - (label.len() as f64 * 3.0), y + height - 10.0);
        ctx.show_text(label).ok();

        Ok(())
    }

    /// Draw an icon using Cairo primitives
    fn draw_icon(&self, ctx: &CairoContext, icon_type: &str, cx: f64, cy: f64, alpha: f64, active: bool) {
        ctx.set_source_rgba(1.0, 1.0, 1.0, alpha);
        ctx.set_line_width(2.0);
        ctx.set_line_cap(cairo::LineCap::Round);
        ctx.set_line_join(cairo::LineJoin::Round);

        match icon_type {
            "wifi" => self.draw_wifi_icon(ctx, cx, cy),
            "bluetooth" => self.draw_bluetooth_icon(ctx, cx, cy),
            "battery" => self.draw_battery_icon(ctx, cx, cy),
            "dnd" => self.draw_dnd_icon(ctx, cx, cy, active),
            "power" => self.draw_power_icon(ctx, cx, cy),
            "restart" => self.draw_restart_icon(ctx, cx, cy),
            "logout" => self.draw_logout_icon(ctx, cx, cy),
            "hibernate" => self.draw_hibernate_icon(ctx, cx, cy),
            _ => {}
        }
    }

    /// Draw WiFi signal bars icon
    fn draw_wifi_icon(&self, ctx: &CairoContext, cx: f64, cy: f64) {
        let pi = std::f64::consts::PI;
        // Draw concentric arcs for signal strength
        for i in 0..3 {
            let radius = 6.0 + (i as f64 * 5.0);
            ctx.arc(cx, cy + 8.0, radius, -pi * 0.75, -pi * 0.25);
            ctx.stroke().ok();
        }
        // Center dot
        ctx.arc(cx, cy + 8.0, 2.0, 0.0, 2.0 * pi);
        ctx.fill().ok();
    }

    /// Draw Bluetooth icon (runic B)
    fn draw_bluetooth_icon(&self, ctx: &CairoContext, cx: f64, cy: f64) {
        // Draw the Bluetooth rune shape
        ctx.move_to(cx, cy - 10.0);
        ctx.line_to(cx, cy + 10.0);
        ctx.move_to(cx, cy - 10.0);
        ctx.line_to(cx + 6.0, cy - 4.0);
        ctx.line_to(cx - 6.0, cy + 4.0);
        ctx.move_to(cx, cy + 10.0);
        ctx.line_to(cx + 6.0, cy + 4.0);
        ctx.line_to(cx - 6.0, cy - 4.0);
        ctx.stroke().ok();
    }

    /// Draw battery icon
    fn draw_battery_icon(&self, ctx: &CairoContext, cx: f64, cy: f64) {
        // Battery body
        self.draw_rounded_rect(ctx, cx - 10.0, cy - 6.0, 18.0, 12.0, 2.0);
        ctx.stroke().ok();
        // Battery tip
        ctx.rectangle(cx + 8.0, cy - 3.0, 3.0, 6.0);
        ctx.fill().ok();
        // Fill level (mock 75%)
        ctx.rectangle(cx - 8.0, cy - 4.0, 12.0, 8.0);
        ctx.fill().ok();
    }

    /// Draw Do Not Disturb icon (bell with slash when inactive)
    fn draw_dnd_icon(&self, ctx: &CairoContext, cx: f64, cy: f64, active: bool) {
        let pi = std::f64::consts::PI;
        // Bell outline
        ctx.arc(cx, cy - 2.0, 8.0, pi, 2.0 * pi);
        ctx.line_to(cx + 8.0, cy + 4.0);
        ctx.line_to(cx - 8.0, cy + 4.0);
        ctx.close_path();
        ctx.stroke().ok();
        // Bell clapper
        ctx.arc(cx, cy + 6.0, 2.0, 0.0, 2.0 * pi);
        ctx.fill().ok();
        // Diagonal slash only when DND is OFF (not active)
        if !active {
            ctx.set_source_rgba(0.9, 0.3, 0.3, 1.0);
            ctx.set_line_width(2.5);
            ctx.move_to(cx - 10.0, cy - 8.0);
            ctx.line_to(cx + 10.0, cy + 8.0);
            ctx.stroke().ok();
        }
    }

    /// Draw power (shutdown) icon
    fn draw_power_icon(&self, ctx: &CairoContext, cx: f64, cy: f64) {
        let pi = std::f64::consts::PI;
        // Power circle (open at top)
        ctx.arc(cx, cy, 8.0, pi * 0.3, pi * 2.7);
        ctx.stroke().ok();
        // Vertical line at top
        ctx.move_to(cx, cy - 10.0);
        ctx.line_to(cx, cy - 2.0);
        ctx.stroke().ok();
    }

    /// Draw restart icon (circular arrow)
    fn draw_restart_icon(&self, ctx: &CairoContext, cx: f64, cy: f64) {
        let pi = std::f64::consts::PI;
        // Circular arrow
        ctx.arc(cx, cy, 8.0, pi * 0.5, pi * 2.2);
        ctx.stroke().ok();
        // Arrow head
        ctx.move_to(cx + 8.0, cy);
        ctx.line_to(cx + 4.0, cy - 4.0);
        ctx.move_to(cx + 8.0, cy);
        ctx.line_to(cx + 4.0, cy + 4.0);
        ctx.stroke().ok();
    }

    /// Draw logout icon (door with arrow)
    fn draw_logout_icon(&self, ctx: &CairoContext, cx: f64, cy: f64) {
        // Door frame
        ctx.move_to(cx - 4.0, cy - 10.0);
        ctx.line_to(cx - 8.0, cy - 10.0);
        ctx.line_to(cx - 8.0, cy + 10.0);
        ctx.line_to(cx - 4.0, cy + 10.0);
        ctx.stroke().ok();
        // Arrow pointing out
        ctx.move_to(cx - 2.0, cy);
        ctx.line_to(cx + 10.0, cy);
        ctx.stroke().ok();
        // Arrow head
        ctx.move_to(cx + 6.0, cy - 4.0);
        ctx.line_to(cx + 10.0, cy);
        ctx.line_to(cx + 6.0, cy + 4.0);
        ctx.stroke().ok();
    }

    /// Draw hibernate icon (crescent moon)
    fn draw_hibernate_icon(&self, ctx: &CairoContext, cx: f64, cy: f64) {
        let pi = std::f64::consts::PI;
        // Crescent moon using two arcs
        ctx.arc(cx, cy, 9.0, pi * 0.3, pi * 1.7);
        ctx.arc_negative(cx + 5.0, cy, 7.0, pi * 1.5, pi * 0.5);
        ctx.close_path();
        ctx.fill().ok();
    }

    /// Draw the volume slider
    fn draw_volume_slider(
        &self,
        ctx: &CairoContext,
        value: f64,
        is_muted: bool,
        x: f64,
        y: f64,
        slider_x: f64,
        slider_width: f64,
        height: f64,
    ) -> Result<()> {
        let width = self.width as f64 - 32.0;

        // Background
        self.draw_rounded_rect(ctx, x, y, width, height, 8.0);
        ctx.set_source_rgba(0.18, 0.18, 0.2, 1.0);
        ctx.fill().ok();

        // Draw speaker icon with Cairo
        let icon_cx = x + 30.0;
        let icon_cy = y + height / 2.0;
        self.draw_speaker_icon(ctx, icon_cx, icon_cy, is_muted, value);

        // Slider track
        let track_y = y + height / 2.0 - 4.0;
        let track_height = 8.0;
        self.draw_rounded_rect(ctx, slider_x, track_y, slider_width, track_height, 4.0);
        ctx.set_source_rgba(0.25, 0.25, 0.28, 1.0);
        ctx.fill().ok();

        // Slider fill
        let fill_width = (slider_width * value.clamp(0.0, 1.0)).max(8.0);
        self.draw_rounded_rect(ctx, slider_x, track_y, fill_width, track_height, 4.0);
        if is_muted {
            ctx.set_source_rgba(0.4, 0.4, 0.45, 1.0);
        } else {
            ctx.set_source_rgba(0.38, 0.68, 0.93, 1.0);
        }
        ctx.fill().ok();

        // Slider knob
        let knob_x = slider_x + fill_width - 8.0;
        let knob_y = y + height / 2.0;
        ctx.arc(knob_x.max(slider_x), knob_y, 10.0, 0.0, 2.0 * std::f64::consts::PI);
        ctx.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        ctx.fill().ok();

        Ok(())
    }

    /// Draw speaker icon with sound waves
    fn draw_speaker_icon(&self, ctx: &CairoContext, cx: f64, cy: f64, muted: bool, volume: f64) {
        ctx.set_line_width(2.0);
        ctx.set_line_cap(cairo::LineCap::Round);

        if muted {
            ctx.set_source_rgba(0.5, 0.5, 0.55, 1.0);
        } else {
            ctx.set_source_rgba(0.9, 0.9, 0.95, 1.0);
        }

        // Speaker body (trapezoid + rectangle)
        ctx.move_to(cx - 8.0, cy - 4.0);
        ctx.line_to(cx - 4.0, cy - 4.0);
        ctx.line_to(cx, cy - 8.0);
        ctx.line_to(cx, cy + 8.0);
        ctx.line_to(cx - 4.0, cy + 4.0);
        ctx.line_to(cx - 8.0, cy + 4.0);
        ctx.close_path();
        ctx.fill().ok();

        if muted {
            // Draw X for mute
            ctx.set_source_rgba(0.9, 0.3, 0.3, 1.0);
            ctx.move_to(cx + 4.0, cy - 5.0);
            ctx.line_to(cx + 12.0, cy + 5.0);
            ctx.move_to(cx + 12.0, cy - 5.0);
            ctx.line_to(cx + 4.0, cy + 5.0);
            ctx.stroke().ok();
        } else {
            // Sound waves based on volume level
            let pi = std::f64::consts::PI;
            ctx.set_source_rgba(0.9, 0.9, 0.95, 1.0);

            if volume > 0.0 {
                ctx.arc(cx + 4.0, cy, 4.0, -pi * 0.4, pi * 0.4);
                ctx.stroke().ok();
            }
            if volume > 0.33 {
                ctx.arc(cx + 4.0, cy, 8.0, -pi * 0.4, pi * 0.4);
                ctx.stroke().ok();
            }
            if volume > 0.66 {
                ctx.arc(cx + 4.0, cy, 12.0, -pi * 0.4, pi * 0.4);
                ctx.stroke().ok();
            }
        }
    }

    /// Draw the brightness slider
    fn draw_brightness_slider(
        &self,
        ctx: &CairoContext,
        value: f64,
        x: f64,
        y: f64,
        slider_x: f64,
        slider_width: f64,
        height: f64,
    ) -> Result<()> {
        let width = self.width as f64 - 32.0;

        // Background
        self.draw_rounded_rect(ctx, x, y, width, height, 8.0);
        ctx.set_source_rgba(0.18, 0.18, 0.2, 1.0);
        ctx.fill().ok();

        // Draw sun icon with Cairo
        let icon_cx = x + 30.0;
        let icon_cy = y + height / 2.0;
        self.draw_sun_icon(ctx, icon_cx, icon_cy, value);

        // Slider track
        let track_y = y + height / 2.0 - 4.0;
        let track_height = 8.0;
        self.draw_rounded_rect(ctx, slider_x, track_y, slider_width, track_height, 4.0);
        ctx.set_source_rgba(0.25, 0.25, 0.28, 1.0);
        ctx.fill().ok();

        // Slider fill
        let fill_width = (slider_width * value.clamp(0.0, 1.0)).max(8.0);
        self.draw_rounded_rect(ctx, slider_x, track_y, fill_width, track_height, 4.0);
        ctx.set_source_rgba(0.95, 0.78, 0.28, 1.0);  // Warm yellow for brightness
        ctx.fill().ok();

        // Slider knob
        let knob_x = slider_x + fill_width - 8.0;
        let knob_y = y + height / 2.0;
        ctx.arc(knob_x.max(slider_x), knob_y, 10.0, 0.0, 2.0 * std::f64::consts::PI);
        ctx.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        ctx.fill().ok();

        Ok(())
    }

    /// Draw sun icon for brightness
    fn draw_sun_icon(&self, ctx: &CairoContext, cx: f64, cy: f64, brightness: f64) {
        let pi = std::f64::consts::PI;
        ctx.set_line_width(2.0);
        ctx.set_line_cap(cairo::LineCap::Round);

        // Brightness-based alpha (dimmer icon at low brightness)
        let alpha = 0.5 + brightness * 0.5;
        ctx.set_source_rgba(0.95, 0.85, 0.4, alpha);

        // Sun center circle
        ctx.arc(cx, cy, 5.0, 0.0, 2.0 * pi);
        ctx.fill().ok();

        // Sun rays
        ctx.set_source_rgba(0.95, 0.85, 0.4, alpha);
        let ray_count = 8;
        let inner_radius = 7.0;
        let outer_radius = 11.0;
        for i in 0..ray_count {
            let angle = (i as f64 / ray_count as f64) * 2.0 * pi;
            let x1 = cx + inner_radius * angle.cos();
            let y1 = cy + inner_radius * angle.sin();
            let x2 = cx + outer_radius * angle.cos();
            let y2 = cy + outer_radius * angle.sin();
            ctx.move_to(x1, y1);
            ctx.line_to(x2, y2);
        }
        ctx.stroke().ok();
    }

    /// Render WiFi network list, returns height used
    fn render_wifi_list(&mut self, ctx: &CairoContext, start_y: f64) -> Result<f64> {
        const ITEM_HEIGHT: f64 = 32.0;
        const MAX_VISIBLE: usize = 3;
        const PADDING: f64 = 8.0;
        const SCROLL_INDICATOR_HEIGHT: f64 = 16.0;

        self.list_items.clear();

        // If password entry is active, show password UI instead of network list
        if let Some(ref ssid) = self.password_entry_ssid.clone() {
            return self.render_password_entry(ctx, start_y, &ssid);
        }

        // Check if scanning
        let is_scanning = self.network.as_ref().map(|n| n.is_scanning()).unwrap_or(false);

        let networks: Vec<_> = self.network.as_ref()
            .map(|n| n.access_points().to_vec())
            .unwrap_or_default();

        if is_scanning || networks.is_empty() {
            ctx.set_source_rgba(0.6, 0.6, 0.6, 1.0);
            ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
            ctx.set_font_size(12.0);
            ctx.move_to(16.0 + PADDING, start_y + ITEM_HEIGHT / 2.0 + 4.0);
            if is_scanning {
                ctx.show_text("Scanning for networks...").ok();
            } else {
                ctx.show_text("No WiFi networks found").ok();
            }
            return Ok(ITEM_HEIGHT + PADDING);
        }

        // Clamp scroll offset to valid range
        let max_scroll = networks.len().saturating_sub(MAX_VISIBLE);
        if self.wifi_scroll_offset > max_scroll {
            self.wifi_scroll_offset = max_scroll;
        }

        let can_scroll_up = self.wifi_scroll_offset > 0;
        let can_scroll_down = self.wifi_scroll_offset < max_scroll;

        let mut y = start_y;

        // Scroll up indicator
        if can_scroll_up {
            ctx.set_source_rgba(0.5, 0.5, 0.5, 1.0);
            ctx.set_font_size(10.0);
            let indicator_text = format!("^ {} more above", self.wifi_scroll_offset);
            ctx.move_to(16.0 + PADDING, y + 12.0);
            ctx.show_text(&indicator_text).ok();
            y += SCROLL_INDICATOR_HEIGHT;
        }

        // Render visible networks
        let visible_networks = networks.iter()
            .skip(self.wifi_scroll_offset)
            .take(MAX_VISIBLE);

        for ap in visible_networks {
            let item_y = y;

            // Background for connected item
            if ap.connected {
                ctx.set_source_rgba(0.2, 0.4, 0.6, 0.3);
                self.draw_rounded_rect(ctx, 16.0, item_y, self.width as f64 - 32.0, ITEM_HEIGHT - 2.0, 4.0);
                ctx.fill().ok();
            }

            // Connected indicator
            if ap.connected {
                ctx.set_source_rgba(0.3, 0.7, 0.4, 1.0);
                ctx.arc(24.0, item_y + ITEM_HEIGHT / 2.0, 4.0, 0.0, 2.0 * std::f64::consts::PI);
                ctx.fill().ok();
            }

            // SSID name
            ctx.set_source_rgba(0.9, 0.9, 0.9, 1.0);
            ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
            ctx.set_font_size(13.0);
            let ssid_x = if ap.connected { 36.0 } else { 24.0 };
            let ssid = if ap.ssid.chars().count() > 20 {
                format!("{}...", ap.ssid.chars().take(18).collect::<String>())
            } else {
                ap.ssid.clone()
            };
            ctx.move_to(ssid_x, item_y + ITEM_HEIGHT / 2.0 + 4.0);
            ctx.show_text(&ssid).ok();

            // Signal strength bars (5 bars, anchored at bottom)
            let bar_x = self.width as f64 - 80.0;
            let bar_width = 3.0;
            let bar_spacing = 2.0;
            let max_bar_height = 14.0;
            let strength = ap.strength as f64 / 100.0;
            // Vertical base for all bars (bottom-aligned)
            let bar_base_y = item_y + (ITEM_HEIGHT + max_bar_height) / 2.0;

            for bar in 0..5 {
                let bar_height = max_bar_height * (bar as f64 + 1.0) / 5.0;
                let filled = strength >= (bar as f64 + 1.0) / 5.0;

                if filled {
                    ctx.set_source_rgba(0.3, 0.7, 0.4, 1.0);
                } else {
                    ctx.set_source_rgba(0.3, 0.3, 0.3, 1.0);
                }

                let bx = bar_x + bar as f64 * (bar_width + bar_spacing);
                let by = bar_base_y - bar_height; // Anchor at bottom
                ctx.rectangle(bx, by, bar_width, bar_height);
                ctx.fill().ok();
            }

            // Security label
            ctx.set_source_rgba(0.6, 0.6, 0.6, 1.0);
            ctx.set_font_size(10.0);
            ctx.move_to(self.width as f64 - 45.0, item_y + ITEM_HEIGHT / 2.0 + 3.0);
            ctx.show_text(&ap.security).ok();

            // Track list item for click handling
            self.list_items.push(ListItem {
                id: ap.ssid.clone(),
                y: item_y,
                height: ITEM_HEIGHT,
            });

            y += ITEM_HEIGHT;
        }

        // Scroll down indicator
        if can_scroll_down {
            let remaining = networks.len() - self.wifi_scroll_offset - MAX_VISIBLE;
            ctx.set_source_rgba(0.5, 0.5, 0.5, 1.0);
            ctx.set_font_size(10.0);
            let indicator_text = format!("v {} more below", remaining);
            ctx.move_to(16.0 + PADDING, y + 12.0);
            ctx.show_text(&indicator_text).ok();
            y += SCROLL_INDICATOR_HEIGHT;
        }

        // Hint for hold-to-toggle
        ctx.set_source_rgba(0.45, 0.45, 0.5, 1.0);
        ctx.set_font_size(10.0);
        ctx.move_to(16.0 + PADDING, y + 14.0);
        ctx.show_text("Hold WiFi button to turn off").ok();
        y += 20.0;

        Ok(y - start_y + PADDING)
    }

    /// Render password entry UI for WiFi connection
    fn render_password_entry(&self, ctx: &CairoContext, start_y: f64, ssid: &str) -> Result<f64> {
        const PADDING: f64 = 8.0;
        let mut y = start_y;

        // Network name header
        ctx.set_source_rgba(0.9, 0.9, 0.9, 1.0);
        ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        ctx.set_font_size(13.0);
        ctx.move_to(16.0 + PADDING, y + 16.0);
        let header = format!("Connect to {}", ssid);
        ctx.show_text(&header).ok();
        y += 28.0;

        // "Password:" label
        ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        ctx.set_source_rgba(0.7, 0.7, 0.7, 1.0);
        ctx.set_font_size(11.0);
        ctx.move_to(16.0 + PADDING, y + 12.0);
        ctx.show_text("Password:").ok();
        y += 18.0;

        // Password input field
        let input_x = 16.0 + PADDING;
        let input_width = self.width as f64 - 32.0 - PADDING * 2.0;
        let input_height = 32.0;

        // Input background
        ctx.set_source_rgba(0.15, 0.15, 0.18, 1.0);
        self.draw_rounded_rect(ctx, input_x, y, input_width, input_height, 4.0);
        ctx.fill().ok();

        // Input border
        ctx.set_source_rgba(0.3, 0.5, 0.7, 1.0);
        self.draw_rounded_rect(ctx, input_x, y, input_width, input_height, 4.0);
        ctx.stroke().ok();

        // Password text (masked with asterisks)
        ctx.set_source_rgba(0.9, 0.9, 0.9, 1.0);
        ctx.set_font_size(14.0);
        let masked: String = "*".repeat(self.password_text.len());
        ctx.move_to(input_x + 8.0, y + input_height / 2.0 + 5.0);
        ctx.show_text(&masked).ok();

        // Cursor - measure actual text width for accurate positioning
        let text_width = if masked.is_empty() {
            0.0
        } else {
            ctx.text_extents(&masked).map(|e| e.x_advance()).unwrap_or(0.0)
        };
        let cursor_x = input_x + 8.0 + text_width;
        ctx.set_source_rgba(0.9, 0.9, 0.9, 1.0);
        ctx.rectangle(cursor_x, y + 6.0, 2.0, input_height - 12.0);
        ctx.fill().ok();

        y += input_height + 8.0;

        // Hint text
        ctx.set_source_rgba(0.5, 0.5, 0.55, 1.0);
        ctx.set_font_size(10.0);
        ctx.move_to(16.0 + PADDING, y + 10.0);
        ctx.show_text("Press Enter to connect, Escape to cancel").ok();
        y += 20.0;

        Ok(y - start_y + PADDING)
    }

    /// Render Bluetooth device list, returns height used
    fn render_bluetooth_list(&mut self, ctx: &CairoContext, start_y: f64) -> Result<f64> {
        const ITEM_HEIGHT: f64 = 32.0;
        const MAX_VISIBLE: usize = 5;
        const PADDING: f64 = 8.0;
        const SCROLL_INDICATOR_HEIGHT: f64 = 18.0;

        self.list_items.clear();

        let devices: Vec<_> = self.bluetooth.as_ref()
            .map(|b| b.devices().to_vec())
            .unwrap_or_default();

        if devices.is_empty() {
            ctx.set_source_rgba(0.6, 0.6, 0.6, 1.0);
            ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
            ctx.set_font_size(12.0);
            ctx.move_to(16.0 + PADDING, start_y + ITEM_HEIGHT / 2.0 + 4.0);
            ctx.show_text("No Bluetooth devices").ok();
            return Ok(ITEM_HEIGHT + PADDING);
        }

        // Clamp scroll offset to valid range
        let max_scroll = devices.len().saturating_sub(MAX_VISIBLE);
        if self.bluetooth_scroll_offset > max_scroll {
            self.bluetooth_scroll_offset = max_scroll;
        }

        let can_scroll_up = self.bluetooth_scroll_offset > 0;
        let can_scroll_down = self.bluetooth_scroll_offset < max_scroll;

        let mut y = start_y;

        // Scroll up indicator
        if can_scroll_up {
            ctx.set_source_rgba(0.5, 0.5, 0.5, 1.0);
            ctx.set_font_size(10.0);
            let indicator_text = format!("^ {} more above", self.bluetooth_scroll_offset);
            ctx.move_to(16.0 + PADDING, y + 12.0);
            ctx.show_text(&indicator_text).ok();
            y += SCROLL_INDICATOR_HEIGHT;
        }

        // Render visible devices
        let visible_devices = devices.iter()
            .skip(self.bluetooth_scroll_offset)
            .take(MAX_VISIBLE);

        for device in visible_devices {
            let item_y = y;

            // Background for connected item
            if device.connected {
                ctx.set_source_rgba(0.2, 0.4, 0.6, 0.3);
                self.draw_rounded_rect(ctx, 16.0, item_y, self.width as f64 - 32.0, ITEM_HEIGHT - 2.0, 4.0);
                ctx.fill().ok();
            }

            // Connected indicator
            if device.connected {
                ctx.set_source_rgba(0.3, 0.7, 0.4, 1.0);
                ctx.arc(24.0, item_y + ITEM_HEIGHT / 2.0, 4.0, 0.0, 2.0 * std::f64::consts::PI);
                ctx.fill().ok();
            }

            // Device name
            ctx.set_source_rgba(0.9, 0.9, 0.9, 1.0);
            ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
            ctx.set_font_size(13.0);
            let name_x = if device.connected { 36.0 } else { 24.0 };
            let name = if device.name.chars().count() > 20 {
                format!("{}...", device.name.chars().take(18).collect::<String>())
            } else {
                device.name.clone()
            };
            ctx.move_to(name_x, item_y + ITEM_HEIGHT / 2.0 + 4.0);
            ctx.show_text(&name).ok();

            // Status label
            let status = if device.connected {
                "Connected"
            } else if device.paired {
                "Paired"
            } else {
                "Available"
            };
            ctx.set_source_rgba(0.6, 0.6, 0.6, 1.0);
            ctx.set_font_size(10.0);
            ctx.move_to(self.width as f64 - 70.0, item_y + ITEM_HEIGHT / 2.0 + 3.0);
            ctx.show_text(status).ok();

            // Track list item for click handling
            self.list_items.push(ListItem {
                id: device.path.clone(),
                y: item_y,
                height: ITEM_HEIGHT,
            });

            y += ITEM_HEIGHT;
        }

        // Scroll down indicator
        if can_scroll_down {
            let remaining = devices.len() - self.bluetooth_scroll_offset - MAX_VISIBLE;
            ctx.set_source_rgba(0.5, 0.5, 0.5, 1.0);
            ctx.set_font_size(10.0);
            let indicator_text = format!("v {} more below", remaining);
            ctx.move_to(16.0 + PADDING, y + 12.0);
            ctx.show_text(&indicator_text).ok();
            y += SCROLL_INDICATOR_HEIGHT;
        }

        // Hint for hold-to-toggle
        ctx.set_source_rgba(0.45, 0.45, 0.5, 1.0);
        ctx.set_font_size(10.0);
        ctx.move_to(16.0 + PADDING, y + 14.0);
        ctx.show_text("Hold Bluetooth button to turn off").ok();
        y += 20.0;

        Ok(y - start_y + PADDING)
    }

    /// Convert X11 keycode to character
    /// This is a simplified mapping for US QWERTY keyboard
    fn keycode_to_char(&self, keycode: u8, state: u16) -> Option<char> {
        let shift = (state & 1) != 0; // Shift modifier

        // Map X11 keycodes to characters (US QWERTY layout)
        // These keycodes are offset by 8 from Linux input keycodes
        let ch = match keycode {
            // Number row
            10 => if shift { '!' } else { '1' },
            11 => if shift { '@' } else { '2' },
            12 => if shift { '#' } else { '3' },
            13 => if shift { '$' } else { '4' },
            14 => if shift { '%' } else { '5' },
            15 => if shift { '^' } else { '6' },
            16 => if shift { '&' } else { '7' },
            17 => if shift { '*' } else { '8' },
            18 => if shift { '(' } else { '9' },
            19 => if shift { ')' } else { '0' },
            20 => if shift { '_' } else { '-' },
            21 => if shift { '+' } else { '=' },

            // Top row (QWERTY)
            24 => if shift { 'Q' } else { 'q' },
            25 => if shift { 'W' } else { 'w' },
            26 => if shift { 'E' } else { 'e' },
            27 => if shift { 'R' } else { 'r' },
            28 => if shift { 'T' } else { 't' },
            29 => if shift { 'Y' } else { 'y' },
            30 => if shift { 'U' } else { 'u' },
            31 => if shift { 'I' } else { 'i' },
            32 => if shift { 'O' } else { 'o' },
            33 => if shift { 'P' } else { 'p' },
            34 => if shift { '{' } else { '[' },
            35 => if shift { '}' } else { ']' },

            // Home row (ASDF)
            38 => if shift { 'A' } else { 'a' },
            39 => if shift { 'S' } else { 's' },
            40 => if shift { 'D' } else { 'd' },
            41 => if shift { 'F' } else { 'f' },
            42 => if shift { 'G' } else { 'g' },
            43 => if shift { 'H' } else { 'h' },
            44 => if shift { 'J' } else { 'j' },
            45 => if shift { 'K' } else { 'k' },
            46 => if shift { 'L' } else { 'l' },
            47 => if shift { ':' } else { ';' },
            48 => if shift { '"' } else { '\'' },
            51 => if shift { '|' } else { '\\' },

            // Bottom row (ZXCV)
            52 => if shift { 'Z' } else { 'z' },
            53 => if shift { 'X' } else { 'x' },
            54 => if shift { 'C' } else { 'c' },
            55 => if shift { 'V' } else { 'v' },
            56 => if shift { 'B' } else { 'b' },
            57 => if shift { 'N' } else { 'n' },
            58 => if shift { 'M' } else { 'm' },
            59 => if shift { '<' } else { ',' },
            60 => if shift { '>' } else { '.' },
            61 => if shift { '?' } else { '/' },

            // Space
            65 => ' ',

            // Grave/tilde
            49 => if shift { '~' } else { '`' },

            _ => return None,
        };
        Some(ch)
    }

    /// Draw a rounded rectangle path
    fn draw_rounded_rect(&self, ctx: &CairoContext, x: f64, y: f64, w: f64, h: f64, r: f64) {
        let degrees = std::f64::consts::PI / 180.0;
        ctx.new_sub_path();
        ctx.arc(x + w - r, y + r, r, -90.0 * degrees, 0.0 * degrees);
        ctx.arc(x + w - r, y + h - r, r, 0.0 * degrees, 90.0 * degrees);
        ctx.arc(x + r, y + h - r, r, 90.0 * degrees, 180.0 * degrees);
        ctx.arc(x + r, y + r, r, 180.0 * degrees, 270.0 * degrees);
        ctx.close_path();
    }

    /// Copy surface to window
    fn copy_to_window(&mut self) -> Result<()> {
        let window = self.window.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No window"))?;
        let surface = self.surface.as_mut()
            .ok_or_else(|| anyhow::anyhow!("No surface"))?;

        surface.flush();

        let data = {
            let data_ref = surface.data()
                .map_err(|e| anyhow::anyhow!("Failed to get surface data: {}", e))?;
            data_ref.to_vec()
        };

        let gc = self.conn.generate_id()?;
        self.conn.inner().create_gc(gc, window.id(), &Default::default())?;

        self.conn.inner().put_image(
            x11rb::protocol::xproto::ImageFormat::Z_PIXMAP,
            window.id(),
            gc,
            self.width as u16,
            self.height as u16,
            0,
            0,
            0,
            window.depth(),
            &data,
        )?;

        self.conn.inner().free_gc(gc)?;
        self.conn.flush()?;

        Ok(())
    }

    /// Process X11 events for the panel
    pub fn process_events(&mut self) -> Result<bool> {
        // Check for hold threshold while still pressing (fires before release)
        if let Some(press_start) = self.press_start {
            const HOLD_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(500);
            if press_start.elapsed() >= HOLD_THRESHOLD {
                if let Some(btn_name) = self.press_button.take() {
                    self.press_start = None;
                    info!("Hold threshold reached on '{}' - toggling power", btn_name);
                    self.handle_hold_action(&btn_name)?;
                    self.render()?;
                }
            }
        }

        // Periodic WiFi refresh while list is expanded (every 10 seconds)
        if self.expanded == ExpandedSection::WiFi {
            const WIFI_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
            let should_refresh = self.last_wifi_scan
                .map(|last| last.elapsed() >= WIFI_REFRESH_INTERVAL)
                .unwrap_or(true);

            if should_refresh {
                if let Some(ref mut network) = self.network {
                    debug!("Periodic WiFi refresh");
                    if let Err(e) = network.scan_networks() {
                        debug!("WiFi refresh failed: {}", e);
                    }
                    self.last_wifi_scan = Some(std::time::Instant::now());
                    self.render()?;
                }
            }
        }

        while let Some(event) = self.conn.poll_event()? {
            match event {
                x11rb::protocol::Event::Expose(e) => {
                    if self.window.as_ref().map(|w| w.id()) == Some(e.window) {
                        debug!("Panel exposure event, re-rendering");
                        self.render()?;
                    }
                }
                x11rb::protocol::Event::FocusOut(e) => {
                    if self.window.as_ref().map(|w| w.id()) == Some(e.event) {
                        use x11rb::protocol::xproto::NotifyMode;
                        if e.mode == NotifyMode::NORMAL {
                            // Grace period: ignore FocusOut shortly after showing
                            // This prevents hiding when garbar's click causes focus bounce
                            if let Some(show_time) = self.show_time {
                                let elapsed = show_time.elapsed();
                                if elapsed < std::time::Duration::from_millis(150) {
                                    debug!("Ignoring FocusOut during grace period ({}ms)", elapsed.as_millis());
                                    continue;
                                }
                            }
                            debug!("Panel lost focus (normal), hiding");
                            self.hide()?;
                            return Ok(true);
                        } else if e.mode == NotifyMode::GRAB {
                            // Another app is grabbing (e.g., garshot for screenshot)
                            // Release our pointer grab so they can use it
                            debug!("Focus lost to grab, releasing pointer");
                            let _ = self.conn.inner().ungrab_pointer(CURRENT_TIME);
                            self.conn.flush()?;
                        } else {
                            debug!("Panel focus event mode={:?}, ignoring", e.mode);
                        }
                    }
                }
                x11rb::protocol::Event::FocusIn(e) => {
                    if self.window.as_ref().map(|w| w.id()) == Some(e.event) {
                        use x11rb::protocol::xproto::NotifyMode;
                        if e.mode == NotifyMode::UNGRAB {
                            // Grab ended, re-grab pointer for click-outside detection
                            debug!("Focus returned after ungrab, re-grabbing pointer");
                            if let Some(ref window) = self.window {
                                let _ = self.conn.inner().grab_pointer(
                                    false,
                                    window.id(),
                                    EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
                                    GrabMode::ASYNC,
                                    GrabMode::ASYNC,
                                    x11rb::NONE,
                                    x11rb::NONE,
                                    CURRENT_TIME,
                                );
                                self.conn.flush()?;
                            }
                        }
                    }
                }
                x11rb::protocol::Event::KeyPress(e) => {
                    // Handle password entry keyboard input
                    if self.password_entry_ssid.is_some() {
                        match e.detail {
                            9 => {
                                // Escape - cancel password entry
                                debug!("Escape pressed, cancelling password entry");
                                self.password_entry_ssid = None;
                                self.password_text.clear();
                                self.update_panel_height()?;
                                self.render()?;
                            }
                            36 => {
                                // Enter - submit password
                                if !self.password_text.is_empty() {
                                    let ssid = self.password_entry_ssid.take().unwrap();
                                    let password = std::mem::take(&mut self.password_text);
                                    info!("Connecting to '{}' with password", ssid);

                                    if let Some(ref mut network) = self.network {
                                        if let Err(e) = network.connect_with_password(&ssid, &password) {
                                            warn!("WiFi connect with password failed: {}", e);
                                        }
                                        // Refresh network list
                                        let _ = network.scan_networks();
                                        self.last_wifi_scan = Some(std::time::Instant::now());
                                    }
                                    self.update_panel_height()?;
                                    self.render()?;
                                }
                            }
                            22 => {
                                // Backspace - delete last character
                                self.password_text.pop();
                                self.render()?;
                            }
                            _ => {
                                // Try to convert keycode to character
                                if let Some(ch) = self.keycode_to_char(e.detail, e.state.into()) {
                                    self.password_text.push(ch);
                                    self.render()?;
                                }
                            }
                        }
                    } else {
                        // Normal mode - Escape hides panel
                        if e.detail == 9 {
                            debug!("Escape pressed, hiding panel");
                            self.hide()?;
                            return Ok(true);
                        }
                    }
                }
                x11rb::protocol::Event::ButtonPress(e) => {
                    let x = e.event_x as i32;
                    let y = e.event_y as i32;

                    // Check if click is outside panel bounds (click-outside-to-close)
                    if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
                        debug!("Click outside panel at ({}, {}), closing", x, y);
                        self.hide()?;
                        return Ok(true);
                    }

                    // Button 4 = scroll up, Button 5 = scroll down
                    if e.detail == 4 {
                        // Scroll up
                        match self.expanded {
                            ExpandedSection::WiFi => {
                                if self.wifi_scroll_offset > 0 {
                                    self.wifi_scroll_offset -= 1;
                                    self.render()?;
                                }
                            }
                            ExpandedSection::Bluetooth => {
                                if self.bluetooth_scroll_offset > 0 {
                                    self.bluetooth_scroll_offset -= 1;
                                    self.render()?;
                                }
                            }
                            ExpandedSection::None => {}
                        }
                    } else if e.detail == 5 {
                        // Scroll down
                        match self.expanded {
                            ExpandedSection::WiFi => {
                                let network_count = self.network.as_ref()
                                    .map(|n| n.access_points().len())
                                    .unwrap_or(0);
                                let max_scroll = network_count.saturating_sub(3); // MAX_VISIBLE = 3
                                if self.wifi_scroll_offset < max_scroll {
                                    self.wifi_scroll_offset += 1;
                                    self.render()?;
                                }
                            }
                            ExpandedSection::Bluetooth => {
                                let device_count = self.bluetooth.as_ref()
                                    .map(|b| b.devices().len())
                                    .unwrap_or(0);
                                let max_scroll = device_count.saturating_sub(5); // MAX_VISIBLE = 5
                                if self.bluetooth_scroll_offset < max_scroll {
                                    self.bluetooth_scroll_offset += 1;
                                    self.render()?;
                                }
                            }
                            ExpandedSection::None => {}
                        }
                    }

                    // Button 1 = left click, start potential drag or hold
                    if e.detail == 1 {
                        self.handle_button_press(x as f64, y as f64)?;
                    }
                }
                x11rb::protocol::Event::ButtonRelease(e) => {
                    if self.window.as_ref().map(|w| w.id()) == Some(e.event) && e.detail == 1 {
                        // Check for click action (hold action fires proactively in process_events)
                        if let Some(press_start) = self.press_start.take() {
                            let held_button = self.press_button.take();
                            let duration = press_start.elapsed();
                            const HOLD_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(500);

                            if let Some(btn_name) = held_button {
                                if duration < HOLD_THRESHOLD {
                                    // Short click - expand/collapse
                                    info!("Short click on '{}' ({:?}) - toggling list", btn_name, duration);
                                    self.handle_click_action(&btn_name)?;
                                    self.render()?;
                                }
                                // Note: hold action already fired proactively, no need to handle here
                            }
                        }

                        // End drag - apply the final value
                        if let Some(ref module_name) = self.dragging.take() {
                            self.apply_slider_value(module_name, self.drag_value)?;
                            self.last_slider_apply = None; // Reset throttle state
                            self.last_render = None;
                        }
                    }
                }
                x11rb::protocol::Event::MotionNotify(e) => {
                    let x = e.event_x as f64;
                    let y = e.event_y as f64;

                    // Handle drag motion
                    if self.dragging.is_some() {
                        self.handle_drag(x, y)?;
                    } else {
                        // Update hover state
                        self.update_hover(x, y)?;
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }

    /// Handle button press - start drag, toggle mute, or click toggle buttons
    fn handle_button_press(&mut self, x: f64, y: f64) -> Result<()> {
        debug!("Panel button press at ({}, {})", x, y);

        // Clear any previous press state
        self.press_start = None;
        self.press_button = None;

        // Check toggle buttons first
        let buttons = self.toggle_buttons.clone();
        for btn in &buttons {
            if x >= btn.x && x < btn.x + btn.width && y >= btn.y && y < btn.y + btn.height {
                // WiFi and Bluetooth support hold-to-toggle-power
                if btn.name == "wifi" || btn.name == "bluetooth" {
                    debug!("Starting hold timer for '{}'", btn.name);
                    self.press_start = Some(std::time::Instant::now());
                    self.press_button = Some(btn.name.clone());
                    return Ok(());
                }
                // Other buttons act immediately
                info!("Toggle button '{}' clicked", btn.name);
                self.handle_toggle_action(&btn.name)?;
                self.render()?;
                return Ok(());
            }
        }

        // Check list items (WiFi networks or Bluetooth devices)
        let items = self.list_items.clone();
        let expanded = self.expanded;
        for item in &items {
            // Check if click is within item's row (full width minus padding)
            if y >= item.y && y < item.y + item.height && x >= 16.0 && x < (self.width as f64 - 16.0) {
                info!("List item '{}' clicked", item.id);
                self.handle_list_item_click(&item.id, expanded)?;
                self.render()?;
                return Ok(());
            }
        }

        // Check slider rows
        let rows = self.slider_rows.clone();
        for row in &rows {
            // Check if click is within this row's vertical bounds
            if y >= row.y && y < row.y + row.height {
                // Check if click is on the icon (mute toggle for volume)
                if x >= row.icon_x && x < row.icon_x + row.icon_width {
                    if row.name == "volume" {
                        if let Some(ref mut vol) = self.volume {
                            vol.toggle_mute()?;
                            let state = vol.state();
                            info!("Volume mute toggled: {}", if state.muted { "muted" } else { "unmuted" });
                        }
                        self.render()?;
                        return Ok(());
                    }
                }

                // Check if click is on the slider - start dragging
                if x >= row.slider_x && x <= row.slider_x + row.slider_width {
                    // Start dragging - only update visual, don't apply to PulseAudio yet
                    self.dragging = Some(row.name.clone());
                    self.drag_value = ((x - row.slider_x) / row.slider_width).clamp(0.0, 1.0);
                    self.render()?;
                    return Ok(());
                }
            }
        }

        Ok(())
    }

    /// Handle toggle button action
    fn handle_toggle_action(&mut self, name: &str) -> Result<()> {
        match name {
            // Power actions
            "shutdown" => {
                if let Some(ref power) = self.power {
                    if let Err(e) = power.shutdown() {
                        warn!("Shutdown failed: {}", e);
                    }
                }
            }
            "restart" => {
                if let Some(ref power) = self.power {
                    if let Err(e) = power.reboot() {
                        warn!("Reboot failed: {}", e);
                    }
                }
            }
            "logout" => {
                if let Some(ref power) = self.power {
                    if let Err(e) = power.logout() {
                        warn!("Logout failed: {}", e);
                    }
                }
            }
            "hibernate" => {
                if let Some(ref power) = self.power {
                    if let Err(e) = power.hibernate() {
                        warn!("Hibernate failed: {}", e);
                    }
                }
            }
            // WiFi - toggle expansion or scan
            "wifi" => {
                if self.expanded == ExpandedSection::WiFi {
                    // Collapse
                    self.expanded = ExpandedSection::None;
                    self.wifi_scroll_offset = 0;
                    info!("WiFi picker collapsed");
                    self.update_panel_height()?;
                } else {
                    // Expand - show "Scanning..." first, then scan
                    self.expanded = ExpandedSection::WiFi;
                    if let Some(ref mut network) = self.network {
                        network.set_scanning(true);
                    }
                    self.update_panel_height()?;
                    self.render()?;
                    self.conn.flush()?;

                    // Now do the blocking scan
                    if let Some(ref mut network) = self.network {
                        if let Err(e) = network.scan_networks() {
                            debug!("WiFi scan failed: {}", e);
                        } else {
                            let aps = network.access_points();
                            info!("WiFi picker expanded, {} networks", aps.len());
                        }
                        self.last_wifi_scan = Some(std::time::Instant::now());
                    }
                    self.update_panel_height()?;
                }
            }
            // Bluetooth - toggle expansion or scan
            "bluetooth" => {
                if self.expanded == ExpandedSection::Bluetooth {
                    // Collapse
                    self.expanded = ExpandedSection::None;
                    self.bluetooth_scroll_offset = 0;
                    info!("Bluetooth picker collapsed");
                } else {
                    // Expand and scan
                    self.expanded = ExpandedSection::Bluetooth;
                    if let Some(ref mut bt) = self.bluetooth {
                        if let Err(e) = bt.scan_devices() {
                            debug!("Bluetooth scan failed: {}", e);
                        } else {
                            let devices = bt.devices();
                            info!("Bluetooth picker expanded, {} devices", devices.len());
                        }
                    }
                }
                // Recalculate height and resize window
                self.update_panel_height()?;
            }
            // DND via dunstctl or local state
            "dnd" => {
                if let Some(ref mut dnd) = self.dnd {
                    if let Err(e) = dnd.toggle() {
                        warn!("DND toggle failed: {}", e);
                    }
                }
            }
            // Battery is just a status display, not a toggle
            "battery" => {
                info!("Battery clicked (status only)");
            }
            _ => {
                debug!("Unknown toggle button: {}", name);
            }
        }
        Ok(())
    }

    /// Handle short click on WiFi/Bluetooth - expand/collapse list
    fn handle_click_action(&mut self, name: &str) -> Result<()> {
        match name {
            "wifi" => {
                if self.expanded == ExpandedSection::WiFi {
                    self.expanded = ExpandedSection::None;
                    self.wifi_scroll_offset = 0;
                    info!("WiFi picker collapsed");
                    self.update_panel_height()?;
                } else {
                    self.expanded = ExpandedSection::WiFi;
                    // Set scanning state and render "Scanning..." before blocking scan
                    if let Some(ref mut network) = self.network {
                        network.set_scanning(true);
                    }
                    self.update_panel_height()?;
                    self.render()?;
                    self.conn.flush()?;

                    if let Some(ref mut network) = self.network {
                        if let Err(e) = network.scan_networks() {
                            debug!("WiFi scan failed: {}", e);
                        } else {
                            let aps = network.access_points();
                            info!("WiFi picker expanded, {} networks", aps.len());
                        }
                        self.last_wifi_scan = Some(std::time::Instant::now());
                    }
                    // Panel height may change after scan results
                    self.update_panel_height()?;
                }
            }
            "bluetooth" => {
                if self.expanded == ExpandedSection::Bluetooth {
                    self.expanded = ExpandedSection::None;
                    self.bluetooth_scroll_offset = 0;
                    info!("Bluetooth picker collapsed");
                    self.update_panel_height()?;
                } else {
                    self.expanded = ExpandedSection::Bluetooth;
                    // Show "Scanning..." immediately before blocking scan
                    self.update_panel_height()?;
                    self.render()?;
                    self.conn.flush()?;

                    if let Some(ref mut bt) = self.bluetooth {
                        if let Err(e) = bt.scan_devices() {
                            debug!("Bluetooth scan failed: {}", e);
                        } else {
                            let devices = bt.devices();
                            info!("Bluetooth picker expanded, {} devices", devices.len());
                        }
                    }
                    self.update_panel_height()?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle long hold on WiFi/Bluetooth - toggle power on/off
    fn handle_hold_action(&mut self, name: &str) -> Result<()> {
        match name {
            "wifi" => {
                if let Some(ref mut network) = self.network {
                    let currently_enabled = network.is_wifi_enabled();
                    if currently_enabled {
                        info!("Turning WiFi OFF");
                        if let Err(e) = network.disable_wifi() {
                            warn!("Failed to disable WiFi: {}", e);
                        }
                    } else {
                        info!("Turning WiFi ON");
                        if let Err(e) = network.enable_wifi() {
                            warn!("Failed to enable WiFi: {}", e);
                        }
                    }
                    // Collapse list and refresh state
                    self.expanded = ExpandedSection::None;
                    self.update_panel_height()?;
                }
            }
            "bluetooth" => {
                if let Some(ref mut bt) = self.bluetooth {
                    let currently_powered = bt.is_powered();
                    if currently_powered {
                        info!("Turning Bluetooth OFF");
                        if let Err(e) = bt.power_off() {
                            warn!("Failed to power off Bluetooth: {}", e);
                        }
                    } else {
                        info!("Turning Bluetooth ON");
                        if let Err(e) = bt.power_on() {
                            warn!("Failed to power on Bluetooth: {}", e);
                        }
                    }
                    // Collapse list and refresh state
                    self.expanded = ExpandedSection::None;
                    self.update_panel_height()?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle list item click (WiFi network or Bluetooth device)
    fn handle_list_item_click(&mut self, id: &str, section: ExpandedSection) -> Result<()> {
        match section {
            ExpandedSection::WiFi => {
                // id is the SSID
                if let Some(ref mut network) = self.network {
                    let _aps = network.access_points();
                    // Check if already connected to this network
                    let is_connected = network.connected_ssid()
                        .map(|s| s == id)
                        .unwrap_or(false);

                    if is_connected {
                        // Disconnect
                        info!("Disconnecting from WiFi network: {}", id);
                        if let Err(e) = network.disconnect() {
                            warn!("WiFi disconnect failed: {}", e);
                        }
                        // Refresh network list
                        let _ = network.scan_networks();
                        self.last_wifi_scan = Some(std::time::Instant::now());
                    } else {
                        // Check if network needs password
                        if network.network_needs_password(id) {
                            info!("Network '{}' requires password, showing entry", id);
                            self.password_entry_ssid = Some(id.to_string());
                            self.password_text.clear();
                            self.update_panel_height()?;
                            self.render()?;
                            return Ok(());
                        } else {
                            // Connect to the network (open or saved credentials)
                            info!("Connecting to WiFi network: {}", id);
                            if let Err(e) = network.connect_to_network(id) {
                                warn!("WiFi connect failed: {}", e);
                            }
                            // Refresh network list
                            let _ = network.scan_networks();
                            self.last_wifi_scan = Some(std::time::Instant::now());
                        }
                    }
                }
            }
            ExpandedSection::Bluetooth => {
                // id is the device path
                if let Some(ref mut bt) = self.bluetooth {
                    let devices = bt.devices();
                    // Find device and check connection status
                    let device = devices.iter().find(|d| d.path == id);
                    if let Some(dev) = device {
                        if dev.connected {
                            // Disconnect
                            info!("Disconnecting Bluetooth device: {} ({})", dev.name, id);
                            if let Err(e) = bt.disconnect_device(id) {
                                warn!("Bluetooth disconnect failed: {}", e);
                            }
                        } else {
                            // Connect
                            info!("Connecting Bluetooth device: {} ({})", dev.name, id);
                            if let Err(e) = bt.connect_device(id) {
                                warn!("Bluetooth connect failed: {}", e);
                            }
                        }
                    }
                    // Refresh device list
                    let _ = bt.scan_devices();
                }
            }
            ExpandedSection::None => {}
        }
        Ok(())
    }

    /// Handle drag motion - update visual at 60fps, apply values less frequently
    fn handle_drag(&mut self, x: f64, _y: f64) -> Result<()> {
        if let Some(ref module_name) = self.dragging.clone() {
            // Find the row for this module
            if let Some(row) = self.slider_rows.iter().find(|r| r.name == *module_name) {
                // Always update drag_value for smooth visual tracking
                self.drag_value = ((x - row.slider_x) / row.slider_width).clamp(0.0, 1.0);

                let now = std::time::Instant::now();

                // Throttle value application to 100ms (10 updates/sec for wpctl)
                let should_apply = self.last_slider_apply
                    .map(|last| now.duration_since(last).as_millis() >= 100)
                    .unwrap_or(true);

                if should_apply {
                    let value = self.drag_value;
                    match module_name.as_str() {
                        "brightness" => {
                            // Brightness via sysfs - fast, apply directly
                            if let Some(ref mut bright) = self.brightness {
                                let _ = bright.set_brightness(value);
                            }
                        }
                        "volume" => {
                            // Volume via wpctl - spawn fire-and-forget
                            use std::process::{Command, Stdio};
                            let _ = Command::new("wpctl")
                                .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{:.2}", value)])
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn();
                        }
                        _ => {}
                    }
                    self.last_slider_apply = Some(now);
                }

                // Throttle rendering to ~60fps (16ms) for smooth visuals without excessive redraws
                let should_render = self.last_render
                    .map(|last| now.duration_since(last).as_millis() >= 16)
                    .unwrap_or(true);

                if should_render {
                    self.render()?;
                    self.last_render = Some(now);
                }
            }
        }
        Ok(())
    }

    /// Update hover state based on mouse position
    fn update_hover(&mut self, x: f64, y: f64) -> Result<()> {
        let mut new_hover: Option<String> = None;

        // Check toggle buttons
        for btn in &self.toggle_buttons {
            if x >= btn.x && x < btn.x + btn.width && y >= btn.y && y < btn.y + btn.height {
                new_hover = Some(btn.name.clone());
                break;
            }
        }

        // Only re-render if hover state changed
        if new_hover != self.hovered_button {
            self.hovered_button = new_hover;
            self.render()?;
        }

        Ok(())
    }

    /// Apply slider value to the actual module (called on release)
    fn apply_slider_value(&mut self, module_name: &str, value: f64) -> Result<()> {
        match module_name {
            "volume" => {
                if let Some(ref mut vol) = self.volume {
                    vol.set_volume(value)?;
                    info!("Volume set to {:.0}%", value * 100.0);
                }
            }
            "brightness" => {
                if let Some(ref mut bright) = self.brightness {
                    bright.set_brightness(value)?;
                    info!("Brightness set to {:.0}%", value * 100.0);
                }
            }
            _ => {}
        }
        self.render()?;
        Ok(())
    }

    /// Get panel connection for external rendering
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Get panel dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}
