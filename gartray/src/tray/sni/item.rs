//! SNI Item - represents a single StatusNotifierItem
//!
//! Fetches properties like icon, tooltip, and menu from the item's D-Bus interface.

use anyhow::{Context, Result};
use zbus::{Connection, proxy};
use tracing::{debug, warn};

/// Proxy for StatusNotifierItem D-Bus interface
#[proxy(
    interface = "org.kde.StatusNotifierItem",
    default_path = "/StatusNotifierItem"
)]
trait StatusNotifierItem {
    /// Unique ID of this item
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    /// Category (ApplicationStatus, Communications, SystemServices, Hardware)
    #[zbus(property)]
    fn category(&self) -> zbus::Result<String>;

    /// Title for accessibility
    #[zbus(property)]
    fn title(&self) -> zbus::Result<String>;

    /// Status (Passive, Active, NeedsAttention)
    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;

    /// Icon name from theme
    #[zbus(property)]
    fn icon_name(&self) -> zbus::Result<String>;

    /// Icon pixmap data (width, height, ARGB32 data)
    #[zbus(property)]
    fn icon_pixmap(&self) -> zbus::Result<Vec<(i32, i32, Vec<u8>)>>;

    /// Attention icon name
    #[zbus(property)]
    fn attention_icon_name(&self) -> zbus::Result<String>;

    /// Menu object path
    #[zbus(property)]
    fn menu(&self) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;

    /// Request context menu at position
    fn context_menu(&self, x: i32, y: i32) -> zbus::Result<()>;

    /// Primary activation (left click)
    fn activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    /// Secondary activation (middle click)
    fn secondary_activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    /// Scroll event
    fn scroll(&self, delta: i32, orientation: &str) -> zbus::Result<()>;
}

/// Represents a StatusNotifierItem
pub struct SniItem {
    proxy: StatusNotifierItemProxy<'static>,
    /// Cached ID
    pub id: String,
    /// Cached icon name
    pub icon_name: Option<String>,
    /// Cached icon pixmap (width, height, ARGB data)
    pub icon_pixmap: Option<(i32, i32, Vec<u8>)>,
    /// Cached title
    pub title: Option<String>,
    /// Cached status
    pub status: String,
}

impl SniItem {
    /// Create a new SniItem by connecting to its D-Bus interface
    pub async fn new(conn: Connection, bus_name: &str, object_path: &str) -> Result<Self> {
        let proxy = StatusNotifierItemProxy::builder(&conn)
            .destination(bus_name.to_string())?
            .path(object_path.to_string())?
            .build()
            .await
            .context("Failed to create SNI proxy")?;

        // Fetch initial properties
        let id = proxy.id().await.unwrap_or_else(|_| "unknown".to_string());
        let icon_name = proxy.icon_name().await.ok();
        let title = proxy.title().await.ok();
        let status = proxy.status().await.unwrap_or_else(|_| "Active".to_string());

        // Try to get icon pixmap if no icon name
        let icon_pixmap = if icon_name.is_none() || icon_name.as_deref() == Some("") {
            match proxy.icon_pixmap().await {
                Ok(pixmaps) if !pixmaps.is_empty() => {
                    // Get the best resolution (last one is usually highest)
                    let (w, h, data) = pixmaps.into_iter().last().unwrap();
                    Some((w, h, data))
                }
                _ => None,
            }
        } else {
            None
        };

        debug!(
            "SNI item created: id={}, icon={:?}, status={}",
            id, icon_name, status
        );

        Ok(Self {
            proxy,
            id,
            icon_name,
            icon_pixmap,
            title,
            status,
        })
    }

    /// Refresh item properties
    pub async fn refresh(&mut self) -> Result<()> {
        self.icon_name = self.proxy.icon_name().await.ok();
        self.title = self.proxy.title().await.ok();
        self.status = self.proxy.status().await.unwrap_or_else(|_| "Active".to_string());

        // Refresh pixmap if needed
        if self.icon_name.is_none() || self.icon_name.as_deref() == Some("") {
            if let Ok(pixmaps) = self.proxy.icon_pixmap().await {
                if !pixmaps.is_empty() {
                    let (w, h, data) = pixmaps.into_iter().last().unwrap();
                    self.icon_pixmap = Some((w, h, data));
                }
            }
        }

        Ok(())
    }

    /// Handle left click (primary activation)
    pub async fn activate(&self, x: i32, y: i32) {
        if let Err(e) = self.proxy.activate(x, y).await {
            warn!("Failed to activate SNI item {}: {}", self.id, e);
        }
    }

    /// Handle middle click (secondary activation)
    pub async fn secondary_activate(&self, x: i32, y: i32) {
        if let Err(e) = self.proxy.secondary_activate(x, y).await {
            warn!("Failed to secondary activate SNI item {}: {}", self.id, e);
        }
    }

    /// Handle right click (context menu)
    pub async fn context_menu(&self, x: i32, y: i32) {
        if let Err(e) = self.proxy.context_menu(x, y).await {
            warn!("Failed to show context menu for SNI item {}: {}", self.id, e);
        }
    }

    /// Handle scroll
    pub async fn scroll(&self, delta: i32, horizontal: bool) {
        let orientation = if horizontal { "horizontal" } else { "vertical" };
        if let Err(e) = self.proxy.scroll(delta, orientation).await {
            warn!("Failed to scroll SNI item {}: {}", self.id, e);
        }
    }

    /// Get the menu object path if available
    pub async fn menu_path(&self) -> Option<String> {
        self.proxy.menu().await.ok().map(|p| p.to_string())
    }
}
