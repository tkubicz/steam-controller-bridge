use std::collections::BTreeSet;
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

pub(super) const PAD_MOTION_DEADZONE_COUNTS: i32 = 192;
pub(super) const PAD_MAX_DELTA_COUNTS: i32 = 32_768;
pub(super) const MOUSE_COUNTS_PER_PIXEL: i32 = 64;
pub(super) const SCROLL_COUNTS_PER_PIXEL: i32 = 192;
pub(super) const FEEDBACK_DISPLACEMENT_COUNTS: i32 = 768;
pub(super) const FEEDBACK_SLOW_INTERVAL: Duration = Duration::from_millis(450);
pub(super) const FEEDBACK_FAST_INTERVAL: Duration = Duration::from_millis(80);
pub(super) const MOTION_SPEED_START_COUNTS_PER_SECOND: f64 = 1_500.0;
pub(super) const MOTION_SPEED_FULL_COUNTS_PER_SECOND: f64 = 12_000.0;
pub(super) const SCROLL_MAX_ACCELERATION: f64 = 3.0;
pub(super) const SCROLL_VELOCITY_BLEND: f64 = 0.35;
pub(super) const SCROLL_MOMENTUM_DECAY_PER_SECOND: f64 = 7.0;
pub(super) const SCROLL_MOMENTUM_STOP_PIXELS_PER_SECOND: f64 = 5.0;
pub(super) const SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND: f64 = 2_400.0;
pub(super) const MOTION_DEFAULT_SECONDS: f64 = 1.0 / 120.0;
pub(super) const MOTION_MIN_SECONDS: f64 = 1.0 / 240.0;
pub(super) const MOTION_SPEED_MAX_SECONDS: f64 = 0.5;
pub(super) const MOMENTUM_FRAME_MAX_SECONDS: f64 = 0.05;

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

    pub(super) const fn mask(self) -> u8 {
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
