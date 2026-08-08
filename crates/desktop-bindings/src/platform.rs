#[cfg(target_os = "macos")]
mod macos {
    use enigo::{
        Axis, Button as EnigoButton, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings,
    };
    use objc2_core_graphics::{CGPreflightPostEventAccess, CGRequestPostEventAccess};
    use objc2_io_kit::{IOHIDAccessType, IOHIDCheckAccess, IOHIDRequestAccess, IOHIDRequestType};

    use crate::{DesktopInputSink, KeyboardKey, Modifier, MouseButton};

    pub struct MacOsDesktopInput {
        enigo: Enigo,
    }

    impl MacOsDesktopInput {
        /// Opens the macOS desktop-input connection.
        ///
        /// # Errors
        /// Returns `permission required` when Accessibility is unavailable, or
        /// another backend construction error.
        pub fn new() -> Result<Self, String> {
            Self::new_with_prompt(false)
        }

        fn new_with_prompt(prompt_for_permission: bool) -> Result<Self, String> {
            let settings = Settings {
                // Permission requests are allowed only through the explicit
                // main-thread helper below. Runtime workers stay non-prompting.
                open_prompt_to_get_permissions: prompt_for_permission,
                release_keys_when_dropped: true,
                ..Settings::default()
            };
            Enigo::new(&settings)
                .map(|enigo| Self { enigo })
                .map_err(|error| match error {
                    enigo::NewConError::NoPermission => {
                        "Accessibility permission required".to_owned()
                    }
                    other => format!("cannot initialize desktop input: {other}"),
                })
        }
    }

    /// Whether macOS has decided about a permission yet, and how.
    ///
    /// The distinction matters: an undecided permission can still be asked for
    /// and macOS will show its dialog, while a refused one cannot -- asking
    /// again does nothing at all, and the only way forward is System Settings.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PermissionState {
        Granted,
        Denied,
        Undecided,
    }

    /// Reports whether this process may observe input, i.e. Input Monitoring.
    #[must_use]
    pub fn input_monitoring_access() -> PermissionState {
        match IOHIDCheckAccess(IOHIDRequestType::ListenEvent) {
            IOHIDAccessType::Granted => PermissionState::Granted,
            IOHIDAccessType::Denied => PermissionState::Denied,
            _ => PermissionState::Undecided,
        }
    }

    /// Asks macOS for Input Monitoring, showing its dialog when undecided.
    ///
    /// Returns whether the permission is granted afterwards. A refusal that
    /// macOS already recorded produces no dialog, so callers should send the
    /// user to System Settings in that case.
    #[must_use]
    pub fn request_input_monitoring_access() -> bool {
        IOHIDRequestAccess(IOHIDRequestType::ListenEvent)
    }

    /// Returns whether macOS currently permits this process to post input events.
    #[must_use]
    pub fn preflight_post_event_access() -> bool {
        CGPreflightPostEventAccess()
    }

    /// Makes macOS's native request for permission to post input events.
    ///
    /// This is the `PostEvent` counterpart to the `ListenEvent` request used for
    /// Input Monitoring. It must be called by an interactive macOS frontend.
    #[must_use]
    pub fn request_post_event_access() -> bool {
        CGRequestPostEventAccess()
    }

    /// Checks whether the Enigo adapter's Accessibility trust is available.
    #[must_use]
    pub fn preflight_accessibility_access() -> bool {
        MacOsDesktopInput::new_with_prompt(false).is_ok()
    }

    /// Requests the Accessibility trust required by the Enigo adapter.
    ///
    /// The menu app calls this on its main thread after creating its native
    /// status item. Keeping it out of runtime workers makes the system prompt
    /// reliably attributable to the foreground application bundle.
    #[must_use]
    pub fn request_accessibility_access() -> bool {
        MacOsDesktopInput::new_with_prompt(true).is_ok()
    }

    impl DesktopInputSink for MacOsDesktopInput {
        fn key(&mut self, key: KeyboardKey, pressed: bool) -> Result<(), String> {
            let key = enigo_key(key)?;
            self.enigo
                .key(key, direction(pressed))
                .map_err(|error| error.to_string())
        }

        fn modifier(&mut self, modifier: Modifier, pressed: bool) -> Result<(), String> {
            let key = modifier_key(modifier);
            self.enigo
                .key(key, direction(pressed))
                .map_err(|error| error.to_string())
        }

        fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> Result<(), String> {
            let button = enigo_button(button);
            self.enigo
                .button(button, direction(pressed))
                .map_err(|error| error.to_string())
        }

        fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.enigo
                .move_mouse(x, y, Coordinate::Rel)
                .map_err(|error| error.to_string())
        }

        fn scroll(&mut self, x: i32, y: i32) -> Result<(), String> {
            if x != 0 {
                self.enigo
                    .smooth_scroll(x, Axis::Horizontal)
                    .map_err(|error| error.to_string())?;
            }
            if y != 0 {
                self.enigo
                    .smooth_scroll(y, Axis::Vertical)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        }
    }

    const fn direction(pressed: bool) -> Direction {
        if pressed {
            Direction::Press
        } else {
            Direction::Release
        }
    }

    const fn modifier_key(modifier: Modifier) -> Key {
        match modifier {
            Modifier::Command => Key::Meta,
            Modifier::Control => Key::Control,
            Modifier::Option => Key::Option,
            Modifier::Shift => Key::Shift,
        }
    }

    const fn enigo_button(button: MouseButton) -> EnigoButton {
        match button {
            MouseButton::Left => EnigoButton::Left,
            MouseButton::Right => EnigoButton::Right,
            MouseButton::Middle => EnigoButton::Middle,
            MouseButton::Back => EnigoButton::Back,
            MouseButton::Forward => EnigoButton::Forward,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn enigo_key(key: KeyboardKey) -> Result<Key, String> {
        let key = match key {
            KeyboardKey::A => Key::Unicode('a'),
            KeyboardKey::B => Key::Unicode('b'),
            KeyboardKey::C => Key::Unicode('c'),
            KeyboardKey::D => Key::Unicode('d'),
            KeyboardKey::E => Key::Unicode('e'),
            KeyboardKey::F => Key::Unicode('f'),
            KeyboardKey::G => Key::Unicode('g'),
            KeyboardKey::H => Key::Unicode('h'),
            KeyboardKey::I => Key::Unicode('i'),
            KeyboardKey::J => Key::Unicode('j'),
            KeyboardKey::K => Key::Unicode('k'),
            KeyboardKey::L => Key::Unicode('l'),
            KeyboardKey::M => Key::Unicode('m'),
            KeyboardKey::N => Key::Unicode('n'),
            KeyboardKey::O => Key::Unicode('o'),
            KeyboardKey::P => Key::Unicode('p'),
            KeyboardKey::Q => Key::Unicode('q'),
            KeyboardKey::R => Key::Unicode('r'),
            KeyboardKey::S => Key::Unicode('s'),
            KeyboardKey::T => Key::Unicode('t'),
            KeyboardKey::U => Key::Unicode('u'),
            KeyboardKey::V => Key::Unicode('v'),
            KeyboardKey::W => Key::Unicode('w'),
            KeyboardKey::X => Key::Unicode('x'),
            KeyboardKey::Y => Key::Unicode('y'),
            KeyboardKey::Z => Key::Unicode('z'),
            KeyboardKey::Digit0 => Key::Unicode('0'),
            KeyboardKey::Digit1 => Key::Unicode('1'),
            KeyboardKey::Digit2 => Key::Unicode('2'),
            KeyboardKey::Digit3 => Key::Unicode('3'),
            KeyboardKey::Digit4 => Key::Unicode('4'),
            KeyboardKey::Digit5 => Key::Unicode('5'),
            KeyboardKey::Digit6 => Key::Unicode('6'),
            KeyboardKey::Digit7 => Key::Unicode('7'),
            KeyboardKey::Digit8 => Key::Unicode('8'),
            KeyboardKey::Digit9 => Key::Unicode('9'),
            KeyboardKey::F1 => Key::F1,
            KeyboardKey::F2 => Key::F2,
            KeyboardKey::F3 => Key::F3,
            KeyboardKey::F4 => Key::F4,
            KeyboardKey::F5 => Key::F5,
            KeyboardKey::F6 => Key::F6,
            KeyboardKey::F7 => Key::F7,
            KeyboardKey::F8 => Key::F8,
            KeyboardKey::F9 => Key::F9,
            KeyboardKey::F10 => Key::F10,
            KeyboardKey::F11 => Key::F11,
            KeyboardKey::F12 => Key::F12,
            KeyboardKey::F13 => Key::F13,
            KeyboardKey::F14 => Key::F14,
            KeyboardKey::F15 => Key::F15,
            KeyboardKey::F16 => Key::F16,
            KeyboardKey::F17 => Key::F17,
            KeyboardKey::F18 => Key::F18,
            KeyboardKey::F19 => Key::F19,
            KeyboardKey::F20 => Key::F20,
            KeyboardKey::F21 | KeyboardKey::F22 | KeyboardKey::F23 | KeyboardKey::F24 => {
                return Err(format!(
                    "{} is not available through the macOS keyboard event API",
                    key.label()
                ));
            }
            KeyboardKey::Escape => Key::Escape,
            KeyboardKey::Tab => Key::Tab,
            KeyboardKey::Return => Key::Return,
            KeyboardKey::NumpadEnter => Key::Other(76),
            KeyboardKey::Space => Key::Space,
            KeyboardKey::Backspace => Key::Backspace,
            KeyboardKey::Delete => Key::Delete,
            // macOS exposes the legacy Help/Insert physical key as Help.
            KeyboardKey::Insert => Key::Help,
            KeyboardKey::Home => Key::Home,
            KeyboardKey::End => Key::End,
            KeyboardKey::PageUp => Key::PageUp,
            KeyboardKey::PageDown => Key::PageDown,
            KeyboardKey::ArrowLeft => Key::LeftArrow,
            KeyboardKey::ArrowRight => Key::RightArrow,
            KeyboardKey::ArrowUp => Key::UpArrow,
            KeyboardKey::ArrowDown => Key::DownArrow,
            KeyboardKey::Grave => Key::Unicode('`'),
            KeyboardKey::Minus => Key::Unicode('-'),
            KeyboardKey::Equal => Key::Unicode('='),
            KeyboardKey::LeftBracket => Key::Unicode('['),
            KeyboardKey::RightBracket => Key::Unicode(']'),
            KeyboardKey::Backslash => Key::Unicode('\\'),
            KeyboardKey::Semicolon => Key::Unicode(';'),
            KeyboardKey::Quote => Key::Unicode('\''),
            KeyboardKey::Comma => Key::Unicode(','),
            KeyboardKey::Period => Key::Unicode('.'),
            KeyboardKey::Slash => Key::Unicode('/'),
            KeyboardKey::Numpad0 => Key::Numpad0,
            KeyboardKey::Numpad1 => Key::Numpad1,
            KeyboardKey::Numpad2 => Key::Numpad2,
            KeyboardKey::Numpad3 => Key::Numpad3,
            KeyboardKey::Numpad4 => Key::Numpad4,
            KeyboardKey::Numpad5 => Key::Numpad5,
            KeyboardKey::Numpad6 => Key::Numpad6,
            KeyboardKey::Numpad7 => Key::Numpad7,
            KeyboardKey::Numpad8 => Key::Numpad8,
            KeyboardKey::Numpad9 => Key::Numpad9,
            KeyboardKey::NumpadAdd => Key::Add,
            KeyboardKey::NumpadSubtract => Key::Subtract,
            KeyboardKey::NumpadMultiply => Key::Multiply,
            KeyboardKey::NumpadDivide => Key::Divide,
            KeyboardKey::NumpadDecimal => Key::Decimal,
            KeyboardKey::MediaPlayPause | KeyboardKey::MediaPrevious | KeyboardKey::MediaNext => {
                return Err(format!(
                    "{} is not available through Enigo's macOS adapter",
                    key.label()
                ));
            }
            KeyboardKey::VolumeMute => Key::VolumeMute,
            KeyboardKey::VolumeDown => Key::VolumeDown,
            KeyboardKey::VolumeUp => Key::VolumeUp,
        };
        Ok(key)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_declared_key_has_an_explicit_macos_conversion_result() {
            for key in KeyboardKey::ALL {
                let result = enigo_key(*key);
                if matches!(
                    key,
                    KeyboardKey::F21
                        | KeyboardKey::F22
                        | KeyboardKey::F23
                        | KeyboardKey::F24
                        | KeyboardKey::MediaPlayPause
                        | KeyboardKey::MediaPrevious
                        | KeyboardKey::MediaNext
                ) {
                    assert!(result.is_err(), "{key:?} must fail explicitly on macOS");
                } else {
                    assert!(result.is_ok(), "missing macOS conversion for {key:?}");
                }
            }
        }

        #[test]
        fn every_modifier_and_mouse_button_has_the_expected_macos_conversion() {
            assert_eq!(modifier_key(Modifier::Command), Key::Meta);
            assert_eq!(modifier_key(Modifier::Control), Key::Control);
            assert_eq!(modifier_key(Modifier::Option), Key::Option);
            assert_eq!(modifier_key(Modifier::Shift), Key::Shift);
            assert_eq!(enigo_button(MouseButton::Left), EnigoButton::Left);
            assert_eq!(enigo_button(MouseButton::Right), EnigoButton::Right);
            assert_eq!(enigo_button(MouseButton::Middle), EnigoButton::Middle);
            assert_eq!(enigo_button(MouseButton::Back), EnigoButton::Back);
            assert_eq!(enigo_button(MouseButton::Forward), EnigoButton::Forward);
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::{
    input_monitoring_access, preflight_accessibility_access, preflight_post_event_access,
    request_accessibility_access, request_input_monitoring_access, request_post_event_access,
    MacOsDesktopInput, PermissionState,
};
