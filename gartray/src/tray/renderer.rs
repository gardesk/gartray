//! Tray rendering - draws SNI icons and manages layout
//!
//! XEMBED icons are X11 windows that draw themselves - we just position them.
//! SNI icons need to be drawn by us using Cairo.

use anyhow::Result;
use cairo::{Context, Format, ImageSurface};
use gartk_x11::Window;
use std::collections::HashMap;
use tracing::{debug, warn};
use x11rb::protocol::xproto::ConnectionExt;

use super::icons::{IconData, argb_to_cairo_bgra};
use super::sni::SniItem;
use crate::config::TrayConfig;

/// Cached icon data (BGRA, ready for Cairo)
#[derive(Clone)]
struct CachedIcon {
    width: i32,
    height: i32,
    data: Vec<u8>,
}

/// Info about where an SNI icon was rendered (for hit testing)
#[derive(Debug, Clone)]
pub struct SniIconPosition {
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Result of a hit test
#[derive(Debug, Clone)]
pub struct SniHitResult {
    pub id: String,
    pub local_x: i32,
    pub local_y: i32,
}

/// Renders tray icons (both XEMBED positioning and SNI drawing)
pub struct TrayRenderer {
    /// Cairo surface for drawing SNI icons
    surface: Option<ImageSurface>,
    /// Current surface width
    width: u32,
    /// Current surface height
    height: u32,
    /// Icon size from config
    icon_size: u32,
    /// Spacing between icons
    spacing: u32,
    /// Background color (ARGB)
    background: u32,
    /// Icon cache by name/id
    icon_cache: HashMap<String, CachedIcon>,
    /// Positions of rendered SNI icons (for hit testing)
    sni_positions: Vec<SniIconPosition>,
}

impl TrayRenderer {
    /// Create a new tray renderer
    pub fn new(config: &TrayConfig) -> Self {
        let bg = parse_color(&config.background).unwrap_or(0xFF1a1a1a);
        Self {
            surface: None,
            width: 200,
            height: config.icon_size,
            icon_size: config.icon_size,
            spacing: config.spacing as u32,
            background: bg,
            icon_cache: HashMap::new(),
            sni_positions: Vec::new(),
        }
    }

    /// Clear the icon cache (e.g., on theme change)
    pub fn clear_cache(&mut self) {
        self.icon_cache.clear();
        debug!("Icon cache cleared");
    }

    /// Hit test a point against rendered SNI icons
    /// Returns the icon ID and local coordinates if hit
    pub fn hit_test(&self, x: i32, y: i32) -> Option<SniHitResult> {
        for pos in &self.sni_positions {
            if x >= pos.x && x < pos.x + pos.width &&
               y >= pos.y && y < pos.y + pos.height {
                return Some(SniHitResult {
                    id: pos.id.clone(),
                    local_x: x - pos.x,
                    local_y: y - pos.y,
                });
            }
        }
        None
    }

    /// Get the icon size
    pub fn icon_size(&self) -> u32 {
        self.icon_size
    }

    /// Ensure surface is allocated with correct size
    fn ensure_surface(&mut self, width: u32, height: u32) -> Result<()> {
        if self.surface.is_none() || self.width != width || self.height != height {
            self.surface = Some(
                ImageSurface::create(Format::ARgb32, width as i32, height as i32)
                    .map_err(|e| anyhow::anyhow!("Failed to create surface: {}", e))?
            );
            self.width = width;
            self.height = height;
        }
        Ok(())
    }

    /// Clear the surface with background color
    fn clear(&self) -> Result<()> {
        if let Some(ref surface) = self.surface {
            let ctx = Context::new(surface)
                .map_err(|e| anyhow::anyhow!("Failed to create context: {}", e))?;

            let r = ((self.background >> 16) & 0xFF) as f64 / 255.0;
            let g = ((self.background >> 8) & 0xFF) as f64 / 255.0;
            let b = (self.background & 0xFF) as f64 / 255.0;
            let a = ((self.background >> 24) & 0xFF) as f64 / 255.0;

            ctx.set_source_rgba(r, g, b, a);
            ctx.paint().map_err(|e| anyhow::anyhow!("Failed to paint: {}", e))?;
        }
        Ok(())
    }

    /// Draw an SNI icon at position
    fn draw_sni_icon(&mut self, item: &SniItem, x: i32, y: i32) -> Result<()> {
        let surface = self.surface.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No surface"))?;

        let ctx = Context::new(surface)
            .map_err(|e| anyhow::anyhow!("Failed to create context: {}", e))?;

        let icon_data = IconData::from_sni(
            item.icon_name.as_deref(),
            item.icon_pixmap.clone(),
        );

        match icon_data {
            IconData::Pixmap { width, height, data } => {
                // Convert ARGB to Cairo's BGRA format with premultiplied alpha
                let bgra = argb_to_cairo_bgra(&data);
                self.draw_pixmap_data(&ctx, x, y, width, height, &bgra)?;
            }
            IconData::ThemeName(ref name) => {
                // Check cache first
                if let Some(cached) = self.icon_cache.get(name) {
                    self.draw_pixmap_data(&ctx, x, y, cached.width, cached.height, &cached.data)?;
                } else if let Some(path) = IconData::find_theme_icon(name, self.icon_size) {
                    // Load from file and cache
                    match IconData::load_from_file(&path, self.icon_size) {
                        Ok(IconData::Pixmap { width, height, data }) => {
                            self.draw_pixmap_data(&ctx, x, y, width, height, &data)?;
                            // Cache for next time
                            self.icon_cache.insert(name.clone(), CachedIcon {
                                width,
                                height,
                                data,
                            });
                        }
                        Ok(_) => {
                            self.draw_placeholder(&ctx, x, y, name);
                        }
                        Err(e) => {
                            warn!("Failed to load icon '{}': {}", name, e);
                            self.draw_placeholder(&ctx, x, y, name);
                        }
                    }
                } else {
                    debug!("Icon '{}' not found in themes", name);
                    self.draw_placeholder(&ctx, x, y, name);
                }
            }
            IconData::File(ref path) => {
                let cache_key = path.to_string_lossy().to_string();
                if let Some(cached) = self.icon_cache.get(&cache_key) {
                    self.draw_pixmap_data(&ctx, x, y, cached.width, cached.height, &cached.data)?;
                } else {
                    match IconData::load_from_file(path, self.icon_size) {
                        Ok(IconData::Pixmap { width, height, data }) => {
                            self.draw_pixmap_data(&ctx, x, y, width, height, &data)?;
                            self.icon_cache.insert(cache_key, CachedIcon {
                                width,
                                height,
                                data,
                            });
                        }
                        Ok(_) | Err(_) => {
                            self.draw_placeholder(&ctx, x, y, "?");
                        }
                    }
                }
            }
            IconData::Placeholder => {
                self.draw_placeholder(&ctx, x, y, "?");
            }
        }

        Ok(())
    }

    /// Draw pixmap data to the surface at position
    fn draw_pixmap_data(&self, ctx: &Context, x: i32, y: i32, width: i32, height: i32, data: &[u8]) -> Result<()> {
        // Cairo needs owned data for the surface
        let data_copy = data.to_vec();
        let stride = width * 4;

        match ImageSurface::create_for_data(
            data_copy.into_boxed_slice(),
            Format::ARgb32,
            width,
            height,
            stride,
        ) {
            Ok(icon_surface) => {
                // Center the icon if it's smaller than icon_size
                let offset_x = (self.icon_size as i32 - width) / 2;
                let offset_y = (self.icon_size as i32 - height) / 2;

                ctx.save().ok();
                ctx.translate((x + offset_x) as f64, (y + offset_y) as f64);
                ctx.set_source_surface(&icon_surface, 0.0, 0.0).ok();
                ctx.paint().ok();
                ctx.restore().ok();
            }
            Err(e) => {
                warn!("Failed to create icon surface: {}", e);
            }
        }

        Ok(())
    }

    /// Draw a placeholder icon (circle with letter)
    fn draw_placeholder(&self, ctx: &Context, x: i32, y: i32, label: &str) {
        let size = self.icon_size as f64;
        let cx = x as f64 + size / 2.0;
        let cy = y as f64 + size / 2.0;
        let radius = size / 2.0 - 2.0;

        // Draw circle background
        ctx.arc(cx, cy, radius, 0.0, 2.0 * std::f64::consts::PI);
        ctx.set_source_rgba(0.3, 0.3, 0.3, 1.0);
        let _ = ctx.fill();

        // Draw border
        ctx.arc(cx, cy, radius, 0.0, 2.0 * std::f64::consts::PI);
        ctx.set_source_rgba(0.5, 0.5, 0.5, 1.0);
        ctx.set_line_width(1.0);
        let _ = ctx.stroke();

        // Draw first letter
        if let Some(c) = label.chars().next() {
            ctx.set_source_rgba(1.0, 1.0, 1.0, 1.0);
            ctx.select_font_face("sans-serif", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
            ctx.set_font_size(size * 0.5);

            let text = c.to_uppercase().to_string();
            if let Ok(extents) = ctx.text_extents(&text) {
                let tx = cx - extents.width() / 2.0 - extents.x_bearing();
                let ty = cy - extents.height() / 2.0 - extents.y_bearing();
                ctx.move_to(tx, ty);
                let _ = ctx.show_text(&text);
            }
        }
    }

    /// Render all SNI icons and copy to window
    /// Returns the x offset after all SNI icons (for XEMBED positioning)
    pub fn render_sni_icons(
        &mut self,
        window: &Window,
        sni_items: &[&SniItem],
        xembed_offset: i32,
    ) -> Result<i32> {
        // Clear previous positions
        self.sni_positions.clear();

        if sni_items.is_empty() {
            return Ok(xembed_offset);
        }

        // Calculate total width needed
        let total_width = xembed_offset as u32
            + (sni_items.len() as u32 * self.icon_size)
            + ((sni_items.len().saturating_sub(1)) as u32 * self.spacing);

        self.ensure_surface(total_width, self.icon_size)?;
        self.clear()?;

        // Draw each SNI icon and track positions
        let mut x = xembed_offset;
        for item in sni_items {
            self.draw_sni_icon(item, x, 0)?;

            // Track position for hit testing
            self.sni_positions.push(SniIconPosition {
                id: item.id.clone(),
                x,
                y: 0,
                width: self.icon_size as i32,
                height: self.icon_size as i32,
            });

            x += self.icon_size as i32 + self.spacing as i32;
        }

        // Copy surface to window
        self.copy_to_window(window)?;

        Ok(x)
    }

    /// Copy the surface to the X11 window
    fn copy_to_window(&mut self, window: &Window) -> Result<()> {
        let surface = self.surface.as_mut()
            .ok_or_else(|| anyhow::anyhow!("No surface"))?;

        surface.flush();

        // Get surface data
        let data = {
            let data_ref = surface.data()
                .map_err(|e| anyhow::anyhow!("Failed to get surface data: {}", e))?;
            data_ref.to_vec()
        };

        let conn = window.connection();

        // Create a GC if we don't have one (use default)
        let gc = conn.generate_id()?;
        conn.inner().create_gc(gc, window.id(), &Default::default())?;

        // Put the image
        conn.inner().put_image(
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

        conn.inner().free_gc(gc)?;
        conn.flush()?;

        Ok(())
    }
}

/// Parse a hex color string like "#1a1a1a" to 0xAARRGGBB format
fn parse_color(s: &str) -> Option<u32> {
    let s = s.trim_start_matches('#');
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(0xFF000000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
    } else if s.len() == 8 {
        u32::from_str_radix(s, 16).ok()
    } else {
        None
    }
}
