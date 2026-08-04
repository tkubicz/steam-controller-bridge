//! Configurable Steam Controller 2 desktop-input bindings.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use steam_controller_protocol::{SteamButton, SteamButtons};

pub const BINDINGS_VERSION: u32 = 1;
pub const MAX_PROFILES: usize = 32;
pub const DEFAULT_PROFILE_ID: &str = "default";
pub const DEFAULT_PROFILE_NAME: &str = "Default";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindableControl {
    L4,
    L5,
    R4,
    R5,
    QuickAccess,
}

impl BindableControl {
    pub const ALL: [Self; 5] = [Self::L4, Self::L5, Self::R4, Self::R5, Self::QuickAccess];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::L4 => "L4",
            Self::L5 => "L5",
            Self::R4 => "R4",
            Self::R5 => "R5",
            Self::QuickAccess => "Quick Access",
        }
    }

    #[must_use]
    pub const fn steam_button(self) -> SteamButton {
        match self {
            Self::L4 => SteamButton::LeftGrip4,
            Self::L5 => SteamButton::LeftGrip5,
            Self::R4 => SteamButton::RightGrip4,
            Self::R5 => SteamButton::RightGrip5,
            Self::QuickAccess => SteamButton::QuickAccess,
        }
    }

    const fn mask(self) -> u8 {
        1 << self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    Command,
    Control,
    Option,
    Shift,
}

impl Modifier {
    pub const ALL: [Self; 4] = [Self::Command, Self::Control, Self::Option, Self::Shift];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Command => "Command",
            Self::Control => "Control",
            Self::Option => "Option",
            Self::Shift => "Shift",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum KeyboardKey {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    Escape,
    Tab,
    Return,
    Space,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Grave,
    Minus,
    Equal,
    LeftBracket,
    RightBracket,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    NumpadSubtract,
    NumpadMultiply,
    NumpadDivide,
    NumpadDecimal,
    NumpadEnter,
    MediaPlayPause,
    MediaPrevious,
    MediaNext,
    VolumeMute,
    VolumeDown,
    VolumeUp,
}

impl KeyboardKey {
    pub const ALL: &'static [Self] = &[
        Self::A,
        Self::B,
        Self::C,
        Self::D,
        Self::E,
        Self::F,
        Self::G,
        Self::H,
        Self::I,
        Self::J,
        Self::K,
        Self::L,
        Self::M,
        Self::N,
        Self::O,
        Self::P,
        Self::Q,
        Self::R,
        Self::S,
        Self::T,
        Self::U,
        Self::V,
        Self::W,
        Self::X,
        Self::Y,
        Self::Z,
        Self::Digit0,
        Self::Digit1,
        Self::Digit2,
        Self::Digit3,
        Self::Digit4,
        Self::Digit5,
        Self::Digit6,
        Self::Digit7,
        Self::Digit8,
        Self::Digit9,
        Self::F1,
        Self::F2,
        Self::F3,
        Self::F4,
        Self::F5,
        Self::F6,
        Self::F7,
        Self::F8,
        Self::F9,
        Self::F10,
        Self::F11,
        Self::F12,
        Self::F13,
        Self::F14,
        Self::F15,
        Self::F16,
        Self::F17,
        Self::F18,
        Self::F19,
        Self::F20,
        Self::F21,
        Self::F22,
        Self::F23,
        Self::F24,
        Self::Escape,
        Self::Tab,
        Self::Return,
        Self::Space,
        Self::Backspace,
        Self::Delete,
        Self::Insert,
        Self::Home,
        Self::End,
        Self::PageUp,
        Self::PageDown,
        Self::ArrowLeft,
        Self::ArrowRight,
        Self::ArrowUp,
        Self::ArrowDown,
        Self::Grave,
        Self::Minus,
        Self::Equal,
        Self::LeftBracket,
        Self::RightBracket,
        Self::Backslash,
        Self::Semicolon,
        Self::Quote,
        Self::Comma,
        Self::Period,
        Self::Slash,
        Self::Numpad0,
        Self::Numpad1,
        Self::Numpad2,
        Self::Numpad3,
        Self::Numpad4,
        Self::Numpad5,
        Self::Numpad6,
        Self::Numpad7,
        Self::Numpad8,
        Self::Numpad9,
        Self::NumpadAdd,
        Self::NumpadSubtract,
        Self::NumpadMultiply,
        Self::NumpadDivide,
        Self::NumpadDecimal,
        Self::NumpadEnter,
        Self::MediaPlayPause,
        Self::MediaPrevious,
        Self::MediaNext,
        Self::VolumeMute,
        Self::VolumeDown,
        Self::VolumeUp,
    ];

    #[must_use]
    pub fn label(self) -> String {
        let debug = format!("{self:?}");
        debug.strip_prefix("Digit").unwrap_or(&debug).to_owned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl MouseButton {
    pub const ALL: [Self; 5] = [
        Self::Left,
        Self::Right,
        Self::Middle,
        Self::Back,
        Self::Forward,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Middle => "Middle",
            Self::Back => "Back",
            Self::Forward => "Forward",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BindingAction {
    KeyChord {
        key: KeyboardKey,
        #[serde(default)]
        modifiers: BTreeSet<Modifier>,
    },
    MouseButton {
        button: MouseButton,
    },
}

impl BindingAction {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::KeyChord { key, modifiers } => {
                let mut parts = Modifier::ALL
                    .into_iter()
                    .filter(|modifier| modifiers.contains(modifier))
                    .map(|modifier| modifier.label().to_owned())
                    .collect::<Vec<_>>();
                parts.push(key.label());
                parts.join("+")
            }
            Self::MouseButton { button } => format!("Mouse {}", button.label()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ControlBindings {
    pub l4: Option<BindingAction>,
    pub l5: Option<BindingAction>,
    pub r4: Option<BindingAction>,
    pub r5: Option<BindingAction>,
    pub quick_access: Option<BindingAction>,
}

impl ControlBindings {
    #[must_use]
    pub const fn get(&self, control: BindableControl) -> Option<&BindingAction> {
        match control {
            BindableControl::L4 => self.l4.as_ref(),
            BindableControl::L5 => self.l5.as_ref(),
            BindableControl::R4 => self.r4.as_ref(),
            BindableControl::R5 => self.r5.as_ref(),
            BindableControl::QuickAccess => self.quick_access.as_ref(),
        }
    }

    pub fn get_mut(&mut self, control: BindableControl) -> &mut Option<BindingAction> {
        match control {
            BindableControl::L4 => &mut self.l4,
            BindableControl::L5 => &mut self.l5,
            BindableControl::R4 => &mut self.r4,
            BindableControl::R5 => &mut self.r5,
            BindableControl::QuickAccess => &mut self.quick_access,
        }
    }

    #[must_use]
    pub fn configured_count(&self) -> usize {
        BindableControl::ALL
            .into_iter()
            .filter(|control| self.get(*control).is_some())
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub bindings: ControlBindings,
}

impl Default for BindingProfile {
    fn default() -> Self {
        Self {
            id: DEFAULT_PROFILE_ID.to_owned(),
            name: DEFAULT_PROFILE_NAME.to_owned(),
            bindings: ControlBindings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingStore {
    pub version: u32,
    pub profiles: Vec<BindingProfile>,
}

impl Default for BindingStore {
    fn default() -> Self {
        Self {
            version: BINDINGS_VERSION,
            profiles: vec![BindingProfile::default()],
        }
    }
}

impl BindingStore {
    /// Validates the complete persisted store.
    ///
    /// # Errors
    /// Returns a descriptive error for unsupported versions, invalid names or IDs,
    /// duplicate profiles, or an invalid profile count.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != BINDINGS_VERSION {
            return Err(format!("unsupported bindings version {}", self.version));
        }
        if self.profiles.is_empty() || self.profiles.len() > MAX_PROFILES {
            return Err(format!(
                "bindings must contain 1 to {MAX_PROFILES} profiles"
            ));
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for profile in &self.profiles {
            if profile.id.is_empty()
                || profile.id.len() > 64
                || !profile
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(format!("invalid profile ID {:?}", profile.id));
            }
            let trimmed = profile.name.trim();
            if trimmed.is_empty() || trimmed.chars().count() > 48 || trimmed != profile.name {
                return Err(format!("invalid profile name {:?}", profile.name));
            }
            if !ids.insert(profile.id.to_ascii_lowercase()) {
                return Err(format!("duplicate profile ID {:?}", profile.id));
            }
            if !names.insert(profile.name.to_lowercase()) {
                return Err(format!("duplicate profile name {:?}", profile.name));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn profile_by_id(&self, id: &str) -> Option<&BindingProfile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    #[must_use]
    pub fn profile_by_name(&self, name: &str) -> Option<&BindingProfile> {
        let folded = name.to_lowercase();
        self.profiles
            .iter()
            .find(|profile| profile.name.to_lowercase() == folded)
    }

    #[must_use]
    pub fn next_profile_id(&self) -> String {
        for suffix in 1_u32.. {
            let candidate = format!("profile-{suffix}");
            if self.profile_by_id(&candidate).is_none() {
                return candidate;
            }
        }
        unreachable!("an unbounded suffix space always has a free profile ID")
    }

    /// Creates an empty profile and returns its immutable generated ID.
    ///
    /// # Errors
    /// Returns an error when the profile limit or name constraints are violated.
    pub fn create_profile(&mut self, name: &str) -> Result<String, String> {
        if self.profiles.len() == MAX_PROFILES {
            return Err(format!("at most {MAX_PROFILES} profiles are supported"));
        }
        let name = self.available_name(name, None)?;
        let id = self.next_profile_id();
        self.profiles.push(BindingProfile {
            id: id.clone(),
            name,
            bindings: ControlBindings::default(),
        });
        Ok(id)
    }

    /// Duplicates a profile under a new immutable generated ID.
    ///
    /// # Errors
    /// Returns an error when the source is missing or the limit/name is invalid.
    pub fn duplicate_profile(&mut self, source_id: &str, name: &str) -> Result<String, String> {
        if self.profiles.len() == MAX_PROFILES {
            return Err(format!("at most {MAX_PROFILES} profiles are supported"));
        }
        let source = self
            .profile_by_id(source_id)
            .cloned()
            .ok_or_else(|| format!("profile {source_id:?} does not exist"))?;
        let name = self.available_name(name, None)?;
        let id = self.next_profile_id();
        self.profiles.push(BindingProfile {
            id: id.clone(),
            name,
            bindings: source.bindings,
        });
        Ok(id)
    }

    /// Renames a profile without changing its persisted ID.
    ///
    /// # Errors
    /// Returns an error when the profile is missing or the new name is invalid.
    pub fn rename_profile(&mut self, id: &str, name: &str) -> Result<(), String> {
        let name = self.available_name(name, Some(id))?;
        let profile = self
            .profiles
            .iter_mut()
            .find(|profile| profile.id == id)
            .ok_or_else(|| format!("profile {id:?} does not exist"))?;
        profile.name = name;
        Ok(())
    }

    /// Deletes a profile while preserving the required final profile.
    ///
    /// # Errors
    /// Returns an error when the profile is missing or is the final profile.
    pub fn delete_profile(&mut self, id: &str) -> Result<BindingProfile, String> {
        if self.profiles.len() == 1 {
            return Err("the final binding profile cannot be deleted".to_owned());
        }
        let index = self
            .profiles
            .iter()
            .position(|profile| profile.id == id)
            .ok_or_else(|| format!("profile {id:?} does not exist"))?;
        Ok(self.profiles.remove(index))
    }

    fn available_name(&self, name: &str, excluding_id: Option<&str>) -> Result<String, String> {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 48 {
            return Err("profile names must contain 1 to 48 characters".to_owned());
        }
        if self.profiles.iter().any(|profile| {
            Some(profile.id.as_str()) != excluding_id && profile.name.eq_ignore_ascii_case(trimmed)
        }) {
            return Err(format!("duplicate profile name {trimmed:?}"));
        }
        Ok(trimmed.to_owned())
    }
}

/// Returns the standard per-user bindings file location.
///
/// # Errors
/// Returns an error if `HOME` is unavailable.
pub fn default_store_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set; cannot locate the bindings directory")?;
    Ok(home.join("Library/Application Support/Steam Controller Bridge/bindings.json"))
}

/// Loads and validates a binding store.
///
/// # Errors
/// Returns an error when the file cannot be read, decoded, or validated.
pub fn load_store(path: &Path) -> Result<BindingStore, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read '{}': {error}", path.display()))?;
    let store: BindingStore = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid bindings JSON in '{}': {error}", path.display()))?;
    store.validate()?;
    Ok(store)
}

/// Loads a store or atomically creates the all-unbound default when missing.
///
/// # Errors
/// Returns an error for I/O, serialization, or validation failures.
pub fn load_or_create_store(path: &Path) -> Result<BindingStore, String> {
    match load_store(path) {
        Ok(store) => Ok(store),
        Err(_) if !path.exists() => {
            let store = BindingStore::default();
            save_store(path, &store)?;
            Ok(store)
        }
        Err(error) => Err(error),
    }
}

/// Validates and atomically saves a binding store.
///
/// # Errors
/// Returns an error for invalid stores or failed directory, write, or rename operations.
pub fn save_store(path: &Path, store: &BindingStore) -> Result<(), String> {
    store.validate()?;
    let directory = path
        .parent()
        .ok_or_else(|| format!("bindings path '{}' has no parent", path.display()))?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temporary = path.with_extension(format!("json.{}.{nonce}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(store).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OutputKey {
    Modifier(Modifier),
    Key(KeyboardKey),
}

pub trait DesktopInputSink {
    /// Emits a keyboard transition.
    ///
    /// # Errors
    /// Returns an error if the platform cannot inject the transition.
    fn key(&mut self, key: KeyboardKey, pressed: bool) -> Result<(), String>;

    /// Emits a modifier-key transition.
    ///
    /// # Errors
    /// Returns an error if the platform cannot inject the transition.
    fn modifier(&mut self, modifier: Modifier, pressed: bool) -> Result<(), String>;

    /// Emits a mouse-button transition.
    ///
    /// # Errors
    /// Returns an error if the platform cannot inject the transition.
    fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> Result<(), String>;
}

pub struct BindingEngine {
    profile: BindingProfile,
    previous_mask: Option<u8>,
    blocked_mask: u8,
    active: BTreeMap<BindableControl, BindingAction>,
    key_counts: BTreeMap<OutputKey, u16>,
    mouse_counts: BTreeMap<MouseButton, u16>,
}

impl BindingEngine {
    #[must_use]
    pub fn new(profile: BindingProfile) -> Self {
        Self {
            profile,
            previous_mask: None,
            blocked_mask: 0,
            active: BTreeMap::new(),
            key_counts: BTreeMap::new(),
            mouse_counts: BTreeMap::new(),
        }
    }

    #[must_use]
    pub const fn profile(&self) -> &BindingProfile {
        &self.profile
    }

    #[must_use]
    pub fn held_output_count(&self) -> usize {
        self.key_counts.len() + self.mouse_counts.len()
    }

    /// Observes a full controller button snapshot and emits its binding edges.
    ///
    /// The first snapshot is a non-emitting baseline. Any sink error triggers a
    /// best-effort release and blocks held controls until they are released.
    ///
    /// # Errors
    /// Returns the first desktop-input injection failure.
    pub fn observe(
        &mut self,
        buttons: SteamButtons,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        let mask = bindable_mask(buttons);
        let Some(previous) = self.previous_mask else {
            self.previous_mask = Some(mask);
            self.blocked_mask = mask;
            return Ok(());
        };
        self.blocked_mask &= mask;
        let changed = previous ^ mask;
        let result = self.apply_changes(changed, mask, sink);
        self.previous_mask = Some(mask);
        if let Err(error) = result {
            let _ = self.release_all(sink);
            self.blocked_mask = mask;
            return Err(error);
        }
        Ok(())
    }

    /// Releases the old profile and installs a replacement without synthesizing
    /// presses for controls already held.
    ///
    /// # Errors
    /// Returns an error if releasing an old desktop input fails.
    pub fn replace_profile(
        &mut self,
        profile: BindingProfile,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        let held = self.previous_mask.unwrap_or_default();
        let release = self.release_all(sink);
        self.profile = profile;
        self.blocked_mask = held;
        release
    }

    /// Releases all outputs and forgets the source baseline.
    ///
    /// # Errors
    /// Returns the first failed release after attempting every held output.
    pub fn disconnect(&mut self, sink: &mut dyn DesktopInputSink) -> Result<(), String> {
        let result = self.release_all(sink);
        self.previous_mask = None;
        self.blocked_mask = 0;
        result
    }

    fn apply_changes(
        &mut self,
        changed: u8,
        current: u8,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        for control in BindableControl::ALL {
            if changed & control.mask() != 0 && current & control.mask() == 0 {
                if let Some(action) = self.active.remove(&control) {
                    self.release_action(&action, sink)?;
                }
            }
        }
        for control in BindableControl::ALL {
            if changed & control.mask() == 0
                || current & control.mask() == 0
                || self.blocked_mask & control.mask() != 0
            {
                continue;
            }
            if let Some(action) = self.profile.bindings.get(control).cloned() {
                self.press_action(&action, sink)?;
                self.active.insert(control, action);
            }
        }
        Ok(())
    }

    fn press_action(
        &mut self,
        action: &BindingAction,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        match action {
            BindingAction::KeyChord { key, modifiers } => {
                for modifier in Modifier::ALL {
                    if modifiers.contains(&modifier) {
                        self.press_key(OutputKey::Modifier(modifier), sink)?;
                    }
                }
                self.press_key(OutputKey::Key(*key), sink)
            }
            BindingAction::MouseButton { button } => self.press_mouse(*button, sink),
        }
    }

    fn release_action(
        &mut self,
        action: &BindingAction,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        match action {
            BindingAction::KeyChord { key, modifiers } => {
                self.release_key(OutputKey::Key(*key), sink)?;
                for modifier in Modifier::ALL.into_iter().rev() {
                    if modifiers.contains(&modifier) {
                        self.release_key(OutputKey::Modifier(modifier), sink)?;
                    }
                }
                Ok(())
            }
            BindingAction::MouseButton { button } => self.release_mouse(*button, sink),
        }
    }

    fn press_key(
        &mut self,
        output: OutputKey,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        let count = self.key_counts.entry(output).or_default();
        if *count == 0 {
            emit_key(sink, output, true)?;
        }
        *count = count.saturating_add(1);
        Ok(())
    }

    fn release_key(
        &mut self,
        output: OutputKey,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        let Some(count) = self.key_counts.get_mut(&output) else {
            return Ok(());
        };
        *count -= 1;
        if *count == 0 {
            emit_key(sink, output, false)?;
            self.key_counts.remove(&output);
        }
        Ok(())
    }

    fn press_mouse(
        &mut self,
        button: MouseButton,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        let count = self.mouse_counts.entry(button).or_default();
        if *count == 0 {
            sink.mouse_button(button, true)?;
        }
        *count = count.saturating_add(1);
        Ok(())
    }

    fn release_mouse(
        &mut self,
        button: MouseButton,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        let Some(count) = self.mouse_counts.get_mut(&button) else {
            return Ok(());
        };
        *count -= 1;
        if *count == 0 {
            sink.mouse_button(button, false)?;
            self.mouse_counts.remove(&button);
        }
        Ok(())
    }

    fn release_all(&mut self, sink: &mut dyn DesktopInputSink) -> Result<(), String> {
        let mut first_error = None;
        for (button, _) in std::mem::take(&mut self.mouse_counts) {
            if let Err(error) = sink.mouse_button(button, false) {
                first_error.get_or_insert(error);
            }
        }
        let keys = std::mem::take(&mut self.key_counts);
        for (key, _) in keys.iter().rev() {
            if let Err(error) = emit_key(sink, *key, false) {
                first_error.get_or_insert(error);
            }
        }
        self.active.clear();
        first_error.map_or(Ok(()), Err)
    }
}

fn emit_key(
    sink: &mut dyn DesktopInputSink,
    output: OutputKey,
    pressed: bool,
) -> Result<(), String> {
    match output {
        OutputKey::Modifier(modifier) => sink.modifier(modifier, pressed),
        OutputKey::Key(key) => sink.key(key, pressed),
    }
}

#[must_use]
pub fn bindable_mask(buttons: SteamButtons) -> u8 {
    BindableControl::ALL
        .into_iter()
        .fold(0_u8, |mask, control| {
            if buttons.contains(control.steam_button()) {
                mask | control.mask()
            } else {
                mask
            }
        })
}

#[cfg(target_os = "macos")]
mod macos {
    use enigo::{Button as EnigoButton, Direction, Enigo, Key, Keyboard, Mouse, Settings};
    use objc2_core_graphics::{CGPreflightPostEventAccess, CGRequestPostEventAccess};

    use super::{DesktopInputSink, KeyboardKey, Modifier, MouseButton};

    pub struct MacOsDesktopInput {
        enigo: Enigo,
    }

    impl MacOsDesktopInput {
        /// Opens the macOS desktop-input connection.
        ///
        /// # Errors
        /// Returns `permission required` when Accessibility is unavailable, or
        /// another backend construction error.
        pub fn new() -> Result<Self, String> {
            Self::new_with_prompt(false)
        }

        fn new_with_prompt(prompt_for_permission: bool) -> Result<Self, String> {
            let settings = Settings {
                // Permission requests are allowed only through the explicit
                // main-thread helper below. Runtime workers stay non-prompting.
                open_prompt_to_get_permissions: prompt_for_permission,
                release_keys_when_dropped: true,
                ..Settings::default()
            };
            Enigo::new(&settings)
                .map(|enigo| Self { enigo })
                .map_err(|error| match error {
                    enigo::NewConError::NoPermission => {
                        "Accessibility permission required".to_owned()
                    }
                    other => format!("cannot initialize desktop input: {other}"),
                })
        }
    }

    /// Returns whether macOS currently permits this process to post input events.
    #[must_use]
    pub fn preflight_post_event_access() -> bool {
        CGPreflightPostEventAccess()
    }

    /// Makes macOS's native request for permission to post input events.
    ///
    /// This is the `PostEvent` counterpart to the `ListenEvent` request used for
    /// Input Monitoring. It must be called by an interactive macOS frontend.
    #[must_use]
    pub fn request_post_event_access() -> bool {
        CGRequestPostEventAccess()
    }

    /// Checks whether the Enigo adapter's Accessibility trust is available.
    #[must_use]
    pub fn preflight_accessibility_access() -> bool {
        MacOsDesktopInput::new_with_prompt(false).is_ok()
    }

    /// Requests the Accessibility trust required by the Enigo adapter.
    ///
    /// The menu app calls this on its main thread after creating its native
    /// status item. Keeping it out of runtime workers makes the system prompt
    /// reliably attributable to the foreground application bundle.
    #[must_use]
    pub fn request_accessibility_access() -> bool {
        MacOsDesktopInput::new_with_prompt(true).is_ok()
    }

    impl DesktopInputSink for MacOsDesktopInput {
        fn key(&mut self, key: KeyboardKey, pressed: bool) -> Result<(), String> {
            let key = enigo_key(key)?;
            self.enigo
                .key(key, direction(pressed))
                .map_err(|error| error.to_string())
        }

        fn modifier(&mut self, modifier: Modifier, pressed: bool) -> Result<(), String> {
            let key = modifier_key(modifier);
            self.enigo
                .key(key, direction(pressed))
                .map_err(|error| error.to_string())
        }

        fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> Result<(), String> {
            let button = enigo_button(button);
            self.enigo
                .button(button, direction(pressed))
                .map_err(|error| error.to_string())
        }
    }

    const fn direction(pressed: bool) -> Direction {
        if pressed {
            Direction::Press
        } else {
            Direction::Release
        }
    }

    const fn modifier_key(modifier: Modifier) -> Key {
        match modifier {
            Modifier::Command => Key::Meta,
            Modifier::Control => Key::Control,
            Modifier::Option => Key::Option,
            Modifier::Shift => Key::Shift,
        }
    }

    const fn enigo_button(button: MouseButton) -> EnigoButton {
        match button {
            MouseButton::Left => EnigoButton::Left,
            MouseButton::Right => EnigoButton::Right,
            MouseButton::Middle => EnigoButton::Middle,
            MouseButton::Back => EnigoButton::Back,
            MouseButton::Forward => EnigoButton::Forward,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn enigo_key(key: KeyboardKey) -> Result<Key, String> {
        let key = match key {
            KeyboardKey::A => Key::Unicode('a'),
            KeyboardKey::B => Key::Unicode('b'),
            KeyboardKey::C => Key::Unicode('c'),
            KeyboardKey::D => Key::Unicode('d'),
            KeyboardKey::E => Key::Unicode('e'),
            KeyboardKey::F => Key::Unicode('f'),
            KeyboardKey::G => Key::Unicode('g'),
            KeyboardKey::H => Key::Unicode('h'),
            KeyboardKey::I => Key::Unicode('i'),
            KeyboardKey::J => Key::Unicode('j'),
            KeyboardKey::K => Key::Unicode('k'),
            KeyboardKey::L => Key::Unicode('l'),
            KeyboardKey::M => Key::Unicode('m'),
            KeyboardKey::N => Key::Unicode('n'),
            KeyboardKey::O => Key::Unicode('o'),
            KeyboardKey::P => Key::Unicode('p'),
            KeyboardKey::Q => Key::Unicode('q'),
            KeyboardKey::R => Key::Unicode('r'),
            KeyboardKey::S => Key::Unicode('s'),
            KeyboardKey::T => Key::Unicode('t'),
            KeyboardKey::U => Key::Unicode('u'),
            KeyboardKey::V => Key::Unicode('v'),
            KeyboardKey::W => Key::Unicode('w'),
            KeyboardKey::X => Key::Unicode('x'),
            KeyboardKey::Y => Key::Unicode('y'),
            KeyboardKey::Z => Key::Unicode('z'),
            KeyboardKey::Digit0 => Key::Unicode('0'),
            KeyboardKey::Digit1 => Key::Unicode('1'),
            KeyboardKey::Digit2 => Key::Unicode('2'),
            KeyboardKey::Digit3 => Key::Unicode('3'),
            KeyboardKey::Digit4 => Key::Unicode('4'),
            KeyboardKey::Digit5 => Key::Unicode('5'),
            KeyboardKey::Digit6 => Key::Unicode('6'),
            KeyboardKey::Digit7 => Key::Unicode('7'),
            KeyboardKey::Digit8 => Key::Unicode('8'),
            KeyboardKey::Digit9 => Key::Unicode('9'),
            KeyboardKey::F1 => Key::F1,
            KeyboardKey::F2 => Key::F2,
            KeyboardKey::F3 => Key::F3,
            KeyboardKey::F4 => Key::F4,
            KeyboardKey::F5 => Key::F5,
            KeyboardKey::F6 => Key::F6,
            KeyboardKey::F7 => Key::F7,
            KeyboardKey::F8 => Key::F8,
            KeyboardKey::F9 => Key::F9,
            KeyboardKey::F10 => Key::F10,
            KeyboardKey::F11 => Key::F11,
            KeyboardKey::F12 => Key::F12,
            KeyboardKey::F13 => Key::F13,
            KeyboardKey::F14 => Key::F14,
            KeyboardKey::F15 => Key::F15,
            KeyboardKey::F16 => Key::F16,
            KeyboardKey::F17 => Key::F17,
            KeyboardKey::F18 => Key::F18,
            KeyboardKey::F19 => Key::F19,
            KeyboardKey::F20 => Key::F20,
            KeyboardKey::F21 | KeyboardKey::F22 | KeyboardKey::F23 | KeyboardKey::F24 => {
                return Err(format!(
                    "{} is not available through the macOS keyboard event API",
                    key.label()
                ));
            }
            KeyboardKey::Escape => Key::Escape,
            KeyboardKey::Tab => Key::Tab,
            KeyboardKey::Return => Key::Return,
            KeyboardKey::NumpadEnter => Key::Other(76),
            KeyboardKey::Space => Key::Space,
            KeyboardKey::Backspace => Key::Backspace,
            KeyboardKey::Delete => Key::Delete,
            // macOS exposes the legacy Help/Insert physical key as Help.
            KeyboardKey::Insert => Key::Help,
            KeyboardKey::Home => Key::Home,
            KeyboardKey::End => Key::End,
            KeyboardKey::PageUp => Key::PageUp,
            KeyboardKey::PageDown => Key::PageDown,
            KeyboardKey::ArrowLeft => Key::LeftArrow,
            KeyboardKey::ArrowRight => Key::RightArrow,
            KeyboardKey::ArrowUp => Key::UpArrow,
            KeyboardKey::ArrowDown => Key::DownArrow,
            KeyboardKey::Grave => Key::Unicode('`'),
            KeyboardKey::Minus => Key::Unicode('-'),
            KeyboardKey::Equal => Key::Unicode('='),
            KeyboardKey::LeftBracket => Key::Unicode('['),
            KeyboardKey::RightBracket => Key::Unicode(']'),
            KeyboardKey::Backslash => Key::Unicode('\\'),
            KeyboardKey::Semicolon => Key::Unicode(';'),
            KeyboardKey::Quote => Key::Unicode('\''),
            KeyboardKey::Comma => Key::Unicode(','),
            KeyboardKey::Period => Key::Unicode('.'),
            KeyboardKey::Slash => Key::Unicode('/'),
            KeyboardKey::Numpad0 => Key::Numpad0,
            KeyboardKey::Numpad1 => Key::Numpad1,
            KeyboardKey::Numpad2 => Key::Numpad2,
            KeyboardKey::Numpad3 => Key::Numpad3,
            KeyboardKey::Numpad4 => Key::Numpad4,
            KeyboardKey::Numpad5 => Key::Numpad5,
            KeyboardKey::Numpad6 => Key::Numpad6,
            KeyboardKey::Numpad7 => Key::Numpad7,
            KeyboardKey::Numpad8 => Key::Numpad8,
            KeyboardKey::Numpad9 => Key::Numpad9,
            KeyboardKey::NumpadAdd => Key::Add,
            KeyboardKey::NumpadSubtract => Key::Subtract,
            KeyboardKey::NumpadMultiply => Key::Multiply,
            KeyboardKey::NumpadDivide => Key::Divide,
            KeyboardKey::NumpadDecimal => Key::Decimal,
            KeyboardKey::MediaPlayPause | KeyboardKey::MediaPrevious | KeyboardKey::MediaNext => {
                return Err(format!(
                    "{} is not available through Enigo's macOS adapter",
                    key.label()
                ));
            }
            KeyboardKey::VolumeMute => Key::VolumeMute,
            KeyboardKey::VolumeDown => Key::VolumeDown,
            KeyboardKey::VolumeUp => Key::VolumeUp,
        };
        Ok(key)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_declared_key_has_an_explicit_macos_conversion_result() {
            for key in KeyboardKey::ALL {
                let result = enigo_key(*key);
                if matches!(
                    key,
                    KeyboardKey::F21
                        | KeyboardKey::F22
                        | KeyboardKey::F23
                        | KeyboardKey::F24
                        | KeyboardKey::MediaPlayPause
                        | KeyboardKey::MediaPrevious
                        | KeyboardKey::MediaNext
                ) {
                    assert!(result.is_err(), "{key:?} must fail explicitly on macOS");
                } else {
                    assert!(result.is_ok(), "missing macOS conversion for {key:?}");
                }
            }
        }

        #[test]
        fn every_modifier_and_mouse_button_has_the_expected_macos_conversion() {
            assert_eq!(modifier_key(Modifier::Command), Key::Meta);
            assert_eq!(modifier_key(Modifier::Control), Key::Control);
            assert_eq!(modifier_key(Modifier::Option), Key::Option);
            assert_eq!(modifier_key(Modifier::Shift), Key::Shift);
            assert_eq!(enigo_button(MouseButton::Left), EnigoButton::Left);
            assert_eq!(enigo_button(MouseButton::Right), EnigoButton::Right);
            assert_eq!(enigo_button(MouseButton::Middle), EnigoButton::Middle);
            assert_eq!(enigo_button(MouseButton::Back), EnigoButton::Back);
            assert_eq!(enigo_button(MouseButton::Forward), EnigoButton::Forward);
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::{
    preflight_accessibility_access, preflight_post_event_access, request_accessibility_access,
    request_post_event_access, MacOsDesktopInput,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MockSink {
        events: Vec<String>,
        fail_next: bool,
    }

    impl DesktopInputSink for MockSink {
        fn key(&mut self, key: KeyboardKey, pressed: bool) -> Result<(), String> {
            self.push(format!("key:{key:?}:{pressed}"))
        }

        fn modifier(&mut self, modifier: Modifier, pressed: bool) -> Result<(), String> {
            self.push(format!("modifier:{modifier:?}:{pressed}"))
        }

        fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> Result<(), String> {
            self.push(format!("mouse:{button:?}:{pressed}"))
        }
    }

    impl MockSink {
        fn push(&mut self, event: String) -> Result<(), String> {
            if self.fail_next {
                self.fail_next = false;
                Err("injected failure".to_owned())
            } else {
                self.events.push(event);
                Ok(())
            }
        }
    }

    fn buttons(pressed: &[BindableControl]) -> SteamButtons {
        SteamButtons(pressed.iter().fold(0_u32, |mask, control| {
            mask | (1_u32 << control.steam_button() as u8)
        }))
    }

    fn chord(key: KeyboardKey, modifiers: &[Modifier]) -> BindingAction {
        BindingAction::KeyChord {
            key,
            modifiers: modifiers.iter().copied().collect(),
        }
    }

    #[test]
    fn store_round_trips_and_defaults_are_unbound() {
        let store = BindingStore::default();
        assert_eq!(store.profiles[0].bindings.configured_count(), 0);
        let bytes = serde_json::to_vec(&store).unwrap();
        let decoded: BindingStore = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, store);
        decoded.validate().unwrap();
    }

    #[test]
    fn documented_version_one_json_parses_with_stable_action_names() {
        let json = r#"{
          "version": 1,
          "profiles": [{
            "id": "default",
            "name": "Default",
            "bindings": {
              "l4": null,
              "l5": null,
              "r4": {"kind": "key_chord", "key": "F5", "modifiers": []},
              "r5": {"kind": "key_chord", "key": "F9", "modifiers": ["command"]},
              "quick_access": {"kind": "mouse_button", "button": "middle"}
            }
          }]
        }"#;
        let store: BindingStore = serde_json::from_str(json).unwrap();
        store.validate().unwrap();
        assert_eq!(store.profiles[0].bindings.configured_count(), 3);
        assert_eq!(
            store.profiles[0].bindings.r4.as_ref().unwrap().label(),
            "F5"
        );
        assert_eq!(
            store.profiles[0]
                .bindings
                .quick_access
                .as_ref()
                .unwrap()
                .label(),
            "Mouse Middle"
        );
        assert!(
            serde_json::from_str::<BindingStore>(&json.replace("key_chord", "raw_keycode"))
                .is_err()
        );
        assert!(serde_json::from_str::<BindingStore>(&json.replace(
            "\"modifiers\": []",
            "\"modifiers\": [], \"raw_keycode\": 96"
        ))
        .is_err());
    }

    #[test]
    fn store_rejects_duplicate_names_and_invalid_profile_counts() {
        let mut store = BindingStore::default();
        store.profiles.push(BindingProfile {
            id: "other".to_owned(),
            name: "default".to_owned(),
            bindings: ControlBindings::default(),
        });
        assert!(store
            .validate()
            .unwrap_err()
            .contains("duplicate profile name"));
        store.profiles.clear();
        assert!(store.validate().is_err());

        let mut oversized = BindingStore::default();
        for index in 1..=MAX_PROFILES {
            oversized.profiles.push(BindingProfile {
                id: format!("profile-{index}"),
                name: format!("Profile {index}"),
                bindings: ControlBindings::default(),
            });
        }
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn profile_operations_trim_names_preserve_ids_and_enforce_bounds() {
        let mut store = BindingStore::default();
        store.profiles[0].bindings.r4 = Some(chord(KeyboardKey::F5, &[]));
        let created = store.create_profile("  Gaming  ").unwrap();
        assert_eq!(store.profile_by_id(&created).unwrap().name, "Gaming");
        let duplicate = store
            .duplicate_profile(DEFAULT_PROFILE_ID, "Default Copy")
            .unwrap();
        assert_eq!(
            store
                .profile_by_id(&duplicate)
                .unwrap()
                .bindings
                .configured_count(),
            1
        );
        store.rename_profile(&created, "Games").unwrap();
        assert_eq!(store.profile_by_id(&created).unwrap().id, created);
        assert!(store.rename_profile(&created, "default").is_err());
        store.delete_profile(&duplicate).unwrap();
        store.delete_profile(&created).unwrap();
        assert!(store.delete_profile(DEFAULT_PROFILE_ID).is_err());
    }

    #[test]
    fn store_is_created_and_atomically_persisted() {
        let directory = std::env::temp_dir().join(format!(
            "desktop-bindings-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("persistence")
        ));
        let path = directory.join("bindings.json");
        let mut store = load_or_create_store(&path).unwrap();
        assert_eq!(store, BindingStore::default());
        store.create_profile("Second").unwrap();
        save_store(&path, &store).unwrap();
        assert_eq!(load_store(&path).unwrap(), store);
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(directory);
    }

    #[test]
    fn first_snapshot_is_baseline_and_press_release_mirrors_chord() {
        let mut profile = BindingProfile::default();
        profile.bindings.r4 = Some(chord(KeyboardKey::F5, &[Modifier::Command]));
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        engine
            .observe(buttons(&[BindableControl::R4]), &mut sink)
            .unwrap();
        engine
            .observe(buttons(&[BindableControl::R4]), &mut sink)
            .unwrap();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        assert_eq!(
            sink.events,
            [
                "modifier:Command:true",
                "key:F5:true",
                "key:F5:false",
                "modifier:Command:false"
            ]
        );
    }

    #[test]
    fn duplicate_bindings_reference_count_shared_outputs() {
        let mut profile = BindingProfile::default();
        profile.bindings.l4 = Some(chord(KeyboardKey::F9, &[]));
        profile.bindings.r4 = Some(chord(KeyboardKey::F9, &[]));
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        engine
            .observe(buttons(&[BindableControl::L4]), &mut sink)
            .unwrap();
        engine
            .observe(
                buttons(&[BindableControl::L4, BindableControl::R4]),
                &mut sink,
            )
            .unwrap();
        engine
            .observe(buttons(&[BindableControl::R4]), &mut sink)
            .unwrap();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        assert_eq!(sink.events, ["key:F9:true", "key:F9:false"]);
    }

    #[test]
    fn overlapping_chords_do_not_release_a_shared_modifier_early() {
        let mut profile = BindingProfile::default();
        profile.bindings.l4 = Some(chord(KeyboardKey::S, &[Modifier::Command]));
        profile.bindings.r4 = Some(chord(KeyboardKey::F5, &[Modifier::Command]));
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        engine
            .observe(buttons(&[BindableControl::L4]), &mut sink)
            .unwrap();
        engine
            .observe(
                buttons(&[BindableControl::L4, BindableControl::R4]),
                &mut sink,
            )
            .unwrap();
        engine
            .observe(buttons(&[BindableControl::R4]), &mut sink)
            .unwrap();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        assert_eq!(
            sink.events,
            [
                "modifier:Command:true",
                "key:S:true",
                "key:F5:true",
                "key:S:false",
                "key:F5:false",
                "modifier:Command:false"
            ]
        );
    }

    #[test]
    fn profile_switch_releases_and_blocks_controls_held_during_switch() {
        let mut first = BindingProfile::default();
        first.bindings.l4 = Some(chord(KeyboardKey::F5, &[]));
        let mut second = BindingProfile {
            id: "second".to_owned(),
            name: "Second".to_owned(),
            ..BindingProfile::default()
        };
        second.bindings.l4 = Some(chord(KeyboardKey::F9, &[]));
        let mut engine = BindingEngine::new(first);
        let mut sink = MockSink::default();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        engine
            .observe(buttons(&[BindableControl::L4]), &mut sink)
            .unwrap();
        engine.replace_profile(second, &mut sink).unwrap();
        engine
            .observe(buttons(&[BindableControl::L4]), &mut sink)
            .unwrap();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        engine
            .observe(buttons(&[BindableControl::L4]), &mut sink)
            .unwrap();
        assert_eq!(sink.events, ["key:F5:true", "key:F5:false", "key:F9:true"]);
    }

    #[test]
    fn sink_failure_releases_existing_outputs_and_rebaselines() {
        let mut profile = BindingProfile::default();
        profile.bindings.l4 = Some(chord(KeyboardKey::F5, &[]));
        profile.bindings.r4 = Some(chord(KeyboardKey::F9, &[]));
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        engine
            .observe(buttons(&[BindableControl::L4]), &mut sink)
            .unwrap();
        sink.fail_next = true;
        assert!(engine
            .observe(
                buttons(&[BindableControl::L4, BindableControl::R4]),
                &mut sink
            )
            .is_err());
        assert_eq!(engine.held_output_count(), 0);
        assert!(sink.events.contains(&"key:F5:false".to_owned()));
    }

    #[test]
    fn rapid_mouse_transitions_and_disconnect_never_leave_output_held() {
        let mut profile = BindingProfile::default();
        profile.bindings.quick_access = Some(BindingAction::MouseButton {
            button: MouseButton::Forward,
        });
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        for _ in 0..20 {
            engine
                .observe(buttons(&[BindableControl::QuickAccess]), &mut sink)
                .unwrap();
            engine.observe(buttons(&[]), &mut sink).unwrap();
        }
        engine
            .observe(buttons(&[BindableControl::QuickAccess]), &mut sink)
            .unwrap();
        engine.disconnect(&mut sink).unwrap();
        assert_eq!(engine.held_output_count(), 0);
        assert_eq!(
            sink.events.last().map(String::as_str),
            Some("mouse:Forward:false")
        );
    }
}
