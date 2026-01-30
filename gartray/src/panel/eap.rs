//! EAP/802.1X authentication types and UI state for enterprise WiFi

/// Supported EAP methods
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EapMethod {
    #[default]
    Peap,
    Ttls,
}

impl EapMethod {
    /// Get the NetworkManager string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            EapMethod::Peap => "peap",
            EapMethod::Ttls => "ttls",
        }
    }

    /// Get the display name for the UI
    pub fn display_name(&self) -> &'static str {
        match self {
            EapMethod::Peap => "PEAP",
            EapMethod::Ttls => "TTLS",
        }
    }

    /// Get all available EAP methods
    pub fn all() -> &'static [EapMethod] {
        &[EapMethod::Peap, EapMethod::Ttls]
    }

    /// Cycle to the next method
    pub fn next(self) -> Self {
        match self {
            EapMethod::Peap => EapMethod::Ttls,
            EapMethod::Ttls => EapMethod::Peap,
        }
    }

    /// Cycle to the previous method
    pub fn prev(self) -> Self {
        match self {
            EapMethod::Peap => EapMethod::Ttls,
            EapMethod::Ttls => EapMethod::Peap,
        }
    }
}

/// Phase 2 (inner) authentication methods
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase2Auth {
    #[default]
    Mschapv2,
    Mschap,
    Pap,
    Chap,
    Gtc,
}

impl Phase2Auth {
    /// Get the NetworkManager string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase2Auth::Mschapv2 => "mschapv2",
            Phase2Auth::Mschap => "mschap",
            Phase2Auth::Pap => "pap",
            Phase2Auth::Chap => "chap",
            Phase2Auth::Gtc => "gtc",
        }
    }

    /// Get the display name for the UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Phase2Auth::Mschapv2 => "MSCHAPv2",
            Phase2Auth::Mschap => "MSCHAP",
            Phase2Auth::Pap => "PAP",
            Phase2Auth::Chap => "CHAP",
            Phase2Auth::Gtc => "GTC",
        }
    }

    /// Get all available Phase 2 methods
    pub fn all() -> &'static [Phase2Auth] {
        &[
            Phase2Auth::Mschapv2,
            Phase2Auth::Mschap,
            Phase2Auth::Pap,
            Phase2Auth::Chap,
            Phase2Auth::Gtc,
        ]
    }

    /// Cycle to the next method
    pub fn next(self) -> Self {
        match self {
            Phase2Auth::Mschapv2 => Phase2Auth::Mschap,
            Phase2Auth::Mschap => Phase2Auth::Pap,
            Phase2Auth::Pap => Phase2Auth::Chap,
            Phase2Auth::Chap => Phase2Auth::Gtc,
            Phase2Auth::Gtc => Phase2Auth::Mschapv2,
        }
    }

    /// Cycle to the previous method
    pub fn prev(self) -> Self {
        match self {
            Phase2Auth::Mschapv2 => Phase2Auth::Gtc,
            Phase2Auth::Mschap => Phase2Auth::Mschapv2,
            Phase2Auth::Pap => Phase2Auth::Mschap,
            Phase2Auth::Chap => Phase2Auth::Pap,
            Phase2Auth::Gtc => Phase2Auth::Chap,
        }
    }
}

/// Form fields for keyboard navigation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EapFormField {
    #[default]
    EapMethod,
    Identity,
    Password,
    AnonymousIdentity,
    CaCertPath,
    Phase2Auth,
    ConnectButton,
    CancelButton,
}

impl EapFormField {
    /// Move to the next field
    pub fn next(self) -> Self {
        match self {
            Self::EapMethod => Self::Identity,
            Self::Identity => Self::Password,
            Self::Password => Self::AnonymousIdentity,
            Self::AnonymousIdentity => Self::CaCertPath,
            Self::CaCertPath => Self::Phase2Auth,
            Self::Phase2Auth => Self::ConnectButton,
            Self::ConnectButton => Self::CancelButton,
            Self::CancelButton => Self::EapMethod,
        }
    }

    /// Move to the previous field
    pub fn prev(self) -> Self {
        match self {
            Self::EapMethod => Self::CancelButton,
            Self::Identity => Self::EapMethod,
            Self::Password => Self::Identity,
            Self::AnonymousIdentity => Self::Password,
            Self::CaCertPath => Self::AnonymousIdentity,
            Self::Phase2Auth => Self::CaCertPath,
            Self::ConnectButton => Self::Phase2Auth,
            Self::CancelButton => Self::ConnectButton,
        }
    }

    /// Check if this field is a text input field
    pub fn is_text_field(self) -> bool {
        matches!(
            self,
            Self::Identity | Self::Password | Self::AnonymousIdentity | Self::CaCertPath
        )
    }

    /// Check if this field is a dropdown selector
    pub fn is_dropdown(self) -> bool {
        matches!(self, Self::EapMethod | Self::Phase2Auth)
    }

    /// Check if this field is a button
    pub fn is_button(self) -> bool {
        matches!(self, Self::ConnectButton | Self::CancelButton)
    }
}

/// Form state for EAP connection dialog
#[derive(Debug, Clone, Default)]
pub struct EapFormState {
    /// Network SSID being configured
    pub ssid: String,
    /// Selected EAP method
    pub eap_method: EapMethod,
    /// Username/Identity
    pub identity: String,
    /// Password
    pub password: String,
    /// Anonymous identity (outer tunnel)
    pub anonymous_identity: String,
    /// CA certificate path
    pub ca_cert_path: String,
    /// Phase 2 authentication method
    pub phase2_auth: Phase2Auth,
    /// Currently focused field (for keyboard nav)
    pub focused_field: EapFormField,
    /// Cursor position in the currently focused text field
    pub cursor_pos: usize,
    /// Tab completion candidates for CA cert path
    pub completions: Vec<String>,
    /// Current completion index
    pub completion_index: usize,
    /// Error message to display
    pub error_message: Option<String>,
    /// Blink counter for cursor animation
    pub blink_counter: u32,
}

impl EapFormState {
    /// Create a new EAP form state for the given SSID
    pub fn new(ssid: String) -> Self {
        Self {
            ssid,
            ..Default::default()
        }
    }

    /// Get the current text value of the focused field
    pub fn current_text(&self) -> &str {
        match self.focused_field {
            EapFormField::Identity => &self.identity,
            EapFormField::Password => &self.password,
            EapFormField::AnonymousIdentity => &self.anonymous_identity,
            EapFormField::CaCertPath => &self.ca_cert_path,
            _ => "",
        }
    }

    /// Get mutable reference to the current text field
    pub fn current_text_mut(&mut self) -> Option<&mut String> {
        match self.focused_field {
            EapFormField::Identity => Some(&mut self.identity),
            EapFormField::Password => Some(&mut self.password),
            EapFormField::AnonymousIdentity => Some(&mut self.anonymous_identity),
            EapFormField::CaCertPath => Some(&mut self.ca_cert_path),
            _ => None,
        }
    }

    /// Insert a character at the cursor position
    pub fn insert_char(&mut self, c: char) {
        let pos = self.cursor_pos;
        let mut inserted = false;
        if let Some(text) = self.current_text_mut() {
            if pos <= text.len() {
                text.insert(pos, c);
                inserted = true;
            }
        }
        if inserted {
            self.cursor_pos += 1;
        }
        self.clear_completions();
    }

    /// Delete the character before the cursor (backspace)
    pub fn backspace(&mut self) {
        let pos = self.cursor_pos;
        if pos > 0 {
            if let Some(text) = self.current_text_mut() {
                if pos <= text.len() {
                    text.remove(pos - 1);
                }
            }
            self.cursor_pos -= 1;
        }
        self.clear_completions();
    }

    /// Delete the character at the cursor (delete key)
    pub fn delete(&mut self) {
        let pos = self.cursor_pos;
        if let Some(text) = self.current_text_mut() {
            if pos < text.len() {
                text.remove(pos);
            }
        }
        self.clear_completions();
    }

    /// Move cursor left
    pub fn cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    /// Move cursor right
    pub fn cursor_right(&mut self) {
        let len = self.current_text().len();
        if self.cursor_pos < len {
            self.cursor_pos += 1;
        }
    }

    /// Move cursor to start
    pub fn cursor_home(&mut self) {
        self.cursor_pos = 0;
    }

    /// Move cursor to end
    pub fn cursor_end(&mut self) {
        self.cursor_pos = self.current_text().len();
    }

    /// Clear any tab completions
    pub fn clear_completions(&mut self) {
        self.completions.clear();
        self.completion_index = 0;
    }

    /// Cycle the EAP method forward
    pub fn cycle_eap_method_next(&mut self) {
        self.eap_method = self.eap_method.next();
    }

    /// Cycle the EAP method backward
    pub fn cycle_eap_method_prev(&mut self) {
        self.eap_method = self.eap_method.prev();
    }

    /// Cycle the Phase2 auth forward
    pub fn cycle_phase2_next(&mut self) {
        self.phase2_auth = self.phase2_auth.next();
    }

    /// Cycle the Phase2 auth backward
    pub fn cycle_phase2_prev(&mut self) {
        self.phase2_auth = self.phase2_auth.prev();
    }

    /// Move focus to the next field
    pub fn focus_next(&mut self) {
        self.focused_field = self.focused_field.next();
        self.update_cursor_for_field();
    }

    /// Move focus to the previous field
    pub fn focus_prev(&mut self) {
        self.focused_field = self.focused_field.prev();
        self.update_cursor_for_field();
    }

    /// Update cursor position when switching fields
    fn update_cursor_for_field(&mut self) {
        self.cursor_pos = self.current_text().len();
        self.clear_completions();
    }

    /// Validate the form and return an error message if invalid
    pub fn validate(&self) -> Option<String> {
        if self.identity.trim().is_empty() {
            return Some("Username is required".to_string());
        }
        if self.password.is_empty() {
            return Some("Password is required".to_string());
        }
        // Validate CA cert path if provided
        if !self.ca_cert_path.is_empty() {
            let path = std::path::Path::new(&self.ca_cert_path);
            if !path.exists() {
                return Some(format!("CA certificate not found: {}", self.ca_cert_path));
            }
        }
        None
    }

    /// Increment blink counter (for cursor animation)
    pub fn tick_blink(&mut self) {
        self.blink_counter = self.blink_counter.wrapping_add(1);
    }

    /// Check if cursor should be visible (blinking)
    pub fn cursor_visible(&self) -> bool {
        (self.blink_counter / 30) % 2 == 0
    }
}
