//! Quick settings popup window
//!
//! Creates and manages the popup panel window with all settings modules.

use anyhow::{Context, Result};
use cairo::{Context as CairoContext, Format, ImageSurface};
use gartk_x11::{Connection, Window, WindowConfig, WindowType};
use x11rb::protocol::xproto::{ConnectionExt, EventMask};
use tracing::{debug, info};

use crate::config::PanelConfig;

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
}

impl PopupPanel {
    /// Create a new popup panel
    pub fn new(config: &PanelConfig) -> Result<Self> {
        let conn = Connection::connect(None).context("Failed to connect to X11")?;

        Ok(Self {
            conn,
            window: None,
            surface: None,
            config: config.clone(),
            visible: false,
            width: config.width,
            height: 300, // Will be calculated based on modules
            bg_color: (0.1, 0.1, 0.1, 0.95),
        })
    }

    /// Show the panel near a position (e.g., near the tray)
    pub fn show(&mut self, x: i32, y: i32) -> Result<()> {
        if self.window.is_none() {
            self.create_window(x, y)?;
        }

        if let Some(ref window) = self.window {
            window.map()?;
            self.conn.flush()?;
        }

        self.visible = true;
        info!("Panel shown at ({}, {})", x, y);
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
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Create the popup window
    fn create_window(&mut self, x: i32, y: i32) -> Result<()> {
        // Get screen dimensions for positioning
        let screen = self.conn.screen();
        let screen_width = screen.width_in_pixels as i32;
        let screen_height = screen.height_in_pixels as i32;

        // Adjust position to keep panel on screen
        let panel_x = (x - self.width as i32 / 2)
            .max(8)
            .min(screen_width - self.width as i32 - 8);
        let panel_y = if y < screen_height / 2 {
            y + 8 // Below the click point
        } else {
            y - self.height as i32 - 8 // Above the click point
        };

        let window = Window::create(
            self.conn.clone(),
            WindowConfig::popup()
                .title("gartray-panel")
                .class("gartray")
                .size(self.width, self.height)
                .position(panel_x, panel_y)
                .background(0xFF1a1a1a)
                .map_on_create(false),
        )?;

        // Request focus events so we can close on click-outside
        self.conn.inner().change_window_attributes(
            window.id(),
            &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                .event_mask(EventMask::FOCUS_CHANGE | EventMask::KEY_PRESS),
        )?;

        self.window = Some(window);
        self.surface = Some(
            ImageSurface::create(Format::ARgb32, self.width as i32, self.height as i32)
                .map_err(|e| anyhow::anyhow!("Failed to create surface: {}", e))?
        );

        info!("Created panel window at ({}, {})", panel_x, panel_y);
        Ok(())
    }

    /// Render the panel content
    pub fn render(&mut self) -> Result<()> {
        let surface = self.surface.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No surface"))?;

        let ctx = CairoContext::new(surface)
            .map_err(|e| anyhow::anyhow!("Failed to create context: {}", e))?;

        // Clear with background
        let (r, g, b, a) = self.bg_color;
        ctx.set_source_rgba(r, g, b, a);
        ctx.paint().ok();

        // Draw rounded rectangle background
        let radius = 12.0;
        self.draw_rounded_rect(&ctx, 0.0, 0.0, self.width as f64, self.height as f64, radius);
        ctx.set_source_rgba(r, g, b, a);
        ctx.fill().ok();

        // Draw border
        self.draw_rounded_rect(&ctx, 0.5, 0.5, self.width as f64 - 1.0, self.height as f64 - 1.0, radius);
        ctx.set_source_rgba(0.3, 0.3, 0.3, 1.0);
        ctx.set_line_width(1.0);
        ctx.stroke().ok();

        // Draw content placeholder
        ctx.set_source_rgba(1.0, 1.0, 1.0, 1.0);
        ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        ctx.set_font_size(14.0);
        ctx.move_to(20.0, 30.0);
        ctx.show_text("Quick Settings").ok();

        // Draw module placeholders
        let mut y = 60.0;
        for module in &self.config.modules {
            self.draw_module_placeholder(&ctx, module, 16.0, y)?;
            y += 50.0;
        }

        // Copy to window
        self.copy_to_window()?;

        Ok(())
    }

    /// Draw a module placeholder
    fn draw_module_placeholder(&self, ctx: &CairoContext, name: &str, x: f64, y: f64) -> Result<()> {
        let width = self.width as f64 - 32.0;
        let height = 40.0;

        // Background
        self.draw_rounded_rect(ctx, x, y, width, height, 8.0);
        ctx.set_source_rgba(0.2, 0.2, 0.2, 1.0);
        ctx.fill().ok();

        // Label
        ctx.set_source_rgba(1.0, 1.0, 1.0, 0.9);
        ctx.set_font_size(12.0);
        ctx.move_to(x + 12.0, y + 25.0);
        ctx.show_text(&format!("{}: TODO", name)).ok();

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
                x11rb::protocol::Event::FocusOut(_) => {
                    debug!("Panel lost focus, hiding");
                    self.hide()?;
                    return Ok(true);
                }
                x11rb::protocol::Event::KeyPress(e) => {
                    // Escape key
                    if e.detail == 9 {
                        debug!("Escape pressed, hiding panel");
                        self.hide()?;
                        return Ok(true);
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }
}
