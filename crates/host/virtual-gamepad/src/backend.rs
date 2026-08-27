use bridge_output::{OutputDiagnostics, OutputFeedback};
use gamepad_state::GamepadState;

use crate::VirtualGamepadError;

pub(crate) trait Backend: Send {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), VirtualGamepadError>;
    fn send_neutral(&mut self) -> Result<(), VirtualGamepadError>;
    fn service(&mut self) -> Result<(), VirtualGamepadError>;
    fn take_feedback(&mut self) -> Option<OutputFeedback>;
    fn diagnostics(&self) -> OutputDiagnostics;
    fn shutdown(&mut self) -> Result<(), VirtualGamepadError>;
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

#[cfg(target_os = "macos")]
pub(crate) use macos::VirtualDevice;
#[cfg(not(target_os = "macos"))]
pub(crate) use unsupported::VirtualDevice;
