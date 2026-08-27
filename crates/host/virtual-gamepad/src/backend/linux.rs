use std::io;
use std::path::Path;

use bridge_output::{OutputDiagnostics, OutputFeedback};
use evdevil::event::{Abs, AbsEvent, InputEvent, Key, KeyEvent, KeyState};
use evdevil::uinput::{AbsSetup, UinputDevice};
use evdevil::{AbsInfo, Bus, InputId};
use gamepad_state::{Button, GamepadState};

use super::Backend;
use crate::contract::{DEFAULT_PRODUCT_ID, DEFAULT_VENDOR_ID};
use crate::linux_mapping::{self, LinuxGamepadState};
use crate::{VirtualGamepadError, VirtualGamepadErrorClass, VIRTUAL_GAMEPAD_NAME};

const UINPUT_PATH: &str = "/dev/uinput";
const DEVICE_ID: InputId = InputId::new(Bus::USB, DEFAULT_VENDOR_ID, DEFAULT_PRODUCT_ID, 0x0114);
const BUTTON_KEYS: [(Button, Key); 11] = [
    (Button::South, Key::BTN_SOUTH),
    (Button::East, Key::BTN_EAST),
    (Button::West, Key::BTN_WEST),
    (Button::North, Key::BTN_NORTH),
    (Button::LeftShoulder, Key::BTN_TL),
    (Button::RightShoulder, Key::BTN_TR),
    (Button::LeftStick, Key::BTN_THUMBL),
    (Button::RightStick, Key::BTN_THUMBR),
    (Button::Back, Key::BTN_SELECT),
    (Button::Start, Key::BTN_START),
    (Button::Guide, Key::BTN_MODE),
];
pub struct LinuxUinputOutput {
    device: Option<UinputDevice>,
    previous: Option<LinuxGamepadState>,
    events: Vec<InputEvent>,
    diagnostics: OutputDiagnostics,
}

const _: fn(Option<&Path>) -> Result<LinuxUinputOutput, VirtualGamepadError> =
    LinuxUinputOutput::open;

impl LinuxUinputOutput {
    pub fn open(device_path: Option<&Path>) -> Result<Self, VirtualGamepadError> {
        validate_device_path(device_path)?;
        let device = build_device()?;
        device.set_nonblocking(true).map_err(|error| {
            initialization_error("failed to make the uinput device nonblocking", &error)
        })?;
        let mut output = Self {
            device: Some(device),
            previous: None,
            events: Vec::with_capacity(BUTTON_KEYS.len() + 8),
            diagnostics: OutputDiagnostics::default(),
        };
        output.send_neutral()?;
        Ok(output)
    }

    fn write_state(&mut self, state: LinuxGamepadState) -> Result<(), VirtualGamepadError> {
        if self.previous == Some(state) {
            self.diagnostics.virtual_reports_coalesced += 1;
            return Ok(());
        }
        self.events.clear();
        append_button_events(
            self.previous.map(|previous| previous.buttons),
            state.buttons,
            &mut self.events,
        );
        for (axis, value, changed) in [
            (
                Abs::X,
                state.left_x,
                self.previous.is_none_or(|p| p.left_x != state.left_x),
            ),
            (
                Abs::Y,
                state.left_y,
                self.previous.is_none_or(|p| p.left_y != state.left_y),
            ),
            (
                Abs::RX,
                state.right_x,
                self.previous.is_none_or(|p| p.right_x != state.right_x),
            ),
            (
                Abs::RY,
                state.right_y,
                self.previous.is_none_or(|p| p.right_y != state.right_y),
            ),
            (
                Abs::Z,
                state.left_trigger,
                self.previous
                    .is_none_or(|p| p.left_trigger != state.left_trigger),
            ),
            (
                Abs::RZ,
                state.right_trigger,
                self.previous
                    .is_none_or(|p| p.right_trigger != state.right_trigger),
            ),
            (
                Abs::HAT0X,
                state.hat_x,
                self.previous.is_none_or(|p| p.hat_x != state.hat_x),
            ),
            (
                Abs::HAT0Y,
                state.hat_y,
                self.previous.is_none_or(|p| p.hat_y != state.hat_y),
            ),
        ] {
            if changed {
                self.events
                    .push(InputEvent::from(AbsEvent::new(axis, value)));
            }
        }
        self.device()?
            .write_events(&self.events)
            .map_err(|error| runtime_error("failed to write a uinput state batch", &error))?;
        self.previous = Some(state);
        self.diagnostics.virtual_reports_dispatched += 1;
        Ok(())
    }

    fn device(&self) -> Result<&UinputDevice, VirtualGamepadError> {
        self.device.as_ref().ok_or_else(|| {
            let error = io::Error::new(io::ErrorKind::BrokenPipe, "uinput handle is closed");
            runtime_error("the Linux virtual gamepad has already shut down", &error)
        })
    }
}

impl Backend for LinuxUinputOutput {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), VirtualGamepadError> {
        let state = linux_mapping::encode(state)?;
        self.write_state(state)
    }

    fn send_neutral(&mut self) -> Result<(), VirtualGamepadError> {
        self.send_state(&GamepadState::neutral())
    }

    fn service(&mut self) -> Result<(), VirtualGamepadError> {
        Ok(())
    }

    fn take_feedback(&mut self) -> Option<OutputFeedback> {
        None
    }

    fn diagnostics(&self) -> OutputDiagnostics {
        self.diagnostics
    }

    fn shutdown(&mut self) -> Result<(), VirtualGamepadError> {
        if self.device.is_none() {
            return Ok(());
        }
        let neutral_result = self.send_neutral();
        self.device.take();
        neutral_result
    }
}

impl Drop for LinuxUinputOutput {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn build_device() -> Result<UinputDevice, VirtualGamepadError> {
    let stick = AbsInfo::new(-32_768, 32_767);
    let trigger = AbsInfo::new(0, 255);
    let hat = AbsInfo::new(-1, 1);
    UinputDevice::builder()
        .map_err(|error| open_error(&error))?
        .with_input_id(DEVICE_ID)
        .and_then(|builder| builder.with_keys(BUTTON_KEYS.map(|(_, key)| key)))
        .and_then(|builder| {
            builder.with_abs_axes([
                AbsSetup::new(Abs::X, stick),
                AbsSetup::new(Abs::Y, stick),
                AbsSetup::new(Abs::RX, stick),
                AbsSetup::new(Abs::RY, stick),
                AbsSetup::new(Abs::Z, trigger),
                AbsSetup::new(Abs::RZ, trigger),
                AbsSetup::new(Abs::HAT0X, hat),
                AbsSetup::new(Abs::HAT0Y, hat),
            ])
        })
        .and_then(|builder| builder.build(VIRTUAL_GAMEPAD_NAME))
        .map_err(|error| initialization_error("failed to create the Linux virtual gamepad", &error))
}

fn append_button_events(
    previous: Option<gamepad_state::GamepadButtons>,
    buttons: gamepad_state::GamepadButtons,
    events: &mut Vec<InputEvent>,
) {
    for (button, key) in BUTTON_KEYS {
        let pressed = buttons.contains(button);
        if previous.is_none_or(|previous| previous.contains(button) != pressed) {
            events.push(InputEvent::from(KeyEvent::new(
                key,
                if pressed {
                    KeyState::PRESSED
                } else {
                    KeyState::RELEASED
                },
            )));
        }
    }
}

fn validate_device_path(device_path: Option<&Path>) -> Result<(), VirtualGamepadError> {
    if device_path.is_none_or(|path| path == Path::new(UINPUT_PATH)) {
        return Ok(());
    }
    Err(VirtualGamepadError::new(
        VirtualGamepadErrorClass::InvalidConfiguration,
        format!(
            "unsupported Linux uinput device path {}; only {UINPUT_PATH} is supported",
            device_path
                .unwrap_or_else(|| Path::new(UINPUT_PATH))
                .display()
        ),
    ))
}

fn open_error(error: &io::Error) -> VirtualGamepadError {
    let class = match error.kind() {
        io::ErrorKind::NotFound => VirtualGamepadErrorClass::DriverMissing,
        io::ErrorKind::PermissionDenied => VirtualGamepadErrorClass::PermissionDenied,
        _ => VirtualGamepadErrorClass::BackendUnavailable,
    };
    VirtualGamepadError::new(
        class,
        format!("cannot open {UINPUT_PATH} for read and write: {error}"),
    )
}

fn initialization_error(context: &str, error: &io::Error) -> VirtualGamepadError {
    let class = match error.kind() {
        io::ErrorKind::NotFound => VirtualGamepadErrorClass::DriverMissing,
        io::ErrorKind::PermissionDenied => VirtualGamepadErrorClass::PermissionDenied,
        _ => VirtualGamepadErrorClass::BackendUnavailable,
    };
    VirtualGamepadError::new(class, format!("{context}: {error}"))
}

fn runtime_error(context: &str, error: &io::Error) -> VirtualGamepadError {
    VirtualGamepadError::new(
        VirtualGamepadErrorClass::DispatchFailed,
        format!("{context}: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use evdevil::event::EventKind;
    use gamepad_state::GamepadButtons;

    use super::*;

    #[test]
    fn accepts_only_the_kernel_uinput_path() {
        validate_device_path(None).unwrap();
        validate_device_path(Some(Path::new(UINPUT_PATH))).unwrap();
        let error = validate_device_path(Some(Path::new("/tmp/uinput"))).unwrap_err();
        assert_eq!(
            error.class(),
            VirtualGamepadErrorClass::InvalidConfiguration
        );
    }

    #[test]
    fn classifies_open_failures_for_runtime_policy() {
        assert_eq!(
            open_error(&io::Error::from(io::ErrorKind::NotFound)).class(),
            VirtualGamepadErrorClass::DriverMissing
        );
        assert_eq!(
            open_error(&io::Error::from(io::ErrorKind::PermissionDenied)).class(),
            VirtualGamepadErrorClass::PermissionDenied
        );
        assert_eq!(
            open_error(&io::Error::from(io::ErrorKind::Other)).class(),
            VirtualGamepadErrorClass::BackendUnavailable
        );
    }

    #[test]
    fn classifies_permission_denied_during_setup() {
        let error = initialization_error(
            "setup failed",
            &io::Error::from(io::ErrorKind::PermissionDenied),
        );
        assert_eq!(error.class(), VirtualGamepadErrorClass::PermissionDenied);
    }

    #[test]
    fn maps_each_standard_button_to_its_linux_key() {
        for (button, expected_key) in BUTTON_KEYS {
            let mut buttons = GamepadButtons::default();
            buttons.set(button, true);
            let mut events = Vec::new();
            append_button_events(Some(GamepadButtons::default()), buttons, &mut events);
            assert_eq!(events.len(), 1, "{button:?}");
            let EventKind::Key(event) = events[0].kind() else {
                panic!("{button:?} did not produce a key event")
            };
            assert_eq!(event.key(), expected_key, "{button:?}");
            assert_eq!(event.state(), KeyState::PRESSED, "{button:?}");
        }
    }

    #[test]
    fn omits_extension_buttons() {
        let mut buttons = GamepadButtons::default();
        for button in [
            Button::LeftGrip,
            Button::RightGrip,
            Button::Extra1,
            Button::Extra2,
            Button::Extra3,
        ] {
            buttons.set(button, true);
        }
        let mut events = Vec::new();
        append_button_events(Some(GamepadButtons::default()), buttons, &mut events);
        assert!(events.is_empty());
    }
}
