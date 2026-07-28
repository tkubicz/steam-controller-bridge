use bridge_runtime::{BridgeStatus, RuntimeState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuModel {
    pub bridge: String,
    pub puck: String,
    pub controller: String,
    pub xiao: String,
    pub battery: String,
    pub error: String,
    pub start_enabled: bool,
    pub stop_enabled: bool,
}

impl MenuModel {
    #[must_use]
    pub fn from_status(status: &BridgeStatus) -> Self {
        let puck = status.puck.identity.as_ref().map_or_else(
            || "Puck: Not detected".to_owned(),
            |info| {
                format!(
                    "Puck: {} (interface {})",
                    if status.puck.connected {
                        "Connected"
                    } else {
                        "Disconnected"
                    },
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
            puck,
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

    #[test]
    fn stopped_and_running_states_enable_the_right_actions() {
        let stopped = MenuModel::from_status(&BridgeStatus::default());
        assert!(stopped.start_enabled);
        assert!(!stopped.stop_enabled);

        let running = MenuModel::from_status(&BridgeStatus {
            state: RuntimeState::Running,
            detail: "Bridge running".to_owned(),
            battery_percent: Some(87),
            ..BridgeStatus::default()
        });
        assert!(!running.start_enabled);
        assert!(running.stop_enabled);
        assert_eq!(running.battery, "Battery: 87%");
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
