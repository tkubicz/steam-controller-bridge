#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use bridge_output::{GamepadOutput, OutputFeedback};
use evdevil::event::{
    Abs, AbsEvent, EventKind, EventType, InputEvent, Key, KeyEvent, KeyState, Syn,
};
use evdevil::ff::{Effect, Feature, Rumble};
use evdevil::{Bus, Evdev};
use gamepad_state::{Button, GamepadButtons, GamepadState, HatState};
use virtual_gamepad::{
    VirtualGamepad, VirtualGamepadConfig, DEFAULT_PRODUCT_ID, DEFAULT_VENDOR_ID,
    LINUX_VIRTUAL_GAMEPAD_VERSION, VIRTUAL_GAMEPAD_NAME,
};

const WAIT_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const DISCOVERY_POLL_INTERVAL: Duration = Duration::from_millis(25);

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[test]
#[ignore = "requires ordinary-user read/write access to /dev/uinput and the resulting evdev node"]
fn linux_uinput_round_trip() -> TestResult {
    require_ordinary_user()?;
    let existing = matching_event_paths()?;
    let mut output = VirtualGamepad::open(&VirtualGamepadConfig::default())?;
    let (event_path, evdev) = wait_for_new_gamepad(&existing)?;
    evdev.set_nonblocking(true)?;

    assert_device_contract(&evdev)?;

    let state = test_state();
    output.send_state(&state)?;
    assert_eq!(read_report(&evdev)?, expected_active_report());

    let upload_device = evdev.try_clone()?;
    let effect_id = service_blocking_request(&mut output, move || {
        upload_device.upload_ff_effect(Effect::from(Rumble::new(30_000, 12_000)))
    })?;
    evdev.control_ff(effect_id, true)?;
    assert_eq!(
        wait_for_feedback(&mut output)?,
        OutputFeedback::Rumble {
            low_frequency: 30_000,
            high_frequency: 12_000,
        }
    );

    evdev.control_ff(effect_id, false)?;
    assert_eq!(
        wait_for_feedback(&mut output)?,
        OutputFeedback::Rumble {
            low_frequency: 0,
            high_frequency: 0,
        }
    );

    let erase_device = evdev.try_clone()?;
    service_blocking_request(&mut output, move || erase_device.erase_ff_effect(effect_id))?;

    output.send_neutral()?;
    assert_eq!(read_report(&evdev)?, expected_neutral_report());
    drop(evdev);
    output.shutdown()?;
    wait_for_removal(&event_path)?;
    Ok(())
}

fn require_ordinary_user() -> io::Result<()> {
    let status = fs::read_to_string("/proc/self/status")?;
    let effective_uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|uids| uids.split_whitespace().nth(1))
        .and_then(|uid| uid.parse::<u32>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cannot read effective UID"))?;
    if effective_uid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "run this acceptance test as the ordinary active desktop user, without sudo",
        ));
    }
    Ok(())
}

fn matching_event_paths() -> io::Result<BTreeSet<PathBuf>> {
    Ok(matching_gamepads()?
        .into_iter()
        .map(|(path, _)| path)
        .collect())
}

fn wait_for_new_gamepad(existing: &BTreeSet<PathBuf>) -> io::Result<(PathBuf, Evdev)> {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        let mut matches: Vec<_> = matching_gamepads()?
            .into_iter()
            .filter(|(path, _)| !existing.contains(path))
            .collect();
        match matches.len() {
            1 => return Ok(matches.pop().expect("length checked")),
            count if count > 1 => {
                return Err(io::Error::other(format!(
                    "found {count} new matching virtual gamepads"
                )));
            }
            _ if Instant::now() >= deadline => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the virtual gamepad event node did not appear",
                ));
            }
            _ => thread::sleep(DISCOVERY_POLL_INTERVAL),
        }
    }
}

fn matching_gamepads() -> io::Result<Vec<(PathBuf, Evdev)>> {
    Ok(evdevil::enumerate()?
        .filter_map(Result::ok)
        .filter(|(_, device)| is_virtual_gamepad(device))
        .collect())
}

fn is_virtual_gamepad(device: &Evdev) -> bool {
    device.name().is_ok_and(|name| name == VIRTUAL_GAMEPAD_NAME)
        && device.input_id().is_ok_and(|id| {
            id.bus() == Bus::USB
                && id.vendor() == DEFAULT_VENDOR_ID
                && id.product() == DEFAULT_PRODUCT_ID
                && id.version() == LINUX_VIRTUAL_GAMEPAD_VERSION
        })
}

fn assert_device_contract(device: &Evdev) -> io::Result<()> {
    assert_eq!(device.name()?, VIRTUAL_GAMEPAD_NAME);
    let id = device.input_id()?;
    assert_eq!(id.bus(), Bus::USB);
    assert_eq!(id.vendor(), DEFAULT_VENDOR_ID);
    assert_eq!(id.product(), DEFAULT_PRODUCT_ID);
    assert_eq!(id.version(), LINUX_VIRTUAL_GAMEPAD_VERSION);

    let keys = device.supported_keys()?;
    let expected_keys = [
        Key::BTN_SOUTH,
        Key::BTN_EAST,
        Key::BTN_WEST,
        Key::BTN_NORTH,
        Key::BTN_TL,
        Key::BTN_TR,
        Key::BTN_THUMBL,
        Key::BTN_THUMBR,
        Key::BTN_SELECT,
        Key::BTN_START,
        Key::BTN_MODE,
    ];
    assert_eq!(keys.len(), expected_keys.len());
    assert!(expected_keys.into_iter().all(|key| keys.contains(key)));

    let axes = device.supported_abs_axes()?;
    let expected_axes = [
        Abs::X,
        Abs::Y,
        Abs::RX,
        Abs::RY,
        Abs::Z,
        Abs::RZ,
        Abs::HAT0X,
        Abs::HAT0Y,
    ];
    assert_eq!(axes.len(), expected_axes.len());
    assert!(expected_axes.into_iter().all(|axis| axes.contains(axis)));
    for axis in [Abs::X, Abs::Y, Abs::RX, Abs::RY] {
        let info = device.abs_info(axis)?;
        assert_eq!((info.minimum(), info.maximum()), (-32_768, 32_767));
    }
    for axis in [Abs::Z, Abs::RZ] {
        let info = device.abs_info(axis)?;
        assert_eq!((info.minimum(), info.maximum()), (0, 255));
    }
    for axis in [Abs::HAT0X, Abs::HAT0Y] {
        let info = device.abs_info(axis)?;
        assert_eq!((info.minimum(), info.maximum()), (-1, 1));
    }

    let features = device.supported_ff_features()?;
    assert_eq!(features.len(), 1);
    assert!(features.contains(Feature::RUMBLE));
    assert_eq!(device.supported_ff_effects()?, 16);
    Ok(())
}

fn test_state() -> GamepadState {
    let mut buttons = GamepadButtons::default();
    buttons.set(Button::South, true);
    buttons.set(Button::Guide, true);
    GamepadState {
        buttons,
        hat: HatState::NorthEast,
        left_x: -1.0,
        left_y: 1.0,
        right_x: 1.0,
        right_y: -1.0,
        left_trigger: 0.5,
        right_trigger: 1.0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EventSignature {
    event_type: EventType,
    code: u16,
    value: i32,
}

impl From<InputEvent> for EventSignature {
    fn from(event: InputEvent) -> Self {
        Self {
            event_type: event.event_type(),
            code: event.raw_code(),
            value: event.raw_value(),
        }
    }
}

fn expected_active_report() -> Vec<EventSignature> {
    [
        InputEvent::from(KeyEvent::new(Key::BTN_SOUTH, KeyState::PRESSED)),
        InputEvent::from(KeyEvent::new(Key::BTN_MODE, KeyState::PRESSED)),
        InputEvent::from(AbsEvent::new(Abs::X, -32_767)),
        InputEvent::from(AbsEvent::new(Abs::Y, -32_767)),
        InputEvent::from(AbsEvent::new(Abs::RX, 32_767)),
        InputEvent::from(AbsEvent::new(Abs::RY, 32_767)),
        InputEvent::from(AbsEvent::new(Abs::Z, 128)),
        InputEvent::from(AbsEvent::new(Abs::RZ, 255)),
        InputEvent::from(AbsEvent::new(Abs::HAT0X, 1)),
        InputEvent::from(AbsEvent::new(Abs::HAT0Y, -1)),
        InputEvent::from(Syn::REPORT),
    ]
    .into_iter()
    .map(EventSignature::from)
    .collect()
}

fn expected_neutral_report() -> Vec<EventSignature> {
    [
        InputEvent::from(KeyEvent::new(Key::BTN_SOUTH, KeyState::RELEASED)),
        InputEvent::from(KeyEvent::new(Key::BTN_MODE, KeyState::RELEASED)),
        InputEvent::from(AbsEvent::new(Abs::X, 0)),
        InputEvent::from(AbsEvent::new(Abs::Y, 0)),
        InputEvent::from(AbsEvent::new(Abs::RX, 0)),
        InputEvent::from(AbsEvent::new(Abs::RY, 0)),
        InputEvent::from(AbsEvent::new(Abs::Z, 0)),
        InputEvent::from(AbsEvent::new(Abs::RZ, 0)),
        InputEvent::from(AbsEvent::new(Abs::HAT0X, 0)),
        InputEvent::from(AbsEvent::new(Abs::HAT0Y, 0)),
        InputEvent::from(Syn::REPORT),
    ]
    .into_iter()
    .map(EventSignature::from)
    .collect()
}

fn read_report(device: &Evdev) -> io::Result<Vec<EventSignature>> {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    let mut buffer = [InputEvent::zeroed(); 32];
    let mut report = Vec::new();
    loop {
        match device.read_events(&mut buffer) {
            Ok(count) => {
                for event in buffer[..count].iter().copied() {
                    let complete = matches!(
                        event.kind(),
                        EventKind::Syn(event) if event.syn() == Syn::REPORT
                    );
                    report.push(EventSignature::from(event));
                    if complete {
                        return Ok(report);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "a complete evdev state report was not received",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn service_blocking_request<T, F>(output: &mut VirtualGamepad, request: F) -> TestResult<T>
where
    T: Send,
    F: FnOnce() -> io::Result<T> + Send,
{
    thread::scope(|scope| {
        let worker = scope.spawn(request);
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            if worker.is_finished() {
                return Ok(worker
                    .join()
                    .map_err(|_| io::Error::other("force-feedback request thread panicked"))??);
            }
            if let Err(error) = output.service() {
                let _ = output.shutdown();
                return Err(error.into());
            }
            if Instant::now() >= deadline {
                let _ = output.shutdown();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "force-feedback request was not serviced",
                )
                .into());
            }
            thread::sleep(POLL_INTERVAL);
        }
    })
}

fn wait_for_feedback(output: &mut VirtualGamepad) -> TestResult<OutputFeedback> {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        output.service()?;
        if let Some(feedback) = output.take_feedback() {
            return Ok(feedback);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "virtual gamepad did not publish force feedback",
            )
            .into());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_removal(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    while path.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{} was not removed after uinput shutdown", path.display()),
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}
