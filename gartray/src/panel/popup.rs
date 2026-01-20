//! Quick settings popup window
//!
//! Creates and manages the popup panel window with all settings modules.

use anyhow::{Context, Result};
use cairo::{Context as CairoContext, Format, ImageSurface};
use gartk_x11::{Connection, Window, WindowConfig};
use x11rb::protocol::xproto::{ConnectionExt, EventMask, GrabMode};
use x11rb::protocol::randr::ConnectionExt as RandrConnectionExt;
use x11rb::CURRENT_TIME;
use tracing::{debug, info, warn};

use crate::config::PanelConfig;
use crate::panel::volume::VolumeModule;
use crate::panel::brightness::BrightnessModule;
use crate::panel::battery::BatteryModule;

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
    /// Slider rows for hit testing (volume, brightness)
    slider_rows: Vec<SliderRow>,
    /// Toggle buttons for hit testing
    toggle_buttons: Vec<ToggleButton>,
    /// Currently dragging a slider (module name)
    dragging: Option<String>,
    /// Pending value during drag (avoids blocking PulseAudio calls)
    drag_value: f64,
}

impl PopupPanel {
    /// Create a new popup panel
    pub fn new(config: &PanelConfig) -> Result<Self> {
        let conn = Connection::connect(None).context("Failed to connect to X11")?;

        // Calculate height: header + toggle grid (2 rows) + power buttons + volume slider
        // Header: 50px, Toggle grid: 2x68=136px, Power row: 55px, Slider: 50px, Padding
        let height = 320;

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
            slider_rows: Vec::new(),
            toggle_buttons: Vec::new(),
            dragging: None,
            drag_value: 0.0,
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

        self.create_window(x, y)?;

        if let Some(ref window) = self.window {
            window.map()?;
            self.conn.flush()?;
            info!("Panel window {} mapped at ({}, {})", window.id(), self.pos_x, self.pos_y);

            // Grab pointer to detect clicks outside the panel
            let _ = self.conn.inner().grab_pointer(
                true,  // owner_events - send events to owner window
                window.id(),
                EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::BUTTON1_MOTION,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                x11rb::NONE,  // confine_to - don't confine
                x11rb::NONE,  // cursor - use default
                CURRENT_TIME,
            );
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
                        | EventMask::BUTTON1_MOTION  // For slider dragging
                ),
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

            // Row 1: WiFi, Bluetooth
            self.draw_toggle_button(&ctx, "WiFi", "wifi", false, 16.0, grid_y, btn_width, btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "wifi".to_string(), x: 16.0, y: grid_y, width: btn_width, height: btn_height, active: false,
            });

            self.draw_toggle_button(&ctx, "Bluetooth", "bluetooth", false, 16.0 + btn_width + btn_spacing, grid_y, btn_width, btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "bluetooth".to_string(), x: 16.0 + btn_width + btn_spacing, y: grid_y, width: btn_width, height: btn_height, active: false,
            });

            // Row 2: Battery (status), Do Not Disturb
            let row2_y = grid_y + btn_height + btn_spacing;
            let battery_text = if let Some(ref bat) = self.battery {
                let state = bat.state();
                format!("{:.0}%", state.percentage)
            } else {
                "N/A".to_string()
            };
            self.draw_toggle_button(&ctx, &battery_text, "battery", false, 16.0, row2_y, btn_width, btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "battery".to_string(), x: 16.0, y: row2_y, width: btn_width, height: btn_height, active: false,
            });

            self.draw_toggle_button(&ctx, "DND", "dnd", false, 16.0 + btn_width + btn_spacing, row2_y, btn_width, btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "dnd".to_string(), x: 16.0 + btn_width + btn_spacing, y: row2_y, width: btn_width, height: btn_height, active: false,
            });

            // === Power Button Row (4 smaller buttons) ===
            let power_y = row2_y + btn_height + btn_spacing;
            let power_btn_width = (self.width as f64 - 56.0) / 4.0;  // 4 columns
            let power_btn_height = 50.0;
            let power_spacing = 8.0;

            // Shutdown
            let shutdown_x = 16.0;
            self.draw_toggle_button(&ctx, "Shut", "power", false, shutdown_x, power_y, power_btn_width, power_btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "shutdown".to_string(), x: shutdown_x, y: power_y, width: power_btn_width, height: power_btn_height, active: false,
            });

            // Restart
            let restart_x = shutdown_x + power_btn_width + power_spacing;
            self.draw_toggle_button(&ctx, "Restart", "restart", false, restart_x, power_y, power_btn_width, power_btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "restart".to_string(), x: restart_x, y: power_y, width: power_btn_width, height: power_btn_height, active: false,
            });

            // Logout
            let logout_x = restart_x + power_btn_width + power_spacing;
            self.draw_toggle_button(&ctx, "Logout", "logout", false, logout_x, power_y, power_btn_width, power_btn_height)?;
            self.toggle_buttons.push(ToggleButton {
                name: "logout".to_string(), x: logout_x, y: power_y, width: power_btn_width, height: power_btn_height, active: false,
            });

            // Hibernate/Sleep
            let hibernate_x = logout_x + power_btn_width + power_spacing;
            self.draw_toggle_button(&ctx, "Sleep", "hibernate", false, hibernate_x, power_y, power_btn_width, power_btn_height)?;
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
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<()> {
        // Background
        self.draw_rounded_rect(ctx, x, y, width, height, 8.0);
        if active {
            ctx.set_source_rgba(0.38, 0.68, 0.93, 0.3);  // Cyan tint when active
        } else {
            ctx.set_source_rgba(0.18, 0.18, 0.2, 1.0);
        }
        ctx.fill().ok();

        // Border when active
        if active {
            self.draw_rounded_rect(ctx, x + 1.0, y + 1.0, width - 2.0, height - 2.0, 7.0);
            ctx.set_source_rgba(0.38, 0.68, 0.93, 0.8);
            ctx.set_line_width(2.0);
            ctx.stroke().ok();
        }

        // Draw icon using Cairo primitives
        let icon_x = x + width / 2.0;
        let icon_y = y + 25.0;
        let alpha = if active { 1.0 } else { 0.7 };
        self.draw_icon(ctx, icon_type, icon_x, icon_y, alpha);

        // Label
        ctx.set_source_rgba(1.0, 1.0, 1.0, alpha);
        ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        ctx.set_font_size(11.0);
        ctx.move_to(x + width / 2.0 - (label.len() as f64 * 3.0), y + height - 10.0);
        ctx.show_text(label).ok();

        Ok(())
    }

    /// Draw an icon using Cairo primitives
    fn draw_icon(&self, ctx: &CairoContext, icon_type: &str, cx: f64, cy: f64, alpha: f64) {
        ctx.set_source_rgba(1.0, 1.0, 1.0, alpha);
        ctx.set_line_width(2.0);
        ctx.set_line_cap(cairo::LineCap::Round);
        ctx.set_line_join(cairo::LineJoin::Round);

        match icon_type {
            "wifi" => self.draw_wifi_icon(ctx, cx, cy),
            "bluetooth" => self.draw_bluetooth_icon(ctx, cx, cy),
            "battery" => self.draw_battery_icon(ctx, cx, cy),
            "dnd" => self.draw_dnd_icon(ctx, cx, cy),
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

    /// Draw Do Not Disturb icon (bell with slash)
    fn draw_dnd_icon(&self, ctx: &CairoContext, cx: f64, cy: f64) {
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
        // Diagonal slash
        ctx.set_source_rgba(0.9, 0.3, 0.3, 1.0);
        ctx.set_line_width(2.5);
        ctx.move_to(cx - 10.0, cy - 8.0);
        ctx.line_to(cx + 10.0, cy + 8.0);
        ctx.stroke().ok();
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

        // Slider knob - position it so it doesn't overlap with percentage
        let knob_x = slider_x + fill_width - 8.0;
        let knob_y = y + height / 2.0;
        ctx.arc(knob_x.max(slider_x), knob_y, 10.0, 0.0, 2.0 * std::f64::consts::PI);
        ctx.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        ctx.fill().ok();

        // Percentage text - moved to fixed position outside slider area
        ctx.set_source_rgba(0.7, 0.7, 0.75, 1.0);
        ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        ctx.set_font_size(10.0);
        let pct_text = format!("{:.0}%", value * 100.0);
        // Position text at far right, with enough room for "100%"
        ctx.move_to(x + width - 32.0, y + height / 2.0 + 4.0);
        ctx.show_text(&pct_text).ok();

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
                        debug!("Panel lost focus, hiding");
                        self.hide()?;
                        return Ok(true);
                    }
                }
                x11rb::protocol::Event::KeyPress(e) => {
                    // Escape key (keycode 9)
                    if e.detail == 9 {
                        debug!("Escape pressed, hiding panel");
                        self.hide()?;
                        return Ok(true);
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

                    // Button 1 = left click, start potential drag
                    if e.detail == 1 {
                        self.handle_button_press(x as f64, y as f64)?;
                    }
                }
                x11rb::protocol::Event::ButtonRelease(e) => {
                    if self.window.as_ref().map(|w| w.id()) == Some(e.event) && e.detail == 1 {
                        // End drag - apply the final value
                        if let Some(ref module_name) = self.dragging.take() {
                            self.apply_slider_value(module_name, self.drag_value)?;
                        }
                    }
                }
                x11rb::protocol::Event::MotionNotify(e) => {
                    if self.window.as_ref().map(|w| w.id()) == Some(e.event) {
                        // Handle drag motion
                        if self.dragging.is_some() {
                            self.handle_drag(e.event_x as f64, e.event_y as f64)?;
                        }
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

        // Check toggle buttons first
        let buttons = self.toggle_buttons.clone();
        for btn in &buttons {
            if x >= btn.x && x < btn.x + btn.width && y >= btn.y && y < btn.y + btn.height {
                info!("Toggle button '{}' clicked", btn.name);
                // TODO: Implement actual toggle functionality for WiFi, Bluetooth, etc.
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

    /// Handle drag motion - update visual only (no PulseAudio calls)
    fn handle_drag(&mut self, x: f64, _y: f64) -> Result<()> {
        if let Some(ref module_name) = self.dragging.clone() {
            // Find the row for this module
            if let Some(row) = self.slider_rows.iter().find(|r| r.name == *module_name) {
                // Only update drag_value, don't call PulseAudio
                self.drag_value = ((x - row.slider_x) / row.slider_width).clamp(0.0, 1.0);
                self.render()?;
            }
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
