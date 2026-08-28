use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use bridge_output::{OutputDiagnostics, OutputFeedback};
use evdevil::event::{
    Abs, AbsEvent, EventKind, ForceFeedbackCode, InputEvent, Key, KeyEvent, KeyState, UinputCode,
};
use evdevil::ff::{self, EffectKind};
use evdevil::uinput::{AbsSetup, UinputDevice};
use evdevil::{AbsInfo, Bus, InputId};
use gamepad_state::{Button, GamepadState};

use super::linux_feedback::{RumbleEffects, RumbleParameters, MAX_EFFECTS};
use super::linux_mapping::{self, LinuxGamepadState};
use super::Backend;
use crate::contract::{DEFAULT_PRODUCT_ID, DEFAULT_VENDOR_ID};
use crate::{VirtualGamepadError, VirtualGamepadErrorClass, VIRTUAL_GAMEPAD_NAME};

const UINPUT_PATH: &str = "/dev/uinput";
const MAX_SERVICE_EVENTS: usize = 64;
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
    service_events: [InputEvent; MAX_SERVICE_EVENTS],
    rumble: RumbleEffects,
    diagnostics: OutputDiagnostics,
}

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
            service_events: [InputEvent::zeroed(); MAX_SERVICE_EVENTS],
            rumble: RumbleEffects::default(),
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
        append_state_events(self.previous, state, &mut self.events);
        if self.events.is_empty() {
            self.previous = Some(state);
            self.diagnostics.virtual_reports_coalesced += 1;
            return Ok(());
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

fn append_state_events(
    previous: Option<LinuxGamepadState>,
    state: LinuxGamepadState,
    events: &mut Vec<InputEvent>,
) {
    append_button_events(
        previous.map(|previous| previous.buttons),
        state.buttons,
        events,
    );
    for (axis, value, changed) in [
        (
            Abs::X,
            state.left_x,
            previous.is_none_or(|p| p.left_x != state.left_x),
        ),
        (
            Abs::Y,
            state.left_y,
            previous.is_none_or(|p| p.left_y != state.left_y),
        ),
        (
            Abs::RX,
            state.right_x,
            previous.is_none_or(|p| p.right_x != state.right_x),
        ),
        (
            Abs::RY,
            state.right_y,
            previous.is_none_or(|p| p.right_y != state.right_y),
        ),
        (
            Abs::Z,
            state.left_trigger,
            previous.is_none_or(|p| p.left_trigger != state.left_trigger),
        ),
        (
            Abs::RZ,
            state.right_trigger,
            previous.is_none_or(|p| p.right_trigger != state.right_trigger),
        ),
        (
            Abs::HAT0X,
            state.hat_x,
            previous.is_none_or(|p| p.hat_x != state.hat_x),
        ),
        (
            Abs::HAT0Y,
            state.hat_y,
            previous.is_none_or(|p| p.hat_y != state.hat_y),
        ),
    ] {
        if changed {
            events.push(InputEvent::from(AbsEvent::new(axis, value)));
        }
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
        let Some(device) = self.device.as_ref() else {
            self.rumble.clear();
            return Err(VirtualGamepadError::new(
                VirtualGamepadErrorClass::DispatchFailed,
                "the Linux virtual gamepad has already shut down",
            ));
        };
        let count = match device.read_events(&mut self.service_events) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => 0,
            Err(error) => {
                self.rumble.clear();
                return Err(runtime_error(
                    "failed to read Linux force-feedback events",
                    &error,
                ));
            }
        };
        for event in self.service_events[..count].iter().copied() {
            if let Err(error) = process_event(device, &mut self.rumble, event) {
                self.rumble.clear();
                return Err(runtime_error(
                    "failed to service Linux force feedback",
                    &error,
                ));
            }
        }
        self.rumble.refresh(Instant::now());
        Ok(())
    }

    fn take_feedback(&mut self) -> Option<OutputFeedback> {
        self.rumble.take_feedback()
    }

    fn diagnostics(&self) -> OutputDiagnostics {
        self.diagnostics
    }

    fn shutdown(&mut self) -> Result<(), VirtualGamepadError> {
        if self.device.is_none() {
            return Ok(());
        }
        let neutral_result = self.send_neutral();
        self.rumble.clear();
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
        .map_err(|error| initialization_error("failed to initialize Linux uinput", &error))?
        .with_input_id(DEVICE_ID)
        .and_then(|builder| builder.with_keys(BUTTON_KEYS.map(|(_, key)| key)))
        .and_then(|builder| builder.with_ff_features([ff::Feature::RUMBLE]))
        .and_then(|builder| builder.with_ff_effects_max(u32::from(MAX_EFFECTS)))
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

fn process_event(
    device: &UinputDevice,
    rumble: &mut RumbleEffects,
    event: InputEvent,
) -> io::Result<()> {
    match event.kind() {
        EventKind::Uinput(event) => match event.code() {
            UinputCode::FF_UPLOAD => handle_upload(device, rumble, &event),
            UinputCode::FF_ERASE => handle_erase(device, rumble, &event),
            _ => Ok(()),
        },
        EventKind::ForceFeedback(event) => {
            if let ForceFeedbackCode::ControlEffect(id) = event.code() {
                let count = u32::try_from(event.raw_value()).unwrap_or(0);
                let _ = rumble.control(id.raw(), count, Instant::now());
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn handle_upload(
    device: &UinputDevice,
    rumble: &mut RumbleEffects,
    event: &evdevil::event::UinputEvent,
) -> io::Result<()> {
    let result = device.ff_upload(event, |upload| {
        let parameters = effect_parameters(upload.effect())?;
        rumble
            .upload(upload.effect_id().raw(), parameters, Instant::now())
            .map_err(invalid_effect_request)
    });
    match result {
        Err(error) if is_effect_rejection(&error) => Ok(()),
        result => result,
    }
}

fn effect_parameters(effect: &ff::Effect<'_>) -> io::Result<RumbleParameters> {
    let EffectKind::Rumble(rumble) = effect.kind() else {
        return Err(invalid_effect_request(
            "only FF_RUMBLE effects are supported",
        ));
    };
    let replay = effect.replay();
    Ok(RumbleParameters {
        strong: rumble.strong_magnitude(),
        weak: rumble.weak_magnitude(),
        delay: Duration::from_millis(u64::from(replay.delay())),
        length: Duration::from_millis(u64::from(replay.length())),
    })
}

fn handle_erase(
    device: &UinputDevice,
    rumble: &mut RumbleEffects,
    event: &evdevil::event::UinputEvent,
) -> io::Result<()> {
    device.ff_erase(event, |erase| {
        rumble
            .erase(erase.effect_id().raw())
            .map_err(invalid_effect_request)
    })
}

fn invalid_effect_request(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        EffectRequestRejected(error.to_string()),
    )
}

fn is_effect_rejection(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|source| source.downcast_ref::<EffectRequestRejected>().is_some())
}

#[derive(Debug)]
struct EffectRequestRejected(String);

impl std::fmt::Display for EffectRequestRejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for EffectRequestRejected {}

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

    fn axis_events(events: &[InputEvent]) -> Vec<(Abs, i32)> {
        events
            .iter()
            .filter_map(|event| match event.kind() {
                EventKind::Abs(event) => Some((event.abs(), event.value())),
                _ => None,
            })
            .collect()
    }

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
    fn classifies_initialization_failures_for_runtime_policy() {
        assert_eq!(
            initialization_error("setup failed", &io::Error::from(io::ErrorKind::NotFound)).class(),
            VirtualGamepadErrorClass::DriverMissing
        );
        assert_eq!(
            initialization_error(
                "setup failed",
                &io::Error::from(io::ErrorKind::PermissionDenied)
            )
            .class(),
            VirtualGamepadErrorClass::PermissionDenied
        );
        assert_eq!(
            initialization_error("setup failed", &io::Error::from(io::ErrorKind::Other)).class(),
            VirtualGamepadErrorClass::BackendUnavailable
        );
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

    #[test]
    fn maps_and_diffs_every_linux_axis() {
        let previous = LinuxGamepadState {
            buttons: GamepadButtons::default(),
            left_x: 1,
            left_y: 2,
            right_x: 3,
            right_y: 4,
            left_trigger: 5,
            right_trigger: 6,
            hat_x: 0,
            hat_y: 0,
        };
        let mut events = Vec::new();
        append_state_events(None, previous, &mut events);
        assert_eq!(
            axis_events(&events),
            [
                (Abs::X, 1),
                (Abs::Y, 2),
                (Abs::RX, 3),
                (Abs::RY, 4),
                (Abs::Z, 5),
                (Abs::RZ, 6),
                (Abs::HAT0X, 0),
                (Abs::HAT0Y, 0),
            ]
        );

        for (expected, state) in [
            (
                (Abs::X, 10),
                LinuxGamepadState {
                    left_x: 10,
                    ..previous
                },
            ),
            (
                (Abs::Y, 20),
                LinuxGamepadState {
                    left_y: 20,
                    ..previous
                },
            ),
            (
                (Abs::RX, 30),
                LinuxGamepadState {
                    right_x: 30,
                    ..previous
                },
            ),
            (
                (Abs::RY, 40),
                LinuxGamepadState {
                    right_y: 40,
                    ..previous
                },
            ),
            (
                (Abs::Z, 50),
                LinuxGamepadState {
                    left_trigger: 50,
                    ..previous
                },
            ),
            (
                (Abs::RZ, 60),
                LinuxGamepadState {
                    right_trigger: 60,
                    ..previous
                },
            ),
            (
                (Abs::HAT0X, -1),
                LinuxGamepadState {
                    hat_x: -1,
                    ..previous
                },
            ),
            (
                (Abs::HAT0Y, -1),
                LinuxGamepadState {
                    hat_y: -1,
                    ..previous
                },
            ),
        ] {
            events.clear();
            append_state_events(Some(previous), state, &mut events);
            assert_eq!(axis_events(&events), [expected], "{expected:?}");
        }
    }

    #[test]
    fn unobservable_states_increment_the_coalesced_report_diagnostic() {
        let state = linux_mapping::encode(&GamepadState::neutral()).unwrap();
        let mut output = LinuxUinputOutput {
            device: None,
            previous: Some(state),
            events: Vec::new(),
            service_events: [InputEvent::zeroed(); MAX_SERVICE_EVENTS],
            rumble: RumbleEffects::default(),
            diagnostics: OutputDiagnostics::default(),
        };

        output.write_state(state).unwrap();
        let mut extension_only = state;
        extension_only.buttons.set(Button::LeftGrip, true);
        output.write_state(extension_only).unwrap();

        assert_eq!(output.diagnostics.virtual_reports_coalesced, 2);
        assert_eq!(output.diagnostics.virtual_reports_dispatched, 0);
        assert_eq!(output.previous, Some(extension_only));
    }

    #[test]
    fn accepts_rumble_parameters_and_rejects_other_effect_types() {
        let effect = ff::Effect::from(ff::Rumble::new(10, 20)).with_replay(ff::Replay::new(30, 40));
        assert_eq!(
            effect_parameters(&effect).unwrap(),
            RumbleParameters {
                strong: 10,
                weak: 20,
                delay: Duration::from_millis(40),
                length: Duration::from_millis(30),
            }
        );
        let unsupported = ff::Effect::from(ff::Constant::new(100));
        let error = effect_parameters(&unsupported).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(is_effect_rejection(&error));
        assert!(!is_effect_rejection(&io::Error::new(
            io::ErrorKind::InvalidInput,
            "uinput ioctl failed",
        )));
    }
}
