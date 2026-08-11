use bridge_runtime::{
    AutomaticShutdownPhase, BridgeStatus, DesktopBindingsState, FirmwareVersion, PuckDockAction,
    RuntimeState,
};

const MAX_PROBLEM_CHARS: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Off,
    Waiting,
    Ready,
    Error,
}

impl TrayState {
    #[must_use]
    pub const fn tooltip(self) -> &'static str {
        match self {
            Self::Off => "Steam Controller Bridge - Off",
            Self::Waiting => "Steam Controller Bridge - On, waiting",
            Self::Ready => "Steam Controller Bridge - Controller ready",
            Self::Error => "Steam Controller Bridge - Action required",
        }
    }
}

/// Marks the menu lines that need attention. A plain warning sign reads
/// correctly in a menu on every macOS version; muda cannot colour an
/// individual item's title.
pub const WARNING: &str = "⚠";

/// The bridge is either stopped, so the one run control starts it, or it is
/// not, so that control stops it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAction {
    Start,
    Stop,
}

impl RunAction {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start Bridge",
            Self::Stop => "Stop Bridge",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuModel {
    pub bridge: String,
    pub status: String,
    pub input: String,
    pub controller: String,
    pub xiao: String,
    pub firmware: String,
    pub battery: String,
    pub haptics: String,
    pub current_profile: String,
    pub automatic_shutdown: String,
    pub problem: String,
    pub has_error: bool,
    pub tray_state: TrayState,
    /// What the single run control does, which is also what it is labelled.
    pub run_action: RunAction,
    pub run_enabled: bool,
    /// Set when the bridge cannot type because macOS has not granted
    /// Accessibility, which the menu calls out rather than burying.
    pub permission_required: bool,
}

impl MenuModel {
    #[must_use]
    pub fn from_status(status: &BridgeStatus) -> Self {
        let tray_state = tray_state(status);
        let starts = matches!(status.state, RuntimeState::Stopped | RuntimeState::Error);
        Self {
            bridge: bridge_label(status.state),
            status: status_label(status, tray_state),
            input: input_label(status),
            controller: format!(
                "Controller: {}",
                if status.controller.connected && status.source.active {
                    "Connected"
                } else {
                    "Not connected"
                }
            ),
            xiao: format!(
                "XIAO: {}",
                if status.xiao.handshake_complete {
                    "Ready"
                } else if status.xiao.path.is_some() {
                    "Connecting"
                } else {
                    "Not detected"
                }
            ),
            firmware: firmware_label(status),
            battery: status.battery_percent.map_or_else(
                || "Battery: Unknown".to_owned(),
                |percent| format!("Battery: {percent}%"),
            ),
            haptics: format!("Haptics: {:?}", status.haptics.state),
            current_profile: current_profile_label(status),
            automatic_shutdown: automatic_shutdown_label(status),
            problem: status.last_error.as_deref().map_or_else(
                || "Problem: None".to_owned(),
                |error| format!("{WARNING} Problem: {}", friendly_error(error)),
            ),
            has_error: status.last_error.is_some(),
            tray_state,
            run_action: if starts {
                RunAction::Start
            } else {
                RunAction::Stop
            },
            run_enabled: starts || !matches!(status.state, RuntimeState::Stopping),
            permission_required: status.bindings.state == DesktopBindingsState::PermissionRequired,
        }
    }

    pub(crate) fn apply_external_error(&mut self, error: &str) {
        self.problem = format!("{WARNING} Problem: {}", friendly_error(error));
        self.has_error = true;
        self.tray_state = TrayState::Error;
        "Status: Action required".clone_into(&mut self.status);
    }
}

/// A warning here never flips the tray icon or the Problem line: the bridge
/// still works on old firmware, so the nudge stays inside the hardware block.
fn firmware_label(status: &BridgeStatus) -> String {
    let firmware = status.xiao.firmware;
    match firmware {
        FirmwareVersion::Reported(revision) if firmware.update_recommended() => {
            format!("{WARNING} Firmware: rev {revision} · Update recommended")
        }
        FirmwareVersion::Reported(revision) => format!("Firmware: rev {revision}"),
        FirmwareVersion::Unreported => format!("{WARNING} Firmware: Update recommended"),
        FirmwareVersion::UnsupportedFormat(_) => "Firmware: Newer than this app".to_owned(),
        FirmwareVersion::Malformed => format!("{WARNING} Firmware: Reflash recommended"),
        FirmwareVersion::Pending => "Firmware: Unknown".to_owned(),
    }
}

fn current_profile_label(status: &BridgeStatus) -> String {
    let profile = status
        .bindings
        .active_profile_name
        .as_deref()
        .unwrap_or("None");
    let state = match status.bindings.state {
        DesktopBindingsState::Disabled => "Disabled",
        DesktopBindingsState::Ready => "Ready",
        DesktopBindingsState::PermissionRequired => "Permission required",
        DesktopBindingsState::Degraded => "Degraded",
    };
    if status.bindings.state == DesktopBindingsState::PermissionRequired {
        return format!("{WARNING} Current Profile: {profile} · {state}");
    }
    format!("Current Profile: {profile} · {state}")
}

fn automatic_shutdown_label(status: &BridgeStatus) -> String {
    let automatic = status.automatic_shutdown;
    let value = match automatic.phase {
        AutomaticShutdownPhase::PoweringOff => "Powering off…".to_owned(),
        AutomaticShutdownPhase::Sleeping => "Controller sleeping".to_owned(),
        AutomaticShutdownPhase::Degraded => "Degraded".to_owned(),
        AutomaticShutdownPhase::Disabled => "Off".to_owned(),
        AutomaticShutdownPhase::Monitoring => automatic.configured_timeout.map_or_else(
            || {
                if automatic.puck_dock_action == PuckDockAction::PowerOff {
                    "On Puck".to_owned()
                } else {
                    "Off".to_owned()
                }
            },
            |timeout| {
                format!(
                    "Idle {} / {}{}",
                    format_duration(automatic.neutral_idle_age.unwrap_or_default()),
                    format_duration(timeout),
                    if automatic.puck_dock_action == PuckDockAction::PowerOff {
                        " · Puck"
                    } else {
                        ""
                    }
                )
            },
        ),
    };
    format!("Auto shutdown: {value}")
}

fn format_duration(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}:{seconds:02}")
}

fn tray_state(status: &BridgeStatus) -> TrayState {
    if status.last_error.is_some() || status.state == RuntimeState::Error {
        return TrayState::Error;
    }
    if matches!(status.state, RuntimeState::Stopped | RuntimeState::Stopping) {
        return TrayState::Off;
    }
    if status.state == RuntimeState::Running
        && status.source.active
        && status.controller.connected
        && status.xiao.handshake_complete
    {
        return TrayState::Ready;
    }
    TrayState::Waiting
}

fn bridge_label(state: RuntimeState) -> String {
    let value = match state {
        RuntimeState::Stopped => "Off",
        RuntimeState::Stopping => "Stopping…",
        RuntimeState::Error => "Error",
        RuntimeState::Discovering
        | RuntimeState::Waiting
        | RuntimeState::Starting
        | RuntimeState::Running => "On",
    };
    format!("Bridge: {value}")
}

fn status_label(status: &BridgeStatus, tray_state: TrayState) -> String {
    let value = match status.state {
        RuntimeState::Stopped => "Not running",
        RuntimeState::Stopping => "Stopping…",
        RuntimeState::Error => "Stopped after an error",
        RuntimeState::Starting => "Starting…",
        RuntimeState::Discovering => "Looking for hardware",
        RuntimeState::Running if tray_state == TrayState::Ready => "Ready",
        RuntimeState::Running if !status.xiao.handshake_complete => "Waiting for XIAO",
        RuntimeState::Waiting if status.last_error.is_some() => "Action required",
        RuntimeState::Waiting if status.detail.contains("XIAO") => "Waiting for XIAO",
        RuntimeState::Running | RuntimeState::Waiting => "Waiting for controller",
    };
    format!("Status: {value}")
}

fn input_label(status: &BridgeStatus) -> String {
    status.source.transport.map_or_else(
        || "Input: Not detected".to_owned(),
        |transport| {
            format!(
                "Input: {transport} · {}",
                if status.source.active {
                    "Active"
                } else {
                    "Waiting"
                }
            )
        },
    )
}

fn friendly_error(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("updater suspension recovery") {
        return "Update cleanup delayed; Quit will retry".to_owned();
    }
    if lower.contains("not permitted") || lower.contains("e00002e2") {
        return "Input Monitoring permission required".to_owned();
    }
    if lower.contains("already owned") {
        return "Controller is already in use".to_owned();
    }
    if lower.contains("device or resource busy") {
        return "XIAO serial port is busy".to_owned();
    }
    if lower.contains("multiple active steam controller") {
        return "Multiple active controllers found".to_owned();
    }
    if lower.contains("multiple valid xiao") || lower.contains("multiple xiao bridges") {
        return "Multiple XIAO bridges found".to_owned();
    }
    if lower.contains("lizard-mode") {
        return "Controller safety setup failed".to_owned();
    }
    if lower.contains("automatic controller shutdown") {
        return "Controller could not be powered off".to_owned();
    }
    if lower.contains("hello handshake") || lower.contains("hello-handshake") {
        return "XIAO firmware handshake failed".to_owned();
    }

    let first_clause = error
        .split(['\n', ';'])
        .next()
        .unwrap_or(error)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    truncate_chars(&first_clause, MAX_PROBLEM_CHARS)
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    let mut characters = value.chars();
    let prefix: String = characters.by_ref().take(maximum).collect();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_runtime::{ControllerSourceStatus, XiaoStatus};
    use steam_controller_device::{ControllerTransport, HidDeviceInfo};

    fn source_status(
        product_id: u16,
        interface_number: i32,
        transport: ControllerTransport,
        product: &str,
    ) -> ControllerSourceStatus {
        ControllerSourceStatus {
            identity: Some(HidDeviceInfo {
                id: "controller-source".to_owned(),
                path: "controller-source".to_owned(),
                vendor_id: 0x28de,
                product_id,
                usage_page: 0xff00,
                usage: 1,
                interface_number,
                serial_number: Some("redacted".to_owned()),
                manufacturer: Some("Valve Corporation".to_owned()),
                product: Some(product.to_owned()),
                transport: match transport {
                    ControllerTransport::Puck => "USB".to_owned(),
                    ControllerTransport::Bluetooth => "Bluetooth".to_owned(),
                },
            }),
            transport: Some(transport),
            connected: true,
            active: true,
        }
    }

    fn ready_status(transport: ControllerTransport) -> BridgeStatus {
        BridgeStatus {
            state: RuntimeState::Running,
            detail: "Bridge running".to_owned(),
            source: source_status(
                if transport == ControllerTransport::Puck {
                    0x1304
                } else {
                    0x1303
                },
                if transport == ControllerTransport::Puck {
                    2
                } else {
                    -1
                },
                transport,
                if transport == ControllerTransport::Puck {
                    "Steam Controller Puck"
                } else {
                    "Steam Ctrl (BT)"
                },
            ),
            controller: bridge_runtime::ControllerStatus {
                connected: true,
                last_state_age: Some(std::time::Duration::ZERO),
            },
            xiao: XiaoStatus {
                path: Some("/dev/cu.usbmodem-example".to_owned()),
                usb_serial: Some("redacted".to_owned()),
                handshake_complete: true,
                firmware: FirmwareVersion::Reported(1),
            },
            ..BridgeStatus::default()
        }
    }

    #[test]
    fn stopped_and_ready_states_are_compact_and_enable_the_right_actions() {
        let stopped = MenuModel::from_status(&BridgeStatus::default());
        assert_eq!(stopped.bridge, "Bridge: Off");
        assert_eq!(stopped.status, "Status: Not running");
        assert_eq!(stopped.tray_state, TrayState::Off);
        assert_eq!(stopped.run_action, RunAction::Start);
        assert_eq!(stopped.run_action.label(), "Start Bridge");
        assert!(stopped.run_enabled);

        let mut status = ready_status(ControllerTransport::Bluetooth);
        status.battery_percent = Some(87);
        status.haptics = bridge_runtime::HapticsStatus {
            state: bridge_runtime::HapticsState::Active,
            ..bridge_runtime::HapticsStatus::default()
        };
        let running = MenuModel::from_status(&status);
        assert_eq!(running.bridge, "Bridge: On");
        assert_eq!(running.status, "Status: Ready");
        assert_eq!(running.tray_state, TrayState::Ready);
        assert_eq!(running.run_action, RunAction::Stop);
        assert_eq!(running.run_action.label(), "Stop Bridge");
        assert!(running.run_enabled);
        assert_eq!(running.battery, "Battery: 87%");
        assert_eq!(running.haptics, "Haptics: Active");
        assert_eq!(running.current_profile, "Current Profile: None · Disabled");
        assert_eq!(running.automatic_shutdown, "Auto shutdown: Off");
    }

    #[test]
    fn the_run_control_reads_as_the_action_it_performs() {
        let state_of = |state| {
            let mut status = ready_status(ControllerTransport::Bluetooth);
            status.state = state;
            MenuModel::from_status(&status)
        };
        // Stopped or broken: the control offers to start, and can be used.
        for state in [RuntimeState::Stopped, RuntimeState::Error] {
            let model = state_of(state);
            assert_eq!(model.run_action, RunAction::Start, "{state:?}");
            assert!(model.run_enabled, "{state:?}");
        }
        // Already doing something: the control offers to stop.
        for state in [RuntimeState::Running, RuntimeState::Starting] {
            let model = state_of(state);
            assert_eq!(model.run_action, RunAction::Stop, "{state:?}");
            assert!(model.run_enabled, "{state:?}");
        }
        // Mid-stop there is nothing useful to ask for.
        let stopping = state_of(RuntimeState::Stopping);
        assert_eq!(stopping.run_action, RunAction::Stop);
        assert!(!stopping.run_enabled);
    }

    #[test]
    fn firmware_lines_warn_only_below_the_minimum_and_never_change_the_icon() {
        let minimum = bridge_runtime::MINIMUM_FIRMWARE_REVISION;
        let cases = [
            (FirmwareVersion::Pending, "Firmware: Unknown".to_owned()),
            (
                FirmwareVersion::Reported(minimum),
                format!("Firmware: rev {minimum}"),
            ),
            (
                FirmwareVersion::Unreported,
                "⚠ Firmware: Update recommended".to_owned(),
            ),
            (
                FirmwareVersion::UnsupportedFormat(2),
                "Firmware: Newer than this app".to_owned(),
            ),
            (
                FirmwareVersion::Malformed,
                "⚠ Firmware: Reflash recommended".to_owned(),
            ),
        ];
        for (firmware, expected) in cases {
            let mut status = ready_status(ControllerTransport::Bluetooth);
            status.xiao.firmware = firmware;
            let model = MenuModel::from_status(&status);
            assert_eq!(model.firmware, expected);
            assert_eq!(
                model.firmware.starts_with(WARNING),
                firmware.update_recommended()
            );
            // Old firmware still works: the nudge must not read as an error.
            assert!(!model.has_error);
            assert_eq!(model.tray_state, TrayState::Ready);
        }
    }

    #[test]
    fn an_outdated_reported_revision_names_the_revision_it_warns_about() {
        // Reachable once MINIMUM_FIRMWARE_REVISION exceeds 1; pinned here so
        // the label shape is already settled.
        let mut status = ready_status(ControllerTransport::Bluetooth);
        status.xiao.firmware = FirmwareVersion::Reported(0);
        let model = MenuModel::from_status(&status);
        if FirmwareVersion::Reported(0).update_recommended() {
            assert_eq!(model.firmware, "⚠ Firmware: rev 0 · Update recommended");
        }
    }

    #[test]
    fn a_problem_carries_the_warning_mark_and_no_problem_does_not() {
        let healthy = MenuModel::from_status(&ready_status(ControllerTransport::Bluetooth));
        assert_eq!(healthy.problem, "Problem: None");
        assert!(!healthy.problem.starts_with(WARNING));
        assert!(!healthy.has_error);

        let mut status = ready_status(ControllerTransport::Bluetooth);
        status.last_error = Some("something went wrong: internal detail".to_owned());
        let broken = MenuModel::from_status(&status);
        assert!(
            broken.problem.starts_with(WARNING),
            "a reported problem should be marked: {}",
            broken.problem,
        );
        assert!(broken.has_error);
    }

    #[test]
    fn a_missing_permission_is_called_out_rather_than_buried() {
        let mut status = ready_status(ControllerTransport::Bluetooth);
        for (state, flagged) in [
            (DesktopBindingsState::Ready, false),
            (DesktopBindingsState::Disabled, false),
            (DesktopBindingsState::Degraded, false),
            (DesktopBindingsState::PermissionRequired, true),
        ] {
            status.bindings = bridge_runtime::DesktopBindingsStatus {
                state,
                ..bridge_runtime::DesktopBindingsStatus::default()
            };
            let model = MenuModel::from_status(&status);
            assert_eq!(model.permission_required, flagged, "{state:?}");
            assert_eq!(
                model.current_profile.starts_with(WARNING),
                flagged,
                "the warning mark should appear only when a permission is missing: {state:?}",
            );
        }
    }

    #[test]
    fn binding_permission_and_profile_are_visible_without_marking_gamepad_failed() {
        let mut status = ready_status(ControllerTransport::Puck);
        status.bindings = bridge_runtime::DesktopBindingsStatus {
            state: DesktopBindingsState::PermissionRequired,
            active_profile_id: Some("gaming".to_owned()),
            active_profile_name: Some("Gaming".to_owned()),
            configured_binding_count: 2,
            last_error: Some("Accessibility permission required".to_owned()),
            ..bridge_runtime::DesktopBindingsStatus::default()
        };
        let model = MenuModel::from_status(&status);
        assert_eq!(
            model.current_profile,
            "⚠ Current Profile: Gaming · Permission required"
        );
        assert!(model.permission_required);
        assert_eq!(model.tray_state, TrayState::Ready);
        assert!(!model.has_error);
    }

    #[test]
    fn controller_transport_lines_are_short_and_do_not_expose_identity_details() {
        let bluetooth = MenuModel::from_status(&ready_status(ControllerTransport::Bluetooth));
        assert_eq!(bluetooth.input, "Input: Bluetooth · Active");
        assert_eq!(bluetooth.controller, "Controller: Connected");

        let puck = MenuModel::from_status(&ready_status(ControllerTransport::Puck));
        assert_eq!(puck.input, "Input: Puck · Active");
        assert!(!puck.input.contains("serial"));
        assert!(!puck.input.contains("interface"));
    }

    #[test]
    fn permission_failures_have_a_friendly_summary_and_error_icon() {
        let model = MenuModel::from_status(&BridgeStatus {
            state: RuntimeState::Waiting,
            last_error: Some(
                "index 58: HID backend failed: hid_open_path: (0xE00002E2) not permitted; \
                 more internal details"
                    .to_owned(),
            ),
            ..BridgeStatus::default()
        });
        assert_eq!(
            model.problem,
            "⚠ Problem: Input Monitoring permission required"
        );
        assert_eq!(model.status, "Status: Action required");
        assert_eq!(model.tray_state, TrayState::Error);
        assert!(model.has_error);
    }

    #[test]
    fn unknown_errors_are_bounded_without_corrupting_unicode() {
        let error = "Unrecognized failure ".to_owned() + &"ą".repeat(100);
        let summary = friendly_error(&error);
        assert!(summary.chars().count() <= MAX_PROBLEM_CHARS + 1);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn external_recovery_errors_override_a_healthy_runtime_model() {
        let mut model = MenuModel::from_status(&ready_status(ControllerTransport::Puck));

        model.apply_external_error("Updater suspension recovery timed out");

        assert_eq!(
            model.problem,
            "⚠ Problem: Update cleanup delayed; Quit will retry"
        );
        assert_eq!(model.status, "Status: Action required");
        assert_eq!(model.tray_state, TrayState::Error);
        assert!(model.has_error);
    }

    #[test]
    fn automatic_shutdown_state_is_compact_and_failures_are_friendly() {
        let mut status = ready_status(ControllerTransport::Puck);
        status.automatic_shutdown = bridge_runtime::AutomaticShutdownStatus {
            configured_timeout: Some(std::time::Duration::from_mins(15)),
            puck_dock_action: PuckDockAction::PowerOff,
            neutral_idle_age: Some(std::time::Duration::from_secs(65)),
            phase: AutomaticShutdownPhase::Monitoring,
            ..bridge_runtime::AutomaticShutdownStatus::default()
        };
        let monitoring = MenuModel::from_status(&status);
        assert_eq!(
            monitoring.automatic_shutdown,
            "Auto shutdown: Idle 1:05 / 15:00 · Puck"
        );

        status.automatic_shutdown.phase = AutomaticShutdownPhase::Degraded;
        status.last_error = Some(
            "automatic controller shutdown failed; gameplay continues: backend detail".to_owned(),
        );
        let degraded = MenuModel::from_status(&status);
        assert_eq!(degraded.automatic_shutdown, "Auto shutdown: Degraded");
        assert_eq!(
            degraded.problem,
            "⚠ Problem: Controller could not be powered off"
        );
    }

    #[test]
    fn running_without_a_complete_path_is_visibly_waiting() {
        let mut status = ready_status(ControllerTransport::Bluetooth);
        status.xiao.handshake_complete = false;
        let model = MenuModel::from_status(&status);
        assert_eq!(model.tray_state, TrayState::Waiting);
        assert_eq!(model.status, "Status: Waiting for XIAO");
    }
}
