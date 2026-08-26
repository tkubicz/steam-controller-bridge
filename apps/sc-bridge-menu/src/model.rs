use std::borrow::Cow;

use bridge_runtime::{
    AutomaticShutdownPhase, BridgeStatus, DesktopBindingsState, FirmwareTarget, FirmwareVersion,
    OutputBackend, PuckDockAction, RuntimeState, BRIDGE_BUSY_ERROR_MARKER,
};
use desktop_bindings::BindingStore;
use platform_capabilities::CapabilityId;

use crate::app_center_protocol::{AppCenterPage, FirmwareDetails};
use crate::app_state::{
    AppSettings, AppState, IdleShutdownChoice, OutputPreference, OVERLAY_HOLD_CHOICES,
};

const MAX_PROBLEM_CHARS: usize = 48;
const RUN_TOGGLE_ID: &str = "run-toggle";
const COPY_ERROR_ID: &str = "copy-error";
const COPY_DIAGNOSTICS_ID: &str = "copy-diagnostics";
const INPUT_MONITORING_ID: &str = "input-monitoring";
const ACCESSIBILITY_ID: &str = "accessibility";
const CONTROLLER_HID_ACCESS_ID: &str = "capability-controller-hid-access";
const BRIDGE_DEVICE_ACCESS_ID: &str = "capability-bridge-device-access";
const VIRTUAL_GAMEPAD_ACCESS_ID: &str = "capability-virtual-gamepad-access";
const DESKTOP_INPUT_ACCESS_ID: &str = "capability-desktop-input-access";
const POST_EVENT_ID: &str = "capability-post-event";
const ENABLE_BINDINGS_ID: &str = "enable-bindings";
const EDIT_BINDINGS_ID: &str = "edit-bindings";
const BINDING_PROFILE_PREFIX: &str = "binding-profile:";
const OPEN_LOGS_ID: &str = "open-logs";
const ABOUT_ID: &str = "about";
const CHANGELOG_ID: &str = "changelog";
const UPDATES_ID: &str = "updates";
const QUIT_ID: &str = "quit";
const IDLE_NEVER_ID: &str = "idle-never";
const IDLE_5_ID: &str = "idle-5";
const IDLE_10_ID: &str = "idle-10";
const IDLE_15_ID: &str = "idle-15";
const IDLE_30_ID: &str = "idle-30";
const PUCK_DOCK_ID: &str = "puck-dock-power-off";
const OUTPUT_BRIDGE_DEVICE_ID: &str = "output-bridge-device";
const OUTPUT_VIRTUAL_HID_ID: &str = "output-virtual-hid";
const OVERLAY_ENABLED_ID: &str = "profile-overlay-enabled";
const OVERLAY_HOLD_PREFIX: &str = "profile-overlay-hold:";

macro_rules! capability_action_mappings {
    ($($capability:path => $action_id:ident),+ $(,)?) => {
        fn capability_action_id(id: CapabilityId) -> &'static str {
            match id {
                $($capability => $action_id,)+
            }
        }

        fn capability_action_from_id(id: &str) -> Option<CapabilityId> {
            match id {
                $($action_id => Some($capability),)+
                _ => None,
            }
        }
    };
}

capability_action_mappings! {
    CapabilityId::ControllerHidAccess => CONTROLLER_HID_ACCESS_ID,
    CapabilityId::BridgeDeviceAccess => BRIDGE_DEVICE_ACCESS_ID,
    CapabilityId::VirtualGamepadAccess => VIRTUAL_GAMEPAD_ACCESS_ID,
    CapabilityId::DesktopInputAccess => DESKTOP_INPUT_ACCESS_ID,
    CapabilityId::InputMonitoring => INPUT_MONITORING_ID,
    CapabilityId::PostEvent => POST_EVENT_ID,
    CapabilityId::Accessibility => ACCESSIBILITY_ID,
}

impl IdleShutdownChoice {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Never => "Never",
            Self::FiveMinutes => "5 minutes",
            Self::TenMinutes => "10 minutes",
            Self::FifteenMinutes => "15 minutes",
            Self::ThirtyMinutes => "30 minutes",
        }
    }

    const fn id(self) -> &'static str {
        match self {
            Self::Never => IDLE_NEVER_ID,
            Self::FiveMinutes => IDLE_5_ID,
            Self::TenMinutes => IDLE_10_ID,
            Self::FifteenMinutes => IDLE_15_ID,
            Self::ThirtyMinutes => IDLE_30_ID,
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            IDLE_NEVER_ID => Some(Self::Never),
            IDLE_5_ID => Some(Self::FiveMinutes),
            IDLE_10_ID => Some(Self::TenMinutes),
            IDLE_15_ID => Some(Self::FifteenMinutes),
            IDLE_30_ID => Some(Self::ThirtyMinutes),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileOverlayHoldChoice(u64);

impl ProfileOverlayHoldChoice {
    pub const ALL: [Self; 2] = [Self(OVERLAY_HOLD_CHOICES[0]), Self(OVERLAY_HOLD_CHOICES[1])];

    #[must_use]
    pub const fn milliseconds(self) -> u64 {
        self.0
    }

    fn from_milliseconds(milliseconds: u64) -> Option<Self> {
        OVERLAY_HOLD_CHOICES
            .contains(&milliseconds)
            .then_some(Self(milliseconds))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    ToggleRun,
    SetIdleShutdown(IdleShutdownChoice),
    TogglePuckDockPowerOff,
    SelectOutput(OutputPreference),
    CopyFullError,
    CopyDiagnostics,
    OpenCapabilitySettings(CapabilityId),
    RequestDesktopPermissions,
    EditBindingProfiles,
    OpenLogs,
    OpenWindow(AppCenterPage),
    Quit,
    ToggleProfileOverlay,
    SelectBindingProfile(String),
    SetProfileOverlayHold(ProfileOverlayHoldChoice),
}

impl MenuAction {
    #[must_use]
    pub fn id(&self) -> Cow<'_, str> {
        let id = match self {
            Self::ToggleRun => RUN_TOGGLE_ID,
            Self::SetIdleShutdown(choice) => choice.id(),
            Self::TogglePuckDockPowerOff => PUCK_DOCK_ID,
            Self::SelectOutput(OutputPreference::BridgeDevice) => OUTPUT_BRIDGE_DEVICE_ID,
            Self::SelectOutput(OutputPreference::VirtualHid) => OUTPUT_VIRTUAL_HID_ID,
            Self::CopyFullError => COPY_ERROR_ID,
            Self::CopyDiagnostics => COPY_DIAGNOSTICS_ID,
            Self::OpenCapabilitySettings(id) => capability_action_id(*id),
            Self::RequestDesktopPermissions => ENABLE_BINDINGS_ID,
            Self::EditBindingProfiles => EDIT_BINDINGS_ID,
            Self::OpenLogs => OPEN_LOGS_ID,
            Self::OpenWindow(AppCenterPage::About) => ABOUT_ID,
            Self::OpenWindow(AppCenterPage::Changelog) => CHANGELOG_ID,
            Self::OpenWindow(AppCenterPage::Updates) => UPDATES_ID,
            Self::Quit => QUIT_ID,
            Self::ToggleProfileOverlay => OVERLAY_ENABLED_ID,
            Self::SelectBindingProfile(profile_id) => {
                return Cow::Owned(Self::id_for_binding_profile(profile_id));
            }
            Self::SetProfileOverlayHold(choice) => {
                return Cow::Owned(format!("{OVERLAY_HOLD_PREFIX}{}", choice.milliseconds()));
            }
        };
        Cow::Borrowed(id)
    }

    #[must_use]
    pub fn id_for_binding_profile(profile_id: &str) -> String {
        format!("{BINDING_PROFILE_PREFIX}{profile_id}")
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        if let Some(choice) = IdleShutdownChoice::from_id(id) {
            return Some(Self::SetIdleShutdown(choice));
        }
        if let Some(capability) = capability_action_from_id(id) {
            return Some(Self::OpenCapabilitySettings(capability));
        }
        match id {
            RUN_TOGGLE_ID => Some(Self::ToggleRun),
            PUCK_DOCK_ID => Some(Self::TogglePuckDockPowerOff),
            OUTPUT_BRIDGE_DEVICE_ID => Some(Self::SelectOutput(OutputPreference::BridgeDevice)),
            OUTPUT_VIRTUAL_HID_ID => Some(Self::SelectOutput(OutputPreference::VirtualHid)),
            COPY_ERROR_ID => Some(Self::CopyFullError),
            COPY_DIAGNOSTICS_ID => Some(Self::CopyDiagnostics),
            ENABLE_BINDINGS_ID => Some(Self::RequestDesktopPermissions),
            EDIT_BINDINGS_ID => Some(Self::EditBindingProfiles),
            OPEN_LOGS_ID => Some(Self::OpenLogs),
            ABOUT_ID if app_center_available() => Some(Self::OpenWindow(AppCenterPage::About)),
            CHANGELOG_ID if app_center_available() => {
                Some(Self::OpenWindow(AppCenterPage::Changelog))
            }
            UPDATES_ID if app_center_available() => Some(Self::OpenWindow(AppCenterPage::Updates)),
            QUIT_ID => Some(Self::Quit),
            OVERLAY_ENABLED_ID => Some(Self::ToggleProfileOverlay),
            _ => id
                .strip_prefix(BINDING_PROFILE_PREFIX)
                .map(|profile_id| Self::SelectBindingProfile(profile_id.to_owned()))
                .or_else(|| {
                    let milliseconds = id.strip_prefix(OVERLAY_HOLD_PREFIX)?.parse().ok()?;
                    ProfileOverlayHoldChoice::from_milliseconds(milliseconds)
                        .map(Self::SetProfileOverlayHold)
                }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingProfileChoice<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownControlsModel {
    pub idle_shutdown_minutes: Option<u64>,
    pub power_off_on_puck: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputControlsModel {
    pub selected: OutputPreference,
    pub output_change_pending: bool,
    pub virtual_hid_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileControlsModel<'a> {
    binding_store: &'a BindingStore,
    selected_binding_profile_id: &'a str,
    pub profile_overlay_enabled: bool,
    pub profile_overlay_hold_ms: u64,
}

impl ProfileControlsModel<'_> {
    pub fn binding_profiles(&self) -> impl ExactSizeIterator<Item = BindingProfileChoice<'_>> {
        self.binding_store
            .profiles
            .iter()
            .map(|profile| BindingProfileChoice {
                id: &profile.id,
                label: &profile.name,
                selected: profile
                    .id
                    .eq_ignore_ascii_case(self.selected_binding_profile_id),
            })
    }

    #[must_use]
    pub const fn selected_binding_profile_id(&self) -> &str {
        self.selected_binding_profile_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowControlsModel {
    pub app_center_available: bool,
    pub update_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayControlsModel<'a> {
    pub shutdown: ShutdownControlsModel,
    pub output: OutputControlsModel,
    pub profiles: ProfileControlsModel<'a>,
    pub window: WindowControlsModel,
}

impl<'a> TrayControlsModel<'a> {
    #[must_use]
    pub fn from_state(state: &'a AppState) -> Self {
        #[cfg(feature = "updater")]
        let update_available = state.last_update_available.unwrap_or(false);
        #[cfg(not(feature = "updater"))]
        let update_available = false;
        Self::from_parts(
            &state.settings,
            &state.binding_store,
            state.virtual_hid_enabled,
            state.output_change.is_some(),
            update_available,
        )
    }

    fn from_parts(
        settings: &'a AppSettings,
        binding_store: &'a BindingStore,
        virtual_hid_enabled: bool,
        output_change_pending: bool,
        update_available: bool,
    ) -> Self {
        Self {
            shutdown: ShutdownControlsModel {
                idle_shutdown_minutes: settings.idle_shutdown_minutes,
                power_off_on_puck: settings.power_off_on_puck,
            },
            output: OutputControlsModel {
                selected: settings.output,
                output_change_pending,
                virtual_hid_enabled,
            },
            profiles: ProfileControlsModel {
                binding_store,
                selected_binding_profile_id: &settings.active_binding_profile,
                profile_overlay_enabled: settings.profile_overlay_enabled,
                profile_overlay_hold_ms: settings.profile_overlay_hold_ms,
            },
            window: WindowControlsModel {
                app_center_available: app_center_available(),
                update_available: app_center_available() && update_available,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowModel {
    pub firmware: FirmwareDetails,
}

impl WindowModel {
    #[must_use]
    pub fn from_status(status: &BridgeStatus) -> Self {
        Self {
            firmware: FirmwareDetails::from_output(
                status.output.capabilities.firmware,
                status.output.firmware,
            ),
        }
    }
}

#[must_use]
pub const fn app_center_available() -> bool {
    cfg!(feature = "updater")
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareRowVisibility {
    pub section: bool,
    pub firmware: bool,
    pub controller_details: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareStatusRow {
    Input,
    Output,
    Firmware,
    Controller,
    Battery,
    Haptics,
}

#[must_use]
pub fn hardware_status_rows(visibility: HardwareRowVisibility) -> Vec<HardwareStatusRow> {
    if !visibility.section {
        return Vec::new();
    }
    let mut rows = vec![HardwareStatusRow::Input, HardwareStatusRow::Output];
    if visibility.firmware {
        rows.push(HardwareStatusRow::Firmware);
    }
    rows.push(HardwareStatusRow::Controller);
    if visibility.controller_details {
        rows.extend([HardwareStatusRow::Battery, HardwareStatusRow::Haptics]);
    }
    rows
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuModel {
    pub bridge: String,
    pub status: String,
    pub input: String,
    pub controller: String,
    pub output: String,
    pub firmware: String,
    pub battery: String,
    pub haptics: String,
    pub hardware_rows: HardwareRowVisibility,
    pub current_profile: String,
    pub automatic_shutdown: String,
    pub problem: String,
    pub has_error: bool,
    pub tray_state: TrayState,
    /// What the single run control does, which is also what it is labelled.
    pub run_action: RunAction,
    pub run_enabled: bool,
    /// Set when desktop bindings need an operating-system capability.
    pub permission_required: bool,
}

impl MenuModel {
    #[must_use]
    pub fn from_status(status: &BridgeStatus) -> Self {
        let tray_state = tray_state(status);
        let starts = matches!(status.state, RuntimeState::Stopped | RuntimeState::Error);
        let controller_present = status.controller.connected && status.source.active;
        Self {
            bridge: bridge_label(status.state),
            status: status_label(status, tray_state),
            input: input_label(status),
            controller: format!(
                "Controller: {}",
                if controller_present {
                    "Connected"
                } else {
                    "Not connected"
                }
            ),
            output: output_label(status),
            firmware: firmware_label(status),
            battery: status.battery_percent.map_or_else(
                || "Battery: Unknown".to_owned(),
                |percent| format!("Battery: {percent}%"),
            ),
            haptics: format!("Haptics: {:?}", status.haptics.state),
            hardware_rows: HardwareRowVisibility {
                section: !starts,
                firmware: status.output.firmware.is_some(),
                controller_details: controller_present,
            },
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

fn firmware_label(status: &BridgeStatus) -> String {
    let Some(firmware) = status.output.firmware else {
        return "Firmware: Not available".to_owned();
    };
    match firmware.version {
        FirmwareVersion::Reported(revision) => match firmware.target {
            FirmwareTarget::Reported(_) => format!("Firmware: rev {revision}"),
            FirmwareTarget::Unreported => format!("Firmware: rev {revision} · Unidentified"),
            FirmwareTarget::Malformed => {
                format!("{WARNING} Firmware: rev {revision} · Invalid target ID")
            }
        },
        FirmwareVersion::Unreported => "Firmware: Not reported".to_owned(),
        FirmwareVersion::UnsupportedFormat(_) => "Firmware: Newer than this app".to_owned(),
        FirmwareVersion::Malformed => format!("{WARNING} Firmware: Information invalid"),
        FirmwareVersion::Pending => "Firmware: Checking…".to_owned(),
    }
}

fn output_label(status: &BridgeStatus) -> String {
    let output = match status.output.backend {
        OutputBackend::BridgeDevice if status.output.ready => bridge_output_name(status),
        OutputBackend::BridgeDevice if status.output.endpoint.is_some() => "Connecting",
        OutputBackend::BridgeDevice => "Not Detected",
        OutputBackend::VirtualHid if status.output.ready => "Virtual Gamepad",
        OutputBackend::VirtualHid => "Virtual Gamepad (Not Ready)",
        OutputBackend::Dump => "Diagnostic Dump",
        OutputBackend::File => "File",
        OutputBackend::Mock => "Mock",
    };
    format!("Output: {output}")
}

#[cfg(feature = "updater")]
fn bridge_output_name(status: &BridgeStatus) -> &'static str {
    if let Some(name) = status
        .output
        .firmware
        .and_then(|firmware| match firmware.target {
            FirmwareTarget::Reported(identifier) => {
                release_updater::firmware_target(identifier.as_str())
                    .map(|target| target.compact_display_name.as_str())
            }
            FirmwareTarget::Unreported | FirmwareTarget::Malformed => None,
        })
    {
        return name;
    }
    "Bridge Device"
}

#[cfg(not(feature = "updater"))]
fn bridge_output_name(_status: &BridgeStatus) -> &'static str {
    "Bridge Device"
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
        && status.output.ready
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
        RuntimeState::Waiting if status.last_error.is_some() => "Action required",
        RuntimeState::Running | RuntimeState::Waiting if !status.output.ready => {
            "Waiting for bridge device"
        }
        RuntimeState::Running | RuntimeState::Waiting => "Waiting for controller",
    };
    format!("Status: {value}")
}

fn input_label(status: &BridgeStatus) -> String {
    status.source.transport.map_or_else(
        || "Input: Not Detected".to_owned(),
        |transport| format!("Input: {transport}"),
    )
}

fn friendly_error(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("updater suspension recovery") {
        return "Update cleanup delayed; Quit will retry".to_owned();
    }
    if lower.contains("e00002e2") {
        return "Input Monitoring permission required".to_owned();
    }
    if lower.contains("already owned") {
        return "Controller is already in use".to_owned();
    }
    if lower.contains(BRIDGE_BUSY_ERROR_MARKER) {
        return "Bridge device is busy".to_owned();
    }
    if lower.contains("multiple active steam controller") {
        return "Multiple active controllers found".to_owned();
    }
    if lower.contains("multiple valid bridge devices") {
        return "Multiple bridge devices found".to_owned();
    }
    if lower.contains("lizard-mode") {
        return "Controller safety setup failed".to_owned();
    }
    if lower.contains("automatic controller shutdown") {
        return "Controller could not be powered off".to_owned();
    }
    if lower.contains("hello handshake") || lower.contains("hello-handshake") {
        return "Bridge-device handshake failed".to_owned();
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
    use bridge_runtime::{ControllerSourceStatus, OutputStatus};
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
            output: OutputStatus {
                endpoint: Some("/dev/cu.usbmodem-example".to_owned()),
                stable_id: Some("redacted".to_owned()),
                ready: true,
                firmware: Some(bridge_runtime::FirmwareInfo {
                    version: FirmwareVersion::Reported(1),
                    target: FirmwareTarget::Reported(
                        bridge_runtime::FirmwareTargetId::new("seeed-xiao-nrf52840").unwrap(),
                    ),
                    ..bridge_runtime::FirmwareInfo::default()
                }),
                ..OutputStatus::configured(&bridge_runtime::OutputSelection::BridgeDevice)
            },
            ..BridgeStatus::default()
        }
    }

    #[test]
    fn stopped_and_ready_states_are_compact_and_enable_the_right_actions() {
        let stopped = MenuModel::from_status(&BridgeStatus::default());
        assert_eq!(stopped.bridge, "Bridge: Off");
        assert_eq!(stopped.status, "Status: Not running");
        assert_eq!(stopped.output, "Output: Not Detected");
        assert!(!stopped.hardware_rows.section);
        assert!(!stopped.hardware_rows.firmware);
        assert!(!stopped.hardware_rows.controller_details);
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
        #[cfg(feature = "updater")]
        assert_eq!(running.output, "Output: XIAO nRF52840");
        #[cfg(not(feature = "updater"))]
        assert_eq!(running.output, "Output: Bridge Device");
        assert!(running.hardware_rows.section);
        assert!(running.hardware_rows.firmware);
        assert!(running.hardware_rows.controller_details);
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
    fn menu_action_ids_round_trip_through_typed_actions() {
        let mut actions = vec![
            MenuAction::ToggleRun,
            MenuAction::SetIdleShutdown(IdleShutdownChoice::Never),
            MenuAction::SetIdleShutdown(IdleShutdownChoice::FifteenMinutes),
            MenuAction::TogglePuckDockPowerOff,
            MenuAction::SelectOutput(OutputPreference::BridgeDevice),
            MenuAction::SelectOutput(OutputPreference::VirtualHid),
            MenuAction::CopyFullError,
            MenuAction::CopyDiagnostics,
            MenuAction::RequestDesktopPermissions,
            MenuAction::EditBindingProfiles,
            MenuAction::OpenLogs,
            MenuAction::Quit,
            MenuAction::ToggleProfileOverlay,
            MenuAction::SelectBindingProfile("gaming".to_owned()),
            MenuAction::SetProfileOverlayHold(ProfileOverlayHoldChoice::ALL[0]),
        ];
        actions.extend(
            [
                CapabilityId::ControllerHidAccess,
                CapabilityId::BridgeDeviceAccess,
                CapabilityId::VirtualGamepadAccess,
                CapabilityId::DesktopInputAccess,
                CapabilityId::InputMonitoring,
                CapabilityId::PostEvent,
                CapabilityId::Accessibility,
            ]
            .map(MenuAction::OpenCapabilitySettings),
        );
        for action in actions {
            assert_eq!(MenuAction::from_id(action.id().as_ref()), Some(action));
        }

        for action in [
            MenuAction::OpenWindow(AppCenterPage::About),
            MenuAction::OpenWindow(AppCenterPage::Changelog),
            MenuAction::OpenWindow(AppCenterPage::Updates),
        ] {
            assert_eq!(
                MenuAction::from_id(action.id().as_ref()),
                app_center_available().then_some(action)
            );
        }
        assert_eq!(MenuAction::from_id("profile-overlay-hold:45000"), None);
        assert_eq!(MenuAction::from_id("not-a-menu-action"), None);
    }

    #[test]
    fn window_model_captures_current_firmware_availability() {
        let status = ready_status(ControllerTransport::Puck);
        let window = WindowModel::from_status(&status);
        assert!(window.firmware.available);
        assert_eq!(
            window.firmware.version,
            crate::app_center_protocol::FirmwareStatus::Reported(1)
        );

        let status = BridgeStatus {
            output: OutputStatus::configured(&bridge_runtime::OutputSelection::VirtualHid(
                bridge_runtime::VirtualHidConfig::new(std::path::PathBuf::from("helper")),
            )),
            ..BridgeStatus::default()
        };
        let window = WindowModel::from_status(&status);
        assert!(!window.firmware.available);
    }

    #[test]
    fn tray_controls_capture_settings_profiles_and_in_flight_state() {
        let mut store = BindingStore::default();
        let gaming_id = store.create_profile("Gaming").unwrap();
        let settings = AppSettings {
            idle_shutdown_minutes: None,
            power_off_on_puck: true,
            output: OutputPreference::VirtualHid,
            active_binding_profile: gaming_id.clone(),
            profile_overlay_enabled: true,
            profile_overlay_hold_ms: OVERLAY_HOLD_CHOICES[1],
            ..AppSettings::default()
        };

        let controls = TrayControlsModel::from_parts(&settings, &store, true, true, true);

        assert_eq!(controls.shutdown.idle_shutdown_minutes, None);
        assert!(controls.shutdown.power_off_on_puck);
        assert_eq!(controls.output.selected, OutputPreference::VirtualHid);
        assert!(controls.output.output_change_pending);
        assert!(controls.output.virtual_hid_enabled);
        assert_eq!(controls.profiles.binding_profiles().len(), 2);
        assert_eq!(
            controls
                .profiles
                .binding_profiles()
                .filter(|profile| profile.selected)
                .map(|profile| profile.id)
                .collect::<Vec<_>>(),
            [gaming_id.as_str()]
        );
        assert!(controls.profiles.profile_overlay_enabled);
        assert_eq!(
            controls.profiles.profile_overlay_hold_ms,
            OVERLAY_HOLD_CHOICES[1]
        );
        assert_eq!(controls.window.update_available, app_center_available());
        assert_eq!(controls.window.app_center_available, app_center_available());
    }

    #[test]
    fn optional_hardware_rows_have_the_requested_pipeline_order() {
        let hidden = HardwareRowVisibility {
            section: false,
            firmware: true,
            controller_details: true,
        };
        assert!(hardware_status_rows(hidden).is_empty());

        assert_eq!(
            hardware_status_rows(HardwareRowVisibility {
                section: true,
                firmware: false,
                controller_details: false,
            }),
            [
                HardwareStatusRow::Input,
                HardwareStatusRow::Output,
                HardwareStatusRow::Controller,
            ]
        );
        assert_eq!(
            hardware_status_rows(HardwareRowVisibility {
                section: true,
                firmware: true,
                controller_details: true,
            }),
            [
                HardwareStatusRow::Input,
                HardwareStatusRow::Output,
                HardwareStatusRow::Firmware,
                HardwareStatusRow::Controller,
                HardwareStatusRow::Battery,
                HardwareStatusRow::Haptics,
            ]
        );
    }

    #[test]
    fn virtual_hid_has_an_explicit_label_and_no_firmware_row() {
        let status = BridgeStatus {
            state: RuntimeState::Running,
            output: OutputStatus {
                ready: true,
                virtual_hid: Some(bridge_runtime::VirtualHidStatus {
                    protocol_version: 1,
                    dry_run: true,
                    ..bridge_runtime::VirtualHidStatus::default()
                }),
                ..OutputStatus::configured(&bridge_runtime::OutputSelection::VirtualHid(
                    bridge_runtime::VirtualHidConfig::new(std::path::PathBuf::from("helper")),
                ))
            },
            ..BridgeStatus::default()
        };
        let model = MenuModel::from_status(&status);
        assert_eq!(model.output, "Output: Virtual Gamepad");
        assert!(!model.hardware_rows.firmware);
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
    fn firmware_lines_report_identity_without_applying_target_update_policy() {
        let cases = [
            (FirmwareVersion::Pending, "Firmware: Checking…".to_owned()),
            (FirmwareVersion::Reported(2), "Firmware: rev 2".to_owned()),
            (
                FirmwareVersion::Unreported,
                "Firmware: Not reported".to_owned(),
            ),
            (
                FirmwareVersion::UnsupportedFormat(2),
                "Firmware: Newer than this app".to_owned(),
            ),
            (
                FirmwareVersion::Malformed,
                "⚠ Firmware: Information invalid".to_owned(),
            ),
        ];
        for (firmware, expected) in cases {
            let mut status = ready_status(ControllerTransport::Bluetooth);
            status.output.firmware.as_mut().unwrap().version = firmware;
            let model = MenuModel::from_status(&status);
            assert_eq!(model.firmware, expected);
            assert!(!model.has_error);
            assert_eq!(model.tray_state, TrayState::Ready);
        }
    }

    #[test]
    fn targetless_firmware_is_unidentified_without_an_update_prompt() {
        let mut status = ready_status(ControllerTransport::Bluetooth);
        let firmware = status.output.firmware.as_mut().unwrap();
        firmware.version = FirmwareVersion::Reported(2);
        firmware.target = FirmwareTarget::Unreported;
        let model = MenuModel::from_status(&status);
        assert_eq!(model.firmware, "Firmware: rev 2 · Unidentified");
        assert_eq!(model.output, "Output: Bridge Device");
        assert!(model.hardware_rows.firmware);
        assert!(!model.firmware.contains("Update"));
    }

    #[test]
    fn output_line_uses_plain_identity_or_discovery_copy_without_a_status_suffix() {
        let mut status = ready_status(ControllerTransport::Bluetooth);
        status.output.ready = false;
        assert_eq!(MenuModel::from_status(&status).output, "Output: Connecting");

        status.output.endpoint = None;
        assert_eq!(
            MenuModel::from_status(&status).output,
            "Output: Not Detected"
        );

        status.output.backend = OutputBackend::Dump;
        status.output.ready = true;
        assert_eq!(
            MenuModel::from_status(&status).output,
            "Output: Diagnostic Dump"
        );

        status.output.firmware = None;
        status.controller.connected = false;
        let unavailable = MenuModel::from_status(&status);
        assert!(!unavailable.hardware_rows.firmware);
        assert!(!unavailable.hardware_rows.controller_details);
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
        assert_eq!(bluetooth.input, "Input: Bluetooth");
        assert_eq!(bluetooth.controller, "Controller: Connected");

        let puck = MenuModel::from_status(&ready_status(ControllerTransport::Puck));
        assert_eq!(puck.input, "Input: Puck");
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
    fn generic_permission_errors_do_not_claim_a_macos_remedy() {
        assert_eq!(
            friendly_error("uinput: operation not permitted"),
            "uinput: operation not permitted"
        );
    }

    #[test]
    fn bridge_busy_errors_are_not_reported_as_controller_ownership() {
        assert_eq!(
            friendly_error(&format!(
                "{BRIDGE_BUSY_ERROR_MARKER} at endpoint; another process may own it"
            )),
            "Bridge device is busy"
        );
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
        status.output.ready = false;
        let model = MenuModel::from_status(&status);
        assert_eq!(model.tray_state, TrayState::Waiting);
        assert_eq!(model.status, "Status: Waiting for bridge device");
    }
}
