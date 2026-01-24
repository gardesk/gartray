//! Icon loading and rendering utilities
//!
//! Handles loading icons from theme names, pixmap data, PNG, and SVG files.

use anyhow::{Context, Result};
use image::GenericImageView;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Represents icon data that can be rendered
#[derive(Debug, Clone)]
pub enum IconData {
    /// Icon loaded from theme by name
    ThemeName(String),
    /// Raw BGRA32 pixmap data ready for Cairo (width, height, data)
    Pixmap { width: i32, height: i32, data: Vec<u8> },
    /// Icon loaded from file path
    File(PathBuf),
    /// Fallback/placeholder
    Placeholder,
}

impl IconData {
    /// Create icon data from an SNI item's icon properties
    pub fn from_sni(icon_name: Option<&str>, icon_pixmap: Option<(i32, i32, Vec<u8>)>) -> Self {
        // Prefer icon name (theme lookup) if available
        if let Some(name) = icon_name {
            if !name.is_empty() {
                return IconData::ThemeName(name.to_string());
            }
        }

        // Fall back to pixmap if available
        if let Some((width, height, data)) = icon_pixmap {
            return IconData::Pixmap { width, height, data };
        }

        // Last resort: placeholder
        IconData::Placeholder
    }

    /// Load icon from file path, scaling to target size
    /// Returns Pixmap variant with BGRA data ready for Cairo
    pub fn load_from_file(path: &Path, target_size: u32) -> Result<Self> {
        let ext = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let (w, h, data) = match ext.to_lowercase().as_str() {
            "svg" => load_svg(path, target_size)?,
            "png" | "jpg" | "jpeg" => load_png(path, target_size)?,
            _ => anyhow::bail!("Unsupported icon format: {}", ext),
        };

        Ok(IconData::Pixmap { width: w, height: h, data })
    }

    /// Try to find icon file from theme
    pub fn find_theme_icon(name: &str, size: u32) -> Option<PathBuf> {
        // Common icon theme search paths
        let search_dirs = [
            dirs::data_dir().map(|p| p.join("icons")),
            dirs::home_dir().map(|p| p.join(".local/share/icons")),
            Some(PathBuf::from("/usr/share/icons")),
            Some(PathBuf::from("/usr/share/pixmaps")),
        ];

        // Common theme names to search (in priority order)
        let themes = ["Adwaita", "breeze", "Papirus", "hicolor"];

        // Sizes to try (closest to requested first, then fallbacks)
        let sizes_to_try = [size, 48, 32, 24, 22, 16];

        // Categories to search
        let categories = ["apps", "status", "devices", "actions", "places"];

        for dir in search_dirs.iter().flatten() {
            for theme in &themes {
                // Try scalable SVGs first (best quality)
                for category in &categories {
                    let svg_path = dir.join(theme).join("scalable").join(category).join(format!("{}.svg", name));
                    if svg_path.exists() {
                        debug!("Found SVG icon: {}", svg_path.display());
                        return Some(svg_path);
                    }

                    // Try symbolic SVG
                    let symbolic_path = dir.join(theme).join("scalable").join(category).join(format!("{}-symbolic.svg", name));
                    if symbolic_path.exists() {
                        debug!("Found symbolic SVG icon: {}", symbolic_path.display());
                        return Some(symbolic_path);
                    }
                }

                // Try PNG at various sizes
                for &sz in &sizes_to_try {
                    for category in &categories {
                        let png_path = dir.join(theme).join(format!("{}x{}", sz, sz)).join(category).join(format!("{}.png", name));
                        if png_path.exists() {
                            debug!("Found PNG icon: {}", png_path.display());
                            return Some(png_path);
                        }
                    }
                }
            }
        }

        // Try pixmaps directory directly
        let pixmap_paths = [
            PathBuf::from(format!("/usr/share/pixmaps/{}.svg", name)),
            PathBuf::from(format!("/usr/share/pixmaps/{}.png", name)),
        ];

        for path in &pixmap_paths {
            if path.exists() {
                debug!("Found pixmap icon: {}", path.display());
                return Some(path.clone());
            }
        }

        debug!("Icon not found: {}", name);
        None
    }
}

/// Load a PNG/JPEG image and scale to target size
/// Returns (width, height, BGRA data) for Cairo
pub fn load_png(path: &Path, target_size: u32) -> Result<(i32, i32, Vec<u8>)> {
    let img = image::open(path)
        .with_context(|| format!("Failed to open image: {}", path.display()))?;

    // Calculate scaling to fit in target_size while preserving aspect ratio
    let (orig_w, orig_h) = img.dimensions();
    let scale = target_size as f32 / orig_w.max(orig_h) as f32;
    let new_w = ((orig_w as f32 * scale).round() as u32).max(1);
    let new_h = ((orig_h as f32 * scale).round() as u32).max(1);

    let resized = img.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3);
    let rgba = resized.into_rgba8();

    // Convert RGBA to BGRA for Cairo (premultiplied alpha)
    let mut data: Vec<u8> = Vec::with_capacity((new_w * new_h * 4) as usize);
    for pixel in rgba.pixels() {
        let [r, g, b, a] = pixel.0;
        // Premultiply alpha for Cairo
        let af = a as f32 / 255.0;
        let r = (r as f32 * af).round() as u8;
        let g = (g as f32 * af).round() as u8;
        let b = (b as f32 * af).round() as u8;
        data.extend_from_slice(&[b, g, r, a]);
    }

    debug!("Loaded PNG icon: {}x{} from {}", new_w, new_h, path.display());
    Ok((new_w as i32, new_h as i32, data))
}

/// Load an SVG and render at target size
/// Returns (width, height, BGRA data) for Cairo
pub fn load_svg(path: &Path, target_size: u32) -> Result<(i32, i32, Vec<u8>)> {
    let svg_data = std::fs::read(path)
        .with_context(|| format!("Failed to read SVG: {}", path.display()))?;

    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(&svg_data, &options)
        .with_context(|| format!("Failed to parse SVG: {}", path.display()))?;

    let svg_size = tree.size();
    let scale = target_size as f32 / svg_size.width().max(svg_size.height());
    let new_w = ((svg_size.width() * scale).round() as u32).max(1);
    let new_h = ((svg_size.height() * scale).round() as u32).max(1);

    let mut pixmap = tiny_skia::Pixmap::new(new_w, new_h)
        .ok_or_else(|| anyhow::anyhow!("Failed to create pixmap for SVG"))?;

    // Clear to transparent
    pixmap.fill(tiny_skia::Color::TRANSPARENT);

    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // tiny-skia uses RGBA premultiplied, convert to BGRA for Cairo
    let mut data = pixmap.take();
    for chunk in data.chunks_exact_mut(4) {
        chunk.swap(0, 2); // R <-> B
    }

    debug!("Loaded SVG icon: {}x{} from {}", new_w, new_h, path.display());
    Ok((new_w as i32, new_h as i32, data))
}

/// Convert SNI ARGB (network byte order) to Cairo BGRA (native, premultiplied)
pub fn argb_to_cairo_bgra(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(4) {
        // SNI sends ARGB in network byte order (big endian): [A, R, G, B]
        let a = chunk[0];
        let r = chunk[1];
        let g = chunk[2];
        let b = chunk[3];

        // Premultiply alpha for Cairo
        let af = a as f32 / 255.0;
        let r = (r as f32 * af).round() as u8;
        let g = (g as f32 * af).round() as u8;
        let b = (b as f32 * af).round() as u8;

        // Cairo native BGRA (little-endian on x86/ARM): [B, G, R, A]
        result.extend_from_slice(&[b, g, r, a]);
    }
    result
}

/// Legacy alias for argb_to_cairo_bgra
#[inline]
pub fn argb_to_bgra(data: &[u8]) -> Vec<u8> {
    argb_to_cairo_bgra(data)
}
