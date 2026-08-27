//! Owned system sleep/wake notifications.
//!
//! `IOKit`'s power API is a C callback API, so this crate is the workspace's one
//! deliberately narrow unsafe boundary. The rest of the project receives typed
//! events and cannot access the raw run-loop, notification-port, or callback
//! pointers.

use std::time::Instant;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::PowerMonitor;

/// A system-power transition relevant to open hardware handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    /// The system has committed to sleeping and hardware teardown must finish
    /// before `deadline`.
    WillSleep { deadline: Instant },
    /// The system has powered back on and hardware may begin its wake settle.
    DidWake,
}

/// Failure to register the host power notification source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerMonitorError(String);

impl PowerMonitorError {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

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
    /// # Errors
    ///
    /// The unsupported implementation never returns an error.
    pub fn new(
        _handler: impl FnMut(PowerEvent) + Send + 'static,
    ) -> Result<Self, PowerMonitorError> {
        Ok(Self)
    }

    #[must_use]
    pub const fn is_live(&self) -> bool {
        false
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn unsupported_provider_is_not_reported_as_live() {
        let monitor = PowerMonitor::new(|_| {}).unwrap();
        assert!(!monitor.is_live());
    }
}
