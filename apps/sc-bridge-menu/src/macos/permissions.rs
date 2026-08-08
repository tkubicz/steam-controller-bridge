#[allow(
    clippy::wildcard_imports,
    reason = "permission sequencing operates on the menu app's private runtime state"
)]
use super::*;

impl MenuApp {
    /// Walks the permission chain, asking macOS for whatever is missing.
    ///
    /// `interactive` marks the run as something the user just asked for. Only
    /// then may this open a System Settings pane: macOS shows no dialog for a
    /// permission it has already recorded a refusal for, so the pane is the
    /// only way forward -- but opening it on every launch would be obnoxious.
    pub(super) fn request_permissions_in_order(&mut self, interactive: bool) {
        // Ask macOS directly rather than inferring the grant from a controller
        // having opened: the two are different questions, and inferring it left
        // this doing nothing at all whenever no controller was attached.
        let input_monitoring = input_monitoring_access() == PermissionState::Granted;
        // Do not even query later TCC services until the preceding service is
        // granted. macOS can otherwise register simultaneous requests as
        // denied without presenting the original Input Monitoring prompt.
        let mut post_event = input_monitoring && preflight_post_event_access();
        let mut accessibility = post_event && preflight_accessibility_access();
        let mut input_monitoring_granted = input_monitoring;

        loop {
            match permission_stage(input_monitoring_granted, post_event, accessibility) {
                PermissionStage::InputMonitoring => {
                    // An undecided permission produces macOS's dialog. A
                    // refusal it already recorded produces nothing, so the
                    // only way forward is the settings pane.
                    let undecided = input_monitoring_access() == PermissionState::Undecided;
                    let granted = undecided && request_input_monitoring_access();
                    self.permission_request_pending =
                        (!granted).then_some(PermissionStage::InputMonitoring);
                    eprintln!(
                        "level=info event=input_monitoring_permission_requested \
                         granted={granted} undecided={undecided} api=IOHIDRequestAccess"
                    );
                    if !granted {
                        if interactive && !undecided {
                            open_privacy_pane(PrivacyPane::InputMonitoring);
                        }
                        return;
                    }
                    input_monitoring_granted = true;
                }
                PermissionStage::PostEvent => {
                    let granted = request_post_event_access();
                    self.permission_request_pending =
                        (!granted).then_some(PermissionStage::PostEvent);
                    eprintln!(
                        "level=info event=post_event_permission_requested granted={granted} \
                         api=CGRequestPostEventAccess"
                    );
                    if !granted {
                        if interactive {
                            open_privacy_pane(PrivacyPane::Accessibility);
                        }
                        return;
                    }
                    post_event = true;
                    accessibility = preflight_accessibility_access();
                }
                PermissionStage::Accessibility => {
                    let granted = request_accessibility_access();
                    self.permission_request_pending =
                        (!granted).then_some(PermissionStage::Accessibility);
                    eprintln!(
                        "level=info event=accessibility_permission_requested granted={granted} \
                         api=AXIsProcessTrustedWithOptions"
                    );
                    if !granted {
                        if interactive {
                            open_privacy_pane(PrivacyPane::Accessibility);
                        }
                        return;
                    }
                    accessibility = true;
                }
                PermissionStage::Ready => {
                    self.permission_request_pending = None;
                    self.activate_desktop_bindings_after_permission();
                    return;
                }
            }
        }
    }

    pub(super) fn observe_permission_grants(&mut self) {
        match self.permission_request_pending {
            Some(PermissionStage::InputMonitoring)
                if input_monitoring_access() == PermissionState::Granted =>
            {
                self.permission_request_pending = None;
                eprintln!("level=info event=input_monitoring_permission_granted");
                self.request_permissions_in_order(false);
            }
            Some(PermissionStage::PostEvent) if preflight_post_event_access() => {
                self.permission_request_pending = None;
                eprintln!("level=info event=post_event_permission_granted");
                self.request_permissions_in_order(false);
            }
            Some(PermissionStage::Accessibility) if preflight_accessibility_access() => {
                self.permission_request_pending = None;
                eprintln!("level=info event=accessibility_permission_granted");
                self.activate_desktop_bindings_after_permission();
            }
            _ => {}
        }
    }

    pub(super) fn activate_desktop_bindings_after_permission(&self) {
        if let Err(error) = self.runtime.request_enable_desktop_bindings() {
            eprintln!("cannot activate desktop bindings after Accessibility grant: {error}");
        }
    }
}
