//! StatusNotifierWatcher D-Bus service
//!
//! This service tracks registered StatusNotifierItem instances and hosts.
//! Apps register their tray items here, and hosts (like gartray) register
//! to receive notifications about items.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use zbus::{interface, Connection, SignalContext};
use tracing::{debug, info, warn};

/// Shared state for the watcher
#[derive(Debug, Default)]
pub struct WatcherState {
    /// Registered StatusNotifierItem services
    pub items: HashSet<String>,
    /// Registered StatusNotifierHost services
    pub hosts: HashSet<String>,
}

/// StatusNotifierWatcher D-Bus interface
pub struct StatusNotifierWatcher {
    state: Arc<Mutex<WatcherState>>,
}

impl StatusNotifierWatcher {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(WatcherState::default())),
        }
    }

    /// Get the shared state
    pub fn state(&self) -> Arc<Mutex<WatcherState>> {
        self.state.clone()
    }
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl StatusNotifierWatcher {
    /// Register a StatusNotifierItem
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(signal_context)] ctx: SignalContext<'_>,
    ) {
        let full_service = if service.starts_with('/') {
            // It's an object path, get the sender's bus name
            format!("{}:{}", ctx.connection().unique_name().unwrap(), service)
        } else {
            service.to_string()
        };

        info!("SNI item registered: {}", full_service);

        {
            let mut state = self.state.lock().unwrap();
            state.items.insert(full_service.clone());
        }

        // Emit signal
        if let Err(e) = Self::status_notifier_item_registered(&ctx, &full_service).await {
            warn!("Failed to emit ItemRegistered signal: {}", e);
        }
    }

    /// Register a StatusNotifierHost
    async fn register_status_notifier_host(
        &self,
        service: &str,
        #[zbus(signal_context)] ctx: SignalContext<'_>,
    ) {
        info!("SNI host registered: {}", service);

        {
            let mut state = self.state.lock().unwrap();
            state.hosts.insert(service.to_string());
        }

        // Emit signal
        if let Err(e) = Self::status_notifier_host_registered(&ctx).await {
            warn!("Failed to emit HostRegistered signal: {}", e);
        }
    }

    /// Get list of registered items
    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        let state = self.state.lock().unwrap();
        state.items.iter().cloned().collect()
    }

    /// Whether a host is registered
    #[zbus(property)]
    async fn is_status_notifier_host_registered(&self) -> bool {
        let state = self.state.lock().unwrap();
        !state.hosts.is_empty()
    }

    /// Protocol version
    #[zbus(property)]
    async fn protocol_version(&self) -> i32 {
        0
    }

    /// Signal: Item registered
    #[zbus(signal)]
    async fn status_notifier_item_registered(
        ctx: &SignalContext<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    /// Signal: Item unregistered
    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        ctx: &SignalContext<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    /// Signal: Host registered
    #[zbus(signal)]
    async fn status_notifier_host_registered(ctx: &SignalContext<'_>) -> zbus::Result<()>;
}

/// Start the StatusNotifierWatcher service
pub async fn start_watcher(conn: &Connection) -> zbus::Result<Arc<Mutex<WatcherState>>> {
    let watcher = StatusNotifierWatcher::new();
    let state = watcher.state();

    // Register the object
    conn.object_server()
        .at("/StatusNotifierWatcher", watcher)
        .await?;

    // Request the well-known name
    conn.request_name("org.kde.StatusNotifierWatcher")
        .await?;

    info!("StatusNotifierWatcher service started");

    Ok(state)
}
