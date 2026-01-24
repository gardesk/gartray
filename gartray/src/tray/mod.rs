//! System tray implementations
//!
//! Supports both XEMBED (legacy X11) and StatusNotifierItem (modern D-Bus) protocols.

pub mod xembed;
pub mod sni;
pub mod icons;
pub mod renderer;
pub mod menu;
pub mod menu_popup;

pub use xembed::{XEmbedManager, TrayButtonEvent};
pub use sni::{StatusNotifierWatcher, StatusNotifierHost, SniItem};
pub use icons::IconData;
pub use renderer::{TrayRenderer, SniHitResult};
pub use menu::{MenuClient, MenuItem};
pub use menu_popup::{MenuPopup, MenuAction};
