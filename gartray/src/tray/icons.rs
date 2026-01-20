//! Icon loading and rendering utilities
//!
//! Handles loading icons from theme names or pixmap data.

use anyhow::Result;
use std::path::PathBuf;

/// Represents icon data that can be rendered
#[derive(Debug, Clone)]
pub enum IconData {
    /// Icon loaded from theme by name
    ThemeName(String),
    /// Raw ARGB32 pixmap data (width, height, data)
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

    /// Try to find icon file from theme
    pub fn find_theme_icon(name: &str, size: u32) -> Option<PathBuf> {
        // Common icon theme search paths
        let search_dirs = [
            dirs::data_dir().map(|p| p.join("icons")),
            Some(PathBuf::from("/usr/share/icons")),
            Some(PathBuf::from("/usr/share/pixmaps")),
        ];

        // Common theme names to search
        let themes = ["hicolor", "Adwaita", "breeze", "Papirus"];

        for dir in search_dirs.iter().flatten() {
            for theme in &themes {
                // Try various sizes and categories
                let paths = [
                    dir.join(theme).join(format!("{}x{}", size, size)).join("apps").join(format!("{}.png", name)),
                    dir.join(theme).join(format!("{}x{}", size, size)).join("status").join(format!("{}.png", name)),
                    dir.join(theme).join("scalable").join("apps").join(format!("{}.svg", name)),
                    dir.join(theme).join("scalable").join("status").join(format!("{}.svg", name)),
                ];

                for path in &paths {
                    if path.exists() {
                        return Some(path.clone());
                    }
                }
            }
        }

        // Try pixmaps directory directly
        let pixmap_paths = [
            PathBuf::from(format!("/usr/share/pixmaps/{}.png", name)),
            PathBuf::from(format!("/usr/share/pixmaps/{}.svg", name)),
        ];

        for path in &pixmap_paths {
            if path.exists() {
                return Some(path.clone());
            }
        }

        None
    }
}

/// Convert ARGB32 data (network byte order from D-Bus) to BGRA for Cairo
pub fn argb_to_bgra(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(4) {
        // D-Bus sends ARGB in network byte order (big endian)
        // Cairo expects BGRA in native byte order
        let a = chunk[0];
        let r = chunk[1];
        let g = chunk[2];
        let b = chunk[3];
        result.extend_from_slice(&[b, g, r, a]);
    }
    result
}
