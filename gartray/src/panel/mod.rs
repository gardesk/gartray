//! Quick settings panel modules
//!
//! Each module provides a widget for the quick settings popup.

pub mod popup;
pub mod volume;
pub mod brightness;
pub mod battery;
pub mod power;
pub mod network;
pub mod bluetooth;
pub mod dnd;

pub use popup::PopupPanel;
pub use volume::VolumeModule;
pub use brightness::BrightnessModule;
pub use battery::BatteryModule;
pub use power::PowerModule;
pub use network::NetworkModule;
pub use bluetooth::BluetoothModule;
pub use dnd::DndModule;

/// Panel module trait
pub trait PanelModule {
    /// Module name for configuration
    fn name(&self) -> &'static str;

    /// Update module state
    fn update(&mut self);
}
