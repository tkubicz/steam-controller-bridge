#[allow(
    clippy::wildcard_imports,
    reason = "capability sequencing operates on the menu app's private runtime state"
)]
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PermissionAdvance {
    Ready,
    Waiting(CapabilityRequestOutcome),
}

pub(super) fn menu_capability_context(
    output: OutputPreference,
    virtual_hid_enabled: bool,
) -> CapabilityContext {
    let output = output.when_virtual_hid_enabled(virtual_hid_enabled);
    CapabilityContext {
        controller_input_enabled: true,
        serial_output_or_firmware_enabled: output == OutputPreference::BridgeDevice
            || cfg!(feature = "updater"),
        virtual_output_enabled: output == OutputPreference::VirtualHid,
        desktop_bindings_enabled: true,
        desktop_session: Some(DesktopSession::MacOs),
    }
}

pub(super) fn advance_permission_requirements(
    provider: &mut dyn PlatformCapabilities,
    context: &CapabilityContext,
) -> Result<PermissionAdvance, platform_capabilities::CapabilityError> {
    let mut completed = Vec::new();
    loop {
        let Some(outcome) = platform_capabilities::request_next(provider, context)? else {
            return Ok(PermissionAdvance::Ready);
        };
        if !outcome.current.is_met() {
            return Ok(PermissionAdvance::Waiting(outcome));
        }
        if completed.contains(&outcome.id) {
            return Err(platform_capabilities::CapabilityError::Request {
                id: outcome.id,
                reason: "request reported success but the capability remained unsatisfied"
                    .to_owned(),
            });
        }
        completed.push(outcome.id);
    }
}

pub(super) const fn should_open_remedy(
    interactive: bool,
    outcome: &CapabilityRequestOutcome,
) -> bool {
    interactive && !matches!(outcome.current, CapabilityState::Pending)
}

impl MenuApp {
    fn capability_context(&self) -> CapabilityContext {
        menu_capability_context(self.state.settings.output, self.state.virtual_hid_enabled)
    }

    /// Walks the active capability requirements, asking macOS for whatever is missing.
    ///
    /// `interactive` marks the run as something the user just asked for. Only
    /// then may this open a System Settings pane: macOS shows no dialog for a
    /// permission it has already recorded a refusal for, so the pane is the
    /// only way forward -- but opening it on every launch would be obnoxious.
    pub(super) fn request_permissions_in_order(&mut self, interactive: bool) {
        let context = self.capability_context();
        match advance_permission_requirements(self.state.capabilities.as_mut(), &context) {
            Ok(PermissionAdvance::Ready) => {
                self.state.permission_request_pending = None;
                self.activate_desktop_bindings_after_permission();
            }
            Ok(PermissionAdvance::Waiting(outcome)) => {
                self.state.permission_request_pending = Some(outcome.id);
                if should_open_remedy(interactive, &outcome) {
                    if let Some(remedy) = &outcome.remedy {
                        apply_capability_remedy(outcome.id, remedy);
                    }
                }
            }
            Err(error) => eprintln!("cannot request desktop permissions: {error}"),
        }
    }

    pub(super) fn observe_permission_grants(&mut self) {
        let Some(id) = self.state.permission_request_pending else {
            return;
        };
        if !self.state.capabilities.probe(id).is_met() {
            return;
        }

        self.state.permission_request_pending = None;
        match id {
            CapabilityId::InputMonitoring => {
                eprintln!("level=info event=input_monitoring_permission_granted");
                self.request_permissions_in_order(false);
            }
            CapabilityId::PostEvent => {
                eprintln!("level=info event=post_event_permission_granted");
                self.request_permissions_in_order(false);
            }
            CapabilityId::Accessibility => {
                eprintln!("level=info event=accessibility_permission_granted");
                self.activate_desktop_bindings_after_permission();
            }
            CapabilityId::ControllerHidAccess
            | CapabilityId::SerialPortAccess
            | CapabilityId::VirtualGamepadAccess
            | CapabilityId::DesktopInputAccess => {
                self.request_permissions_in_order(false);
            }
        }
    }

    pub(super) fn open_capability_settings(&self, id: CapabilityId) {
        if let Some(remedy) = self.state.capabilities.remedy(id) {
            apply_capability_remedy(id, &remedy);
        }
    }

    pub(super) fn activate_desktop_bindings_after_permission(&self) {
        if let Err(error) = self.state.runtime.request_enable_desktop_bindings() {
            eprintln!("cannot activate desktop bindings after Accessibility grant: {error}");
        }
    }
}
