use bridge_runtime::{BridgeStatus, RuntimeState};

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
            Self::Off => "Steam Controller Bridge — Off",
            Self::Waiting => "Steam Controller Bridge — On, waiting",
            Self::Ready => "Steam Controller Bridge — Controller ready",
            Self::Error => "Steam Controller Bridge — Action required",
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
    pub battery: String,
    pub haptics: String,
    pub problem: String,
    pub has_error: bool,
    pub tray_state: TrayState,
    pub start_enabled: bool,
    pub stop_enabled: bool,
}

impl MenuModel {
    #[must_use]
    pub fn from_status(status: &BridgeStatus) -> Self {
        let tray_state = tray_state(status);
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
            battery: status.battery_percent.map_or_else(
                || "Battery: Unknown".to_owned(),
                |percent| format!("Battery: {percent}%"),
            ),
            haptics: format!("Haptics: {:?}", status.haptics.state),
            problem: status.last_error.as_deref().map_or_else(
                || "Problem: None".to_owned(),
                |error| format!("Problem: {}", friendly_error(error)),
            ),
            has_error: status.last_error.is_some(),
            tray_state,
            start_enabled: matches!(status.state, RuntimeState::Stopped | RuntimeState::Error),
            stop_enabled: !matches!(status.state, RuntimeState::Stopped | RuntimeState::Stopping),
        }
    }
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
        assert!(stopped.start_enabled);
        assert!(!stopped.stop_enabled);

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
        assert!(!running.start_enabled);
        assert!(running.stop_enabled);
        assert_eq!(running.battery, "Battery: 87%");
        assert_eq!(running.haptics, "Haptics: Active");
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
            "Problem: Input Monitoring permission required"
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
    fn running_without_a_complete_path_is_visibly_waiting() {
        let mut status = ready_status(ControllerTransport::Bluetooth);
        status.xiao.handshake_complete = false;
        let model = MenuModel::from_status(&status);
        assert_eq!(model.tray_state, TrayState::Waiting);
        assert_eq!(model.status, "Status: Waiting for XIAO");
    }
}
