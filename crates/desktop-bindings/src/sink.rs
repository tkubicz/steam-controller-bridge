#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum OutputKey {
    Modifier(Modifier),
    Key(KeyboardKey),
}

pub trait DesktopInputSink {
    /// Emits a keyboard transition.
    ///
    /// # Errors
    /// Returns an error if the platform cannot inject the transition.
    fn key(&mut self, key: KeyboardKey, pressed: bool) -> Result<(), String>;

    /// Emits a modifier-key transition.
    ///
    /// # Errors
    /// Returns an error if the platform cannot inject the transition.
    fn modifier(&mut self, modifier: Modifier, pressed: bool) -> Result<(), String>;

    /// Emits a mouse-button transition.
    ///
    /// # Errors
    /// Returns an error if the platform cannot inject the transition.
    fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> Result<(), String>;

    /// Moves the pointer by a relative number of pixels.
    ///
    /// # Errors
    /// Returns an error if the platform cannot inject the movement.
    fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String>;

    /// Smooth-scrolls by a relative number of pixels.
    ///
    /// Positive X moves content right and positive Y moves content down.
    ///
    /// # Errors
    /// Returns an error if the platform cannot inject the scroll.
    fn scroll(&mut self, x: i32, y: i32) -> Result<(), String>;
}
use crate::model::{KeyboardKey, Modifier, MouseButton};
