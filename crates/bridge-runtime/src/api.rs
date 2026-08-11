use std::path::PathBuf;
use std::time::Duration;

use bridge_core::{BridgeConfig, BridgeMetrics};
use bridge_output::{DumpFormat, FirmwareVersion, OutputDiagnostics, SerialConfig};
use controller_mapper::MapperConfig;
use desktop_bindings::BindingProfile;
use profile_picker::{PickerConfig, PickerRoster};
use steam_controller_device::{masked_serial, ControllerTransport, HidDeviceInfo};

use crate::DEFAULT_IDLE_SHUTDOWN_TIMEOUT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerSelection {
    AutoActive,
    Index(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerialSelection {
    AutoXiao,
    Port(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LizardMode {
    Suppress,
    Leave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PuckDockAction {
    #[default]
    LeaveOn,
    PowerOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerChargeState {
    Discharging,
    Charging,
    Charged,
    Unknown(u8),
}

impl ControllerChargeState {
    #[must_use]
    pub const fn from_raw(value: u8) -> Self {
        match value {
            1 => Self::Discharging,
            2 => Self::Charging,
            4 => Self::Charged,
            other => Self::Unknown(other),
        }
    }

    #[must_use]
    pub const fn is_external_power(self) -> bool {
        matches!(self, Self::Charging | Self::Charged)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputSelection {
    Serial,
    Dump(DumpFormat),
    File(PathBuf),
    Mock,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub controller: ControllerSelection,
    pub serial: SerialSelection,
    pub output: OutputSelection,
    pub lizard_mode: LizardMode,
    pub bridge: BridgeConfig,
    pub mapper: MapperConfig,
    pub serial_config: SerialConfig,
    pub baud_rate: u32,
    pub recording_path: Option<PathBuf>,
    pub idle_shutdown_timeout: Option<Duration>,
    pub puck_dock_action: PuckDockAction,
    /// Optional desktop-input profile. `None` keeps injection completely disabled.
    pub binding_profile: Option<BindingProfile>,
    /// Optional in-game profile wheel. `None` leaves Quick Access alone entirely.
    pub profile_picker: Option<PickerConfig>,
    /// How many profiles the wheel can choose between, and which is active.
    ///
    /// The runtime never learns their names: it reports the chosen index and
    /// the frontend, which owns the profile store, resolves it.
    pub picker_roster: PickerRoster,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            controller: ControllerSelection::AutoActive,
            serial: SerialSelection::AutoXiao,
            output: OutputSelection::Serial,
            lizard_mode: LizardMode::Suppress,
            bridge: BridgeConfig::default(),
            mapper: MapperConfig::default(),
            serial_config: SerialConfig::default(),
            baud_rate: 115_200,
            recording_path: None,
            idle_shutdown_timeout: Some(DEFAULT_IDLE_SHUTDOWN_TIMEOUT),
            puck_dock_action: PuckDockAction::LeaveOn,
            binding_profile: None,
            profile_picker: None,
            picker_roster: PickerRoster::new(0, None),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    Stopped,
    Discovering,
    Waiting,
    Starting,
    Running,
    Stopping,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControllerSourceStatus {
    pub identity: Option<HidDeviceInfo>,
    pub transport: Option<ControllerTransport>,
    pub connected: bool,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControllerStatus {
    pub connected: bool,
    pub last_state_age: Option<Duration>,
}

#[derive(Clone, PartialEq, Eq, Default)]
pub struct XiaoStatus {
    pub path: Option<String>,
    pub usb_serial: Option<String>,
    pub handshake_complete: bool,
    pub firmware: FirmwareVersion,
}

/// Deliberately lossy for the same reason as [`HidDeviceInfo`]'s: this reaches
/// Copy Diagnostics through `{:?}`, and `usb_serial` is a stable hardware
/// identifier. Read the field directly when the real value is needed.
impl std::fmt::Debug for XiaoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XiaoStatus")
            .field("path", &self.path)
            .field("usb_serial", &masked_serial(self.usb_serial.as_deref()))
            .field("handshake_complete", &self.handshake_complete)
            .field("firmware", &self.firmware)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LizardStatus {
    pub suppressed: bool,
    pub refreshes: u64,
    pub failures: u64,
    pub last_refresh_age: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HapticsState {
    #[default]
    Idle,
    Active,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HapticsStatus {
    pub state: HapticsState,
    pub commands_received: u64,
    pub writes: u64,
    pub refreshes: u64,
    pub coalesced_commands: u64,
    pub failures: u64,
    pub last_command_age: Option<Duration>,
    pub pad_feedback_ticks: u64,
    pub pad_feedback_coalesced: u64,
    pub pad_feedback_failures: u64,
    pub last_pad_feedback_age: Option<Duration>,
    pub pad_feedback_last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DesktopBindingsState {
    #[default]
    Disabled,
    Ready,
    PermissionRequired,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DesktopBindingsStatus {
    pub state: DesktopBindingsState,
    pub active_profile_id: Option<String>,
    pub active_profile_name: Option<String>,
    pub configured_binding_count: usize,
    pub held_output_count: usize,
    pub failures: u64,
    pub last_error: Option<String>,
}

/// What the in-game profile wheel is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProfilePickerStatus {
    /// Whether a hold on Quick Access can open the wheel at all.
    pub enabled: bool,
    /// Whether the wheel is on screen and consuming controls.
    pub open: bool,
    /// Profiles the wheel can currently choose between.
    pub roster_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutomaticShutdownPhase {
    #[default]
    Disabled,
    Monitoring,
    PoweringOff,
    Sleeping,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownTrigger {
    IdleTimeout,
    PuckDock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutomaticShutdownStatus {
    pub configured_timeout: Option<Duration>,
    pub puck_dock_action: PuckDockAction,
    pub puck_dock_episode_handled: bool,
    pub neutral_idle_age: Option<Duration>,
    pub phase: AutomaticShutdownPhase,
    pub trigger: Option<ShutdownTrigger>,
    pub successful_shutdowns: u64,
    pub failures: u64,
    pub last_successful_shutdown_age: Option<Duration>,
    pub retry_after: Option<Duration>,
}

impl Default for AutomaticShutdownStatus {
    fn default() -> Self {
        Self {
            configured_timeout: Some(DEFAULT_IDLE_SHUTDOWN_TIMEOUT),
            puck_dock_action: PuckDockAction::LeaveOn,
            puck_dock_episode_handled: false,
            neutral_idle_age: None,
            phase: AutomaticShutdownPhase::Disabled,
            trigger: None,
            successful_shutdowns: 0,
            failures: 0,
            last_successful_shutdown_age: None,
            retry_after: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BridgeStatus {
    pub revision: u64,
    pub state: RuntimeState,
    pub detail: String,
    pub source: ControllerSourceStatus,
    pub controller: ControllerStatus,
    pub xiao: XiaoStatus,
    pub battery_percent: Option<u8>,
    pub battery_charge_state: Option<ControllerChargeState>,
    pub lizard: LizardStatus,
    pub haptics: HapticsStatus,
    pub bindings: DesktopBindingsStatus,
    pub profile_picker: ProfilePickerStatus,
    pub automatic_shutdown: AutomaticShutdownStatus,
    pub bridge_metrics: BridgeMetrics,
    pub output_diagnostics: OutputDiagnostics,
    pub last_error: Option<String>,
}

impl Default for BridgeStatus {
    fn default() -> Self {
        Self {
            revision: 0,
            state: RuntimeState::Stopped,
            detail: "Bridge stopped".to_owned(),
            source: ControllerSourceStatus::default(),
            controller: ControllerStatus::default(),
            xiao: XiaoStatus::default(),
            battery_percent: None,
            battery_charge_state: None,
            lizard: LizardStatus::default(),
            haptics: HapticsStatus::default(),
            bindings: DesktopBindingsStatus::default(),
            profile_picker: ProfilePickerStatus::default(),
            automatic_shutdown: AutomaticShutdownStatus::default(),
            bridge_metrics: BridgeMetrics::default(),
            output_diagnostics: OutputDiagnostics::default(),
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError(pub(crate) String);

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RuntimeError {}
