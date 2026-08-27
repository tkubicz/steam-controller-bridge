//! Platform-neutral, validated gamepad state.

/// Stable button indices shared by host applications and firmware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Button {
    South = 0,
    East = 1,
    West = 2,
    North = 3,
    LeftShoulder = 4,
    RightShoulder = 5,
    LeftStick = 6,
    RightStick = 7,
    Back = 8,
    Start = 9,
    Guide = 10,
    LeftGrip = 11,
    RightGrip = 12,
    Extra1 = 13,
    Extra2 = 14,
    Extra3 = 15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GamepadButtons(pub u32);

impl GamepadButtons {
    #[must_use]
    pub const fn contains(self, button: Button) -> bool {
        self.0 & (1_u32 << button as u8) != 0
    }

    pub fn set(&mut self, button: Button, pressed: bool) {
        let mask = 1_u32 << button as u8;
        if pressed {
            self.0 |= mask;
        } else {
            self.0 &= !mask;
        }
    }

    pub fn clear(&mut self) {
        self.0 = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum HatState {
    North = 0,
    NorthEast = 1,
    East = 2,
    SouthEast = 3,
    South = 4,
    SouthWest = 5,
    West = 6,
    NorthWest = 7,
    #[default]
    Centered = 8,
}

impl TryFrom<u8> for HatState {
    type Error = InvalidHat;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::North),
            1 => Ok(Self::NorthEast),
            2 => Ok(Self::East),
            3 => Ok(Self::SouthEast),
            4 => Ok(Self::South),
            5 => Ok(Self::SouthWest),
            6 => Ok(Self::West),
            7 => Ok(Self::NorthWest),
            8 => Ok(Self::Centered),
            other => Err(InvalidHat(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidHat(pub u8);

impl std::fmt::Display for InvalidHat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid hat value {}", self.0)
    }
}

impl std::error::Error for InvalidHat {}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GamepadState {
    pub buttons: GamepadButtons,
    pub hat: HatState,
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
    pub left_trigger: f32,
    pub right_trigger: f32,
}

impl GamepadState {
    pub const NEUTRAL: Self = Self {
        buttons: GamepadButtons(0),
        hat: HatState::Centered,
        left_x: 0.0,
        left_y: 0.0,
        right_x: 0.0,
        right_y: 0.0,
        left_trigger: 0.0,
        right_trigger: 0.0,
    };

    #[must_use]
    pub const fn neutral() -> Self {
        Self::NEUTRAL
    }

    /// Replaces non-finite values with neutral and clamps all axes to their ranges.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.left_x = sanitize(self.left_x, -1.0, 1.0);
        self.left_y = sanitize(self.left_y, -1.0, 1.0);
        self.right_x = sanitize(self.right_x, -1.0, 1.0);
        self.right_y = sanitize(self.right_y, -1.0, 1.0);
        self.left_trigger = sanitize(self.left_trigger, 0.0, 1.0);
        self.right_trigger = sanitize(self.right_trigger, 0.0, 1.0);
        self
    }

    /// Checks that every axis is finite and within its documented range.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidState`] for the first non-finite or out-of-range axis.
    pub fn validate(&self) -> Result<(), InvalidState> {
        validate_axis("left_x", self.left_x, -1.0, 1.0)?;
        validate_axis("left_y", self.left_y, -1.0, 1.0)?;
        validate_axis("right_x", self.right_x, -1.0, 1.0)?;
        validate_axis("right_y", self.right_y, -1.0, 1.0)?;
        validate_axis("left_trigger", self.left_trigger, 0.0, 1.0)?;
        validate_axis("right_trigger", self.right_trigger, 0.0, 1.0)
    }
}

/// How much of the controller a host feature is holding back from the game.
///
/// **Which variant is correct depends on whether the state is pinned, and
/// getting it wrong faults the hardware.** Firmware arms its controller-data
/// watchdog only for reports that are *not* exactly neutral, and the host stops
/// refreshing unchanged state once it has sent a neutral one. So a state that is
/// both **pinned** and **non-neutral** leaves the watchdog armed while the host
/// has nothing new to send, and any host stall longer than the watchdog then
/// faults the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputSuppression {
    /// Every control at rest, for a feature that has taken the controller over.
    ///
    /// Required whenever the state is pinned for as long as the feature lasts,
    /// because exactly-neutral is what disarms the watchdog -- which makes the
    /// fault above impossible however badly the host is scheduled. It is also
    /// the right semantics: a user in an overlay is not playing, so a trigger
    /// they happen to be holding should not keep firing.
    Neutral,
    /// Specific buttons withheld while the rest of the controller works.
    ///
    /// For controls a feature has already consumed and that are still physically
    /// held after it finishes -- the button that dismissed an overlay must not
    /// reach the game just because the user has not let go yet. Safe despite the
    /// note above because the state is no longer pinned: it tracks the
    /// controller again, so it either keeps changing or settles at exactly
    /// neutral once the user lets go.
    Buttons(GamepadButtons),
}

impl OutputSuppression {
    /// Withholds whatever this variant covers.
    pub fn apply(self, state: &mut GamepadState) {
        match self {
            Self::Neutral => *state = GamepadState::NEUTRAL,
            Self::Buttons(buttons) => state.buttons.0 &= !buttons.0,
        }
    }
}

fn sanitize(value: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        0.0
    }
}

fn validate_axis(name: &'static str, value: f32, min: f32, max: f32) -> Result<(), InvalidState> {
    if !value.is_finite() {
        Err(InvalidState::NonFinite(name))
    } else if !(min..=max).contains(&value) {
        Err(InvalidState::OutOfRange(name))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidState {
    NonFinite(&'static str),
    OutOfRange(&'static str),
}

impl std::fmt::Display for InvalidState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFinite(name) => write!(f, "{name} is not finite"),
            Self::OutOfRange(name) => write!(f, "{name} is outside its documented range"),
        }
    }
}

impl std::error::Error for InvalidState {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_has_no_active_controls() {
        let state = GamepadState::neutral();
        assert_eq!(state, GamepadState::default());
        assert_eq!(state.hat, HatState::Centered);
        assert_eq!(state.buttons.0, 0);
        assert!(state.validate().is_ok());
    }

    #[test]
    fn buttons_have_stable_independent_bits() {
        let all = [
            Button::South,
            Button::East,
            Button::West,
            Button::North,
            Button::LeftShoulder,
            Button::RightShoulder,
            Button::LeftStick,
            Button::RightStick,
            Button::Back,
            Button::Start,
            Button::Guide,
            Button::LeftGrip,
            Button::RightGrip,
            Button::Extra1,
            Button::Extra2,
            Button::Extra3,
        ];
        let mut buttons = GamepadButtons::default();
        for (index, button) in all.into_iter().enumerate() {
            assert_eq!(button as usize, index);
            buttons.set(button, true);
            assert!(buttons.contains(button));
            buttons.set(button, false);
            assert!(!buttons.contains(button));
        }
        buttons.set(Button::South, true);
        buttons.clear();
        assert_eq!(buttons.0, 0);
    }

    #[test]
    fn sanitization_clamps_and_neutralizes_non_finite_values() {
        let state = GamepadState {
            left_x: -2.0,
            left_y: f32::NAN,
            right_x: f32::INFINITY,
            right_y: 2.0,
            left_trigger: -1.0,
            right_trigger: 2.0,
            ..GamepadState::neutral()
        }
        .sanitized();
        assert_eq!((state.left_x, state.left_y), (-1.0, 0.0));
        assert_eq!((state.right_x, state.right_y), (0.0, 1.0));
        assert_eq!((state.left_trigger, state.right_trigger), (0.0, 1.0));
        assert!(state.validate().is_ok());
    }

    #[test]
    fn validation_rejects_bad_values() {
        let mut state = GamepadState::neutral();
        state.left_x = f32::NAN;
        assert_eq!(state.validate(), Err(InvalidState::NonFinite("left_x")));
        state.left_x = 1.1;
        assert_eq!(state.validate(), Err(InvalidState::OutOfRange("left_x")));
    }

    #[test]
    fn suppression_leaves_no_control_active() {
        // Every control, not just the ones an overlay reads: a partially
        // suppressed state is still non-neutral, and firmware keeps its
        // controller-data watchdog armed for anything that is not exactly
        // neutral. Leaving one control active is what turns a slow host into a
        // faulted device.
        let mut buttons = GamepadButtons::default();
        for button in [Button::South, Button::Extra3, Button::North, Button::Guide] {
            buttons.set(button, true);
        }
        let mut state = GamepadState {
            buttons,
            hat: HatState::North,
            left_x: 0.9,
            left_y: -0.5,
            right_x: -0.3,
            right_y: 0.7,
            left_trigger: 0.4,
            right_trigger: 1.0,
        };

        OutputSuppression::Neutral.apply(&mut state);

        assert_eq!(state, GamepadState::NEUTRAL);
    }

    #[test]
    fn a_suppressed_state_is_exactly_the_neutral_the_wire_agrees_on() {
        // The firmware disarms its watchdog by comparing against its own
        // neutral report, so "close enough to neutral" is not good enough.
        let mut state = GamepadState {
            hat: HatState::SouthWest,
            left_trigger: 0.01,
            ..GamepadState::neutral()
        };
        OutputSuppression::Neutral.apply(&mut state);
        assert_eq!(state.hat, HatState::Centered);
        assert!(state.left_trigger.abs() < f32::EPSILON);
        assert_eq!(state.buttons.0, 0);
        assert!(state.validate().is_ok());
    }

    #[test]
    fn every_hat_value_converts() {
        for value in 0..=8 {
            assert_eq!(HatState::try_from(value).map(u8::from), Ok(value));
        }
        assert_eq!(HatState::try_from(9), Err(InvalidHat(9)));
    }

    impl From<HatState> for u8 {
        fn from(value: HatState) -> Self {
            value as Self
        }
    }
}
