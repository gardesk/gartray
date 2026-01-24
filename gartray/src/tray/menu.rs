//! D-Bus Menu (com.canonical.dbusmenu) client
//!
//! Fetches and parses menus exposed by SNI items.
//! Implements the com.canonical.dbusmenu interface.

use anyhow::{Context, Result};
use std::collections::HashMap;
use zbus::{Connection, proxy};
use zbus::zvariant::OwnedValue;
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
    data: (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>)
) -> MenuItem {
    let (id, props, children_data) = data;
    parse_menu_item_inner(id, &props, &children_data)
}

/// Inner recursive menu item parser
fn parse_menu_item_inner(
    id: i32,
    props: &HashMap<String, OwnedValue>,
    children_data: &[OwnedValue]
) -> MenuItem {
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

    // Parse children recursively
    // Each child is a (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>) variant
    for child_value in children_data {
        if let Ok(child_struct) = <&zbus::zvariant::Structure>::try_from(child_value) {
            let fields = child_struct.fields();
            if fields.len() >= 3 {
                // Extract the tuple (id, props, children)
                let child_id = if let Ok(id) = <i32>::try_from(&fields[0]) {
                    id
                } else {
                    continue;
                };

                // Parse properties dict
                let child_props: HashMap<String, OwnedValue> = if let Ok(dict) = <&zbus::zvariant::Dict>::try_from(&fields[1]) {
                    dict.iter()
                        .filter_map(|(k, v)| {
                            let key = <&str>::try_from(k).ok()?.to_string();
                            Some((key, v.try_to_owned().ok()?))
                        })
                        .collect()
                } else {
                    HashMap::new()
                };

                // Parse children array
                let grandchildren: Vec<OwnedValue> = if let Ok(arr) = <&zbus::zvariant::Array>::try_from(&fields[2]) {
                    arr.iter()
                        .filter_map(|v| v.try_to_owned().ok())
                        .collect()
                } else {
                    Vec::new()
                };

                // Recursively parse the child
                let child_item = parse_menu_item_inner(child_id, &child_props, &grandchildren);
                if child_item.visible {
                    item.children.push(child_item);
                }
            }
        }
    }

    item
}

/// Get a human-readable summary of a menu tree (for debugging)
pub fn menu_summary(item: &MenuItem, depth: usize) -> String {
    let indent = "  ".repeat(depth);
    let mut result = format!(
        "{}[{}] {} (type={}, enabled={}, toggle={:?}:{})\n",
        indent,
        item.id,
        item.label.as_deref().unwrap_or("(no label)"),
        item.item_type,
        item.enabled,
        item.toggle_type,
        item.toggle_state,
    );

    for child in &item.children {
        result.push_str(&menu_summary(child, depth + 1));
    }

    result
}
