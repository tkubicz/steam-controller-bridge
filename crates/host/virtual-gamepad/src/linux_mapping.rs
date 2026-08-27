use gamepad_state::{GamepadButtons, GamepadState, HatState};

use crate::{state_encoding, VirtualGamepadError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LinuxGamepadState {
    pub(crate) buttons: GamepadButtons,
    pub(crate) left_x: i32,
    pub(crate) left_y: i32,
    pub(crate) right_x: i32,
    pub(crate) right_y: i32,
    pub(crate) left_trigger: i32,
    pub(crate) right_trigger: i32,
    pub(crate) hat_x: i32,
    pub(crate) hat_y: i32,
}

pub(crate) fn encode(state: &GamepadState) -> Result<LinuxGamepadState, VirtualGamepadError> {
    state_encoding::validate(state)?;
    let (hat_x, hat_y) = hat_axes(state.hat);
    Ok(LinuxGamepadState {
        buttons: state.buttons,
        left_x: i32::from(state_encoding::stick(state.left_x)),
        left_y: i32::from(state_encoding::stick(-state.left_y)),
        right_x: i32::from(state_encoding::stick(state.right_x)),
        right_y: i32::from(state_encoding::stick(-state.right_y)),
        left_trigger: i32::from(state_encoding::trigger(state.left_trigger)),
        right_trigger: i32::from(state_encoding::trigger(state.right_trigger)),
        hat_x,
        hat_y,
    })
}

const fn hat_axes(hat: HatState) -> (i32, i32) {
    match hat {
        HatState::North => (0, -1),
        HatState::NorthEast => (1, -1),
        HatState::East => (1, 0),
        HatState::SouthEast => (1, 1),
        HatState::South => (0, 1),
        HatState::SouthWest => (-1, 1),
        HatState::West => (-1, 0),
        HatState::NorthWest => (-1, -1),
        HatState::Centered => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use gamepad_state::{Button, GamepadButtons};

    use super::*;

    #[test]
    fn maps_standard_xbox_controls_and_linux_axis_directions() {
        let mut buttons = GamepadButtons::default();
        for button in [
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
        ] {
            buttons.set(button, true);
        }
        for button in [
            Button::LeftGrip,
            Button::RightGrip,
            Button::Extra1,
            Button::Extra2,
            Button::Extra3,
        ] {
            buttons.set(button, true);
        }
        let encoded = encode(&GamepadState {
            buttons,
            hat: HatState::NorthEast,
            left_x: -1.0,
            left_y: 1.0,
            right_x: 1.0,
            right_y: -1.0,
            left_trigger: 0.5,
            right_trigger: 1.0,
        })
        .unwrap();

        assert_eq!(encoded.buttons, buttons);
        assert_eq!(encoded.left_x, -32_767);
        assert_eq!(encoded.left_y, -32_767);
        assert_eq!(encoded.right_x, 32_767);
        assert_eq!(encoded.right_y, 32_767);
        assert_eq!(encoded.left_trigger, 128);
        assert_eq!(encoded.right_trigger, 255);
        assert_eq!((encoded.hat_x, encoded.hat_y), (1, -1));
    }

    #[test]
    fn maps_every_hat_direction() {
        let cases = [
            (HatState::North, (0, -1)),
            (HatState::NorthEast, (1, -1)),
            (HatState::East, (1, 0)),
            (HatState::SouthEast, (1, 1)),
            (HatState::South, (0, 1)),
            (HatState::SouthWest, (-1, 1)),
            (HatState::West, (-1, 0)),
            (HatState::NorthWest, (-1, -1)),
            (HatState::Centered, (0, 0)),
        ];
        for (hat, expected) in cases {
            let encoded = encode(&GamepadState {
                hat,
                ..GamepadState::neutral()
            })
            .unwrap();
            assert_eq!((encoded.hat_x, encoded.hat_y), expected, "{hat:?}");
        }
    }

    #[test]
    fn rejects_invalid_state_before_encoding() {
        let error = encode(&GamepadState {
            left_x: f32::NAN,
            ..GamepadState::neutral()
        })
        .unwrap_err();
        assert_eq!(
            error.class(),
            crate::VirtualGamepadErrorClass::InvalidConfiguration
        );
    }

    #[test]
    fn rejects_buttons_outside_the_bridge_contract() {
        let error = encode(&GamepadState {
            buttons: GamepadButtons(1 << 16),
            ..GamepadState::neutral()
        })
        .unwrap_err();
        assert_eq!(
            error.class(),
            crate::VirtualGamepadErrorClass::InvalidConfiguration
        );
    }
}
