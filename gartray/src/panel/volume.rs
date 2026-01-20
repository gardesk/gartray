//! Volume control module using PulseAudio
//!
//! Provides volume slider and mute toggle for audio output.

use anyhow::{Context, Result};
use libpulse_binding as pulse;
use libpulse_binding::context::Context as PulseContext;
use libpulse_binding::mainloop::standard::Mainloop;
use libpulse_binding::context::subscribe::InterestMaskSet;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

/// Volume state
#[derive(Debug, Clone, Default)]
pub struct VolumeState {
    /// Volume level (0.0 - 1.0)
    pub volume: f64,
    /// Whether muted
    pub muted: bool,
    /// Sink name
    pub sink_name: String,
    /// Sink description
    pub sink_description: String,
}

/// Volume module for controlling PulseAudio
pub struct VolumeModule {
    mainloop: Option<Rc<RefCell<Mainloop>>>,
    context: Option<Rc<RefCell<PulseContext>>>,
    state: Arc<Mutex<VolumeState>>,
    connected: bool,
}

impl VolumeModule {
    /// Create a new volume module
    pub fn new() -> Self {
        Self {
            mainloop: None,
            context: None,
            state: Arc::new(Mutex::new(VolumeState::default())),
            connected: false,
        }
    }

    /// Connect to PulseAudio
    pub fn connect(&mut self) -> Result<()> {
        let mainloop = Rc::new(RefCell::new(
            Mainloop::new().context("Failed to create PulseAudio mainloop")?
        ));

        let context = Rc::new(RefCell::new(
            PulseContext::new(&*mainloop.borrow(), "gartray")
                .context("Failed to create PulseAudio context")?
        ));

        // Connect
        context.borrow_mut()
            .connect(None, pulse::context::FlagSet::NOFLAGS, None)
            .map_err(|_| anyhow::anyhow!("Failed to connect to PulseAudio"))?;

        // Wait for connection
        loop {
            mainloop.borrow_mut().iterate(true);
            match context.borrow().get_state() {
                pulse::context::State::Ready => break,
                pulse::context::State::Failed |
                pulse::context::State::Terminated => {
                    return Err(anyhow::anyhow!("PulseAudio connection failed"));
                }
                _ => {}
            }
        }

        info!("Connected to PulseAudio");
        self.mainloop = Some(mainloop);
        self.context = Some(context);
        self.connected = true;

        // Get initial state
        self.refresh()?;

        Ok(())
    }

    /// Refresh volume state from PulseAudio
    pub fn refresh(&mut self) -> Result<()> {
        if !self.connected {
            return Ok(());
        }

        let context = self.context.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;
        let mainloop = self.mainloop.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No mainloop"))?;

        let state = self.state.clone();

        // Get default sink info
        let introspector = context.borrow().introspect();
        let op = introspector.get_server_info(move |info| {
            if let Some(default_sink) = &info.default_sink_name {
                let mut s = state.lock().unwrap();
                s.sink_name = default_sink.to_string();
            }
        });

        // Wait for operation
        while op.get_state() == pulse::operation::State::Running {
            mainloop.borrow_mut().iterate(true);
        }

        // Now get sink details
        let state = self.state.clone();
        let sink_name = {
            let s = state.lock().unwrap();
            s.sink_name.clone()
        };

        if !sink_name.is_empty() {
            let introspector = context.borrow().introspect();
            let op = introspector.get_sink_info_by_name(&sink_name, move |result| {
                if let pulse::callbacks::ListResult::Item(sink) = result {
                    let mut s = state.lock().unwrap();
                    s.muted = sink.mute;
                    if let Some(desc) = &sink.description {
                        s.sink_description = desc.to_string();
                    }
                    // Calculate average volume
                    let vol = sink.volume.avg().0 as f64 / pulse::volume::Volume::NORMAL.0 as f64;
                    s.volume = vol.min(1.5); // Cap at 150%
                    debug!("Volume: {:.0}%, muted: {}", s.volume * 100.0, s.muted);
                }
            });

            while op.get_state() == pulse::operation::State::Running {
                mainloop.borrow_mut().iterate(true);
            }
        }

        Ok(())
    }

    /// Set volume (0.0 - 1.0)
    pub fn set_volume(&mut self, volume: f64) -> Result<()> {
        if !self.connected {
            return Ok(());
        }

        let context = self.context.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;
        let mainloop = self.mainloop.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No mainloop"))?;

        let sink_name = {
            let s = self.state.lock().unwrap();
            s.sink_name.clone()
        };

        if sink_name.is_empty() {
            return Ok(());
        }

        // Convert to PulseAudio volume
        let pa_vol = (volume.clamp(0.0, 1.5) * pulse::volume::Volume::NORMAL.0 as f64) as u32;
        let mut channel_vol = pulse::volume::ChannelVolumes::default();
        channel_vol.set(2, pulse::volume::Volume(pa_vol));

        let mut introspector = context.borrow().introspect();
        let op = introspector.set_sink_volume_by_name(&sink_name, &channel_vol, None);

        while op.get_state() == pulse::operation::State::Running {
            mainloop.borrow_mut().iterate(true);
        }

        // Update local state
        {
            let mut s = self.state.lock().unwrap();
            s.volume = volume;
        }

        debug!("Set volume to {:.0}%", volume * 100.0);
        Ok(())
    }

    /// Toggle mute
    pub fn toggle_mute(&mut self) -> Result<()> {
        if !self.connected {
            return Ok(());
        }

        let context = self.context.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;
        let mainloop = self.mainloop.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No mainloop"))?;

        let (sink_name, muted) = {
            let s = self.state.lock().unwrap();
            (s.sink_name.clone(), s.muted)
        };

        if sink_name.is_empty() {
            return Ok(());
        }

        let mut introspector = context.borrow().introspect();
        let op = introspector.set_sink_mute_by_name(&sink_name, !muted, None);

        while op.get_state() == pulse::operation::State::Running {
            mainloop.borrow_mut().iterate(true);
        }

        // Update local state
        {
            let mut s = self.state.lock().unwrap();
            s.muted = !muted;
        }

        debug!("Mute toggled to {}", !muted);
        Ok(())
    }

    /// Get current state
    pub fn state(&self) -> VolumeState {
        self.state.lock().unwrap().clone()
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.connected
    }
}

impl Drop for VolumeModule {
    fn drop(&mut self) {
        if let Some(context) = &self.context {
            context.borrow_mut().disconnect();
        }
    }
}
