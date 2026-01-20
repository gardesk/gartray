//! Quick settings popup window
//!
//! Creates and manages the popup panel window with all settings modules.

use anyhow::{Context, Result};
use cairo::{Context as CairoContext, Format, ImageSurface};
use gartk_x11::{Connection, Window, WindowConfig};
use x11rb::protocol::xproto::{ConnectionExt, EventMask};
use x11rb::protocol::randr::ConnectionExt as RandrConnectionExt;
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

/// Module row layout info
#[derive(Debug, Clone)]
struct ModuleRow {
    name: String,
    y: f64,
    height: f64,
    slider_x: f64,
    slider_width: f64,
    /// Icon hit region (for mute toggle)
    icon_x: f64,
    icon_width: f64,
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
    /// Module row layout for hit testing
    module_rows: Vec<ModuleRow>,
    /// Currently dragging a slider (module name)
    dragging: Option<String>,
    /// Pending value during drag (avoids blocking PulseAudio calls)
    drag_value: f64,
}

impl PopupPanel {
    /// Create a new popup panel
    pub fn new(config: &PanelConfig) -> Result<Self> {
        let conn = Connection::connect(None).context("Failed to connect to X11")?;

        // Calculate height based on number of modules
        let module_height = 50;
        let header_height = 50;
        let height = header_height + (config.modules.len() as u32 * module_height) + 20;

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
            module_rows: Vec::new(),
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
        }

        self.visible = true;
        Ok(())
    }

    /// Hide the panel
    pub fn hide(&mut self) -> Result<()> {
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
        // Clear module rows for hit testing
        self.module_rows.clear();

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

            // Draw modules with real data
            let modules = self.config.modules.clone();
            let mut y = 58.0;
            let row_height = 42.0;
            let slider_x = 100.0;
            let slider_width = self.width as f64 - 32.0 - slider_x + 16.0;

            // Check if we're dragging a specific module
            let dragging_module = self.dragging.clone();
            let drag_val = self.drag_value;

            for module_name in &modules {
                let (mut value, label, icon, is_muted) = match module_name.as_str() {
                    "volume" => {
                        if let Some(ref vol) = self.volume {
                            let state = vol.state();
                            let icon = if state.muted { "🔇" } else { "🔊" };
                            (state.volume, "Volume", icon, state.muted)
                        } else {
                            (0.75, "Volume", "🔊", false)
                        }
                    }
                    "brightness" => {
                        if let Some(ref bright) = self.brightness {
                            (bright.state().brightness, "Brightness", "☀", false)
                        } else {
                            (0.8, "Brightness", "☀", false)
                        }
                    }
                    "battery" => {
                        if let Some(ref bat) = self.battery {
                            let state = bat.state();
                            let icon = if state.charging { "🔌" } else { "🔋" };
                            (state.percentage / 100.0, "Battery", icon, false)
                        } else {
                            (0.85, "Battery", "🔋", false)
                        }
                    }
                    _ => (0.5, module_name.as_str(), "", false),
                };

                // Use drag value if we're dragging this module
                if dragging_module.as_ref() == Some(module_name) {
                    value = drag_val;
                }

                // Track row for hit testing (only for adjustable modules)
                if module_name == "volume" || module_name == "brightness" {
                    self.module_rows.push(ModuleRow {
                        name: module_name.clone(),
                        y,
                        height: row_height,
                        slider_x,
                        slider_width,
                        icon_x: 16.0 + 8.0,  // x offset + icon padding
                        icon_width: 30.0,    // Click area for icon
                    });
                }

                self.draw_module_row(&ctx, label, icon, value, is_muted, 16.0, y, slider_x, slider_width)?;
                y += 50.0;
            }

            // ctx is dropped here, releasing the surface lock
        }

        // Copy to window (now surface is free)
        self.copy_to_window()?;

        debug!("Panel rendered successfully");
        Ok(())
    }

    /// Draw a module row with slider
    fn draw_module_row(
        &self,
        ctx: &CairoContext,
        label: &str,
        icon: &str,
        value: f64,
        is_muted: bool,
        x: f64,
        y: f64,
        slider_x: f64,
        slider_width: f64,
    ) -> Result<()> {
        let width = self.width as f64 - 32.0;
        let height = 42.0;

        // Background with slight highlight
        self.draw_rounded_rect(ctx, x, y, width, height, 8.0);
        ctx.set_source_rgba(0.18, 0.18, 0.2, 1.0);
        ctx.fill().ok();

        // Icon
        ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        if is_muted {
            ctx.set_source_rgba(0.5, 0.5, 0.55, 1.0); // Dim if muted
        } else {
            ctx.set_source_rgba(0.7, 0.7, 0.75, 1.0);
        }
        ctx.set_font_size(16.0);
        ctx.move_to(x + 14.0, y + 27.0);
        ctx.show_text(icon).ok();

        // Label
        ctx.set_source_rgba(1.0, 1.0, 1.0, 0.9);
        ctx.set_font_size(12.0);
        ctx.move_to(x + 40.0, y + 26.0);
        ctx.show_text(label).ok();

        // Draw slider track
        let track_y = y + height / 2.0 - 3.0;
        let track_height = 6.0;
        self.draw_rounded_rect(ctx, slider_x, track_y, slider_width, track_height, 3.0);
        ctx.set_source_rgba(0.25, 0.25, 0.28, 1.0);
        ctx.fill().ok();

        // Draw slider fill (value portion)
        let fill_width = (slider_width * value.clamp(0.0, 1.0)).max(6.0);
        self.draw_rounded_rect(ctx, slider_x, track_y, fill_width, track_height, 3.0);
        if is_muted {
            ctx.set_source_rgba(0.4, 0.4, 0.45, 1.0); // Gray if muted
        } else {
            ctx.set_source_rgba(0.38, 0.68, 0.93, 1.0); // Cyan accent
        }
        ctx.fill().ok();

        // Draw slider knob
        let knob_x = slider_x + fill_width - 6.0;
        let knob_y = y + height / 2.0;
        ctx.arc(knob_x.max(slider_x), knob_y, 7.0, 0.0, 2.0 * std::f64::consts::PI);
        ctx.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        ctx.fill().ok();

        // Value text on right
        ctx.set_source_rgba(0.7, 0.7, 0.75, 1.0);
        ctx.set_font_size(11.0);
        let value_text = format!("{:.0}%", value * 100.0);
        ctx.move_to(x + width - 40.0, y + 26.0);
        ctx.show_text(&value_text).ok();

        Ok(())
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
                    if self.window.as_ref().map(|w| w.id()) == Some(e.event) {
                        // Button 1 = left click, start potential drag
                        if e.detail == 1 {
                            self.handle_button_press(e.event_x as f64, e.event_y as f64)?;
                        }
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

    /// Handle button press - start drag or toggle mute
    fn handle_button_press(&mut self, x: f64, y: f64) -> Result<()> {
        debug!("Panel button press at ({}, {})", x, y);

        // Clone module_rows to avoid borrow issues
        let rows = self.module_rows.clone();

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
            if let Some(row) = self.module_rows.iter().find(|r| r.name == *module_name) {
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
