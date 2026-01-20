//! D-Bus Menu (com.canonical.dbusmenu) client
//!
//! Fetches and parses menus exposed by SNI items.

use anyhow::{Context, Result};
use std::collections::HashMap;
use zbus::{Connection, proxy};
use tracing::{debug, warn};

/// Proxy for com.canonical.dbusmenu interface
#[proxy(
    interface = "com.canonical.dbusmenu",
    default_path = "/MenuBar"
)]
trait DBusMenu {
    /// Get the menu layout
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<&str>,
    ) -> zbus::Result<(u32, (i32, HashMap<String, zbus::zvariant::OwnedValue>, Vec<zbus::zvariant::OwnedValue>))>;

    /// Get specific properties of an item
    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        property_names: Vec<&str>,
    ) -> zbus::Result<Vec<(i32, HashMap<String, zbus::zvariant::OwnedValue>)>>;

    /// Trigger an event on an item (e.g., "clicked")
    fn event(
        &self,
        id: i32,
        event_id: &str,
        data: zbus::zvariant::Value<'_>,
        timestamp: u32,
    ) -> zbus::Result<()>;

    /// Signal: Layout updated
    #[zbus(signal)]
    fn layout_updated(&self, revision: u32, parent: i32) -> zbus::Result<()>;

    /// Signal: Items properties updated
    #[zbus(signal)]
    fn items_properties_updated(
        &self,
        updated_props: Vec<(i32, HashMap<String, zbus::zvariant::OwnedValue>)>,
        removed_props: Vec<(i32, Vec<String>)>,
    ) -> zbus::Result<()>;
}

/// A menu item
#[derive(Debug, Clone)]
pub struct MenuItem {
    /// Item ID
    pub id: i32,
    /// Label text
    pub label: Option<String>,
    /// Icon name
    pub icon_name: Option<String>,
    /// Whether item is enabled
    pub enabled: bool,
    /// Whether item is visible
    pub visible: bool,
    /// Item type (standard, separator, etc.)
    pub item_type: String,
    /// Toggle type (checkmark, radio)
    pub toggle_type: Option<String>,
    /// Toggle state (0 = off, 1 = on)
    pub toggle_state: i32,
    /// Submenu items
    pub children: Vec<MenuItem>,
}

impl Default for MenuItem {
    fn default() -> Self {
        Self {
            id: 0,
            label: None,
            icon_name: None,
            enabled: true,
            visible: true,
            item_type: "standard".to_string(),
            toggle_type: None,
            toggle_state: 0,
            children: Vec::new(),
        }
    }
}

/// D-Bus menu client
pub struct MenuClient {
    proxy: DBusMenuProxy<'static>,
}

impl MenuClient {
    /// Create a new menu client
    pub async fn new(conn: Connection, bus_name: &str, object_path: &str) -> Result<Self> {
        let proxy = DBusMenuProxy::builder(&conn)
            .destination(bus_name.to_string())?
            .path(object_path.to_string())?
            .build()
            .await
            .context("Failed to create menu proxy")?;

        Ok(Self { proxy })
    }

    /// Fetch the root menu
    pub async fn get_menu(&self) -> Result<MenuItem> {
        let properties = vec![
            "type",
            "label",
            "enabled",
            "visible",
            "icon-name",
            "toggle-type",
            "toggle-state",
            "children-display",
        ];

        let (_, layout) = self.proxy.get_layout(0, -1, properties).await
            .context("Failed to get menu layout")?;

        Ok(parse_menu_item(layout))
    }

    /// Click a menu item
    pub async fn click(&self, id: i32) -> Result<()> {
        self.proxy.event(
            id,
            "clicked",
            zbus::zvariant::Value::new(0i32),
            0,
        ).await.context("Failed to send click event")?;
        Ok(())
    }
}

/// Parse a menu item from D-Bus layout data
fn parse_menu_item(
    data: (i32, HashMap<String, zbus::zvariant::OwnedValue>, Vec<zbus::zvariant::OwnedValue>)
) -> MenuItem {
    let (id, props, children_data) = data;

    let mut item = MenuItem {
        id,
        ..Default::default()
    };

    // Parse properties
    if let Some(v) = props.get("label") {
        if let Ok(s) = <&str>::try_from(v) {
            // Remove underscores used for mnemonics
            item.label = Some(s.replace('_', ""));
        }
    }

    if let Some(v) = props.get("icon-name") {
        if let Ok(s) = <&str>::try_from(v) {
            item.icon_name = Some(s.to_string());
        }
    }

    if let Some(v) = props.get("enabled") {
        if let Ok(b) = <bool>::try_from(v) {
            item.enabled = b;
        }
    }

    if let Some(v) = props.get("visible") {
        if let Ok(b) = <bool>::try_from(v) {
            item.visible = b;
        }
    }

    if let Some(v) = props.get("type") {
        if let Ok(s) = <&str>::try_from(v) {
            item.item_type = s.to_string();
        }
    }

    if let Some(v) = props.get("toggle-type") {
        if let Ok(s) = <&str>::try_from(v) {
            if !s.is_empty() {
                item.toggle_type = Some(s.to_string());
            }
        }
    }

    if let Some(v) = props.get("toggle-state") {
        if let Ok(i) = <i32>::try_from(v) {
            item.toggle_state = i;
        }
    }

    // Children parsing is complex due to recursive structure
    // For now, we'll skip recursive children - they need special handling
    // TODO: Implement recursive menu parsing
    let _ = children_data;

    item
}
