use std::collections::BTreeSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use steam_controller_protocol::{PadHapticGain, SteamButton, SteamButtons, SteamControllerState};

pub const BINDINGS_VERSION: u32 = 5;
pub const MAX_PROFILES: usize = 32;
pub const MAX_PROFILE_NAME_CHARS: usize = 48;
pub const DEFAULT_PROFILE_ID: &str = "default";
pub const DEFAULT_PROFILE_NAME: &str = "Default";
pub const MIN_PAD_SPEED_PERCENT: u16 = 25;
pub const MAX_PAD_SPEED_PERCENT: u16 = 300;
pub const DEFAULT_PAD_SPEED_PERCENT: u16 = 100;
pub const MAX_PAD_REGIONS: usize = 16;
pub const MAX_REGION_NAME_CHARS: usize = 32;

pub(super) const PAD_MOTION_DEADZONE_COUNTS: i32 = 192;
pub(super) const PAD_EDGE_DEADZONE_START_COUNTS: i32 = 16_384;
pub(super) const PAD_EDGE_DEADZONE_COUNTS: i32 = 2_048;
pub(super) const PAD_EDGE_STOP_PROGRESS_COUNTS: i32 = 256;
pub(super) const PAD_MAX_DELTA_COUNTS: i32 = 32_768;
pub(super) const PAD_STOP_PROGRESS_COUNTS: i32 = 96;
pub(super) const PAD_STOP_WINDOW: Duration = Duration::from_millis(150);
pub(super) const PAD_RELEASE_GUARD: Duration = Duration::from_millis(250);
// Above ordinary press-roll wander in the supplied capture; crossing it after
// the physical press edge is treated as deliberate drag intent.
pub(super) const PAD_DRAG_THRESHOLD_COUNTS: i32 = 2_800;
// Raw captures: normal touch pressure stays under ~1,300 while every press
// drives it past 3,200. The 1,600 crossing leads the click bit by tens of
// milliseconds, so pressure catches the approach roll; the lower exit bound
// adds hysteresis.
pub(super) const PAD_PRESSURE_FREEZE_ENTER: i16 = 1_600;
pub(super) const PAD_PRESSURE_FREEZE_EXIT: i16 = 1_000;
// The captured lizard-mode transfer is essentially linear once its anchored
// noise envelope is escaped: guided fast, slow, cardinal, diagonal, and center
// precision stages all land near this raw-count ratio.
pub(super) const MOUSE_COUNTS_PER_PIXEL: i32 = 128;
pub(super) const MOUSE_MOTION_DEADZONE_COUNTS: i32 = 2_560;
pub(super) const MOUSE_EDGE_DEADZONE_COUNTS: i32 = 3_584;
pub(super) const MOUSE_STOP_PROGRESS_COUNTS: i32 = 384;
pub(super) const MOUSE_EDGE_STOP_PROGRESS_COUNTS: i32 = 768;
pub(super) const MOUSE_STOP_WINDOW: Duration = Duration::from_millis(100);
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
// A fingertip resting on a region boundary wanders by the same capacitive
// centroid noise the motion filter already compensates for. The currently
// occupied region is therefore tested with its shape grown by these margins, so
// resting on a seam holds one action instead of alternating between two.
pub(super) const REGION_HYSTERESIS_PERCENT: f32 = 4.0;
pub(super) const REGION_HYSTERESIS_DEGREES: f32 = 6.0;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadSide {
    Left,
    Right,
}

impl PadSide {
    pub const ALL: [Self; 2] = [Self::Left, Self::Right];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Left => "Left Pad",
            Self::Right => "Right Pad",
        }
    }
}

/// What a pad's continuous finger travel drives. Neither behavior is tied to a
/// side any more: either pad can take either mode, or none at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadMotionMode {
    #[default]
    None,
    Pointer,
    Scroll,
}

impl PadMotionMode {
    pub const ALL: [Self; 3] = [Self::None, Self::Pointer, Self::Scroll];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Pointer => "Relative pointer",
            Self::Scroll => "Accelerated smooth scroll",
        }
    }
}

/// One addressable area of a pad, as an annular sector.
///
/// Angles follow the same convention as the profile wheel's
/// `profile_picker::sector_for`: zero degrees points at twelve o'clock and they
/// increase clockwise, with pad Y positive upwards. Radii are a percentage of
/// the pad's full-scale radius.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PadRegionShape {
    pub start_degrees: u16,
    pub sweep_degrees: u16,
    pub inner_percent: u8,
    pub outer_percent: u8,
}

impl PadRegionShape {
    pub const WHOLE: Self = Self {
        start_degrees: 0,
        sweep_degrees: 360,
        inner_percent: 0,
        outer_percent: 100,
    };

    #[must_use]
    pub fn is_valid(self) -> bool {
        self.start_degrees < 360
            && (1..=360).contains(&self.sweep_degrees)
            && (1..=100).contains(&self.outer_percent)
            && self.inner_percent < self.outer_percent
    }
}

impl Default for PadRegionShape {
    fn default() -> Self {
        Self::WHOLE
    }
}

/// What, within a region, fires its action.
///
/// Gesture support is not implemented, but this is the enum it attaches to: a
/// swipe or rotation becomes another variant here and another arm in the
/// engine's `PadEvent` dispatch, rather than a second binding mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadTrigger {
    Click,
    Touch,
}

impl PadTrigger {
    pub const ALL: [Self; 2] = [Self::Click, Self::Touch];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Click => "Click",
            Self::Touch => "Touch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PadRegion {
    pub id: String,
    pub name: String,
    pub shape: PadRegionShape,
    #[serde(default)]
    pub click: Option<BindingAction>,
    #[serde(default)]
    pub touch: Option<BindingAction>,
}

impl PadRegion {
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>, shape: PadRegionShape) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            shape,
            click: None,
            touch: None,
        }
    }

    #[must_use]
    pub const fn is_bound(&self) -> bool {
        self.click.is_some() || self.touch.is_some()
    }

    #[must_use]
    pub const fn action(&self, trigger: PadTrigger) -> Option<&BindingAction> {
        match trigger {
            PadTrigger::Click => self.click.as_ref(),
            PadTrigger::Touch => self.touch.as_ref(),
        }
    }

    pub fn action_mut(&mut self, trigger: PadTrigger) -> &mut Option<BindingAction> {
        match trigger {
            PadTrigger::Click => &mut self.click,
            PadTrigger::Touch => &mut self.touch,
        }
    }

    /// One region covering the entire pad. This is what a pre-region pad-click
    /// binding migrates into.
    #[must_use]
    pub fn whole() -> Vec<Self> {
        vec![Self::new("region-1", "Whole Pad", PadRegionShape::WHOLE)]
    }

    #[must_use]
    pub fn four_way() -> Vec<Self> {
        Self::compass(4, 0)
    }

    #[must_use]
    pub fn eight_way() -> Vec<Self> {
        Self::compass(8, 0)
    }

    #[must_use]
    pub fn four_way_with_center() -> Vec<Self> {
        Self::compass(4, DEFAULT_CENTER_PERCENT)
    }

    #[must_use]
    pub fn eight_way_with_center() -> Vec<Self> {
        Self::compass(8, DEFAULT_CENTER_PERCENT)
    }

    pub const PRESETS: [PadRegionPreset; 5] = [
        PadRegionPreset {
            label: "Whole pad",
            build: Self::whole,
        },
        PadRegionPreset {
            label: "Four way",
            build: Self::four_way,
        },
        PadRegionPreset {
            label: "Four way + center",
            build: Self::four_way_with_center,
        },
        PadRegionPreset {
            label: "Eight way",
            build: Self::eight_way,
        },
        PadRegionPreset {
            label: "Eight way + center",
            build: Self::eight_way_with_center,
        },
    ];

    /// Equal sectors centred on their compass direction, optionally around a
    /// centre disc. The centre is listed first so first-match-wins resolution
    /// lets it shadow the sectors it sits inside.
    #[must_use]
    fn compass(sectors: u16, center_percent: u8) -> Vec<Self> {
        let names: &[&str] = if sectors == 4 {
            &["Top", "Right", "Bottom", "Left"]
        } else {
            &[
                "Top",
                "Top Right",
                "Right",
                "Bottom Right",
                "Bottom",
                "Bottom Left",
                "Left",
                "Top Left",
            ]
        };
        let sweep = 360 / sectors;
        let mut regions = Vec::with_capacity(names.len() + usize::from(center_percent > 0));
        if center_percent > 0 {
            regions.push(Self::new(
                "region-1",
                "Center",
                PadRegionShape {
                    inner_percent: 0,
                    outer_percent: center_percent,
                    ..PadRegionShape::WHOLE
                },
            ));
        }
        for (index, name) in names.iter().enumerate() {
            let index = u16::try_from(index).unwrap_or(0);
            regions.push(Self::new(
                format!("region-{}", regions.len() + 1),
                *name,
                PadRegionShape {
                    // Sectors are centred on their direction, so the first one
                    // starts half an arc before twelve o'clock.
                    start_degrees: (index * sweep + 360 - sweep / 2) % 360,
                    sweep_degrees: sweep,
                    inner_percent: center_percent,
                    outer_percent: 100,
                },
            ));
        }
        regions
    }
}

pub const DEFAULT_CENTER_PERCENT: u8 = 30;

/// A named starting layout the editor can drop onto a pad.
#[derive(Debug, Clone, Copy)]
pub struct PadRegionPreset {
    pub label: &'static str,
    pub build: fn() -> Vec<PadRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PadConfig {
    pub motion: PadMotionMode,
    pub speed_percent: u16,
    pub momentum: bool,
    pub feedback: PadFeedbackConfig,
    pub regions: Vec<PadRegion>,
}

impl Default for PadConfig {
    fn default() -> Self {
        Self {
            motion: PadMotionMode::None,
            speed_percent: DEFAULT_PAD_SPEED_PERCENT,
            momentum: true,
            feedback: PadFeedbackConfig::default(),
            regions: Vec::new(),
        }
    }
}

impl PadConfig {
    #[must_use]
    pub fn configured_count(&self) -> usize {
        usize::from(self.motion != PadMotionMode::None)
            + self
                .regions
                .iter()
                .filter(|region| region.is_bound())
                .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PadBindings {
    pub left: PadConfig,
    pub right: PadConfig,
}

impl PadBindings {
    #[must_use]
    pub const fn get(&self, side: PadSide) -> &PadConfig {
        match side {
            PadSide::Left => &self.left,
            PadSide::Right => &self.right,
        }
    }

    pub fn get_mut(&mut self, side: PadSide) -> &mut PadConfig {
        match side {
            PadSide::Left => &mut self.left,
            PadSide::Right => &mut self.right,
        }
    }

    #[must_use]
    pub fn configured_count(&self) -> usize {
        self.left.configured_count() + self.right.configured_count()
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

impl From<&SteamControllerState> for DesktopInputSnapshot {
    fn from(state: &SteamControllerState) -> Self {
        Self {
            buttons: state.buttons,
            left_pad: PadSample {
                x: state.left_pad_x,
                y: state.left_pad_y,
                pressure: state.left_pad_pressure,
                touched: state.left_pad_touched,
                pressed: state.left_pad_pressed,
            },
            right_pad: PadSample {
                x: state.right_pad_x,
                y: state.right_pad_y,
                pressure: state.right_pad_pressure,
                touched: state.right_pad_touched,
                pressed: state.right_pad_pressed,
            },
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
