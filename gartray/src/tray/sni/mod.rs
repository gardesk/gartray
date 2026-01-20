//! StatusNotifierItem (SNI) protocol implementation
//!
//! Implements the freedesktop.org StatusNotifierItem specification
//! for modern D-Bus based system tray support.

pub mod watcher;
pub mod host;
pub mod item;

pub use watcher::StatusNotifierWatcher;
pub use host::StatusNotifierHost;
pub use item::SniItem;
