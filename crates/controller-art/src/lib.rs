//! The Steam Controller illustration, shared by the bindings editor and the
//! visualizer.
//!
//! The artwork was traced from reference photographs into a unit square. A
//! `view: egui::Rect` is a square screen rect standing for that unit square and
//! is the only transform involved.
//!
//! Callers describe what each control should look like this frame and the crate
//! decides how to paint it. That one indirection is what lets the editor's
//! "this control is selected" and the visualizer's "this control is pressed"
//! share the same drawing code: they want the same treatment and differ only
//! in what drives it.
//!
//! This crate deliberately knows nothing about bindings or report bits. Each
//! consumer owns its own adapter into [`Control`].

mod geometry;
mod paint;
mod shape;

pub use geometry::{
    body_bounds, body_rect, control_rect, locus_point, normalized_point, trackpad_rect, unit_rect,
    view_for_available, view_for_body,
};
pub use paint::{draw_body, draw_front, draw_rear};

/// Which side of the controller a drawing shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    Front,
    Rear,
}

/// Left or right, for the two trackpads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PadSide {
    Left,
    Right,
}

impl PadSide {
    pub const ALL: [Self; 2] = [Self::Left, Self::Right];

    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }
}

/// Every physically drawable control.
///
/// Named after the **physical** buttons, not the report bits. That distinction
/// matters for two of them: `docs/MAPPING.md` records that the source-bit names
/// are reversed at this boundary, so the bit called `View` is the physical Menu
/// button on the right and the bit called `Menu` is the physical View button on
/// the left. Consumers cross them over in their own adapter.
///
/// The two grip-shell capacitive sensors are absent on purpose: they are the
/// shell itself, not the L4/L5 paddles, and have no geometry to light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Control {
    A,
    B,
    X,
    Y,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    View,
    Menu,
    Steam,
    QuickAccess,
    LeftBumper,
    RightBumper,
    LeftTrigger,
    RightTrigger,
    LeftStick,
    RightStick,
    LeftPad,
    RightPad,
    L4,
    L5,
    R4,
    R5,
}

impl Control {
    pub const ALL: [Self; 24] = [
        Self::A,
        Self::B,
        Self::X,
        Self::Y,
        Self::DpadUp,
        Self::DpadDown,
        Self::DpadLeft,
        Self::DpadRight,
        Self::View,
        Self::Menu,
        Self::Steam,
        Self::QuickAccess,
        Self::LeftBumper,
        Self::RightBumper,
        Self::LeftTrigger,
        Self::RightTrigger,
        Self::LeftStick,
        Self::RightStick,
        Self::LeftPad,
        Self::RightPad,
        Self::L4,
        Self::L5,
        Self::R4,
        Self::R5,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::X => "X",
            Self::Y => "Y",
            Self::DpadUp => "D-pad Up",
            Self::DpadDown => "D-pad Down",
            Self::DpadLeft => "D-pad Left",
            Self::DpadRight => "D-pad Right",
            Self::View => "View",
            Self::Menu => "Menu",
            Self::Steam => "Steam",
            Self::QuickAccess => "Quick Access",
            Self::LeftBumper => "LB",
            Self::RightBumper => "RB",
            Self::LeftTrigger => "LT",
            Self::RightTrigger => "RT",
            Self::LeftStick => "Left stick",
            Self::RightStick => "Right stick",
            Self::LeftPad => "Left pad",
            Self::RightPad => "Right pad",
            Self::L4 => "L4",
            Self::L5 => "L5",
            Self::R4 => "R4",
            Self::R5 => "R5",
        }
    }

    /// The face this control's own geometry lives on.
    ///
    /// The bumpers are the one control drawn on both: the front shows the cap
    /// riding on the shell corner, the rear shows the wing. Their face is
    /// [`Face::Rear`], where the distinct R1/L1 shape is, but [`draw_front`]
    /// lights them too.
    #[must_use]
    pub const fn face(self) -> Face {
        match self {
            Self::LeftTrigger
            | Self::RightTrigger
            | Self::LeftBumper
            | Self::RightBumper
            | Self::L4
            | Self::L5
            | Self::R4
            | Self::R5 => Face::Rear,
            _ => Face::Front,
        }
    }

    /// Whether this control reports a live position or travel, and which.
    #[must_use]
    pub const fn analog_kind(self) -> Option<AnalogKind> {
        match self {
            Self::LeftStick | Self::RightStick | Self::LeftPad | Self::RightPad => {
                Some(AnalogKind::Position)
            }
            Self::LeftTrigger | Self::RightTrigger => Some(AnalogKind::Trigger),
            _ => None,
        }
    }
}

/// Which live treatment a control accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalogKind {
    Position,
    Trigger,
}

/// The discrete emphasis a control carries this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Highlight {
    #[default]
    Idle,
    /// Under the pointer.
    Hover,
    /// Selected for editing, or held down.
    Active,
}

/// Live analog treatment. The variants are mutually exclusive so a stick
/// cannot be given trigger travel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Analog {
    /// A control with a position sensor.
    ///
    /// The two fields are independent because the underlying sensors are. A
    /// stick always has a meaningful deflection but is only *touched* when its
    /// capacitive ring says so; a trackpad's coordinates are stale unless a
    /// finger is down, so its position disappears while its touch is false.
    Position {
        /// Where the contact is inside the control's own area — x right, y up,
        /// each `-1.0..=1.0`. `None` when no position is meaningful now.
        offset: Option<[f32; 2]>,
        /// Capacitive contact. Drives the outline halo, not the dot.
        touched: bool,
    },
    /// Trigger pull, `0.0..=1.0`.
    Trigger { travel: f32 },
}

/// How one control should look this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ControlState {
    pub highlight: Highlight,
    pub analog: Option<Analog>,
}

impl ControlState {
    pub const IDLE: Self = Self {
        highlight: Highlight::Idle,
        analog: None,
    };

    /// A control that is held down or selected.
    #[must_use]
    pub const fn active() -> Self {
        Self {
            highlight: Highlight::Active,
            analog: None,
        }
    }

    /// A control the pointer is over.
    #[must_use]
    pub const fn hovered() -> Self {
        Self {
            highlight: Highlight::Hover,
            analog: None,
        }
    }
}

/// Every control resolves to idle. Useful as a base for callers that only set
/// a few.
#[must_use]
pub fn idle(_: Control) -> ControlState {
    ControlState::IDLE
}

#[cfg(test)]
mod tests;
