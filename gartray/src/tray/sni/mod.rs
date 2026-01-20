//! StatusNotifierItem (SNI) protocol implementation
//!
//! Implements the freedesktop.org StatusNotifierItem specification
//! for modern D-Bus based system tray support.

mod watcher;
mod host;
mod item;

pub use watcher::StatusNotifierWatcher;
pub use host::StatusNotifierHost;
pub use item::SniItem;
