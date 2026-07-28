use bridge_runtime::{BridgeStatus, RuntimeState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuModel {
    pub bridge: String,
    pub input: String,
    pub controller: String,
    pub xiao: String,
    pub battery: String,
    pub haptics: String,
    pub error: String,
    pub start_enabled: bool,
    pub stop_enabled: bool,
}

impl MenuModel {
    #[must_use]
    pub fn from_status(status: &BridgeStatus) -> Self {
        let input = status.source.identity.as_ref().map_or_else(
            || "Input: Not detected".to_owned(),
            |info| {
                let state = if status.source.connected && status.source.active {
                    "Active"
                } else if status.source.connected {
                    "Waiting for reports"
                } else {
                    "Disconnected"
                };
                let transport = status
                    .source
                    .transport
                    .map_or_else(|| "Unknown".to_owned(), |value| value.to_string());
                let product = info.product.as_deref().unwrap_or("<unknown>");
                let serial = info.serial_number.as_deref().unwrap_or("no serial");
                format!(
                    "Input: {transport} — {state} ({product}, serial {serial}, interface {})",
                    info.interface_number
                )
            },
        );
        let xiao = status.xiao.path.as_ref().map_or_else(
            || "XIAO: Not detected".to_owned(),
            |path| {
                format!(
                    "XIAO: {} ({path})",
                    if status.xiao.handshake_complete {
                        "Ready"
                    } else {
                        "Handshaking"
                    }
                )
            },
        );
        Self {
            bridge: format!("Bridge: {:?} — {}", status.state, status.detail),
            input,
            controller: format!(
                "Controller: {}",
                if status.controller.connected {
                    "Connected"
                } else {
                    "Not connected"
                }
            ),
            xiao,
            battery: status.battery_percent.map_or_else(
                || "Battery: Unknown".to_owned(),
                |percent| format!("Battery: {percent}%"),
            ),
            haptics: format!("Haptics: {:?}", status.haptics.state),
            error: status.last_error.as_ref().map_or_else(
                || "Last error: None".to_owned(),
                |error| format!("Last error: {error}"),
            ),
            start_enabled: matches!(status.state, RuntimeState::Stopped | RuntimeState::Error),
            stop_enabled: !matches!(status.state, RuntimeState::Stopped | RuntimeState::Stopping),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_runtime::ControllerSourceStatus;
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

    #[test]
    fn stopped_and_running_states_enable_the_right_actions() {
        let stopped = MenuModel::from_status(&BridgeStatus::default());
        assert!(stopped.start_enabled);
        assert!(!stopped.stop_enabled);

        let running = MenuModel::from_status(&BridgeStatus {
            state: RuntimeState::Running,
            detail: "Bridge running".to_owned(),
            battery_percent: Some(87),
            haptics: bridge_runtime::HapticsStatus {
                state: bridge_runtime::HapticsState::Active,
                ..bridge_runtime::HapticsStatus::default()
            },
            ..BridgeStatus::default()
        });
        assert!(!running.start_enabled);
        assert!(running.stop_enabled);
        assert_eq!(running.battery, "Battery: 87%");
        assert_eq!(running.haptics, "Haptics: Active");
    }

    #[test]
    fn bluetooth_source_is_named_in_the_menu() {
        let model = MenuModel::from_status(&BridgeStatus {
            source: source_status(
                0x1303,
                -1,
                ControllerTransport::Bluetooth,
                "Steam Ctrl (BT)",
            ),
            ..BridgeStatus::default()
        });
        assert_eq!(
            model.input,
            "Input: Bluetooth — Active (Steam Ctrl (BT), serial redacted, interface -1)"
        );
    }

    #[test]
    fn puck_source_remains_named_in_the_menu() {
        let model = MenuModel::from_status(&BridgeStatus {
            source: source_status(
                0x1304,
                2,
                ControllerTransport::Puck,
                "Steam Controller Puck",
            ),
            ..BridgeStatus::default()
        });
        assert_eq!(
            model.input,
            "Input: Puck — Active (Steam Controller Puck, serial redacted, interface 2)"
        );
    }

    #[test]
    fn unknown_battery_and_actionable_errors_are_visible() {
        let model = MenuModel::from_status(&BridgeStatus {
            state: RuntimeState::Error,
            last_error: Some("Quit Steam and retry".to_owned()),
            ..BridgeStatus::default()
        });
        assert_eq!(model.battery, "Battery: Unknown");
        assert_eq!(model.error, "Last error: Quit Steam and retry");
        assert!(model.start_enabled);
    }
}
