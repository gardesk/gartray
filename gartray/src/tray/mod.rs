//! System tray implementations
//!
//! Supports both XEMBED (legacy X11) and StatusNotifierItem (modern D-Bus) protocols.

pub mod xembed;
pub mod sni;

// TODO: Sprint 3+
// pub mod icons;
// pub mod menu;

pub use xembed::XEmbedManager;
pub use sni::{StatusNotifierWatcher, StatusNotifierHost, SniItem};
