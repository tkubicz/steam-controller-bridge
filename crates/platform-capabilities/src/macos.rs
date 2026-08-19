use desktop_input::DesktopSession;

use crate::{
    CapabilityContext, CapabilityError, CapabilityId, CapabilityState, PlatformCapabilities,
    Remedy, RequirementGroup,
};

const INPUT_MONITORING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent";
const ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMonitoringState {
    Granted,
    Denied,
    Undecided,
}

trait MacOsPermissionApi {
    fn input_monitoring_access(&self) -> InputMonitoringState;
    fn request_input_monitoring_access(&mut self) -> bool;
    fn preflight_post_event_access(&self) -> bool;
    fn request_post_event_access(&mut self) -> bool;
    fn preflight_accessibility_access(&self) -> bool;
    fn request_accessibility_access(&mut self) -> bool;
}

/// macOS capability provider backed by the existing native TCC helpers.
pub struct MacOsCapabilities {
    api: Box<dyn MacOsPermissionApi>,
    diagnostic: Box<dyn FnMut(&str)>,
}

impl MacOsCapabilities {
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn new() -> Self {
        Self {
            api: Box::new(NativeMacOsPermissionApi),
            diagnostic: Box::new(|line| eprintln!("{line}")),
        }
    }

    fn input_monitoring_state(&self) -> CapabilityState {
        match self.api.input_monitoring_access() {
            InputMonitoringState::Granted => CapabilityState::Satisfied,
            InputMonitoringState::Denied => CapabilityState::Blocked {
                reason: "Input Monitoring permission denied".to_owned(),
            },
            InputMonitoringState::Undecided => CapabilityState::Undecided,
        }
    }

    fn post_event_state(&self) -> CapabilityState {
        if self.api.preflight_post_event_access() {
            CapabilityState::Satisfied
        } else {
            CapabilityState::Blocked {
                reason: "Post Event permission required".to_owned(),
            }
        }
    }

    fn accessibility_state(&self) -> CapabilityState {
        if self.api.preflight_accessibility_access() {
            CapabilityState::Satisfied
        } else {
            CapabilityState::Blocked {
                reason: "Accessibility permission required".to_owned(),
            }
        }
    }

    fn request_input_monitoring(&mut self) -> CapabilityState {
        let access = self.api.input_monitoring_access();
        let undecided = access == InputMonitoringState::Undecided;
        let granted = match access {
            InputMonitoringState::Granted => true,
            InputMonitoringState::Undecided => self.api.request_input_monitoring_access(),
            InputMonitoringState::Denied => false,
        };
        (self.diagnostic)(&format!(
            "level=info event=input_monitoring_permission_requested \
             granted={granted} undecided={undecided} api=IOHIDRequestAccess"
        ));
        if granted {
            CapabilityState::Satisfied
        } else if undecided {
            CapabilityState::Pending
        } else {
            CapabilityState::Blocked {
                reason: "Input Monitoring permission denied".to_owned(),
            }
        }
    }

    fn request_post_event(&mut self) -> CapabilityState {
        let granted = self.api.request_post_event_access();
        (self.diagnostic)(&format!(
            "level=info event=post_event_permission_requested granted={granted} \
             api=CGRequestPostEventAccess"
        ));
        if granted {
            CapabilityState::Satisfied
        } else {
            CapabilityState::Blocked {
                reason: "Post Event permission required".to_owned(),
            }
        }
    }

    fn request_accessibility(&mut self) -> CapabilityState {
        let granted = self.api.request_accessibility_access();
        (self.diagnostic)(&format!(
            "level=info event=accessibility_permission_requested granted={granted} \
             api=AXIsProcessTrustedWithOptions"
        ));
        if granted {
            CapabilityState::Satisfied
        } else {
            CapabilityState::Blocked {
                reason: "Accessibility permission required".to_owned(),
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl Default for MacOsCapabilities {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformCapabilities for MacOsCapabilities {
    fn requirements(&self, context: &CapabilityContext) -> Vec<RequirementGroup> {
        if context.desktop_bindings_enabled
            && context.desktop_session.as_ref() != Some(&DesktopSession::MacOs)
        {
            return vec![RequirementGroup::Independent(vec![
                CapabilityId::DesktopInputAccess,
            ])];
        }

        let mut ordered = Vec::new();
        if context.controller_input_enabled || context.desktop_bindings_enabled {
            ordered.push(CapabilityId::InputMonitoring);
        }
        if context.desktop_bindings_enabled {
            ordered.extend([CapabilityId::PostEvent, CapabilityId::Accessibility]);
        }
        if ordered.is_empty() {
            Vec::new()
        } else {
            vec![RequirementGroup::Ordered(ordered)]
        }
    }

    fn probe(&self, id: CapabilityId) -> CapabilityState {
        match id {
            CapabilityId::InputMonitoring => self.input_monitoring_state(),
            CapabilityId::PostEvent => self.post_event_state(),
            CapabilityId::Accessibility => self.accessibility_state(),
            CapabilityId::DesktopInputAccess => CapabilityState::Unavailable {
                reason: "the macOS provider requires a macOS desktop session".to_owned(),
            },
            CapabilityId::ControllerHidAccess
            | CapabilityId::SerialPortAccess
            | CapabilityId::VirtualGamepadAccess => CapabilityState::NotRequired,
        }
    }

    fn request(&mut self, id: CapabilityId) -> Result<CapabilityState, CapabilityError> {
        Ok(match id {
            CapabilityId::InputMonitoring => self.request_input_monitoring(),
            CapabilityId::PostEvent => self.request_post_event(),
            CapabilityId::Accessibility => self.request_accessibility(),
            CapabilityId::DesktopInputAccess => CapabilityState::Unavailable {
                reason: "the macOS provider requires a macOS desktop session".to_owned(),
            },
            CapabilityId::ControllerHidAccess
            | CapabilityId::SerialPortAccess
            | CapabilityId::VirtualGamepadAccess => CapabilityState::NotRequired,
        })
    }

    fn remedy(&self, id: CapabilityId) -> Option<Remedy> {
        match id {
            CapabilityId::InputMonitoring => {
                Some(Remedy::OpenUrl(INPUT_MONITORING_SETTINGS_URL.to_owned()))
            }
            CapabilityId::PostEvent | CapabilityId::Accessibility => {
                Some(Remedy::OpenUrl(ACCESSIBILITY_SETTINGS_URL.to_owned()))
            }
            CapabilityId::ControllerHidAccess
            | CapabilityId::SerialPortAccess
            | CapabilityId::VirtualGamepadAccess
            | CapabilityId::DesktopInputAccess => None,
        }
    }
}

#[cfg(target_os = "macos")]
struct NativeMacOsPermissionApi;

#[cfg(target_os = "macos")]
impl MacOsPermissionApi for NativeMacOsPermissionApi {
    fn input_monitoring_access(&self) -> InputMonitoringState {
        match desktop_input::input_monitoring_access() {
            desktop_input::PermissionState::Granted => InputMonitoringState::Granted,
            desktop_input::PermissionState::Denied => InputMonitoringState::Denied,
            desktop_input::PermissionState::Undecided => InputMonitoringState::Undecided,
        }
    }

    fn request_input_monitoring_access(&mut self) -> bool {
        desktop_input::request_input_monitoring_access()
    }

    fn preflight_post_event_access(&self) -> bool {
        desktop_input::preflight_post_event_access()
    }

    fn request_post_event_access(&mut self) -> bool {
        desktop_input::request_post_event_access()
    }

    fn preflight_accessibility_access(&self) -> bool {
        desktop_input::preflight_accessibility_access()
    }

    fn request_accessibility_access(&mut self) -> bool {
        desktop_input::request_accessibility_access()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::{evaluate_requirements, request_next};

    use super::*;

    #[derive(Debug)]
    #[allow(
        clippy::struct_excessive_bools,
        reason = "the fake exposes each independently controlled native permission result"
    )]
    struct FakeState {
        input_monitoring: InputMonitoringState,
        post_event: bool,
        accessibility: bool,
        grant_input_monitoring: bool,
        grant_post_event: bool,
        grant_accessibility: bool,
        calls: Vec<&'static str>,
    }

    impl Default for FakeState {
        fn default() -> Self {
            Self {
                input_monitoring: InputMonitoringState::Undecided,
                post_event: false,
                accessibility: false,
                grant_input_monitoring: false,
                grant_post_event: false,
                grant_accessibility: false,
                calls: Vec::new(),
            }
        }
    }

    #[derive(Clone)]
    struct FakeApi(Rc<RefCell<FakeState>>);

    impl MacOsPermissionApi for FakeApi {
        fn input_monitoring_access(&self) -> InputMonitoringState {
            let mut state = self.0.borrow_mut();
            state.calls.push("probe_input_monitoring");
            state.input_monitoring
        }

        fn request_input_monitoring_access(&mut self) -> bool {
            let mut state = self.0.borrow_mut();
            state.calls.push("request_input_monitoring");
            if state.grant_input_monitoring {
                state.input_monitoring = InputMonitoringState::Granted;
            }
            state.grant_input_monitoring
        }

        fn preflight_post_event_access(&self) -> bool {
            let mut state = self.0.borrow_mut();
            state.calls.push("probe_post_event");
            state.post_event
        }

        fn request_post_event_access(&mut self) -> bool {
            let mut state = self.0.borrow_mut();
            state.calls.push("request_post_event");
            if state.grant_post_event {
                state.post_event = true;
            }
            state.grant_post_event
        }

        fn preflight_accessibility_access(&self) -> bool {
            let mut state = self.0.borrow_mut();
            state.calls.push("probe_accessibility");
            state.accessibility
        }

        fn request_accessibility_access(&mut self) -> bool {
            let mut state = self.0.borrow_mut();
            state.calls.push("request_accessibility");
            if state.grant_accessibility {
                state.accessibility = true;
            }
            state.grant_accessibility
        }
    }

    fn provider(
        state: Rc<RefCell<FakeState>>,
        diagnostics: Rc<RefCell<Vec<String>>>,
    ) -> MacOsCapabilities {
        MacOsCapabilities {
            api: Box::new(FakeApi(state)),
            diagnostic: Box::new(move |line| diagnostics.borrow_mut().push(line.to_owned())),
        }
    }

    fn desktop_context() -> CapabilityContext {
        CapabilityContext {
            controller_input_enabled: true,
            desktop_bindings_enabled: true,
            desktop_session: Some(DesktopSession::MacOs),
            ..CapabilityContext::default()
        }
    }

    #[test]
    fn requirements_follow_the_active_macos_features() {
        let state = Rc::new(RefCell::new(FakeState::default()));
        let provider = provider(state, Rc::new(RefCell::new(Vec::new())));

        assert!(provider
            .requirements(&CapabilityContext::default())
            .is_empty());
        assert_eq!(
            provider.requirements(&CapabilityContext {
                controller_input_enabled: true,
                ..CapabilityContext::default()
            }),
            [RequirementGroup::Ordered(vec![
                CapabilityId::InputMonitoring,
            ])]
        );
        assert_eq!(
            provider.requirements(&desktop_context()),
            [RequirementGroup::Ordered(vec![
                CapabilityId::InputMonitoring,
                CapabilityId::PostEvent,
                CapabilityId::Accessibility,
            ])]
        );
    }

    #[test]
    fn probing_an_ordered_chain_never_invokes_a_request_or_later_service() {
        let state = Rc::new(RefCell::new(FakeState::default()));
        let provider = provider(Rc::clone(&state), Rc::new(RefCell::new(Vec::new())));

        let report = evaluate_requirements(&provider, &desktop_context());

        assert_eq!(report.unsatisfied[0].id, CapabilityId::InputMonitoring);
        assert_eq!(state.borrow().calls, ["probe_input_monitoring"]);
    }

    #[test]
    fn the_three_native_requests_keep_their_exact_diagnostic_strings() {
        let state = Rc::new(RefCell::new(FakeState {
            input_monitoring: InputMonitoringState::Denied,
            ..FakeState::default()
        }));
        let diagnostics = Rc::new(RefCell::new(Vec::new()));
        let mut provider = provider(Rc::clone(&state), Rc::clone(&diagnostics));

        assert!(matches!(
            provider.request(CapabilityId::InputMonitoring).unwrap(),
            CapabilityState::Blocked { .. }
        ));
        assert!(matches!(
            provider.request(CapabilityId::PostEvent).unwrap(),
            CapabilityState::Blocked { .. }
        ));
        assert!(matches!(
            provider.request(CapabilityId::Accessibility).unwrap(),
            CapabilityState::Blocked { .. }
        ));

        assert_eq!(
            diagnostics.borrow().as_slice(),
            [
                "level=info event=input_monitoring_permission_requested granted=false undecided=false api=IOHIDRequestAccess",
                "level=info event=post_event_permission_requested granted=false api=CGRequestPostEventAccess",
                "level=info event=accessibility_permission_requested granted=false api=AXIsProcessTrustedWithOptions",
            ]
        );
        assert!(!state.borrow().calls.contains(&"request_input_monitoring"));
    }

    #[test]
    fn request_flow_walks_input_monitoring_post_event_then_accessibility() {
        let state = Rc::new(RefCell::new(FakeState {
            grant_input_monitoring: true,
            grant_post_event: true,
            grant_accessibility: true,
            ..FakeState::default()
        }));
        let diagnostics = Rc::new(RefCell::new(Vec::new()));
        let mut provider = provider(Rc::clone(&state), diagnostics);

        for expected in [
            CapabilityId::InputMonitoring,
            CapabilityId::PostEvent,
            CapabilityId::Accessibility,
        ] {
            let outcome = request_next(&mut provider, &desktop_context())
                .unwrap()
                .unwrap();
            assert_eq!(outcome.id, expected);
            assert_eq!(outcome.current, CapabilityState::Satisfied);
        }
        assert!(request_next(&mut provider, &desktop_context())
            .unwrap()
            .is_none());

        let calls = &state.borrow().calls;
        let request_calls = calls
            .iter()
            .copied()
            .filter(|call| call.starts_with("request_"))
            .collect::<Vec<_>>();
        assert_eq!(
            request_calls,
            [
                "request_input_monitoring",
                "request_post_event",
                "request_accessibility",
            ]
        );
    }

    #[test]
    fn an_undecided_input_monitoring_request_remains_pending() {
        let state = Rc::new(RefCell::new(FakeState::default()));
        let diagnostics = Rc::new(RefCell::new(Vec::new()));
        let mut provider = provider(state, Rc::clone(&diagnostics));

        assert_eq!(
            provider.request(CapabilityId::InputMonitoring).unwrap(),
            CapabilityState::Pending
        );
        assert_eq!(
            diagnostics.borrow().as_slice(),
            ["level=info event=input_monitoring_permission_requested granted=false undecided=true api=IOHIDRequestAccess"]
        );
    }

    #[test]
    fn remedies_point_to_the_existing_distinct_system_settings_panes() {
        let provider = provider(
            Rc::new(RefCell::new(FakeState::default())),
            Rc::new(RefCell::new(Vec::new())),
        );
        let input = provider.remedy(CapabilityId::InputMonitoring);
        let accessibility = provider.remedy(CapabilityId::Accessibility);

        assert_ne!(input, accessibility);
        for remedy in [input, accessibility] {
            let Some(Remedy::OpenUrl(url)) = remedy else {
                panic!("macOS permission must have a System Settings remedy");
            };
            assert!(url.starts_with("x-apple.systempreferences:"));
        }
    }
}
