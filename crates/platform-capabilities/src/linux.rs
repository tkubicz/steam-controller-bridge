use crate::{
    CapabilityContext, CapabilityError, CapabilityId, CapabilityState, PlatformCapabilities,
    Remedy, RequirementGroup,
};

const DEVICE_RULE_NAME: &str = "60-steam-controller-bridge.rules";
const DEVICE_ACCESS_DOCUMENTATION: &str = concat!(
    env!("CARGO_PKG_REPOSITORY"),
    "/blob/main/packaging/linux/README.md"
);

trait LinuxAccessApi {
    fn controller_hid_access(&self) -> CapabilityState;
    fn bridge_device_access(&self) -> CapabilityState;
}

/// Linux capability provider for controller HID and bridge-device access.
pub struct LinuxCapabilities {
    api: Box<dyn LinuxAccessApi>,
}

impl LinuxCapabilities {
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn new() -> Self {
        Self {
            api: Box::new(NativeLinuxAccessApi::new()),
        }
    }
}

#[cfg(target_os = "linux")]
impl Default for LinuxCapabilities {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformCapabilities for LinuxCapabilities {
    fn requirements(&self, context: &CapabilityContext) -> Vec<RequirementGroup> {
        let mut independent = Vec::new();
        if context.controller_input_enabled {
            independent.push(CapabilityId::ControllerHidAccess);
        }
        if context.bridge_device_or_firmware_enabled {
            independent.push(CapabilityId::BridgeDeviceAccess);
        }
        if independent.is_empty() {
            Vec::new()
        } else {
            vec![RequirementGroup::Independent(independent)]
        }
    }

    fn probe(&self, id: CapabilityId) -> CapabilityState {
        match id {
            CapabilityId::ControllerHidAccess => self.api.controller_hid_access(),
            CapabilityId::BridgeDeviceAccess => self.api.bridge_device_access(),
            CapabilityId::VirtualGamepadAccess | CapabilityId::DesktopInputAccess => {
                CapabilityState::Unavailable {
                    reason: "the selected Linux provider is not implemented yet".to_owned(),
                }
            }
            CapabilityId::InputMonitoring
            | CapabilityId::PostEvent
            | CapabilityId::Accessibility => CapabilityState::NotRequired,
        }
    }

    fn request(&mut self, id: CapabilityId) -> Result<CapabilityState, CapabilityError> {
        Ok(self.probe(id))
    }

    fn remedy(&self, id: CapabilityId) -> Option<Remedy> {
        let text = match id {
            CapabilityId::ControllerHidAccess => format!(
                "For an active desktop session, install the narrowly matched {DEVICE_RULE_NAME} under /etc/udev/rules.d, or use the copy installed by your package under /usr/lib/udev/rules.d. Follow {DEVICE_ACCESS_DOCUMENTATION}. For a headless service, use the documented dedicated-group fallback. Reload udev rules and reconnect the controller, or restart the service after changing groups"
            ),
            CapabilityId::BridgeDeviceAccess => format!(
                "For an active desktop session with the official XIAO bridge, install the narrowly matched {DEVICE_RULE_NAME} under /etc/udev/rules.d, or use the copy installed by your package under /usr/lib/udev/rules.d. Follow {DEVICE_ACCESS_DOCUMENTATION}. For a third-party bridge, add an equally narrow rule for its USB identity or use the distribution's serial-access group. For a headless service, use the documented dedicated-group fallback with an exact device match. Reload udev rules and reconnect the bridge, or start a new login or restart the service after changing groups"
            ),
            CapabilityId::VirtualGamepadAccess
            | CapabilityId::DesktopInputAccess
            | CapabilityId::InputMonitoring
            | CapabilityId::PostEvent
            | CapabilityId::Accessibility => return None,
        };
        Some(Remedy::Instructions {
            text,
            command: None,
        })
    }
}

#[cfg(target_os = "linux")]
struct NativeLinuxAccessApi {
    controller: std::cell::RefCell<Option<steam_controller_device::ControllerEnumerator>>,
}

#[cfg(target_os = "linux")]
impl NativeLinuxAccessApi {
    fn new() -> Self {
        Self {
            controller: std::cell::RefCell::new(None),
        }
    }
}

#[cfg(target_os = "linux")]
impl LinuxAccessApi for NativeLinuxAccessApi {
    fn controller_hid_access(&self) -> CapabilityState {
        let mut controller = self.controller.borrow_mut();
        if controller.is_none() {
            match steam_controller_device::ControllerEnumerator::new() {
                Ok(enumerator) => *controller = Some(enumerator),
                Err(error) => {
                    return CapabilityState::Unavailable {
                        reason: format!(
                            "cannot initialize Linux controller HID enumeration: {error}"
                        ),
                    };
                }
            }
        }
        match controller
            .as_mut()
            .expect("controller enumerator was initialized")
            .enumerate()
        {
            Ok(devices) => access_state(
                devices.into_iter().map(|device| device.path),
                "controller HID endpoint",
                check_path_access,
            ),
            Err(error) => CapabilityState::Unavailable {
                reason: format!("cannot enumerate Linux controller HID endpoints: {error}"),
            },
        }
    }

    fn bridge_device_access(&self) -> CapabilityState {
        match bridge_output::available_bridge_endpoints() {
            Ok(endpoints) => access_state(
                endpoints
                    .into_iter()
                    .filter(bridge_output::BridgeEndpoint::is_bridge_device)
                    .filter_map(|endpoint| endpoint.serial_path().map(str::to_owned)),
                "bridge-device endpoint",
                check_path_access,
            ),
            Err(error) => CapabilityState::Unavailable {
                reason: format!("cannot enumerate Linux bridge-device endpoints: {error}"),
            },
        }
    }
}

#[cfg(target_os = "linux")]
fn check_path_access(path: &str) -> Result<(), PathAccessError> {
    use rustix::fs::{accessat, Access, AtFlags, CWD};
    use rustix::io::Errno;

    match accessat(
        CWD,
        path,
        Access::READ_OK | Access::WRITE_OK,
        AtFlags::EACCESS,
    ) {
        Ok(()) => Ok(()),
        Err(Errno::ACCESS | Errno::PERM) => Err(PathAccessError::Denied),
        Err(Errno::NOENT) => Err(PathAccessError::Missing),
        Err(error) => Err(PathAccessError::Other(error.to_string())),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathAccessError {
    Denied,
    Missing,
    Other(String),
}

fn access_state(
    paths: impl IntoIterator<Item = String>,
    device: &str,
    mut check: impl FnMut(&str) -> Result<(), PathAccessError>,
) -> CapabilityState {
    use std::collections::BTreeSet;

    let mut denied = None;
    let mut unavailable = None;
    for path in paths.into_iter().collect::<BTreeSet<_>>() {
        match check(&path) {
            Ok(()) => return CapabilityState::Satisfied,
            Err(PathAccessError::Denied) => {
                denied.get_or_insert(path);
            }
            Err(PathAccessError::Missing) => {}
            Err(PathAccessError::Other(error)) => {
                unavailable.get_or_insert((path, error));
            }
        }
    }

    if let Some(path) = denied {
        CapabilityState::Blocked {
            reason: format!("Linux {device} {path} is not readable and writable"),
        }
    } else if let Some((path, error)) = unavailable {
        CapabilityState::Unavailable {
            reason: format!("cannot inspect Linux {device} {path}: {error}"),
        }
    } else {
        CapabilityState::Satisfied
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::evaluate_requirements;

    use super::*;

    #[derive(Clone)]
    struct FakeApi {
        controller: CapabilityState,
        bridge: CapabilityState,
        probes: Rc<RefCell<Vec<CapabilityId>>>,
    }

    impl LinuxAccessApi for FakeApi {
        fn controller_hid_access(&self) -> CapabilityState {
            self.probes
                .borrow_mut()
                .push(CapabilityId::ControllerHidAccess);
            self.controller.clone()
        }

        fn bridge_device_access(&self) -> CapabilityState {
            self.probes
                .borrow_mut()
                .push(CapabilityId::BridgeDeviceAccess);
            self.bridge.clone()
        }
    }

    fn provider(
        controller: CapabilityState,
        bridge: CapabilityState,
    ) -> (LinuxCapabilities, Rc<RefCell<Vec<CapabilityId>>>) {
        let probes = Rc::new(RefCell::new(Vec::new()));
        (
            LinuxCapabilities {
                api: Box::new(FakeApi {
                    controller,
                    bridge,
                    probes: Rc::clone(&probes),
                }),
            },
            probes,
        )
    }

    #[test]
    fn requirements_follow_active_linux_hid_and_bridge_features() {
        let (provider, probes) = provider(CapabilityState::Satisfied, CapabilityState::Satisfied);

        assert!(provider
            .requirements(&CapabilityContext::default())
            .is_empty());
        assert_eq!(
            provider.requirements(&CapabilityContext {
                controller_input_enabled: true,
                ..CapabilityContext::default()
            }),
            [RequirementGroup::Independent(vec![
                CapabilityId::ControllerHidAccess,
            ])]
        );
        assert_eq!(
            provider.requirements(&CapabilityContext {
                bridge_device_or_firmware_enabled: true,
                ..CapabilityContext::default()
            }),
            [RequirementGroup::Independent(vec![
                CapabilityId::BridgeDeviceAccess,
            ])]
        );
        assert_eq!(
            provider.requirements(&CapabilityContext {
                controller_input_enabled: true,
                bridge_device_or_firmware_enabled: true,
                ..CapabilityContext::default()
            }),
            [RequirementGroup::Independent(vec![
                CapabilityId::ControllerHidAccess,
                CapabilityId::BridgeDeviceAccess,
            ])]
        );
        assert!(probes.borrow().is_empty());
    }

    #[test]
    fn independent_access_failures_are_both_reported() {
        let (provider, probes) = provider(
            CapabilityState::Blocked {
                reason: "hidraw denied".to_owned(),
            },
            CapabilityState::Blocked {
                reason: "bridge denied".to_owned(),
            },
        );
        let context = CapabilityContext {
            controller_input_enabled: true,
            bridge_device_or_firmware_enabled: true,
            ..CapabilityContext::default()
        };

        let report = evaluate_requirements(&provider, &context);

        assert_eq!(report.unsatisfied.len(), 2);
        assert_eq!(
            probes.borrow().as_slice(),
            [
                CapabilityId::ControllerHidAccess,
                CapabilityId::BridgeDeviceAccess,
            ]
        );
        assert!(report.unsatisfied.iter().all(|requirement| matches!(
            requirement.remedy,
            Some(Remedy::Instructions { command: None, .. })
        )));
        let Some(Remedy::Instructions { text, .. }) =
            provider.remedy(CapabilityId::BridgeDeviceAccess)
        else {
            panic!("bridge-device access must provide instructions");
        };
        assert!(!text.contains("rule in packaging/linux/"));
        assert!(text.contains("/etc/udev/rules.d"));
        assert!(text.contains("/usr/lib/udev/rules.d"));
        assert!(text.contains(DEVICE_ACCESS_DOCUMENTATION));
        assert!(text.contains("third-party bridge"));
        assert!(text.contains("serial-access group"));

        let Some(Remedy::Instructions { text, .. }) =
            provider.remedy(CapabilityId::ControllerHidAccess)
        else {
            panic!("controller HID access must provide instructions");
        };
        assert!(!text.contains("rule in packaging/linux/"));
        assert!(text.contains("/etc/udev/rules.d"));
        assert!(text.contains("/usr/lib/udev/rules.d"));
        assert!(text.contains(DEVICE_ACCESS_DOCUMENTATION));
    }

    #[test]
    fn request_rechecks_access_without_claiming_a_system_prompt() {
        let (mut provider, probes) =
            provider(CapabilityState::Satisfied, CapabilityState::Satisfied);

        assert_eq!(
            provider.request(CapabilityId::BridgeDeviceAccess).unwrap(),
            CapabilityState::Satisfied
        );
        assert_eq!(
            probes.borrow().as_slice(),
            [CapabilityId::BridgeDeviceAccess]
        );
        assert!(!matches!(
            provider.remedy(CapabilityId::BridgeDeviceAccess),
            Some(Remedy::RequestFromSystem)
        ));
    }

    #[test]
    fn endpoint_access_needs_one_usable_candidate_and_tolerates_absence() {
        assert_eq!(
            access_state(Vec::new(), "device", |_| unreachable!()),
            CapabilityState::Satisfied
        );
        assert_eq!(
            access_state(["missing".to_owned()], "device", |_| Err(
                PathAccessError::Missing
            )),
            CapabilityState::Satisfied
        );
        assert_eq!(
            access_state(
                ["denied".to_owned(), "usable".to_owned()],
                "device",
                |path| if path == "usable" {
                    Ok(())
                } else {
                    Err(PathAccessError::Denied)
                }
            ),
            CapabilityState::Satisfied
        );
    }

    #[test]
    fn endpoint_access_distinguishes_denial_from_probe_failure() {
        assert!(matches!(
            access_state(["denied".to_owned()], "device", |_| {
                Err(PathAccessError::Denied)
            }),
            CapabilityState::Blocked { reason } if reason.contains("denied")
        ));
        assert!(matches!(
            access_state(["broken".to_owned()], "device", |_| {
                Err(PathAccessError::Other("I/O error".to_owned()))
            }),
            CapabilityState::Unavailable { reason }
                if reason.contains("broken") && reason.contains("I/O error")
        ));
    }
}
