//! StatusNotifierHost - receives and manages SNI items
//!
//! The host registers with the watcher and receives notifications
//! when items are registered/unregistered.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use anyhow::{Context, Result};
use zbus::Connection;
use tracing::{debug, info, warn, error};

use super::item::SniItem;
use super::watcher::WatcherState;

/// StatusNotifierHost manages SNI items for display
pub struct StatusNotifierHost {
    conn: Connection,
    watcher_state: Arc<Mutex<WatcherState>>,
    /// Active SNI items by service name
    items: HashMap<String, SniItem>,
}

impl StatusNotifierHost {
    /// Create a new host connected to the watcher
    pub async fn new(conn: Connection, watcher_state: Arc<Mutex<WatcherState>>) -> Result<Self> {
        let host = Self {
            conn,
            watcher_state,
            items: HashMap::new(),
        };

        Ok(host)
    }

    /// Register ourselves as a host with the watcher
    pub async fn register(&self) -> Result<()> {
        let unique_name = self.conn.unique_name()
            .context("No unique name on connection")?
            .to_string();

        // Register with the watcher
        {
            let mut state = self.watcher_state.lock().unwrap();
            state.hosts.insert(unique_name.clone());
        }

        info!("Registered as StatusNotifierHost: {}", unique_name);
        Ok(())
    }

    /// Query and add all existing items from the watcher
    pub async fn query_existing_items(&mut self) -> Result<()> {
        let items: Vec<String> = {
            let state = self.watcher_state.lock().unwrap();
            state.items.iter().cloned().collect()
        };

        for service in items {
            if let Err(e) = self.add_item(&service).await {
                warn!("Failed to add existing item {}: {}", service, e);
            }
        }

        Ok(())
    }

    /// Add a new SNI item
    pub async fn add_item(&mut self, service: &str) -> Result<()> {
        if self.items.contains_key(service) {
            debug!("Item already tracked: {}", service);
            return Ok(());
        }

        // Parse service into bus name and object path
        let (bus_name, object_path) = parse_service(service)?;

        info!("Adding SNI item: {} at {}", bus_name, object_path);

        let item = SniItem::new(self.conn.clone(), &bus_name, &object_path).await?;
        self.items.insert(service.to_string(), item);

        Ok(())
    }

    /// Remove an SNI item
    pub fn remove_item(&mut self, service: &str) {
        if self.items.remove(service).is_some() {
            info!("Removed SNI item: {}", service);
        }
    }

    /// Get all current items
    pub fn items(&self) -> impl Iterator<Item = &SniItem> {
        self.items.values()
    }

    /// Get item count
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Handle item registered signal
    pub async fn on_item_registered(&mut self, service: &str) {
        if let Err(e) = self.add_item(service).await {
            error!("Failed to add registered item {}: {}", service, e);
        }
    }

    /// Handle item unregistered signal
    pub fn on_item_unregistered(&mut self, service: &str) {
        self.remove_item(service);
    }

    /// Refresh all items (call periodically to update icons)
    pub async fn refresh_all(&mut self) {
        for item in self.items.values_mut() {
            if let Err(e) = item.refresh().await {
                warn!("Failed to refresh SNI item {}: {}", item.id, e);
            }
        }
    }

    /// Get mutable access to items for updates
    pub fn items_mut(&mut self) -> impl Iterator<Item = &mut SniItem> {
        self.items.values_mut()
    }
}

/// Parse service string into bus name and object path
fn parse_service(service: &str) -> Result<(String, String)> {
    // Service can be in format:
    // - "bus.name" (uses default path /StatusNotifierItem)
    // - "bus.name/path" (explicit path)
    // - ":1.123:/path" (unique name with path)

    if let Some(idx) = service.find(":/") {
        // Has explicit path with colon separator
        let bus_name = &service[..idx];
        let path = &service[idx + 1..];
        Ok((bus_name.to_string(), path.to_string()))
    } else if let Some(idx) = service.rfind('/') {
        // Has path separated by /
        let bus_name = &service[..idx];
        let path = &service[idx..];
        Ok((bus_name.to_string(), path.to_string()))
    } else {
        // Just bus name, use default path
        Ok((service.to_string(), "/StatusNotifierItem".to_string()))
    }
}
