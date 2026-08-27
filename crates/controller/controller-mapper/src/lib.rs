//! Steam Controller 2 to generic gamepad mapping and reusable filters.

use gamepad_state::{Button, GamepadState, HatState};
use steam_controller_protocol::{SteamButton, SteamControllerState};

pub trait StateFilter {
    fn apply(&mut self, state: &mut GamepadState, delta_time: f32);

    fn reset(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightAxisSource {
    RightPad,
    #[default]
    RightStick,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapperConfig {
    pub left_stick_dead_zone: f32,
    pub right_axis_dead_zone: f32,
    pub trigger_dead_zone: f32,
    pub stick_sensitivity: f32,
    pub stick_saturation: f32,
    pub trigger_saturation: f32,
    pub axis_inversion: AxisInversionFilter,
    pub smoothing_time_constant: Option<f32>,
    /// Source value treated as a full trigger pull. `OpenPuck` observes about 0x8000.
    pub trigger_full_scale: u16,
    pub right_axis_source: RightAxisSource,
}

impl Default for MapperConfig {
    fn default() -> Self {
        Self {
            left_stick_dead_zone: 0.08,
            right_axis_dead_zone: 0.08,
            trigger_dead_zone: 0.02,
            stick_sensitivity: 1.0,
            stick_saturation: 1.0,
            trigger_saturation: 1.0,
            axis_inversion: AxisInversionFilter::NONE,
            smoothing_time_constant: None,
            trigger_full_scale: 0x8000,
            right_axis_source: RightAxisSource::RightStick,
        }
    }
}

impl MapperConfig {
    /// Validates all profile parameters before they enter the hot path.
    ///
    /// # Errors
    ///
    /// Returns [`MappingError`] for non-finite values, invalid normalized
    /// ranges, a non-positive sensitivity, or a zero trigger calibration.
    pub fn validate(&self) -> Result<(), MappingError> {
        normalized("left_stick_dead_zone", self.left_stick_dead_zone, false)?;
        normalized("right_axis_dead_zone", self.right_axis_dead_zone, false)?;
        normalized("trigger_dead_zone", self.trigger_dead_zone, false)?;
        normalized("stick_saturation", self.stick_saturation, true)?;
        normalized("trigger_saturation", self.trigger_saturation, true)?;
        if !self.stick_sensitivity.is_finite() || self.stick_sensitivity <= 0.0 {
            return Err(MappingError::InvalidConfig("stick_sensitivity"));
        }
        if self
            .smoothing_time_constant
            .is_some_and(|value| !value.is_finite() || value <= 0.0)
        {
            return Err(MappingError::InvalidConfig("smoothing_time_constant"));
        }
        if self.trigger_full_scale == 0 {
            return Err(MappingError::InvalidConfig("trigger_full_scale"));
        }
        Ok(())
    }
}

fn normalized(name: &'static str, value: f32, nonzero: bool) -> Result<(), MappingError> {
    let lower_valid = if nonzero { value > 0.0 } else { value >= 0.0 };
    if value.is_finite() && lower_valid && value <= 1.0 {
        Ok(())
    } else {
        Err(MappingError::InvalidConfig(name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingError {
    InvalidConfig(&'static str),
}

impl std::fmt::Display for MappingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(name) => write!(f, "invalid mapper configuration field {name}"),
        }
    }
}

impl std::error::Error for MappingError {}

pub struct ControllerMapper {
    config: MapperConfig,
    smoothing: LowPassFilter,
}

impl ControllerMapper {
    /// Builds a validated mapper profile.
    ///
    /// # Errors
    ///
    /// Returns [`MappingError`] when any configuration field is invalid.
    pub fn new(config: MapperConfig) -> Result<Self, MappingError> {
        config.validate()?;
        Ok(Self {
            smoothing: LowPassFilter::new(config.smoothing_time_constant),
            config,
        })
    }

    #[must_use]
    pub const fn config(&self) -> &MapperConfig {
        &self.config
    }

    /// Maps and filters one decoded state. Discrete controls are never smoothed.
    #[must_use]
    pub fn map(&mut self, input: &SteamControllerState, delta_time: f32) -> GamepadState {
        let mut state = self.map_unfiltered(input);
        let mut axis_inversion = self.config.axis_inversion;
        axis_inversion.apply(&mut state, delta_time);
        RadialDeadZoneFilter {
            left: self.config.left_stick_dead_zone,
            right: self.config.right_axis_dead_zone,
        }
        .apply(&mut state, delta_time);
        TriggerDeadZoneFilter {
            dead_zone: self.config.trigger_dead_zone,
        }
        .apply(&mut state, delta_time);
        SensitivityFilter {
            exponent: self.config.stick_sensitivity,
        }
        .apply(&mut state, delta_time);
        SaturationFilter {
            sticks: self.config.stick_saturation,
            triggers: self.config.trigger_saturation,
        }
        .apply(&mut state, delta_time);
        self.smoothing.apply(&mut state, delta_time);
        ClampFilter.apply(&mut state, delta_time);
        state
    }

    pub fn reset(&mut self) {
        self.smoothing.reset();
    }

    fn map_unfiltered(&self, input: &SteamControllerState) -> GamepadState {
        let mut state = GamepadState::neutral();
        for source in DIRECT_GAMEPAD_BUTTONS {
            if let Some(target) = gamepad_button(source) {
                map_button(input, source, &mut state, target);
            }
        }
        state.buttons.set(
            Button::RightStick,
            input.buttons.contains(SteamButton::RightStickPress)
                || (self.config.right_axis_source == RightAxisSource::RightPad
                    && input.buttons.contains(SteamButton::RightPadClick)),
        );
        state.hat = map_hat(input);
        state.left_x = normalize_axis(input.left_stick_x);
        state.left_y = normalize_axis(input.left_stick_y);
        let (right_x, right_y) = match self.config.right_axis_source {
            RightAxisSource::RightPad if input.right_pad_touched => {
                (input.right_pad_x, input.right_pad_y)
            }
            RightAxisSource::RightPad => (0, 0),
            RightAxisSource::RightStick => (input.right_stick_x, input.right_stick_y),
        };
        state.right_x = normalize_axis(right_x);
        state.right_y = normalize_axis(right_y);
        state.left_trigger = normalize_trigger(input.left_trigger, self.config.trigger_full_scale);
        state.right_trigger =
            normalize_trigger(input.right_trigger, self.config.trigger_full_scale);
        state
    }
}

const DIRECT_GAMEPAD_BUTTONS: [SteamButton; 16] = [
    SteamButton::A,
    SteamButton::B,
    SteamButton::X,
    SteamButton::Y,
    SteamButton::LeftShoulder,
    SteamButton::RightShoulder,
    SteamButton::LeftStickPress,
    SteamButton::RightStickPress,
    SteamButton::View,
    SteamButton::Menu,
    SteamButton::Steam,
    SteamButton::LeftGrip4,
    SteamButton::RightGrip4,
    SteamButton::LeftGrip5,
    SteamButton::RightGrip5,
    SteamButton::QuickAccess,
];

/// Returns the stable, direct generic-gamepad mapping for a Steam button.
///
/// D-pad directions map to the hat, and pad/touch/trigger-click controls are
/// conditional or host-only, so they return `None`. Triton's counterintuitive
/// naming is preserved here: SDL/OpenPuck map View to Start and Menu to Back.
#[must_use]
pub const fn gamepad_button(source: SteamButton) -> Option<Button> {
    match source {
        SteamButton::A => Some(Button::South),
        SteamButton::B => Some(Button::East),
        SteamButton::X => Some(Button::West),
        SteamButton::Y => Some(Button::North),
        SteamButton::LeftShoulder => Some(Button::LeftShoulder),
        SteamButton::RightShoulder => Some(Button::RightShoulder),
        SteamButton::LeftStickPress => Some(Button::LeftStick),
        SteamButton::RightStickPress => Some(Button::RightStick),
        SteamButton::View => Some(Button::Start),
        SteamButton::Menu => Some(Button::Back),
        SteamButton::Steam => Some(Button::Guide),
        SteamButton::LeftGrip4 => Some(Button::LeftGrip),
        SteamButton::RightGrip4 => Some(Button::RightGrip),
        SteamButton::LeftGrip5 => Some(Button::Extra1),
        SteamButton::RightGrip5 => Some(Button::Extra2),
        SteamButton::QuickAccess => Some(Button::Extra3),
        SteamButton::DpadDown
        | SteamButton::DpadRight
        | SteamButton::DpadLeft
        | SteamButton::DpadUp
        | SteamButton::RightStickTouch
        | SteamButton::RightPadTouch
        | SteamButton::RightPadClick
        | SteamButton::RightTriggerClick
        | SteamButton::LeftStickTouch
        | SteamButton::LeftPadTouch
        | SteamButton::LeftPadClick
        | SteamButton::LeftTriggerClick
        | SteamButton::RightGripTouch
        | SteamButton::LeftGripTouch => None,
    }
}

impl Default for ControllerMapper {
    fn default() -> Self {
        Self::new(MapperConfig::default()).expect("default mapper configuration is valid")
    }
}

fn map_button(
    input: &SteamControllerState,
    source: SteamButton,
    output: &mut GamepadState,
    target: Button,
) {
    output.buttons.set(target, input.buttons.contains(source));
}

fn map_hat(input: &SteamControllerState) -> HatState {
    let up = input.buttons.contains(SteamButton::DpadUp);
    let down = input.buttons.contains(SteamButton::DpadDown);
    let left = input.buttons.contains(SteamButton::DpadLeft);
    let right = input.buttons.contains(SteamButton::DpadRight);
    let vertical = i8::from(up) - i8::from(down);
    let horizontal = i8::from(right) - i8::from(left);
    match (horizontal, vertical) {
        (0, 1) => HatState::North,
        (1, 1) => HatState::NorthEast,
        (1, 0) => HatState::East,
        (1, -1) => HatState::SouthEast,
        (0, -1) => HatState::South,
        (-1, -1) => HatState::SouthWest,
        (-1, 0) => HatState::West,
        (-1, 1) => HatState::NorthWest,
        _ => HatState::Centered,
    }
}

/// Normalizes one signed controller axis to the mapper's input range.
#[must_use]
pub fn normalize_axis(value: i16) -> f32 {
    (f32::from(value) / 32767.0).clamp(-1.0, 1.0)
}

/// Normalizes trigger travel against the profile's physical full-pull value.
/// A zero scale is treated as neutral for diagnostic callers; mapper profiles
/// reject it during validation.
#[must_use]
pub fn normalize_trigger(value: u16, full_scale: u16) -> f32 {
    if full_scale == 0 {
        return 0.0;
    }
    (f32::from(value) / f32::from(full_scale)).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisDeadZoneFilter {
    pub dead_zone: f32,
}

impl StateFilter for AxisDeadZoneFilter {
    fn apply(&mut self, state: &mut GamepadState, _delta_time: f32) {
        state.left_x = axis_dead_zone(state.left_x, self.dead_zone);
        state.left_y = axis_dead_zone(state.left_y, self.dead_zone);
        state.right_x = axis_dead_zone(state.right_x, self.dead_zone);
        state.right_y = axis_dead_zone(state.right_y, self.dead_zone);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadialDeadZoneFilter {
    pub left: f32,
    pub right: f32,
}

impl StateFilter for RadialDeadZoneFilter {
    fn apply(&mut self, state: &mut GamepadState, _delta_time: f32) {
        (state.left_x, state.left_y) = radial_dead_zone(state.left_x, state.left_y, self.left);
        (state.right_x, state.right_y) = radial_dead_zone(state.right_x, state.right_y, self.right);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriggerDeadZoneFilter {
    pub dead_zone: f32,
}

impl StateFilter for TriggerDeadZoneFilter {
    fn apply(&mut self, state: &mut GamepadState, _delta_time: f32) {
        state.left_trigger = trigger_dead_zone(state.left_trigger, self.dead_zone);
        state.right_trigger = trigger_dead_zone(state.right_trigger, self.dead_zone);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct AxisInversionFilter {
    pub left_x: bool,
    pub left_y: bool,
    pub right_x: bool,
    pub right_y: bool,
}

impl AxisInversionFilter {
    pub const NONE: Self = Self {
        left_x: false,
        left_y: false,
        right_x: false,
        right_y: false,
    };
}

impl StateFilter for AxisInversionFilter {
    fn apply(&mut self, state: &mut GamepadState, _delta_time: f32) {
        if self.left_x {
            state.left_x = -state.left_x;
        }
        if self.left_y {
            state.left_y = -state.left_y;
        }
        if self.right_x {
            state.right_x = -state.right_x;
        }
        if self.right_y {
            state.right_y = -state.right_y;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensitivityFilter {
    pub exponent: f32,
}

impl StateFilter for SensitivityFilter {
    fn apply(&mut self, state: &mut GamepadState, _delta_time: f32) {
        for value in [
            &mut state.left_x,
            &mut state.left_y,
            &mut state.right_x,
            &mut state.right_y,
        ] {
            *value = signed_curve(*value, self.exponent);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SaturationFilter {
    pub sticks: f32,
    pub triggers: f32,
}

impl StateFilter for SaturationFilter {
    fn apply(&mut self, state: &mut GamepadState, _delta_time: f32) {
        for value in [
            &mut state.left_x,
            &mut state.left_y,
            &mut state.right_x,
            &mut state.right_y,
        ] {
            *value = saturate_signed(*value, self.sticks);
        }
        state.left_trigger = saturate_unsigned(state.left_trigger, self.triggers);
        state.right_trigger = saturate_unsigned(state.right_trigger, self.triggers);
    }
}

pub struct LowPassFilter {
    time_constant: Option<f32>,
    previous: GamepadState,
}

impl LowPassFilter {
    #[must_use]
    pub const fn new(time_constant: Option<f32>) -> Self {
        Self {
            time_constant,
            previous: GamepadState::NEUTRAL,
        }
    }
}

impl StateFilter for LowPassFilter {
    fn apply(&mut self, state: &mut GamepadState, delta_time: f32) {
        let Some(time_constant) = self.time_constant else {
            self.previous = *state;
            return;
        };
        let alpha = if delta_time.is_finite() && delta_time > 0.0 {
            1.0 - (-delta_time / time_constant).exp()
        } else {
            0.0
        };
        state.left_x = lerp(self.previous.left_x, state.left_x, alpha);
        state.left_y = lerp(self.previous.left_y, state.left_y, alpha);
        state.right_x = lerp(self.previous.right_x, state.right_x, alpha);
        state.right_y = lerp(self.previous.right_y, state.right_y, alpha);
        state.left_trigger = lerp(self.previous.left_trigger, state.left_trigger, alpha);
        state.right_trigger = lerp(self.previous.right_trigger, state.right_trigger, alpha);
        self.previous = *state;
    }

    fn reset(&mut self) {
        self.previous = GamepadState::neutral();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClampFilter;

impl StateFilter for ClampFilter {
    fn apply(&mut self, state: &mut GamepadState, _delta_time: f32) {
        *state = state.sanitized();
    }
}

fn axis_dead_zone(value: f32, dead_zone: f32) -> f32 {
    let value = finite_or_zero(value).clamp(-1.0, 1.0);
    let dead_zone = finite_or_zero(dead_zone).clamp(0.0, 0.999_999);
    if value.abs() <= dead_zone {
        0.0
    } else {
        value.signum() * ((value.abs() - dead_zone) / (1.0 - dead_zone))
    }
}

fn radial_dead_zone(x: f32, y: f32, dead_zone: f32) -> (f32, f32) {
    let x = finite_or_zero(x);
    let y = finite_or_zero(y);
    let magnitude = x.hypot(y);
    let dead_zone = finite_or_zero(dead_zone).clamp(0.0, 0.999_999);
    if magnitude <= dead_zone || magnitude == 0.0 {
        return (0.0, 0.0);
    }
    let scaled = ((magnitude.min(1.0) - dead_zone) / (1.0 - dead_zone)).clamp(0.0, 1.0);
    (x / magnitude * scaled, y / magnitude * scaled)
}

fn trigger_dead_zone(value: f32, dead_zone: f32) -> f32 {
    let value = finite_or_zero(value).clamp(0.0, 1.0);
    let dead_zone = finite_or_zero(dead_zone).clamp(0.0, 0.999_999);
    if value <= dead_zone {
        0.0
    } else {
        (value - dead_zone) / (1.0 - dead_zone)
    }
}

fn signed_curve(value: f32, exponent: f32) -> f32 {
    let value = finite_or_zero(value).clamp(-1.0, 1.0);
    let exponent = if exponent.is_finite() && exponent > 0.0 {
        exponent
    } else {
        1.0
    };
    value.signum() * value.abs().powf(exponent)
}

fn saturate_signed(value: f32, threshold: f32) -> f32 {
    let threshold = finite_or_zero(threshold).clamp(f32::EPSILON, 1.0);
    (finite_or_zero(value) / threshold).clamp(-1.0, 1.0)
}

fn saturate_unsigned(value: f32, threshold: f32) -> f32 {
    let threshold = finite_or_zero(threshold).clamp(f32::EPSILON, 1.0);
    (finite_or_zero(value) / threshold).clamp(0.0, 1.0)
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn lerp(previous: f32, current: f32, alpha: f32) -> f32 {
    previous + (current - previous) * alpha.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use steam_controller_protocol::{
        DecodedReport, SteamControllerDecoder, INPUT_REPORT_ID, INPUT_REPORT_SIZE,
    };

    fn source_state() -> SteamControllerState {
        let mut report = vec![0_u8; INPUT_REPORT_SIZE];
        report[0] = INPUT_REPORT_ID;
        let DecodedReport::ControllerState(state) = SteamControllerDecoder::new()
            .decode(INPUT_REPORT_ID, &report)
            .unwrap()
        else {
            panic!("state expected");
        };
        state
    }

    fn set_button(state: &mut SteamControllerState, button: SteamButton) {
        state.buttons.0 |= 1_u32 << button as u8;
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1.0e-6, "{actual} != {expected}");
    }

    #[test]
    fn maps_every_documented_output_button() {
        for source in DIRECT_GAMEPAD_BUTTONS {
            let target = gamepad_button(source).expect("direct mapping table must stay complete");
            let mut input = source_state();
            set_button(&mut input, source);
            let output = ControllerMapper::default().map(&input, 0.004);
            assert!(output.buttons.contains(target), "{source:?} -> {target:?}");
        }
    }

    #[test]
    fn view_and_menu_follow_sdl_xbox_button_semantics() {
        let mut view = source_state();
        set_button(&mut view, SteamButton::View);
        let view_output = ControllerMapper::default().map(&view, 0.004);
        assert!(view_output.buttons.contains(Button::Start));
        assert!(!view_output.buttons.contains(Button::Back));

        let mut menu = source_state();
        set_button(&mut menu, SteamButton::Menu);
        let menu_output = ControllerMapper::default().map(&menu, 0.004);
        assert!(menu_output.buttons.contains(Button::Back));
        assert!(!menu_output.buttons.contains(Button::Start));
    }

    #[test]
    fn maps_all_hat_directions_and_cancels_opposites() {
        let cases = [
            (&[SteamButton::DpadUp][..], HatState::North),
            (
                &[SteamButton::DpadUp, SteamButton::DpadRight],
                HatState::NorthEast,
            ),
            (&[SteamButton::DpadRight], HatState::East),
            (
                &[SteamButton::DpadDown, SteamButton::DpadRight],
                HatState::SouthEast,
            ),
            (&[SteamButton::DpadDown], HatState::South),
            (
                &[SteamButton::DpadDown, SteamButton::DpadLeft],
                HatState::SouthWest,
            ),
            (&[SteamButton::DpadLeft], HatState::West),
            (
                &[SteamButton::DpadUp, SteamButton::DpadLeft],
                HatState::NorthWest,
            ),
        ];
        for (buttons, expected) in cases {
            let mut input = source_state();
            for button in buttons {
                set_button(&mut input, *button);
            }
            assert_eq!(ControllerMapper::default().map(&input, 0.004).hat, expected);
        }
        let mut input = source_state();
        set_button(&mut input, SteamButton::DpadUp);
        set_button(&mut input, SteamButton::DpadDown);
        assert_eq!(
            ControllerMapper::default().map(&input, 0.004).hat,
            HatState::Centered
        );
    }

    #[test]
    fn normalizes_sticks_and_observed_trigger_scale() {
        let mut input = source_state();
        input.left_stick_x = -32767;
        input.left_stick_y = 32767;
        input.right_stick_x = 32767;
        input.right_stick_y = -32767;
        input.left_trigger = 0;
        input.right_trigger = 0x8000;
        let config = MapperConfig {
            left_stick_dead_zone: 0.0,
            right_axis_dead_zone: 0.0,
            trigger_dead_zone: 0.0,
            ..MapperConfig::default()
        };
        let output = ControllerMapper::new(config).unwrap().map(&input, 0.004);
        assert!((output.left_x + std::f32::consts::FRAC_1_SQRT_2).abs() < 1.0e-6);
        assert!((output.left_y - std::f32::consts::FRAC_1_SQRT_2).abs() < 1.0e-6);
        assert!((output.right_x - std::f32::consts::FRAC_1_SQRT_2).abs() < 1.0e-6);
        assert!((output.right_y + std::f32::consts::FRAC_1_SQRT_2).abs() < 1.0e-6);
        assert_eq!((output.left_trigger, output.right_trigger), (0.0, 1.0));
    }

    #[test]
    fn neutral_input_remains_neutral() {
        let output = ControllerMapper::default().map(&source_state(), 0.004);
        assert_eq!(output, GamepadState::neutral());
    }

    #[test]
    fn physical_right_stick_is_default_and_right_pad_is_optional() {
        let mut input = source_state();
        input.right_stick_x = 32767;
        input.right_pad_touched = true;
        input.right_pad_x = -32767;
        input.right_pad_y = 0;
        set_button(&mut input, SteamButton::RightPadClick);
        let mut mapper = ControllerMapper::default();
        let default_output = mapper.map(&input, 0.004);
        assert_close(default_output.right_x, 1.0);
        assert!(!default_output.buttons.contains(Button::RightStick));

        set_button(&mut input, SteamButton::RightStickPress);
        assert!(mapper
            .map(&input, 0.004)
            .buttons
            .contains(Button::RightStick));

        let config = MapperConfig {
            right_axis_source: RightAxisSource::RightPad,
            right_axis_dead_zone: 0.0,
            ..MapperConfig::default()
        };
        let pad_output = ControllerMapper::new(config).unwrap().map(&input, 0.004);
        assert_close(pad_output.right_x, -1.0);
        assert!(pad_output.buttons.contains(Button::RightStick));
    }

    #[test]
    fn dead_zones_rescale_smoothly_and_keep_full_range() {
        assert_close(axis_dead_zone(0.1, 0.2), 0.0);
        assert!((axis_dead_zone(0.6, 0.2) - 0.5).abs() < 1.0e-6);
        assert_close(axis_dead_zone(1.0, 0.2), 1.0);
        assert_eq!(radial_dead_zone(0.1, 0.1, 0.2), (0.0, 0.0));
        let (x, y) = radial_dead_zone(1.0, 0.0, 0.2);
        assert_eq!((x, y), (1.0, 0.0));
        assert_close(trigger_dead_zone(0.1, 0.2), 0.0);
        assert_close(trigger_dead_zone(1.0, 0.2), 1.0);
    }

    #[test]
    fn inversion_sensitivity_saturation_and_clamp_are_finite() {
        let mut state = GamepadState {
            left_x: 0.5,
            left_y: f32::NAN,
            right_x: 0.5,
            right_y: -0.5,
            left_trigger: f32::INFINITY,
            right_trigger: 0.5,
            ..GamepadState::neutral()
        };
        AxisInversionFilter {
            left_x: true,
            right_y: true,
            ..AxisInversionFilter::default()
        }
        .apply(&mut state, 0.0);
        SensitivityFilter { exponent: 2.0 }.apply(&mut state, 0.0);
        SaturationFilter {
            sticks: 0.5,
            triggers: 0.5,
        }
        .apply(&mut state, 0.0);
        ClampFilter.apply(&mut state, 0.0);
        assert_close(state.left_x, -0.5);
        assert_close(state.left_y, 0.0);
        assert_close(state.right_y, 0.5);
        assert_close(state.left_trigger, 0.0);
        assert_close(state.right_trigger, 1.0);
        assert!(state.validate().is_ok());
    }

    #[test]
    fn mapper_applies_profile_axis_inversion() {
        let mut input = source_state();
        input.left_stick_x = 32767;
        input.right_stick_y = -32767;
        let config = MapperConfig {
            left_stick_dead_zone: 0.0,
            right_axis_dead_zone: 0.0,
            axis_inversion: AxisInversionFilter {
                left_x: true,
                right_y: true,
                ..AxisInversionFilter::NONE
            },
            ..MapperConfig::default()
        };
        let output = ControllerMapper::new(config).unwrap().map(&input, 0.004);
        assert_close(output.left_x, -1.0);
        assert_close(output.right_y, 1.0);
    }

    #[test]
    fn smoothing_converges_and_reset_returns_history_to_neutral() {
        let mut filter = LowPassFilter::new(Some(0.1));
        let target = GamepadState {
            left_x: 1.0,
            ..GamepadState::neutral()
        };
        let mut state = target;
        filter.apply(&mut state, 0.1);
        assert!(state.left_x > 0.0 && state.left_x < 1.0);
        for _ in 0..100 {
            state = target;
            filter.apply(&mut state, 0.1);
        }
        assert!((state.left_x - 1.0).abs() < 1.0e-5);
        filter.reset();
        state = target;
        filter.apply(&mut state, 0.0);
        assert_close(state.left_x, 0.0);
    }

    #[test]
    fn smoothing_does_not_delay_discrete_controls() {
        let mut input = source_state();
        set_button(&mut input, SteamButton::A);
        set_button(&mut input, SteamButton::DpadUp);
        let config = MapperConfig {
            smoothing_time_constant: Some(0.1),
            ..MapperConfig::default()
        };
        let output = ControllerMapper::new(config).unwrap().map(&input, 0.001);
        assert!(output.buttons.contains(Button::South));
        assert_eq!(output.hat, HatState::North);
    }

    #[test]
    fn mapper_reset_clears_smoothing_history_after_disconnect() {
        let mut input = source_state();
        input.left_stick_x = 32767;
        let config = MapperConfig {
            left_stick_dead_zone: 0.0,
            smoothing_time_constant: Some(0.1),
            ..MapperConfig::default()
        };
        let mut mapper = ControllerMapper::new(config).unwrap();
        assert!(mapper.map(&input, 0.1).left_x > 0.0);
        mapper.reset();
        assert_close(mapper.map(&input, 0.0).left_x, 0.0);
    }

    #[test]
    fn invalid_profiles_are_rejected() {
        for config in [
            MapperConfig {
                left_stick_dead_zone: f32::NAN,
                ..MapperConfig::default()
            },
            MapperConfig {
                right_axis_dead_zone: 1.1,
                ..MapperConfig::default()
            },
            MapperConfig {
                stick_sensitivity: 0.0,
                ..MapperConfig::default()
            },
            MapperConfig {
                smoothing_time_constant: Some(-1.0),
                ..MapperConfig::default()
            },
            MapperConfig {
                trigger_full_scale: 0,
                ..MapperConfig::default()
            },
        ] {
            assert!(ControllerMapper::new(config).is_err());
        }
    }
}
