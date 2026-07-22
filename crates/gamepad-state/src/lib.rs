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
