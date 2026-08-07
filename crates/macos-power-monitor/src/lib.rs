//! Owned system sleep/wake notifications for macOS.
//!
//! `IOKit`'s power API is a C callback API, so this crate is the workspace's one
//! deliberately narrow unsafe boundary. The rest of the project receives typed
//! events and cannot access the raw run-loop, notification-port, or callback
//! pointers.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::PowerMonitor;

/// A system-power transition relevant to open hardware handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    /// The system has committed to sleeping. The callback must finish its
    /// hardware teardown before returning so `IOKit` can acknowledge the sleep.
    WillSleep,
    /// The system has powered back on and hardware may begin its wake settle.
    DidWake,
}

/// Failure to register the macOS `IOKit` power notification source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerMonitorError(String);

impl std::fmt::Display for PowerMonitorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PowerMonitorError {}

#[cfg(not(target_os = "macos"))]
#[derive(Debug, Default)]
pub struct PowerMonitor;

#[cfg(not(target_os = "macos"))]
impl PowerMonitor {
    /// Non-macOS builds have no supported hardware runtime, so the monitor is
    /// an inert value that keeps cross-platform compilation straightforward.
    ///
    /// # Errors
    ///
    /// The inert implementation never returns an error.
    pub fn new(
        _handler: impl FnMut(PowerEvent) + Send + 'static,
    ) -> Result<Self, PowerMonitorError> {
        Ok(Self)
    }
}
