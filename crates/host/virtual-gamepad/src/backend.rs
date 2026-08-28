use bridge_output::{OutputDiagnostics, OutputFeedback, OutputFeedbackSemantics};
use gamepad_state::GamepadState;

use crate::VirtualGamepadError;

pub(crate) trait Backend: Send {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), VirtualGamepadError>;
    fn send_neutral(&mut self) -> Result<(), VirtualGamepadError>;
    fn service(&mut self) -> Result<(), VirtualGamepadError>;
    fn feedback_semantics(&self) -> OutputFeedbackSemantics {
        OutputFeedbackSemantics::Leased
    }
    fn take_feedback(&mut self) -> Option<OutputFeedback>;
    fn diagnostics(&self) -> OutputDiagnostics;
    fn shutdown(&mut self) -> Result<(), VirtualGamepadError>;
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "linux", test))]
#[path = "backend/linux/feedback.rs"]
mod linux_feedback;
#[cfg(any(target_os = "linux", test))]
#[path = "backend/linux/mapping.rs"]
mod linux_mapping;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

#[cfg(target_os = "linux")]
pub(crate) use linux::LinuxUinputOutput;
#[cfg(target_os = "macos")]
pub(crate) use macos::VirtualDevice;
#[cfg(not(target_os = "macos"))]
pub(crate) use unsupported::VirtualDevice;
