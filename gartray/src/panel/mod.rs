//! Quick settings panel modules
//!
//! Each module provides a widget for the quick settings popup.

// pub mod volume;
// pub mod brightness;
// pub mod network;
// pub mod bluetooth;
// pub mod battery;
// pub mod power;

/// Panel module trait
pub trait PanelModule {
    /// Module name for configuration
    fn name(&self) -> &'static str;

    /// Update module state
    fn update(&mut self);

    /// Render the module widget
    // fn render(&self, renderer: &mut Renderer, rect: Rect);
}
