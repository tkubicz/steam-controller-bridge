use std::path::PathBuf;

use bridge_output::{
    GamepadOutput, OutputDiagnostics, OutputError, OutputFeedback, OutputFeedbackSemantics,
};
use gamepad_state::GamepadState;

use crate::backend::Backend;
#[cfg(target_os = "linux")]
use crate::backend::LinuxUinputOutput;
#[cfg(target_os = "macos")]
use crate::VirtualHidOutput;
use crate::{
    VirtualGamepadError, VirtualGamepadErrorClass, VirtualHidConfig, VirtualHidHelperMetadata,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VirtualGamepadBackendKind {
    #[default]
    Automatic,
    MacOsHelper,
    LinuxUinput,
    WindowsProvider,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VirtualGamepadConfig {
    pub backend: VirtualGamepadBackendKind,
    pub macos_helper_path: Option<PathBuf>,
    pub linux_device_path: Option<PathBuf>,
}

impl VirtualGamepadConfig {
    #[must_use]
    pub fn macos_helper(path: PathBuf) -> Self {
        Self {
            backend: VirtualGamepadBackendKind::MacOsHelper,
            macos_helper_path: Some(path),
            linux_device_path: None,
        }
    }
}

pub struct VirtualGamepad {
    backend: Option<Box<dyn Backend>>,
    macos_helper_metadata: Option<VirtualHidHelperMetadata>,
    terminal_feedback: Option<OutputFeedback>,
    feedback_semantics: OutputFeedbackSemantics,
}

impl VirtualGamepad {
    /// Opens the selected host virtual-gamepad backend.
    ///
    /// # Errors
    ///
    /// Returns an actionable error when the requested backend is unavailable
    /// or its configuration is incomplete.
    pub fn open(config: VirtualGamepadConfig) -> Result<Self, VirtualGamepadError> {
        match config.backend {
            VirtualGamepadBackendKind::Automatic => Self::open_automatic(&config),
            VirtualGamepadBackendKind::MacOsHelper => {
                Self::open_macos_path(config.macos_helper_path)
            }
            VirtualGamepadBackendKind::LinuxUinput => {
                Self::open_linux_path(config.linux_device_path.as_deref())
            }
            VirtualGamepadBackendKind::WindowsProvider => {
                Err(backend_unavailable(
                    "the Windows virtual-gamepad backend is unavailable in this build; select another output backend",
                ))
            }
        }
    }

    /// Opens the macOS helper with the legacy provider-specific configuration.
    ///
    /// # Errors
    ///
    /// Returns an unavailable-backend error off macOS, or the existing helper
    /// error when startup fails on macOS.
    pub fn open_macos_helper(config: &VirtualHidConfig) -> Result<Self, VirtualGamepadError> {
        #[cfg(target_os = "macos")]
        {
            VirtualHidOutput::open(config.clone()).map(|backend| {
                let feedback_semantics = Backend::feedback_semantics(&backend);
                Self {
                    macos_helper_metadata: Some(backend.helper_metadata()),
                    backend: Some(Box::new(backend)),
                    terminal_feedback: None,
                    feedback_semantics,
                }
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = config;
            Err(backend_unavailable(
                "the macOS helper backend is only available on macOS; select a backend supported by this host",
            ))
        }
    }

    #[must_use]
    pub fn macos_helper_metadata(&self) -> Option<VirtualHidHelperMetadata> {
        self.macos_helper_metadata.clone()
    }

    /// Stops the active backend and releases its host resources.
    ///
    /// # Errors
    ///
    /// Returns the backend's classified shutdown error.
    pub fn shutdown(&mut self) -> Result<(), VirtualGamepadError> {
        self.macos_helper_metadata = None;
        let Some(mut backend) = self.backend.take() else {
            return Ok(());
        };
        let result = backend.shutdown();
        self.terminal_feedback = backend.take_feedback();
        result
    }

    #[cfg(target_os = "macos")]
    fn open_automatic(config: &VirtualGamepadConfig) -> Result<Self, VirtualGamepadError> {
        Self::open_macos_path(config.macos_helper_path.clone())
    }

    #[cfg(target_os = "linux")]
    fn open_automatic(config: &VirtualGamepadConfig) -> Result<Self, VirtualGamepadError> {
        Self::open_linux_path(config.linux_device_path.as_deref())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn open_automatic(_config: &VirtualGamepadConfig) -> Result<Self, VirtualGamepadError> {
        Err(backend_unavailable(
            "no virtual-gamepad backend is available for this host; select another output backend",
        ))
    }

    #[cfg(target_os = "macos")]
    fn open_macos_path(path: Option<PathBuf>) -> Result<Self, VirtualGamepadError> {
        let path = path.ok_or_else(|| {
            VirtualGamepadError::new(
                VirtualGamepadErrorClass::MissingHelper,
                "--output virtual-hid requires --virtual-hid-helper PATH",
            )
        })?;
        Self::open_macos_helper(&VirtualHidConfig::new(path))
    }

    #[cfg(not(target_os = "macos"))]
    fn open_macos_path(_path: Option<PathBuf>) -> Result<Self, VirtualGamepadError> {
        Err(backend_unavailable(
            "the macOS helper backend is only available on macOS; select a backend supported by this host",
        ))
    }

    #[cfg(target_os = "linux")]
    fn open_linux_path(path: Option<&std::path::Path>) -> Result<Self, VirtualGamepadError> {
        LinuxUinputOutput::open(path).map(|backend| {
            let feedback_semantics = Backend::feedback_semantics(&backend);
            Self {
                backend: Some(Box::new(backend)),
                macos_helper_metadata: None,
                terminal_feedback: None,
                feedback_semantics,
            }
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn open_linux_path(_path: Option<&std::path::Path>) -> Result<Self, VirtualGamepadError> {
        Err(backend_unavailable(
            "the Linux uinput virtual-gamepad backend is only available on Linux; select a backend supported by this host",
        ))
    }

    fn backend_mut(&mut self) -> Result<&mut (dyn Backend + 'static), OutputError> {
        self.backend.as_deref_mut().ok_or_else(|| {
            OutputError::from(backend_unavailable(
                "the virtual-gamepad backend has already shut down",
            ))
        })
    }
}

impl GamepadOutput for VirtualGamepad {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        self.backend_mut()?
            .send_state(state)
            .map_err(OutputError::from)
    }

    fn send_neutral(&mut self) -> Result<(), OutputError> {
        self.backend_mut()?
            .send_neutral()
            .map_err(OutputError::from)
    }

    fn service(&mut self) -> Result<(), OutputError> {
        self.backend_mut()?.service().map_err(OutputError::from)
    }

    fn feedback_semantics(&self) -> OutputFeedbackSemantics {
        self.feedback_semantics
    }

    fn take_feedback(&mut self) -> Option<OutputFeedback> {
        self.backend
            .as_deref_mut()
            .and_then(Backend::take_feedback)
            .or_else(|| self.terminal_feedback.take())
    }

    fn diagnostics(&self) -> OutputDiagnostics {
        self.backend
            .as_deref()
            .map_or_else(OutputDiagnostics::default, Backend::diagnostics)
    }
}

impl Drop for VirtualGamepad {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn backend_unavailable(message: &str) -> VirtualGamepadError {
    VirtualGamepadError::new(VirtualGamepadErrorClass::BackendUnavailable, message)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct ScriptedBackend {
        calls: Arc<Mutex<Vec<&'static str>>>,
        feedback: Option<OutputFeedback>,
        send_error: Option<VirtualGamepadErrorClass>,
        shutdown_feedback: Option<OutputFeedback>,
        feedback_semantics: OutputFeedbackSemantics,
    }

    impl Backend for ScriptedBackend {
        fn send_state(&mut self, _state: &GamepadState) -> Result<(), VirtualGamepadError> {
            self.calls.lock().unwrap().push("state");
            self.send_error.map_or(Ok(()), |class| {
                Err(VirtualGamepadError::new(class, "scripted failure"))
            })
        }

        fn send_neutral(&mut self) -> Result<(), VirtualGamepadError> {
            self.calls.lock().unwrap().push("neutral");
            Ok(())
        }

        fn service(&mut self) -> Result<(), VirtualGamepadError> {
            self.calls.lock().unwrap().push("service");
            Ok(())
        }

        fn take_feedback(&mut self) -> Option<OutputFeedback> {
            self.feedback.take()
        }

        fn feedback_semantics(&self) -> OutputFeedbackSemantics {
            self.feedback_semantics
        }

        fn diagnostics(&self) -> OutputDiagnostics {
            OutputDiagnostics {
                virtual_reports_dispatched: 7,
                ..OutputDiagnostics::default()
            }
        }

        fn shutdown(&mut self) -> Result<(), VirtualGamepadError> {
            self.calls.lock().unwrap().push("shutdown");
            self.feedback = self.shutdown_feedback.take();
            Ok(())
        }
    }

    #[test]
    fn facade_forwards_the_complete_backend_contract() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut output = VirtualGamepad {
            backend: Some(Box::new(ScriptedBackend {
                calls: Arc::clone(&calls),
                feedback: Some(OutputFeedback::Rumble {
                    low_frequency: 10,
                    high_frequency: 20,
                }),
                send_error: None,
                shutdown_feedback: None,
                feedback_semantics: OutputFeedbackSemantics::Stateful,
            })),
            macos_helper_metadata: Some(VirtualHidHelperMetadata {
                protocol_version: 4,
                ..VirtualHidHelperMetadata::default()
            }),
            terminal_feedback: None,
            feedback_semantics: OutputFeedbackSemantics::Stateful,
        };

        output.send_state(&GamepadState::neutral()).unwrap();
        output.send_neutral().unwrap();
        output.service().unwrap();
        assert_eq!(
            output.feedback_semantics(),
            OutputFeedbackSemantics::Stateful
        );
        assert_eq!(
            output.take_feedback(),
            Some(OutputFeedback::Rumble {
                low_frequency: 10,
                high_frequency: 20,
            })
        );
        assert_eq!(output.diagnostics().virtual_reports_dispatched, 7);
        assert_eq!(output.macos_helper_metadata().unwrap().protocol_version, 4);
        output.shutdown().unwrap();
        assert!(output.macos_helper_metadata().is_none());
        drop(output);

        assert_eq!(
            *calls.lock().unwrap(),
            ["state", "neutral", "service", "shutdown"]
        );
    }

    #[test]
    fn facade_preserves_permanent_and_retryable_error_classification() {
        for (class, permanent) in [
            (VirtualGamepadErrorClass::PermissionDenied, true),
            (VirtualGamepadErrorClass::DispatchFailed, false),
        ] {
            let mut output = VirtualGamepad {
                backend: Some(Box::new(ScriptedBackend {
                    calls: Arc::new(Mutex::new(Vec::new())),
                    feedback: None,
                    send_error: Some(class),
                    shutdown_feedback: None,
                    feedback_semantics: OutputFeedbackSemantics::Leased,
                })),
                macos_helper_metadata: None,
                terminal_feedback: None,
                feedback_semantics: OutputFeedbackSemantics::Leased,
            };
            let error = output.send_state(&GamepadState::neutral()).unwrap_err();
            let expected = format!("{class:?}: scripted failure");
            match (permanent, error) {
                (true, OutputError::Configuration(message))
                | (false, OutputError::Transport(message)) => assert_eq!(message, expected),
                (_, error) => panic!("{class:?} mapped to {error:?}"),
            }
        }
    }

    #[test]
    fn unavailable_windows_backend_never_substitutes_another_provider() {
        let result = VirtualGamepad::open(VirtualGamepadConfig {
            backend: VirtualGamepadBackendKind::WindowsProvider,
            macos_helper_path: Some(PathBuf::from("helper")),
            linux_device_path: Some(PathBuf::from("uinput")),
        });
        let Err(error) = result else {
            panic!("unimplemented backend unexpectedly opened")
        };
        assert_eq!(error.class(), VirtualGamepadErrorClass::BackendUnavailable);
        assert!(error.is_permanent_configuration_failure());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unavailable_linux_backend_never_substitutes_another_provider() {
        let result = VirtualGamepad::open(VirtualGamepadConfig {
            backend: VirtualGamepadBackendKind::LinuxUinput,
            macos_helper_path: None,
            linux_device_path: None,
        });
        let Err(error) = result else {
            panic!("Linux backend unexpectedly opened")
        };
        assert_eq!(error.class(), VirtualGamepadErrorClass::BackendUnavailable);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_explicit_and_automatic_selection_reach_uinput_validation() {
        for backend in [
            VirtualGamepadBackendKind::Automatic,
            VirtualGamepadBackendKind::LinuxUinput,
        ] {
            let result = VirtualGamepad::open(VirtualGamepadConfig {
                backend,
                macos_helper_path: None,
                linux_device_path: Some(PathBuf::from("/tmp/not-uinput")),
            });
            let Err(error) = result else {
                panic!("invalid uinput path unexpectedly opened")
            };
            assert_eq!(
                error.class(),
                VirtualGamepadErrorClass::InvalidConfiguration
            );
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn unsupported_host_reports_automatic_and_macos_backends_as_unavailable() {
        for backend in [
            VirtualGamepadBackendKind::Automatic,
            VirtualGamepadBackendKind::MacOsHelper,
        ] {
            let result = VirtualGamepad::open(VirtualGamepadConfig {
                backend,
                macos_helper_path: Some(PathBuf::from("helper")),
                linux_device_path: None,
            });
            let Err(error) = result else {
                panic!("unsupported backend unexpectedly opened")
            };
            assert_eq!(error.class(), VirtualGamepadErrorClass::BackendUnavailable);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn automatic_selection_requires_the_macos_helper_configuration() {
        let result = VirtualGamepad::open(VirtualGamepadConfig::default());
        let Err(error) = result else {
            panic!("automatic backend opened without its helper path")
        };
        assert_eq!(error.class(), VirtualGamepadErrorClass::MissingHelper);
    }

    #[test]
    fn shutdown_preserves_the_backend_zero_feedback() {
        let mut output = VirtualGamepad {
            backend: Some(Box::new(ScriptedBackend {
                calls: Arc::new(Mutex::new(Vec::new())),
                feedback: Some(OutputFeedback::Rumble {
                    low_frequency: 10,
                    high_frequency: 20,
                }),
                send_error: None,
                shutdown_feedback: Some(OutputFeedback::Rumble {
                    low_frequency: 0,
                    high_frequency: 0,
                }),
                feedback_semantics: OutputFeedbackSemantics::Stateful,
            })),
            macos_helper_metadata: None,
            terminal_feedback: None,
            feedback_semantics: OutputFeedbackSemantics::Stateful,
        };

        output.shutdown().unwrap();
        assert_eq!(
            output.feedback_semantics(),
            OutputFeedbackSemantics::Stateful
        );

        assert_eq!(
            output.take_feedback(),
            Some(OutputFeedback::Rumble {
                low_frequency: 0,
                high_frequency: 0,
            })
        );
        assert_eq!(output.take_feedback(), None);
    }
}
