use gamepad_state::GamepadState;

use crate::{VirtualGamepadError, VirtualGamepadErrorClass};

pub(crate) fn validate(state: &GamepadState) -> Result<(), VirtualGamepadError> {
    state.validate().map_err(|error| {
        VirtualGamepadError::new(
            VirtualGamepadErrorClass::InvalidConfiguration,
            error.to_string(),
        )
    })?;
    if state.buttons.0 & !0xffff != 0 {
        return Err(VirtualGamepadError::new(
            VirtualGamepadErrorClass::InvalidConfiguration,
            "virtual gamepad input accepts only the bridge's 16 defined buttons",
        ));
    }
    Ok(())
}

#[allow(clippy::cast_possible_truncation)]
pub(crate) fn stick(value: f32) -> i16 {
    (value * 32_767.0).round() as i16
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn trigger(value: f32) -> u8 {
    (value * 255.0).round() as u8
}
