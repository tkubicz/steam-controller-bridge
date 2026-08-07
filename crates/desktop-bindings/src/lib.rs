//! Configurable Steam Controller 2 desktop-input bindings.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use steam_controller_protocol::{PadHapticGain, SteamButton, SteamButtons};

pub const BINDINGS_VERSION: u32 = 3;
pub const MAX_PROFILES: usize = 32;
pub const MAX_PROFILE_NAME_CHARS: usize = 48;
pub const DEFAULT_PROFILE_ID: &str = "default";
pub const DEFAULT_PROFILE_NAME: &str = "Default";
pub const MIN_SCROLL_SPEED_PERCENT: u16 = 25;
pub const MAX_SCROLL_SPEED_PERCENT: u16 = 300;
pub const DEFAULT_SCROLL_SPEED_PERCENT: u16 = 100;

const PAD_MOTION_DEADZONE_COUNTS: i32 = 192;
const PAD_MAX_DELTA_COUNTS: i32 = 32_768;
const MOUSE_COUNTS_PER_PIXEL: i32 = 64;
const SCROLL_COUNTS_PER_PIXEL: i32 = 192;
const FEEDBACK_DISPLACEMENT_COUNTS: i32 = 768;
const FEEDBACK_SLOW_INTERVAL: Duration = Duration::from_millis(450);
const FEEDBACK_FAST_INTERVAL: Duration = Duration::from_millis(80);
const MOTION_SPEED_START_COUNTS_PER_SECOND: f64 = 1_500.0;
const MOTION_SPEED_FULL_COUNTS_PER_SECOND: f64 = 12_000.0;
const SCROLL_MAX_ACCELERATION: f64 = 3.0;
const SCROLL_VELOCITY_BLEND: f64 = 0.35;
const SCROLL_MOMENTUM_DECAY_PER_SECOND: f64 = 7.0;
const SCROLL_MOMENTUM_STOP_PIXELS_PER_SECOND: f64 = 5.0;
const SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND: f64 = 2_400.0;
const MOTION_DEFAULT_SECONDS: f64 = 1.0 / 120.0;
const MOTION_MIN_SECONDS: f64 = 1.0 / 240.0;
const MOTION_SPEED_MAX_SECONDS: f64 = 0.5;
const MOMENTUM_FRAME_MAX_SECONDS: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadFeedbackStrength {
    Low,
    Medium,
    High,
}

impl PadFeedbackStrength {
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Low => "Low (-36 dB)",
            Self::Medium => "Medium (-30 dB)",
            Self::High => "High (-24 dB)",
        }
    }

    #[must_use]
    pub const fn haptic_gain(self) -> PadHapticGain {
        match self {
            Self::Low => PadHapticGain::Low,
            Self::Medium => PadHapticGain::Medium,
            Self::High => PadHapticGain::High,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PadFeedbackConfig {
    pub enabled: bool,
    pub strength: PadFeedbackStrength,
}

impl Default for PadFeedbackConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: PadFeedbackStrength::Medium,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PadFunctionConfig {
    pub enabled: bool,
    pub feedback: PadFeedbackConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScrollPadConfig {
    pub enabled: bool,
    pub feedback: PadFeedbackConfig,
    pub speed_percent: u16,
    pub momentum: bool,
}

impl Default for ScrollPadConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            feedback: PadFeedbackConfig::default(),
            speed_percent: DEFAULT_SCROLL_SPEED_PERCENT,
            momentum: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PadBindings {
    pub right_mouse: PadFunctionConfig,
    pub left_scroll: ScrollPadConfig,
}

impl PadBindings {
    #[must_use]
    pub const fn configured_count(self) -> usize {
        self.right_mouse.enabled as usize + self.left_scroll.enabled as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PadSample {
    pub x: i16,
    pub y: i16,
    pub pressure: i16,
    pub touched: bool,
    pub pressed: bool,
}

impl PadSample {
    pub const NEUTRAL: Self = Self {
        x: 0,
        y: 0,
        pressure: 0,
        touched: false,
        pressed: false,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopInputSnapshot {
    pub buttons: SteamButtons,
    pub left_pad: PadSample,
    pub right_pad: PadSample,
}

impl DesktopInputSnapshot {
    #[must_use]
    pub const fn buttons_only(buttons: SteamButtons) -> Self {
        Self {
            buttons,
            left_pad: PadSample::NEUTRAL,
            right_pad: PadSample::NEUTRAL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PadFeedbackRequest {
    pub left: Option<PadFeedbackStrength>,
    pub right: Option<PadFeedbackStrength>,
}

impl PadFeedbackRequest {
    pub const NONE: Self = Self {
        left: None,
        right: None,
    };
}

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
    #[serde(default)]
    pub pads: PadBindings,
}

impl Default for BindingProfile {
    fn default() -> Self {
        Self {
            id: DEFAULT_PROFILE_ID.to_owned(),
            name: DEFAULT_PROFILE_NAME.to_owned(),
            bindings: ControlBindings::default(),
            pads: PadBindings::default(),
        }
    }
}

impl BindingProfile {
    #[must_use]
    pub fn configured_output_count(&self) -> usize {
        self.bindings.configured_count() + self.pads.configured_count()
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
            if trimmed.is_empty()
                || trimmed.chars().count() > MAX_PROFILE_NAME_CHARS
                || trimmed != profile.name
            {
                return Err(format!("invalid profile name {:?}", profile.name));
            }
            if !ids.insert(profile.id.to_ascii_lowercase()) {
                return Err(format!("duplicate profile ID {:?}", profile.id));
            }
            if !names.insert(profile.name.to_lowercase()) {
                return Err(format!("duplicate profile name {:?}", profile.name));
            }
            let scroll_speed = profile.pads.left_scroll.speed_percent;
            if !(MIN_SCROLL_SPEED_PERCENT..=MAX_SCROLL_SPEED_PERCENT).contains(&scroll_speed) {
                return Err(format!(
                    "profile {:?} scroll speed must be between {MIN_SCROLL_SPEED_PERCENT}% and {MAX_SCROLL_SPEED_PERCENT}%",
                    profile.name
                ));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn profile_by_id(&self, id: &str) -> Option<&BindingProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.id.eq_ignore_ascii_case(id))
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
            pads: PadBindings::default(),
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
            pads: source.pads,
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
            .find(|profile| profile.id.eq_ignore_ascii_case(id))
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
            .position(|profile| profile.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| format!("profile {id:?} does not exist"))?;
        Ok(self.profiles.remove(index))
    }

    fn available_name(&self, name: &str, excluding_id: Option<&str>) -> Result<String, String> {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_PROFILE_NAME_CHARS {
            return Err(format!(
                "profile names must contain 1 to {MAX_PROFILE_NAME_CHARS} characters"
            ));
        }
        let folded = trimmed.to_lowercase();
        if self.profiles.iter().any(|profile| {
            let excluded = excluding_id.is_some_and(|id| profile.id.eq_ignore_ascii_case(id));
            !excluded && profile.name.to_lowercase() == folded
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
    parse_store_at_path(path, &bytes)
}

/// Decodes and validates a binding store from an in-memory JSON document.
///
/// # Errors
/// Returns an error when the document cannot be decoded or validated.
pub fn parse_store(bytes: &[u8]) -> Result<BindingStore, String> {
    parse_store_with_migration(bytes).map(|(store, _)| store)
}

fn parse_store_with_migration(bytes: &[u8]) -> Result<(BindingStore, bool), String> {
    let mut store: BindingStore =
        serde_json::from_slice(bytes).map_err(|error| format!("invalid bindings JSON: {error}"))?;
    let migrated = matches!(store.version, 1 | 2);
    if migrated {
        store.version = BINDINGS_VERSION;
    }
    store.validate()?;
    Ok((store, migrated))
}

/// Loads a store or atomically creates the all-unbound default when missing.
///
/// # Errors
/// Returns an error for I/O, serialization, or validation failures.
pub fn load_or_create_store(path: &Path) -> Result<BindingStore, String> {
    match fs::read(path) {
        Ok(bytes) => parse_store_at_path(path, &bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let store = BindingStore::default();
            save_store(path, &store)?;
            Ok(store)
        }
        Err(error) => Err(format!("cannot read '{}': {error}", path.display())),
    }
}

fn parse_store_at_path(path: &Path, bytes: &[u8]) -> Result<BindingStore, String> {
    let (store, migrated) = parse_store_with_migration(bytes)
        .map_err(|error| format!("{error} in '{}'", path.display()))?;
    if migrated {
        save_store(path, &store)?;
    }
    Ok(store)
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
    if let Err(error) = fs::write(&temporary, bytes) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
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

    /// Moves the pointer by a relative number of pixels.
    ///
    /// # Errors
    /// Returns an error if the platform cannot inject the movement.
    fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String>;

    /// Smooth-scrolls by a relative number of pixels.
    ///
    /// Positive X moves content right and positive Y moves content down.
    ///
    /// # Errors
    /// Returns an error if the platform cannot inject the scroll.
    fn scroll(&mut self, x: i32, y: i32) -> Result<(), String>;
}

#[derive(Debug, Default)]
struct PadMotionState {
    previous: Option<(i16, i16)>,
    touched: bool,
    blocked: bool,
    deadzone_x: i32,
    deadzone_y: i32,
    x_residual: i32,
    y_residual: i32,
    feedback_x: i32,
    feedback_y: i32,
    last_feedback: Option<Duration>,
    last_motion: Option<Duration>,
    scroll_fraction_x: f64,
    scroll_fraction_y: f64,
    scroll_velocity_x: f64,
    scroll_velocity_y: f64,
    scroll_last_update: Option<Duration>,
}

impl PadMotionState {
    fn reset_contact(&mut self) {
        self.previous = None;
        self.deadzone_x = 0;
        self.deadzone_y = 0;
        self.x_residual = 0;
        self.y_residual = 0;
        self.feedback_x = 0;
        self.feedback_y = 0;
        self.last_feedback = None;
        self.last_motion = None;
    }

    fn reset_motion(&mut self) {
        self.reset_contact();
        clear_scroll_momentum(self);
    }

    fn block_if_touched(&mut self) {
        self.blocked = self.touched;
        self.reset_motion();
    }
}

pub struct BindingEngine {
    profile: BindingProfile,
    previous_mask: Option<u8>,
    blocked_mask: u8,
    active: BTreeMap<BindableControl, BindingAction>,
    key_counts: BTreeMap<OutputKey, u16>,
    mouse_counts: BTreeMap<MouseButton, u16>,
    left_pad: PadMotionState,
    right_pad: PadMotionState,
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
            left_pad: PadMotionState::default(),
            right_pad: PadMotionState::default(),
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

    /// Reports whether time-based output must continue without a new snapshot.
    ///
    /// Callers may sleep indefinitely while this is false because button and
    /// direct pad output are entirely snapshot-driven. It becomes true only
    /// while released left-pad scroll momentum still needs periodic ticks.
    #[must_use]
    pub fn needs_tick(&self) -> bool {
        self.previous_mask.is_some()
            && !self.left_pad.touched
            && !self.left_pad.blocked
            && self.profile.pads.left_scroll.enabled
            && self.profile.pads.left_scroll.momentum
            && self.left_pad.scroll_last_update.is_some()
    }

    /// Observes a button-only snapshot and emits its binding edges.
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
        self.observe_snapshot(
            DesktopInputSnapshot::buttons_only(buttons),
            Duration::ZERO,
            sink,
        )
        .map(|_| ())
    }

    /// Observes buttons and pads, emitting desktop actions and returning any
    /// finite pad-feedback ticks requested by movement.
    ///
    /// The first snapshot is a non-emitting baseline. Any sink error releases
    /// held outputs and blocks controls and pads until their physical release.
    ///
    /// # Errors
    /// Returns the first desktop-input injection failure.
    pub fn observe_snapshot(
        &mut self,
        snapshot: DesktopInputSnapshot,
        now: Duration,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<PadFeedbackRequest, String> {
        let mask = bindable_mask(snapshot.buttons);
        let Some(previous) = self.previous_mask else {
            self.previous_mask = Some(mask);
            self.blocked_mask = mask;
            baseline_pad(&mut self.left_pad, snapshot.left_pad);
            baseline_pad(&mut self.right_pad, snapshot.right_pad);
            return Ok(PadFeedbackRequest::NONE);
        };
        self.blocked_mask &= mask;
        let changed = previous ^ mask;
        let pads = self.profile.pads;
        let result = self.apply_changes(changed, mask, sink).and_then(|()| {
            let left = process_scroll_pad(
                &mut self.left_pad,
                snapshot.left_pad,
                pads.left_scroll,
                now,
                sink,
            )?;
            let right = process_mouse_pad(
                &mut self.right_pad,
                snapshot.right_pad,
                pads.right_mouse,
                now,
                sink,
            )?;
            Ok(PadFeedbackRequest { left, right })
        });
        self.previous_mask = Some(mask);
        match result {
            Ok(feedback) => Ok(feedback),
            Err(error) => {
                let _ = self.release_all(sink);
                self.blocked_mask = mask;
                self.left_pad.block_if_touched();
                self.right_pad.block_if_touched();
                Err(error)
            }
        }
    }

    /// Advances time-based desktop output such as left-pad scroll momentum.
    ///
    /// This is intentionally independent of controller reports so inertia can
    /// finish even when the HID transport becomes quiet after touch release.
    ///
    /// # Errors
    /// Returns a desktop-input injection failure after clearing pending motion.
    pub fn tick(&mut self, now: Duration, sink: &mut dyn DesktopInputSink) -> Result<(), String> {
        if !self.needs_tick() {
            return Ok(());
        }
        if let Err(error) = advance_scroll_momentum(&mut self.left_pad, now, sink) {
            let _ = self.release_all(sink);
            self.left_pad.reset_motion();
            self.right_pad.block_if_touched();
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
        if self.profile.id.eq_ignore_ascii_case(&profile.id)
            && self.profile.bindings == profile.bindings
            && self.profile.pads == profile.pads
        {
            self.profile = profile;
            return Ok(());
        }
        let held = self.previous_mask.unwrap_or_default();
        let release = self.release_all(sink);
        self.profile = profile;
        self.blocked_mask = held;
        self.left_pad.block_if_touched();
        self.right_pad.block_if_touched();
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
        self.left_pad = PadMotionState::default();
        self.right_pad = PadMotionState::default();
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

fn baseline_pad(state: &mut PadMotionState, sample: PadSample) {
    state.touched = sample.touched;
    state.blocked = sample.touched;
    state.reset_motion();
}

fn process_mouse_pad(
    state: &mut PadMotionState,
    sample: PadSample,
    config: PadFunctionConfig,
    now: Duration,
    sink: &mut dyn DesktopInputSink,
) -> Result<Option<PadFeedbackStrength>, String> {
    state.touched = sample.touched;
    if !sample.touched {
        state.blocked = false;
        state.reset_motion();
        return Ok(None);
    }
    if state.blocked || !config.enabled {
        state.reset_motion();
        return Ok(None);
    }

    let Some((previous_x, previous_y)) = state.previous.replace((sample.x, sample.y)) else {
        state.last_motion = Some(now);
        return Ok(None);
    };
    let raw_x = i32::from(sample.x) - i32::from(previous_x);
    let raw_y = i32::from(sample.y) - i32::from(previous_y);
    if raw_x.abs() > PAD_MAX_DELTA_COUNTS || raw_y.abs() > PAD_MAX_DELTA_COUNTS {
        rebaseline_placement(state, sample);
        return Ok(None);
    }

    let Some((delta_x, delta_y)) = accumulate_deadzone_motion(state, raw_x, raw_y) else {
        return Ok(None);
    };

    state.x_residual += delta_x;
    state.y_residual -= delta_y;
    let pixels_x = take_pixels(&mut state.x_residual, MOUSE_COUNTS_PER_PIXEL);
    let pixels_y = take_pixels(&mut state.y_residual, MOUSE_COUNTS_PER_PIXEL);
    if pixels_x != 0 || pixels_y != 0 {
        sink.mouse_move(pixels_x, pixels_y)?;
    }

    let speed = update_motion_speed(state, delta_x, delta_y, now);
    Ok(process_feedback(
        state,
        config.feedback,
        delta_x,
        delta_y,
        speed,
        now,
    ))
}

fn process_scroll_pad(
    state: &mut PadMotionState,
    sample: PadSample,
    config: ScrollPadConfig,
    now: Duration,
    sink: &mut dyn DesktopInputSink,
) -> Result<Option<PadFeedbackStrength>, String> {
    let was_touched = state.touched;
    state.touched = sample.touched;
    if !config.enabled || state.blocked {
        if !sample.touched {
            state.blocked = false;
        }
        state.reset_motion();
        return Ok(None);
    }
    if !sample.touched {
        if was_touched {
            state.reset_contact();
            state.scroll_last_update = Some(now);
            if !config.momentum {
                clear_scroll_momentum(state);
            }
            return Ok(None);
        }
        if config.momentum {
            advance_scroll_momentum(state, now, sink)?;
        } else {
            clear_scroll_momentum(state);
        }
        return Ok(None);
    }

    if !was_touched {
        clear_scroll_momentum(state);
    }
    let Some((previous_x, previous_y)) = state.previous.replace((sample.x, sample.y)) else {
        state.last_motion = Some(now);
        state.scroll_last_update = Some(now);
        return Ok(None);
    };
    let raw_x = i32::from(sample.x) - i32::from(previous_x);
    let raw_y = i32::from(sample.y) - i32::from(previous_y);
    if raw_x.abs() > PAD_MAX_DELTA_COUNTS || raw_y.abs() > PAD_MAX_DELTA_COUNTS {
        rebaseline_placement(state, sample);
        return Ok(None);
    }
    let Some((delta_x, delta_y)) = accumulate_deadzone_motion(state, raw_x, raw_y) else {
        return Ok(None);
    };

    let speed = update_motion_speed(state, delta_x, delta_y, now);
    let acceleration = scroll_acceleration(speed);
    let profile_scale = f64::from(config.speed_percent) / 100.0;
    let scale = profile_scale * acceleration / f64::from(SCROLL_COUNTS_PER_PIXEL);
    let scroll_x = f64::from(delta_x) * scale;
    let scroll_y = -f64::from(delta_y) * scale;
    emit_fractional_scroll(state, scroll_x, scroll_y, sink)?;

    let seconds = motion_seconds(state.scroll_last_update, now);
    state.scroll_last_update = Some(now);
    let instantaneous_x = (scroll_x / seconds).clamp(
        -SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND,
        SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND,
    );
    let instantaneous_y = (scroll_y / seconds).clamp(
        -SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND,
        SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND,
    );
    state.scroll_velocity_x = blend_velocity(state.scroll_velocity_x, instantaneous_x);
    state.scroll_velocity_y = blend_velocity(state.scroll_velocity_y, instantaneous_y);

    Ok(process_feedback(
        state,
        config.feedback,
        delta_x,
        delta_y,
        speed,
        now,
    ))
}

fn update_motion_speed(
    state: &mut PadMotionState,
    delta_x: i32,
    delta_y: i32,
    now: Duration,
) -> f64 {
    let seconds = motion_seconds(state.last_motion, now);
    state.last_motion = Some(now);
    f64::from(delta_x).hypot(f64::from(delta_y)) / seconds
}

fn motion_seconds(previous: Option<Duration>, now: Duration) -> f64 {
    previous.map_or(MOTION_DEFAULT_SECONDS, |last| {
        now.saturating_sub(last)
            .as_secs_f64()
            .clamp(MOTION_MIN_SECONDS, MOTION_SPEED_MAX_SECONDS)
    })
}

fn normalized_motion_speed(speed: f64) -> f64 {
    ((speed - MOTION_SPEED_START_COUNTS_PER_SECOND)
        / (MOTION_SPEED_FULL_COUNTS_PER_SECOND - MOTION_SPEED_START_COUNTS_PER_SECOND))
        .clamp(0.0, 1.0)
}

fn scroll_acceleration(speed: f64) -> f64 {
    1.0 + normalized_motion_speed(speed) * (SCROLL_MAX_ACCELERATION - 1.0)
}

fn blend_velocity(previous: f64, instantaneous: f64) -> f64 {
    previous * (1.0 - SCROLL_VELOCITY_BLEND) + instantaneous * SCROLL_VELOCITY_BLEND
}

fn emit_fractional_scroll(
    state: &mut PadMotionState,
    x: f64,
    y: f64,
    sink: &mut dyn DesktopInputSink,
) -> Result<(), String> {
    state.scroll_fraction_x += x;
    state.scroll_fraction_y += y;
    let pixels_x = take_fractional_pixels(&mut state.scroll_fraction_x);
    let pixels_y = take_fractional_pixels(&mut state.scroll_fraction_y);
    if pixels_x != 0 || pixels_y != 0 {
        sink.scroll(pixels_x, pixels_y)?;
    }
    Ok(())
}

#[allow(clippy::cast_possible_truncation)]
fn take_fractional_pixels(residual: &mut f64) -> i32 {
    // Direct motion and momentum are bounded far below i32 limits. Truncation
    // deliberately retains the sub-pixel remainder for the next update.
    let whole = residual
        .trunc()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX));
    let pixels = whole as i32;
    *residual -= f64::from(pixels);
    pixels
}

fn advance_scroll_momentum(
    state: &mut PadMotionState,
    now: Duration,
    sink: &mut dyn DesktopInputSink,
) -> Result<(), String> {
    let Some(last) = state.scroll_last_update else {
        return Ok(());
    };
    let seconds = now
        .saturating_sub(last)
        .as_secs_f64()
        .min(MOMENTUM_FRAME_MAX_SECONDS);
    state.scroll_last_update = Some(now);
    if seconds == 0.0 {
        return Ok(());
    }
    emit_fractional_scroll(
        state,
        state.scroll_velocity_x * seconds,
        state.scroll_velocity_y * seconds,
        sink,
    )?;
    let decay = (-SCROLL_MOMENTUM_DECAY_PER_SECOND * seconds).exp();
    state.scroll_velocity_x *= decay;
    state.scroll_velocity_y *= decay;
    if state.scroll_velocity_x.hypot(state.scroll_velocity_y)
        < SCROLL_MOMENTUM_STOP_PIXELS_PER_SECOND
    {
        clear_scroll_momentum(state);
    }
    Ok(())
}

fn clear_scroll_momentum(state: &mut PadMotionState) {
    state.scroll_fraction_x = 0.0;
    state.scroll_fraction_y = 0.0;
    state.scroll_velocity_x = 0.0;
    state.scroll_velocity_y = 0.0;
    state.scroll_last_update = None;
}

fn process_feedback(
    state: &mut PadMotionState,
    config: PadFeedbackConfig,
    delta_x: i32,
    delta_y: i32,
    speed: f64,
    now: Duration,
) -> Option<PadFeedbackStrength> {
    if !config.enabled {
        state.feedback_x = 0;
        state.feedback_y = 0;
        return None;
    }
    // Measure displacement from the last consumed texture point, not total
    // path length. Back-and-forth coordinate noise therefore cancels instead
    // of eventually producing feedback while a finger is stationary.
    state.feedback_x += delta_x;
    state.feedback_y += delta_y;
    let feedback_x = i64::from(state.feedback_x);
    let feedback_y = i64::from(state.feedback_y);
    let feedback_threshold = i64::from(FEEDBACK_DISPLACEMENT_COUNTS);
    if feedback_x * feedback_x + feedback_y * feedback_y < feedback_threshold * feedback_threshold {
        return None;
    }

    let speed_factor = normalized_motion_speed(speed);
    let slow_ms = FEEDBACK_SLOW_INTERVAL.as_secs_f64() * 1_000.0;
    let fast_ms = FEEDBACK_FAST_INTERVAL.as_secs_f64() * 1_000.0;
    let interval =
        Duration::from_secs_f64((slow_ms + (fast_ms - slow_ms) * speed_factor) / 1_000.0);
    let interval_ready = state
        .last_feedback
        .is_none_or(|last| now.saturating_sub(last) >= interval);
    // Each threshold crossing is a complete microtick opportunity. When the
    // rate limiter is closed, drop it instead of retaining a delayed backlog.
    state.feedback_x = 0;
    state.feedback_y = 0;
    if interval_ready {
        state.last_feedback = Some(now);
        Some(config.strength)
    } else {
        None
    }
}

/// Treats an impossibly large per-report delta as a lift-and-replace: motion,
/// deadzone, feedback, and momentum restart from the new contact point.
fn rebaseline_placement(state: &mut PadMotionState, sample: PadSample) {
    state.reset_motion();
    state.previous = Some((sample.x, sample.y));
}

fn accumulate_deadzone_motion(
    state: &mut PadMotionState,
    delta_x: i32,
    delta_y: i32,
) -> Option<(i32, i32)> {
    // Accumulate slow intentional motion, but require its radial displacement
    // to leave the stationary-noise region before forwarding it. Recenter
    // after every accepted vector so a stopped finger gets a fresh deadzone.
    state.deadzone_x += delta_x;
    state.deadzone_y += delta_y;
    let x = i64::from(state.deadzone_x);
    let y = i64::from(state.deadzone_y);
    if x * x + y * y < i64::from(PAD_MOTION_DEADZONE_COUNTS).pow(2) {
        None
    } else {
        let filtered = (state.deadzone_x, state.deadzone_y);
        state.deadzone_x = 0;
        state.deadzone_y = 0;
        Some(filtered)
    }
}

fn take_pixels(residual: &mut i32, counts_per_pixel: i32) -> i32 {
    let pixels = *residual / counts_per_pixel;
    *residual -= pixels * counts_per_pixel;
    pixels
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
    use enigo::{
        Axis, Button as EnigoButton, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings,
    };
    use objc2_core_graphics::{CGPreflightPostEventAccess, CGRequestPostEventAccess};
    use objc2_io_kit::{IOHIDAccessType, IOHIDCheckAccess, IOHIDRequestAccess, IOHIDRequestType};

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

    /// Whether macOS has decided about a permission yet, and how.
    ///
    /// The distinction matters: an undecided permission can still be asked for
    /// and macOS will show its dialog, while a refused one cannot -- asking
    /// again does nothing at all, and the only way forward is System Settings.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PermissionState {
        Granted,
        Denied,
        Undecided,
    }

    /// Reports whether this process may observe input, i.e. Input Monitoring.
    #[must_use]
    pub fn input_monitoring_access() -> PermissionState {
        match IOHIDCheckAccess(IOHIDRequestType::ListenEvent) {
            IOHIDAccessType::Granted => PermissionState::Granted,
            IOHIDAccessType::Denied => PermissionState::Denied,
            _ => PermissionState::Undecided,
        }
    }

    /// Asks macOS for Input Monitoring, showing its dialog when undecided.
    ///
    /// Returns whether the permission is granted afterwards. A refusal that
    /// macOS already recorded produces no dialog, so callers should send the
    /// user to System Settings in that case.
    #[must_use]
    pub fn request_input_monitoring_access() -> bool {
        IOHIDRequestAccess(IOHIDRequestType::ListenEvent)
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

        fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.enigo
                .move_mouse(x, y, Coordinate::Rel)
                .map_err(|error| error.to_string())
        }

        fn scroll(&mut self, x: i32, y: i32) -> Result<(), String> {
            if x != 0 {
                self.enigo
                    .smooth_scroll(x, Axis::Horizontal)
                    .map_err(|error| error.to_string())?;
            }
            if y != 0 {
                self.enigo
                    .smooth_scroll(y, Axis::Vertical)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
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
    input_monitoring_access, preflight_accessibility_access, preflight_post_event_access,
    request_accessibility_access, request_input_monitoring_access, request_post_event_access,
    MacOsDesktopInput, PermissionState,
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

        fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.push(format!("move:{x}:{y}"))
        }

        fn scroll(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.push(format!("scroll:{x}:{y}"))
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

    fn pad_snapshot(
        buttons: SteamButtons,
        left: Option<(i16, i16)>,
        right: Option<(i16, i16)>,
    ) -> DesktopInputSnapshot {
        DesktopInputSnapshot {
            buttons,
            left_pad: left.map_or_else(PadSample::default, |(x, y)| PadSample {
                x,
                y,
                touched: true,
                ..PadSample::default()
            }),
            right_pad: right.map_or_else(PadSample::default, |(x, y)| PadSample {
                x,
                y,
                touched: true,
                ..PadSample::default()
            }),
        }
    }

    #[test]
    fn store_round_trips_and_defaults_are_unbound() {
        let store = BindingStore::default();
        assert_eq!(store.profiles[0].bindings.configured_count(), 0);
        assert_eq!(store.profiles[0].configured_output_count(), 0);
        assert!(!store.profiles[0].pads.left_scroll.enabled);
        assert!(!store.profiles[0].pads.right_mouse.enabled);
        assert!(store.profiles[0].pads.left_scroll.feedback.enabled);
        assert_eq!(
            store.profiles[0].pads.left_scroll.speed_percent,
            DEFAULT_SCROLL_SPEED_PERCENT
        );
        assert!(store.profiles[0].pads.left_scroll.momentum);
        assert_eq!(
            store.profiles[0].pads.right_mouse.feedback.strength,
            PadFeedbackStrength::Medium
        );
        let bytes = serde_json::to_vec(&store).unwrap();
        let decoded: BindingStore = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, store);
        decoded.validate().unwrap();
    }

    #[test]
    fn store_rejects_unknown_pad_feedback_strength() {
        let json = br#"{
          "version": 3,
          "profiles": [{
            "id": "default",
            "name": "Default",
            "bindings": {},
            "pads": {
              "right_mouse": {
                "enabled": true,
                "feedback": {"enabled": true, "strength": "extreme"}
              }
            }
          }]
        }"#;
        assert!(parse_store(json).is_err());
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
        let store = parse_store(json.as_bytes()).unwrap();
        assert_eq!(store.version, BINDINGS_VERSION);
        assert_eq!(store.profiles[0].pads, PadBindings::default());
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
            pads: PadBindings::default(),
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
                pads: PadBindings::default(),
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
    fn profile_identity_rules_match_validation_and_lookup() {
        let mut store = BindingStore::default();
        store.create_profile("Ä").unwrap();
        assert!(store.create_profile("ä").is_err());

        store.profiles[1].id = "PROFILE-1".to_owned();
        store.validate().unwrap();
        assert_eq!(store.profile_by_id("profile-1").unwrap().name, "Ä");
        assert_eq!(store.next_profile_id(), "profile-2");
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
    fn loading_version_one_atomically_migrates_to_version_three() {
        let directory =
            std::env::temp_dir().join(format!("desktop-bindings-migration-{}", std::process::id()));
        let path = directory.join("bindings.json");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            &path,
            br#"{"version":1,"profiles":[{"id":"default","name":"Default","bindings":{"r4":{"kind":"key_chord","key":"F5","modifiers":[]}}}]}"#,
        )
        .unwrap();

        let store = load_store(&path).unwrap();
        assert_eq!(store.version, BINDINGS_VERSION);
        assert_eq!(
            store.profiles[0].bindings.r4.as_ref().unwrap().label(),
            "F5"
        );
        assert_eq!(store.profiles[0].pads, PadBindings::default());
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted["version"], BINDINGS_VERSION);
        assert!(persisted["profiles"][0]["pads"].is_object());
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(directory);
    }

    #[test]
    fn loading_version_two_preserves_pads_and_adds_scroll_defaults() {
        let json = br#"{
          "version": 2,
          "profiles": [{
            "id": "default",
            "name": "Default",
            "bindings": {},
            "pads": {
              "right_mouse": {
                "enabled": true,
                "feedback": {"enabled": false, "strength": "low"}
              },
              "left_scroll": {
                "enabled": true,
                "feedback": {"enabled": true, "strength": "high"}
              }
            }
          }]
        }"#;
        let store = parse_store(json).unwrap();
        assert_eq!(store.version, BINDINGS_VERSION);
        assert!(store.profiles[0].pads.right_mouse.enabled);
        assert!(!store.profiles[0].pads.right_mouse.feedback.enabled);
        let scroll = store.profiles[0].pads.left_scroll;
        assert!(scroll.enabled);
        assert_eq!(scroll.feedback.strength, PadFeedbackStrength::High);
        assert_eq!(scroll.speed_percent, DEFAULT_SCROLL_SPEED_PERCENT);
        assert!(scroll.momentum);
    }

    #[test]
    fn store_rejects_scroll_speed_outside_supported_range() {
        let mut store = BindingStore::default();
        store.profiles[0].pads.left_scroll.speed_percent = MIN_SCROLL_SPEED_PERCENT - 1;
        assert!(store.validate().is_err());
        store.profiles[0].pads.left_scroll.speed_percent = MAX_SCROLL_SPEED_PERCENT + 1;
        assert!(store.validate().is_err());
    }

    #[test]
    fn failed_atomic_rename_cleans_up_the_temporary_file() {
        let directory = std::env::temp_dir().join(format!(
            "desktop-bindings-rename-failure-{}",
            std::process::id()
        ));
        let path = directory.join("bindings.json");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&path).unwrap();

        assert!(save_store(&path, &BindingStore::default()).is_err());
        let leftovers = fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() != "bindings.json")
            .count();
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir_all(directory);
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
    fn metadata_only_profile_update_preserves_held_outputs() {
        let mut profile = BindingProfile::default();
        profile.bindings.r4 = Some(chord(KeyboardKey::F5, &[]));
        let mut engine = BindingEngine::new(profile.clone());
        let mut sink = MockSink::default();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        engine
            .observe(buttons(&[BindableControl::R4]), &mut sink)
            .unwrap();

        profile.name = "Renamed".to_owned();
        engine.replace_profile(profile, &mut sink).unwrap();
        assert_eq!(sink.events, ["key:F5:true"]);
        engine.observe(buttons(&[]), &mut sink).unwrap();
        assert_eq!(sink.events, ["key:F5:true", "key:F5:false"]);
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
    fn right_pad_feedback_cadence_increases_with_motion_speed_without_a_backlog() {
        let mut profile = BindingProfile::default();
        profile.pads.right_mouse.enabled = true;
        assert_eq!(profile.configured_output_count(), 1);
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        let neutral = SteamButtons::default();

        engine
            .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((100, 100))),
                Duration::from_millis(1),
                &mut sink,
            )
            .unwrap();
        let first = engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((868, 100))),
                Duration::from_millis(500),
                &mut sink,
            )
            .unwrap();
        assert_eq!(first.right, Some(PadFeedbackStrength::Medium));
        let slow_limited = engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((1636, 100))),
                Duration::from_millis(800),
                &mut sink,
            )
            .unwrap();
        assert_eq!(slow_limited, PadFeedbackRequest::NONE);
        let slow_ready = engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((2404, 100))),
                Duration::from_millis(1_200),
                &mut sink,
            )
            .unwrap();
        assert_eq!(slow_ready.right, Some(PadFeedbackStrength::Medium));

        let mut fast_engine = BindingEngine::new(engine.profile().clone());
        let mut fast_sink = MockSink::default();
        fast_engine
            .observe_snapshot(
                pad_snapshot(neutral, None, None),
                Duration::ZERO,
                &mut fast_sink,
            )
            .unwrap();
        fast_engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((100, 100))),
                Duration::from_millis(1),
                &mut fast_sink,
            )
            .unwrap();
        let fast_first = fast_engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((868, 100))),
                Duration::from_millis(10),
                &mut fast_sink,
            )
            .unwrap();
        assert_eq!(fast_first.right, Some(PadFeedbackStrength::Medium));
        let fast_limited = fast_engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((1636, 100))),
                Duration::from_millis(60),
                &mut fast_sink,
            )
            .unwrap();
        assert_eq!(fast_limited, PadFeedbackRequest::NONE);
        let fast_ready = fast_engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((2404, 100))),
                Duration::from_millis(110),
                &mut fast_sink,
            )
            .unwrap();
        assert_eq!(fast_ready.right, Some(PadFeedbackStrength::Medium));
    }

    #[test]
    fn stationary_pressed_pad_noise_does_not_emit_feedback() {
        let mut profile = BindingProfile::default();
        profile.pads.right_mouse.enabled = true;
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        let neutral = SteamButtons::default();

        engine
            .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
            .unwrap();
        for (index, (x, y)) in [
            (0, 0),
            (0, 160),
            (0, -160),
            (96, 128),
            (-96, -128),
            (128, -96),
            (-128, 96),
            (0, 160),
            (0, -160),
        ]
        .into_iter()
        .enumerate()
        {
            let mut snapshot = pad_snapshot(neutral, None, Some((x, y)));
            snapshot.right_pad.pressed = true;
            let feedback = engine
                .observe_snapshot(
                    snapshot,
                    Duration::from_millis(u64::try_from(index * 250).unwrap()),
                    &mut sink,
                )
                .unwrap();
            assert_eq!(feedback, PadFeedbackRequest::NONE);
        }
        assert!(sink.events.is_empty());
    }

    #[test]
    fn left_pad_scrolls_both_axes_and_can_disable_feedback() {
        let mut profile = BindingProfile::default();
        profile.pads.left_scroll.enabled = true;
        profile.pads.left_scroll.feedback.enabled = false;
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        let neutral = SteamButtons::default();
        engine
            .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, Some((0, 0)), None),
                Duration::from_millis(1),
                &mut sink,
            )
            .unwrap();
        let feedback = engine
            .observe_snapshot(
                pad_snapshot(neutral, Some((384, 192)), None),
                Duration::from_millis(20),
                &mut sink,
            )
            .unwrap();
        assert_eq!(feedback, PadFeedbackRequest::NONE);
        assert_eq!(sink.events, ["scroll:6:-3"]);
    }

    #[test]
    fn left_pad_scroll_acceleration_and_profile_speed_scale_output() {
        fn scroll_once(duration_ms: u64, speed_percent: u16) -> Vec<String> {
            let mut profile = BindingProfile::default();
            profile.pads.left_scroll.enabled = true;
            profile.pads.left_scroll.feedback.enabled = false;
            profile.pads.left_scroll.speed_percent = speed_percent;
            let mut engine = BindingEngine::new(profile);
            let mut sink = MockSink::default();
            let neutral = SteamButtons::default();
            engine
                .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
                .unwrap();
            engine
                .observe_snapshot(
                    pad_snapshot(neutral, Some((0, 0)), None),
                    Duration::from_millis(1),
                    &mut sink,
                )
                .unwrap();
            engine
                .observe_snapshot(
                    pad_snapshot(neutral, Some((384, 0)), None),
                    Duration::from_millis(duration_ms),
                    &mut sink,
                )
                .unwrap();
            sink.events
        }

        assert_eq!(scroll_once(501, 100), ["scroll:2:0"]);
        assert_eq!(scroll_once(20, 100), ["scroll:6:0"]);
        assert_eq!(scroll_once(20, 50), ["scroll:3:0"]);
        assert_eq!(scroll_once(20, 200), ["scroll:12:0"]);
    }

    #[test]
    fn left_pad_momentum_decays_after_release_and_can_be_disabled() {
        fn run(momentum: bool) -> Vec<String> {
            let mut profile = BindingProfile::default();
            profile.pads.left_scroll.enabled = true;
            profile.pads.left_scroll.feedback.enabled = false;
            profile.pads.left_scroll.momentum = momentum;
            let mut engine = BindingEngine::new(profile);
            let mut sink = MockSink::default();
            let neutral = SteamButtons::default();
            engine
                .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
                .unwrap();
            engine
                .observe_snapshot(
                    pad_snapshot(neutral, Some((0, 0)), None),
                    Duration::from_millis(1),
                    &mut sink,
                )
                .unwrap();
            engine
                .observe_snapshot(
                    pad_snapshot(neutral, Some((768, 0)), None),
                    Duration::from_millis(21),
                    &mut sink,
                )
                .unwrap();
            engine
                .observe_snapshot(
                    pad_snapshot(neutral, None, None),
                    Duration::from_millis(22),
                    &mut sink,
                )
                .unwrap();
            for time_ms in (32..=2_032).step_by(10) {
                engine
                    .tick(Duration::from_millis(time_ms), &mut sink)
                    .unwrap();
            }
            sink.events
        }

        let with_momentum = run(true);
        let without_momentum = run(false);
        assert_eq!(without_momentum, ["scroll:12:0"]);
        assert!(with_momentum.len() > without_momentum.len());
        assert!(with_momentum
            .iter()
            .skip(1)
            .all(|event| event.starts_with("scroll:")));
        assert_eq!(with_momentum.last(), Some(&"scroll:1:0".to_owned()));
    }

    #[test]
    fn ticks_are_needed_only_while_released_scroll_momentum_is_pending() {
        let mut profile = BindingProfile::default();
        profile.pads.left_scroll.enabled = true;
        profile.pads.left_scroll.feedback.enabled = false;
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        let neutral = SteamButtons::default();

        assert!(!engine.needs_tick());
        engine
            .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
            .unwrap();
        assert!(!engine.needs_tick());
        engine
            .observe_snapshot(
                pad_snapshot(neutral, Some((0, 0)), None),
                Duration::from_millis(1),
                &mut sink,
            )
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, Some((768, 0)), None),
                Duration::from_millis(21),
                &mut sink,
            )
            .unwrap();
        assert!(!engine.needs_tick());

        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, None),
                Duration::from_millis(22),
                &mut sink,
            )
            .unwrap();
        assert!(engine.needs_tick());

        for time_ms in (32..=2_032).step_by(10) {
            engine
                .tick(Duration::from_millis(time_ms), &mut sink)
                .unwrap();
            if !engine.needs_tick() {
                break;
            }
        }
        assert!(!engine.needs_tick());

        engine
            .observe_snapshot(
                pad_snapshot(neutral, Some((0, 0)), None),
                Duration::from_millis(2_101),
                &mut sink,
            )
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, Some((768, 0)), None),
                Duration::from_millis(2_121),
                &mut sink,
            )
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, None),
                Duration::from_millis(2_122),
                &mut sink,
            )
            .unwrap();
        assert!(engine.needs_tick());
        engine.disconnect(&mut sink).unwrap();
        assert!(!engine.needs_tick());
    }

    #[test]
    fn pad_motion_deadzone_rejects_noise_and_recenters_after_large_jumps() {
        let mut profile = BindingProfile::default();
        profile.pads.right_mouse.enabled = true;
        profile.pads.right_mouse.feedback.enabled = false;
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        let neutral = SteamButtons::default();

        engine
            .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
            .unwrap();
        for (time_ms, x) in [
            (1, 0),
            (2, 64),
            (3, -64),
            (4, 64),
            (5, -64),
            (6, 128),
            (7, -128),
            (8, 128),
        ] {
            engine
                .observe_snapshot(
                    pad_snapshot(neutral, None, Some((x, 0))),
                    Duration::from_millis(time_ms),
                    &mut sink,
                )
                .unwrap();
        }
        assert!(sink.events.is_empty());

        for (time_ms, x) in [(9, 192), (10, 384), (11, 448)] {
            engine
                .observe_snapshot(
                    pad_snapshot(neutral, None, Some((x, 0))),
                    Duration::from_millis(time_ms),
                    &mut sink,
                )
                .unwrap();
        }
        assert_eq!(sink.events, ["move:3:0", "move:3:0"]);

        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((-32_700, 0))),
                Duration::from_millis(12),
                &mut sink,
            )
            .unwrap();
        engine
            .observe_snapshot(
                DesktopInputSnapshot {
                    right_pad: PadSample {
                        x: -32_508,
                        pressure: i16::MAX,
                        touched: true,
                        pressed: true,
                        ..PadSample::default()
                    },
                    ..pad_snapshot(neutral, None, None)
                },
                Duration::from_millis(13),
                &mut sink,
            )
            .unwrap();
        assert_eq!(sink.events, ["move:3:0", "move:3:0", "move:3:0"]);
    }

    #[test]
    fn pad_touched_during_startup_or_profile_switch_waits_for_release() {
        let mut profile = BindingProfile::default();
        profile.pads.right_mouse.enabled = true;
        let mut engine = BindingEngine::new(profile.clone());
        let mut sink = MockSink::default();
        let neutral = SteamButtons::default();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((0, 0))),
                Duration::ZERO,
                &mut sink,
            )
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((640, 0))),
                Duration::from_millis(20),
                &mut sink,
            )
            .unwrap();
        assert!(sink.events.is_empty());
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, None),
                Duration::from_millis(21),
                &mut sink,
            )
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((0, 0))),
                Duration::from_millis(22),
                &mut sink,
            )
            .unwrap();
        let mut replacement = profile;
        replacement.pads.right_mouse.feedback.strength = PadFeedbackStrength::High;
        engine.replace_profile(replacement, &mut sink).unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((640, 0))),
                Duration::from_millis(40),
                &mut sink,
            )
            .unwrap();
        assert!(sink.events.is_empty());
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
