//! Deterministic states for visual checks, so QA does not depend on catching a
//! physical input at screenshot time.
//!
//! These drive the same `SteamControllerState -> ControlState` path as
//! hardware. They are not a second renderer and not a set of UI-only booleans.

use steam_controller_protocol::{
    AccelerationState, GyroState, SteamButton, SteamButtons, SteamControllerState,
};

use crate::hero::ALL_STEAM_BUTTONS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DemoState {
    Neutral,
    /// Every drawable digital control at once, plus the two grip-shell
    /// sensors that only appear in the button grid.
    Digital,
    /// Distinct stick and pad quadrants, touched pads, one half and one full
    /// trigger, and non-zero IMU axes.
    Analog,
    Disconnected,
}

impl DemoState {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "neutral" => Some(Self::Neutral),
            "digital" => Some(Self::Digital),
            "analog" => Some(Self::Analog),
            "disconnected" => Some(Self::Disconnected),
            _ => None,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Digital => "digital",
            Self::Analog => "analog",
            Self::Disconnected => "disconnected",
        }
    }

    /// The decoded state this mode presents, or `None` for disconnected.
    pub(crate) fn state(self) -> Option<SteamControllerState> {
        match self {
            Self::Disconnected => None,
            Self::Neutral => Some(base()),
            Self::Digital => Some(digital()),
            Self::Analog => Some(analog()),
        }
    }
}

fn base() -> SteamControllerState {
    SteamControllerState {
        report_id: 0x45,
        sequence: 42,
        buttons: SteamButtons(0),
        left_trigger: 0,
        right_trigger: 0,
        left_stick_x: 0,
        left_stick_y: 0,
        right_stick_x: 0,
        right_stick_y: 0,
        left_pad_x: 0,
        left_pad_y: 0,
        left_pad_pressure: 0,
        left_pad_touched: false,
        left_pad_pressed: false,
        right_pad_x: 0,
        right_pad_y: 0,
        right_pad_pressure: 0,
        right_pad_touched: false,
        right_pad_pressed: false,
        left_grip_touched: false,
        right_grip_touched: false,
        imu_timestamp: 0,
        gyro: None,
        acceleration: None,
        raw_report: vec![0x45, 42, 0, 0, 0, 0],
    }
}

fn digital() -> SteamControllerState {
    let mut state = base();
    // Everything at once, including the two bits with no drawn geometry, so a
    // capture proves the grid still carries them.
    let mut mask = 0u32;
    for button in ALL_STEAM_BUTTONS {
        mask |= 1 << (button as u8);
    }
    state.buttons = SteamButtons(mask);
    state.left_pad_touched = true;
    state.right_pad_touched = true;
    state.left_pad_pressed = true;
    state.right_pad_pressed = true;
    state.left_grip_touched = true;
    state.right_grip_touched = true;
    state
}

fn analog() -> SteamControllerState {
    let mut state = base();
    // Four different quadrants, so a mirrored or transposed axis is obvious.
    state.left_stick_x = -24_000;
    state.left_stick_y = 20_000;
    state.right_stick_x = 18_000;
    state.right_stick_y = -22_000;
    state.left_pad_x = 15_000;
    state.left_pad_y = 15_000;
    state.left_pad_touched = true;
    state.right_pad_x = -17_000;
    state.right_pad_y = -12_000;
    state.right_pad_touched = true;
    // One half pull and one full, so the two fill levels read differently.
    state.left_trigger = 0x4000;
    state.right_trigger = 0x8000;
    state.buttons = SteamButtons(
        (1 << (SteamButton::RightTriggerClick as u8))
            | (1 << (SteamButton::LeftPadTouch as u8))
            | (1 << (SteamButton::RightPadTouch as u8))
            | (1 << (SteamButton::LeftStickTouch as u8)),
    );
    state.gyro = Some(GyroState {
        x: 4_200,
        y: -1_800,
        z: 900,
    });
    state.acceleration = Some(AccelerationState {
        x: -3_100,
        y: 8_400,
        z: 2_050,
    });
    state
}

#[cfg(test)]
mod tests {
    use super::{DemoState, ALL_STEAM_BUTTONS};
    use crate::hero::{control_for_button, control_states};
    use controller_art::{Analog, Control, Highlight};

    #[test]
    fn every_mode_round_trips_through_its_name() {
        for mode in [
            DemoState::Neutral,
            DemoState::Digital,
            DemoState::Analog,
            DemoState::Disconnected,
        ] {
            assert_eq!(DemoState::parse(mode.label()), Some(mode));
        }
        assert_eq!(DemoState::parse("nonsense"), None);
    }

    #[test]
    fn disconnected_has_no_state_and_neutral_lights_nothing() {
        assert!(DemoState::Disconnected.state().is_none());
        let state = DemoState::Neutral.state().expect("neutral has a state");
        let paint = control_states(&state, state.buttons, 0x8000);
        assert!(Control::ALL
            .into_iter()
            .all(|control| paint(control).highlight == Highlight::Idle));
    }

    /// The whole point of the digital mode: one capture must exercise every
    /// drawable control.
    #[test]
    fn digital_activates_every_drawable_control() {
        let state = DemoState::Digital.state().expect("digital has a state");
        let paint = control_states(&state, state.buttons, 0x8000);
        for control in Control::ALL {
            assert_eq!(
                paint(control).highlight,
                Highlight::Active,
                "{} stayed idle in the digital demo",
                control.label()
            );
        }
        // And the two undrawable bits are still set, for the grid.
        for button in ALL_STEAM_BUTTONS {
            if control_for_button(button).is_none() {
                assert!(state.buttons.contains(button), "{button:?} must be held");
            }
        }
    }

    #[test]
    fn analog_puts_each_stick_and_pad_in_a_different_quadrant() {
        let state = DemoState::Analog.state().expect("analog has a state");
        let paint = control_states(&state, state.buttons, 0x8000);
        let quadrant = |control| match paint(control).analog {
            Some(Analog::Position {
                offset: Some([x, y]),
                ..
            }) => (x > 0.0, y > 0.0),
            _ => panic!("{control:?} should report a position"),
        };
        let quadrants = [
            quadrant(Control::LeftStick),
            quadrant(Control::RightStick),
            quadrant(Control::LeftPad),
            quadrant(Control::RightPad),
        ];
        for (index, first) in quadrants.iter().enumerate() {
            for second in &quadrants[index + 1..] {
                assert_ne!(first, second, "two controls share a quadrant");
            }
        }
    }

    #[test]
    fn analog_shows_one_half_and_one_full_trigger() {
        let state = DemoState::Analog.state().expect("analog has a state");
        let paint = control_states(&state, state.buttons, 0x8000);
        let travel = |control| match paint(control).analog {
            Some(Analog::Trigger { travel }) => travel,
            _ => panic!("{control:?} should report travel"),
        };
        assert!((travel(Control::LeftTrigger) - 0.5).abs() < 0.01);
        assert!((travel(Control::RightTrigger) - 1.0).abs() < 0.01);
    }

    #[test]
    fn analog_carries_non_zero_imu_axes() {
        let state = DemoState::Analog.state().expect("analog has a state");
        let gyro = state.gyro.expect("gyro present");
        let accel = state.acceleration.expect("acceleration present");
        assert!(gyro.x != 0 && gyro.y != 0 && gyro.z != 0);
        assert!(accel.x != 0 && accel.y != 0 && accel.z != 0);
    }
}
