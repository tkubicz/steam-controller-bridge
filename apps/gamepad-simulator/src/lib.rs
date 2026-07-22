use gamepad_state::{Button, GamepadState, HatState};

/// One deterministic pass through buttons, sticks, triggers, hats, then neutral.
#[must_use]
pub fn automated_sequence(steps_per_axis: u16) -> Vec<GamepadState> {
    let steps = steps_per_axis.max(2);
    let mut states = Vec::new();
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
        Button::LeftGrip,
        Button::RightGrip,
        Button::Extra1,
        Button::Extra2,
        Button::Extra3,
    ] {
        let mut state = GamepadState::neutral();
        state.buttons.set(button, true);
        states.push(state);
    }
    for index in 0..steps {
        let angle = f32::from(index) * std::f32::consts::TAU / f32::from(steps);
        let (sin, cos) = angle.sin_cos();
        states.push(GamepadState {
            left_x: cos,
            left_y: sin,
            right_x: -sin,
            right_y: cos,
            ..GamepadState::neutral()
        });
    }
    for index in 0..steps {
        let value = f32::from(index) / f32::from(steps - 1);
        states.push(GamepadState {
            left_trigger: value,
            right_trigger: 1.0 - value,
            ..GamepadState::neutral()
        });
    }
    for hat in [
        HatState::North,
        HatState::NorthEast,
        HatState::East,
        HatState::SouthEast,
        HatState::South,
        HatState::SouthWest,
        HatState::West,
        HatState::NorthWest,
        HatState::Centered,
    ] {
        states.push(GamepadState {
            hat,
            ..GamepadState::neutral()
        });
    }
    states.push(GamepadState::neutral());
    states
}

/// Applies one line-oriented keyboard command to a persistent state.
///
/// # Errors
///
/// Returns an error when `command` is not one of the documented control names.
pub fn apply_keyboard_command(state: &mut GamepadState, command: &str) -> Result<bool, String> {
    let command = command.trim().to_ascii_lowercase();
    if command == "esc" || command == "exit" {
        return Ok(false);
    }
    if command == "r" || command == "reset" {
        *state = GamepadState::neutral();
        return Ok(true);
    }
    *state = GamepadState::neutral();
    match command.as_str() {
        "w" => state.left_y = 1.0,
        "a" => state.left_x = -1.0,
        "s" => state.left_y = -1.0,
        "d" => state.left_x = 1.0,
        "up" => state.right_y = 1.0,
        "left" => state.right_x = -1.0,
        "down" => state.right_y = -1.0,
        "right" => state.right_x = 1.0,
        "q" => state.left_trigger = 1.0,
        "e" => state.right_trigger = 1.0,
        "i" => state.hat = HatState::North,
        "j" => state.hat = HatState::West,
        "k" => state.hat = HatState::South,
        "l" => state.hat = HatState::East,
        "space" => state.buttons.set(Button::South, true),
        "1" => state.buttons.set(Button::East, true),
        "2" => state.buttons.set(Button::West, true),
        "3" => state.buttons.set(Button::North, true),
        "4" => state.buttons.set(Button::LeftShoulder, true),
        "5" => state.buttons.set(Button::RightShoulder, true),
        "6" => state.buttons.set(Button::Back, true),
        "7" => state.buttons.set(Button::Start, true),
        "8" => state.buttons.set(Button::Guide, true),
        "9" => state.buttons.set(Button::LeftGrip, true),
        "" => {}
        other => return Err(format!("unknown command: {other}")),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automated_sequence_exercises_every_category_and_finishes_neutral() {
        let states = automated_sequence(8);
        for (index, state) in states.iter().take(16).enumerate() {
            assert_eq!(state.buttons.0, 1 << index);
        }
        assert!(states.iter().any(|state| state.left_x.abs() > 0.9));
        assert!(states.iter().any(|state| state.right_y.abs() > 0.9));
        assert!(states
            .iter()
            .any(|state| (state.left_trigger - 1.0).abs() < f32::EPSILON));
        for hat in 0..=8 {
            assert!(states.iter().any(|state| state.hat as u8 == hat));
        }
        assert_eq!(states.last(), Some(&GamepadState::neutral()));
        assert!(states.iter().all(|state| state.validate().is_ok()));
    }

    #[test]
    fn keyboard_commands_map_and_reset() {
        let mut state = GamepadState::neutral();
        assert!(apply_keyboard_command(&mut state, "w").unwrap());
        assert!((state.left_y - 1.0).abs() < f32::EPSILON);
        apply_keyboard_command(&mut state, "space").unwrap();
        assert!(state.buttons.contains(Button::South));
        apply_keyboard_command(&mut state, "r").unwrap();
        assert_eq!(state, GamepadState::neutral());
        assert!(!apply_keyboard_command(&mut state, "esc").unwrap());
        assert!(apply_keyboard_command(&mut state, "nope").is_err());
    }
}
