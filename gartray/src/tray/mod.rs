//! System tray implementations
//!
//! Supports both XEMBED (legacy X11) and StatusNotifierItem (modern D-Bus) protocols.

pub mod xembed;

// TODO: Sprint 2
// pub mod sni;
// pub mod icons;
// pub mod menu;

pub use xembed::XEmbedManager;
