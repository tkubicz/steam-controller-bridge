//! Target-selecting desktop-input factories and platform adapters.

use desktop_bindings::DesktopInputSink;
#[cfg(target_os = "macos")]
use desktop_bindings::{KeyboardKey, Modifier, MouseButton};

#[cfg(target_os = "macos")]
#[path = "backend/macos.rs"]
mod platform;

#[cfg(target_os = "macos")]
pub use platform::{
    input_monitoring_access, preflight_accessibility_access, preflight_post_event_access,
    request_accessibility_access, request_input_monitoring_access, request_post_event_access,
    MacOsDesktopInput, PermissionState,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopSession {
    MacOs,
    X11,
    Wayland { compositor: Option<String> },
    Windows,
}

pub trait DesktopInputFactory {
    /// Detects the active desktop session without constructing an input sink.
    ///
    /// # Errors
    ///
    /// Returns an actionable error when the current session cannot be used.
    fn detect_session(&self) -> Result<DesktopSession, String>;

    /// Creates exactly one adapter for `session`.
    ///
    /// # Errors
    ///
    /// Returns an actionable error when the selected adapter cannot be opened.
    fn create(&mut self, session: &DesktopSession) -> Result<Box<dyn DesktopInputSink>, String>;
}

/// Selects a desktop-input factory for the current target.
///
/// # Errors
///
/// Returns an error until a provider exists for the current target.
pub fn current_factory() -> Result<Box<dyn DesktopInputFactory>, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(MacOsDesktopInputFactory))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("desktop bindings are only available on macOS".to_owned())
    }
}

#[cfg(target_os = "macos")]
struct MacOsDesktopInputFactory;

#[cfg(target_os = "macos")]
impl DesktopInputFactory for MacOsDesktopInputFactory {
    fn detect_session(&self) -> Result<DesktopSession, String> {
        Ok(DesktopSession::MacOs)
    }

    fn create(&mut self, session: &DesktopSession) -> Result<Box<dyn DesktopInputSink>, String> {
        if session != &DesktopSession::MacOs {
            return Err(format!(
                "the macOS desktop-input factory cannot create a {session:?} adapter"
            ));
        }
        MacOsDesktopInput::new().map(|sink| Box::new(sink) as Box<dyn DesktopInputSink>)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn current_factory_detects_only_the_macos_session() {
        let mut factory = current_factory().unwrap();
        assert_eq!(factory.detect_session().unwrap(), DesktopSession::MacOs);
        assert!(factory.create(&DesktopSession::Windows).is_err());
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn unsupported_targets_fail_before_constructing_an_adapter() {
        assert!(current_factory().is_err());
    }
}
