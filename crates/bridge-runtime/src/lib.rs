//! Reusable live Steam Controller 2 bridge orchestration.
//!
//! The runtime deliberately keeps discovery separate from ownership: candidate
//! Puck and Bluetooth collections are only read during discovery, and the
//! lizard-mode feature command is sent only after exactly one active source has
//! been identified.

mod idle_shutdown;

use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bridge_core::{BridgeConfig, BridgeEngine, BridgeMetrics, ProcessOutcome};
use bridge_output::{
    available_serial_devices, DumpFormat, DumpOutput, FileOutput, GamepadOutput, MockOutput,
    OutputDiagnostics, OutputFeedback, SerialConfig, SerialDeviceInfo, SerialOutput,
};
use controller_mapper::MapperConfig;
use desktop_bindings::{
    bindable_mask, BindingEngine, BindingProfile, DesktopInputSink, DesktopInputSnapshot,
    PadFeedbackRequest, PadSample,
};
use gamepad_state::OutputSuppression;
#[cfg(target_os = "macos")]
use macos_power_monitor::{PowerEvent, PowerMonitor};
use profile_picker::{Picker, PickerEvents, PickerInput};
// Frontends drive the wheel and render it, so its vocabulary is part of the
// runtime's public surface.
pub use profile_picker::{PickerConfig, PickerEvent, PickerRoster};
use recording::{
    RecordingError, RecordingEvent, RecordingWriter, KIND_DEVICE_CONNECTED,
    KIND_DEVICE_DISCONNECTED,
};
use serde_json::json;
use steam_controller_device::{
    masked_serial, ControllerEnumerator, ControllerTransport, DeviceError, DeviceEvent,
    HidDeviceInfo, HidSession, LizardModeHeartbeat, RawHidReport,
};

// Frontends render status without depending on the device crate directly.
pub use steam_controller_device::masked_serial as mask_serial_for_display;
use steam_controller_protocol::{
    ConnectionState, DecodedReport, PadHapticGain, PadHapticSide, SteamButton, SteamButtons,
    SteamControllerDecoder, EXTENDED_INPUT_REPORT_ID, EXTENDED_INPUT_REPORT_SIZE, INPUT_REPORT_ID,
    INPUT_REPORT_SIZE,
};

mod status_log;

pub use status_log::{
    format_status_diagnostics, StatusLogChange, StatusLogLevel, StatusLogRecord,
    StatusLogRecordKind, StatusLogTracker, StatusSnapshotReason, STATUS_SNAPSHOT_INTERVAL,
};

use idle_shutdown::IdleActivityTracker;

const DISCOVERY_INTERVAL: Duration = Duration::from_millis(500);
/// The longest a `WillSleep` callback waits for the hardware teardown before
/// acknowledging the sleep anyway. Far above every bounded teardown step, and
/// safely under macOS's ~30-second forced-sleep cap.
const SLEEP_TEARDOWN_ACK_TIMEOUT: Duration = Duration::from_secs(25);
/// How long after a system wake before hardware discovery may reopen ports.
/// The XIAO's CDC interface re-enumerates for a couple of seconds after a
/// wake, and touching it mid-setup is the window the sleep suspension exists
/// to stay out of.
const WAKE_SETTLE_DELAY: Duration = Duration::from_secs(2);
const MIN_STABLE_CONTROLLER_SCAN_INTERVAL: Duration = Duration::from_secs(2);
const MAX_STABLE_CONTROLLER_SCAN_INTERVAL: Duration = Duration::from_secs(10);
const MAX_DISCOVERY_REPORTS_PER_CANDIDATE: usize = 4;
const ACTIVE_SLOT_TIMEOUT: Duration = Duration::from_secs(1);
const INPUT_MAILBOX_CAPACITY: usize = 64;
const DESKTOP_INPUT_MAILBOX_CAPACITY: usize = 64;
const DESKTOP_CONTROL_MAILBOX_CAPACITY: usize = 32;
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
const RUNTIME_POLL_INTERVAL: Duration = Duration::from_millis(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const SUPERVISOR_STALL_THRESHOLD: Duration = Duration::from_millis(50);
const RUMBLE_REFRESH_INTERVAL: Duration = Duration::from_millis(40);
const RUMBLE_LEASE_TIMEOUT: Duration = Duration::from_millis(100);
const RUMBLE_RETRY_INTERVAL: Duration = Duration::from_millis(500);
const PAD_FEEDBACK_RETRY_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_IDLE_SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(15);
const AUTOMATIC_SHUTDOWN_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const POWER_OFF_COOLDOWN: Duration = Duration::from_millis(2_500);
const BATTERY_STATUS_FRESHNESS: Duration = Duration::from_secs(30);
const POWER_OFF_BURST_WRITES: u8 = 3;
const POWER_OFF_BURST_INTERVAL: Duration = Duration::from_millis(10);
const MIN_IDLE_SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(1);
pub const MAX_IDLE_SHUTDOWN_TIMEOUT: Duration = Duration::from_hours(24);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SupervisorStall {
    elapsed: Duration,
    phase: &'static str,
    phase_elapsed: Duration,
}

struct SupervisorIterationTimer {
    started: Instant,
    phase_started: Instant,
    phase: &'static str,
    slowest_phase: &'static str,
    slowest_phase_elapsed: Duration,
    reported: bool,
}

impl SupervisorIterationTimer {
    fn new(initial_phase: &'static str) -> Self {
        Self::new_at(initial_phase, Instant::now())
    }

    fn new_at(initial_phase: &'static str, now: Instant) -> Self {
        Self {
            started: now,
            phase_started: now,
            phase: initial_phase,
            slowest_phase: initial_phase,
            slowest_phase_elapsed: Duration::ZERO,
            reported: false,
        }
    }

    fn enter(&mut self, phase: &'static str) {
        self.enter_at(phase, Instant::now());
    }

    fn enter_at(&mut self, phase: &'static str, now: Instant) {
        self.record_current_phase(now);
        self.phase = phase;
        self.phase_started = now;
    }

    fn take_stall_at(&mut self, now: Instant) -> Option<SupervisorStall> {
        if self.reported {
            return None;
        }
        self.record_current_phase(now);
        self.reported = true;
        let elapsed = now.saturating_duration_since(self.started);
        (elapsed >= SUPERVISOR_STALL_THRESHOLD).then_some(SupervisorStall {
            elapsed,
            phase: self.slowest_phase,
            phase_elapsed: self.slowest_phase_elapsed,
        })
    }

    fn record_current_phase(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.phase_started);
        if elapsed > self.slowest_phase_elapsed {
            self.slowest_phase = self.phase;
            self.slowest_phase_elapsed = elapsed;
        }
    }
}

impl Drop for SupervisorIterationTimer {
    fn drop(&mut self) {
        if let Some(stall) = self.take_stall_at(Instant::now()) {
            eprintln!(
                "level=warn event=supervisor_stall elapsed_ms={} phase={} phase_elapsed_ms={}",
                stall.elapsed.as_millis(),
                stall.phase,
                stall.phase_elapsed.as_millis()
            );
        }
    }
}

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

#[derive(Debug)]
pub struct RuntimeError(String);

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RuntimeError {}

type CommandAck = mpsc::Sender<Result<(), String>>;

enum RuntimeCommand {
    Start(CommandAck),
    Stop(CommandAck),
    Shutdown(CommandAck),
    SetIdleShutdown(Option<Duration>, CommandAck),
    SetPuckDockAction(PuckDockAction, CommandAck),
    SetBindingProfile(Box<Option<BindingProfile>>, CommandAck),
    EnableDesktopBindings(CommandAck),
    SetPickerConfig(Option<PickerConfig>, CommandAck),
    SetPickerRoster(PickerRoster, CommandAck),
    /// Park the device and close every hardware handle ahead of system sleep.
    SuspendForSleep(CommandAck),
    /// Let discovery run again after a system wake.
    ResumeFromWake(CommandAck),
}

/// Where picker events go. Called on the runtime thread, so it must not block.
pub type PickerEventSink = Box<dyn Fn(PickerEvent) + Send>;

pub struct BridgeRuntime;

impl BridgeRuntime {
    #[must_use]
    pub fn spawn(config: RuntimeConfig) -> BridgeHandle {
        Self::spawn_with_picker(config, Box::new(|_| {}))
    }

    /// Spawns the runtime and streams profile-wheel events to `picker_events`.
    ///
    /// The sink runs on the runtime thread between controller reports, so it
    /// must return promptly; hand the event to a channel and wake the UI rather
    /// than doing work inside it. The sink is separate from [`RuntimeConfig`]
    /// because that stays plain, cloneable data.
    #[must_use]
    pub fn spawn_with_picker(
        config: RuntimeConfig,
        picker_events: PickerEventSink,
    ) -> BridgeHandle {
        let status = Arc::new(Mutex::new(BridgeStatus {
            state: RuntimeState::Discovering,
            detail: "Starting bridge runtime".to_owned(),
            automatic_shutdown: AutomaticShutdownStatus {
                configured_timeout: config.idle_shutdown_timeout,
                puck_dock_action: config.puck_dock_action,
                phase: automatic_shutdown_phase(&config),
                ..AutomaticShutdownStatus::default()
            },
            bindings: binding_status_for_profile(config.binding_profile.as_ref()),
            profile_picker: picker_status(&config, false),
            ..BridgeStatus::default()
        }));
        let worker_status = Arc::clone(&status);
        let (command_sender, command_receiver) = mpsc::channel();
        #[cfg(target_os = "macos")]
        let (power_monitor, startup_blocker) = if config.output == OutputSelection::Serial {
            match power_monitor(Arc::clone(&status), command_sender.clone()) {
                Ok(monitor) => (Some(monitor), None),
                Err(error) => {
                    eprintln!("level=error event=system_power_monitor_failed error={error:?}");
                    (None, Some(error))
                }
            }
        } else {
            (None, None)
        };
        #[cfg(not(target_os = "macos"))]
        let startup_blocker = None;
        let join = thread::spawn(move || {
            let mut supervisor = Supervisor::new(
                config,
                worker_status,
                command_receiver,
                picker_events,
                startup_blocker,
            );
            supervisor.run();
        });
        BridgeHandle {
            command_sender,
            status,
            join: Mutex::new(Some(join)),
            #[cfg(target_os = "macos")]
            power_monitor: Mutex::new(power_monitor),
        }
    }
}

#[cfg(target_os = "macos")]
fn power_monitor(
    status: Arc<Mutex<BridgeStatus>>,
    commands: mpsc::Sender<RuntimeCommand>,
) -> Result<PowerMonitor, String> {
    PowerMonitor::new(move |event| match event {
        PowerEvent::WillSleep => {
            let (ack, receiver) = mpsc::channel();
            let result = commands
                .send(RuntimeCommand::SuspendForSleep(ack))
                .map_err(|_| "bridge runtime stopped before system sleep".to_owned())
                .and_then(|()| {
                    // Bounded: blocking here delays the sleep, and a teardown
                    // wedged in a platform call must not hold sleep, the wake
                    // notification, and app quit hostage forever. The bound
                    // stays under macOS's ~30 s forced-sleep cap, and a late
                    // acknowledgement lands in a dropped receiver harmlessly.
                    receiver
                        .recv_timeout(SLEEP_TEARDOWN_ACK_TIMEOUT)
                        .map_err(|_| {
                            "system-sleep hardware teardown did not acknowledge in time".to_owned()
                        })?
                });
            match result {
                Ok(()) => eprintln!("level=info event=system_sleep_hardware_released"),
                Err(error) => {
                    eprintln!(
                        "level=error event=system_sleep_hardware_release_failed error={error:?}"
                    );
                    let mut current = status
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    current.last_error = Some(bounded_error(&error));
                    current.revision = current.revision.wrapping_add(1);
                }
            }
        }
        PowerEvent::DidWake => {
            let (ack, _receiver) = mpsc::channel();
            if commands.send(RuntimeCommand::ResumeFromWake(ack)).is_err() {
                eprintln!("level=warn event=system_wake_runtime_unavailable");
            }
        }
    })
    .map_err(|error| format!("cannot monitor system sleep safely: {error}"))
}

fn picker_status(config: &RuntimeConfig, open: bool) -> ProfilePickerStatus {
    ProfilePickerStatus {
        enabled: config.profile_picker.is_some(),
        open,
        roster_len: config.picker_roster.len,
    }
}

pub struct BridgeHandle {
    command_sender: mpsc::Sender<RuntimeCommand>,
    status: Arc<Mutex<BridgeStatus>>,
    join: Mutex<Option<JoinHandle<()>>>,
    #[cfg(target_os = "macos")]
    power_monitor: Mutex<Option<PowerMonitor>>,
}

impl BridgeHandle {
    /// Queues an idempotent start without blocking the caller.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_start(&self) -> Result<(), RuntimeError> {
        self.request(RuntimeCommand::Start)
    }

    /// Queues a safety-ordered stop without blocking the caller.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_stop(&self) -> Result<(), RuntimeError> {
        self.request(RuntimeCommand::Stop)
    }

    /// Queues an idle-shutdown timeout update without blocking the caller.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_set_idle_shutdown_timeout(
        &self,
        timeout: Option<Duration>,
    ) -> Result<(), RuntimeError> {
        self.request(|ack| RuntimeCommand::SetIdleShutdown(timeout, ack))
    }

    /// Queues an immediate Puck-dock action update without blocking the caller.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_set_puck_dock_action(&self, action: PuckDockAction) -> Result<(), RuntimeError> {
        self.request(|ack| RuntimeCommand::SetPuckDockAction(action, ack))
    }

    /// Queues a binding-profile switch without restarting HID or serial.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_set_binding_profile(
        &self,
        profile: Option<BindingProfile>,
    ) -> Result<(), RuntimeError> {
        self.request(|ack| RuntimeCommand::SetBindingProfile(Box::new(profile), ack))
    }

    /// Explicitly asks macOS to enable desktop bindings, allowing a permission prompt.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_enable_desktop_bindings(&self) -> Result<(), RuntimeError> {
        self.request(RuntimeCommand::EnableDesktopBindings)
    }

    /// Enables, reconfigures, or disables the in-game profile wheel.
    ///
    /// `None` disables it and hands Quick Access back to the desktop bindings.
    /// An open wheel is closed, so the caller should hide the overlay too.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_set_picker_config(
        &self,
        config: Option<PickerConfig>,
    ) -> Result<(), RuntimeError> {
        self.request(|ack| RuntimeCommand::SetPickerConfig(config, ack))
    }

    /// Replaces the wheel roster and waits until the runtime has stopped using
    /// the previous index generation.
    ///
    /// # Errors
    /// Returns an error if the runtime stops or does not acknowledge in time.
    pub fn set_picker_roster(&self, roster: PickerRoster) -> Result<(), RuntimeError> {
        self.command(|ack| RuntimeCommand::SetPickerRoster(roster, ack))
    }

    /// Parks the controller at neutral, closes the serial port and HID
    /// handles, and returns only once that teardown has completed.
    ///
    /// For the frontend's system-sleep hook. The port must be **closed before
    /// the machine sleeps**: serial I/O left in flight across a sleep/wake
    /// transition has panicked macOS's USB CDC driver while the XIAO
    /// re-enumerated. The bridge stays suspended — regardless of its
    /// start/stop setting — until [`BridgeHandle::request_resume_from_wake`].
    ///
    /// # Errors
    /// Returns an error if the runtime thread stops or the teardown fails.
    pub fn suspend_for_sleep(&self) -> Result<(), RuntimeError> {
        self.command(RuntimeCommand::SuspendForSleep)
    }

    /// Lets the bridge look for its hardware again after a system wake.
    ///
    /// Discovery waits [`WAKE_SETTLE_DELAY`](self) first, so the USB stack has
    /// time to finish re-enumerating the XIAO before anything reopens it. A
    /// bridge the user had stopped stays stopped.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_resume_from_wake(&self) -> Result<(), RuntimeError> {
        self.request(RuntimeCommand::ResumeFromWake)
    }

    /// Requests an idempotent runtime start and waits until the request is accepted.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn start(&self) -> Result<(), RuntimeError> {
        self.command(RuntimeCommand::Start)
    }

    /// Requests an idempotent stop. The acknowledgement is sent only after
    /// neutralization and HID release have completed.
    ///
    /// # Errors
    /// Returns an error if the runtime thread stops or cleanup fails.
    pub fn stop(&self) -> Result<(), RuntimeError> {
        self.command(RuntimeCommand::Stop)
    }

    /// Updates the idle-shutdown timeout without restarting HID or serial.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn set_idle_shutdown_timeout(&self, timeout: Option<Duration>) -> Result<(), RuntimeError> {
        self.command(|ack| RuntimeCommand::SetIdleShutdown(timeout, ack))
    }

    /// Updates the immediate Puck-dock action without restarting HID or serial.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn set_puck_dock_action(&self, action: PuckDockAction) -> Result<(), RuntimeError> {
        self.command(|ack| RuntimeCommand::SetPuckDockAction(action, ack))
    }

    /// Switches the active binding profile without restarting HID or serial.
    ///
    /// # Errors
    /// Returns an error when profile cleanup fails or the runtime has stopped.
    pub fn set_binding_profile(&self, profile: Option<BindingProfile>) -> Result<(), RuntimeError> {
        self.command(|ack| RuntimeCommand::SetBindingProfile(Box::new(profile), ack))
    }

    /// Explicitly retries desktop-input initialization and may show macOS's prompt.
    ///
    /// # Errors
    /// Returns an error when permission/backend initialization fails.
    pub fn enable_desktop_bindings(&self) -> Result<(), RuntimeError> {
        self.command(RuntimeCommand::EnableDesktopBindings)
    }

    /// Stops safely and terminates the runtime thread.
    ///
    /// # Errors
    /// Returns an error if cleanup or joining fails.
    pub fn shutdown(&self) -> Result<(), RuntimeError> {
        #[cfg(target_os = "macos")]
        self.stop_power_monitor();
        let result = self.command(RuntimeCommand::Shutdown);
        let join_result = self.join();
        result.and(join_result)
    }

    /// Waits for the runtime thread to terminate.
    ///
    /// # Errors
    /// Returns an error if the runtime thread panicked.
    pub fn join(&self) -> Result<(), RuntimeError> {
        let mut join = self
            .join
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(handle) = join.take() {
            handle
                .join()
                .map_err(|_| RuntimeError("bridge runtime thread panicked".to_owned()))?;
        }
        Ok(())
    }

    #[must_use]
    pub fn status(&self) -> BridgeStatus {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn command(
        &self,
        make_command: impl FnOnce(CommandAck) -> RuntimeCommand,
    ) -> Result<(), RuntimeError> {
        let (sender, receiver) = mpsc::channel();
        self.command_sender
            .send(make_command(sender))
            .map_err(|_| RuntimeError("bridge runtime is no longer running".to_owned()))?;
        receiver
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| RuntimeError("bridge runtime command timed out".to_owned()))?
            .map_err(RuntimeError)
    }

    fn request(
        &self,
        make_command: impl FnOnce(CommandAck) -> RuntimeCommand,
    ) -> Result<(), RuntimeError> {
        let (sender, _receiver) = mpsc::channel();
        self.command_sender
            .send(make_command(sender))
            .map_err(|_| RuntimeError("bridge runtime is no longer running".to_owned()))
    }

    #[cfg(target_os = "macos")]
    fn stop_power_monitor(&self) {
        drop(
            self.power_monitor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
    }
}

impl Drop for BridgeHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn automatic_shutdown_phase(config: &RuntimeConfig) -> AutomaticShutdownPhase {
    if config.idle_shutdown_timeout.is_none() && config.puck_dock_action == PuckDockAction::LeaveOn
    {
        AutomaticShutdownPhase::Disabled
    } else {
        AutomaticShutdownPhase::Monitoring
    }
}

fn validate_idle_shutdown_timeout(timeout: Option<Duration>) -> Result<(), String> {
    if timeout.is_some_and(|value| value < MIN_IDLE_SHUTDOWN_TIMEOUT) {
        return Err("idle-shutdown timeout must be at least one minute".to_owned());
    }
    if timeout.is_some_and(|value| value > MAX_IDLE_SHUTDOWN_TIMEOUT) {
        return Err("idle-shutdown timeout cannot exceed 1440 minutes".to_owned());
    }
    Ok(())
}

struct AutomaticShutdownRuntime {
    phase: AutomaticShutdownPhase,
    trigger: Option<ShutdownTrigger>,
    successful_shutdowns: u64,
    failures: u64,
    last_success: Option<Instant>,
    retry_after: Option<Instant>,
    dock_identity: Option<HidDeviceInfo>,
    dock_episode_handled: bool,
    dock_failure_at: Option<Instant>,
}

impl AutomaticShutdownRuntime {
    fn new(config: &RuntimeConfig) -> Self {
        Self {
            phase: automatic_shutdown_phase(config),
            trigger: None,
            successful_shutdowns: 0,
            failures: 0,
            last_success: None,
            retry_after: None,
            dock_identity: None,
            dock_episode_handled: false,
            dock_failure_at: None,
        }
    }

    fn source_selected(&mut self, info: &HidDeviceInfo, config: &RuntimeConfig) {
        if self
            .dock_identity
            .as_ref()
            .is_some_and(|identity| !same_controller_collection(identity, info))
        {
            self.clear_dock_episode("source_replaced");
        }
        self.dock_identity = Some(info.clone());
        self.phase = automatic_shutdown_phase(config);
        self.trigger = None;
        self.retry_after = None;
        self.dock_failure_at = None;
    }

    fn set_dock_action(&mut self, action: PuckDockAction, config: &RuntimeConfig) {
        if action != config.puck_dock_action {
            self.clear_dock_episode("policy_changed");
        }
    }

    fn clear_dock_episode(&mut self, reason: &str) {
        if self.dock_episode_handled {
            eprintln!("level=info event=puck_dock_episode_cleared reason={reason:?}");
        }
        self.dock_episode_handled = false;
        self.dock_failure_at = None;
    }

    fn observe_charge_state(
        &mut self,
        info: &HidDeviceInfo,
        charge_state: ControllerChargeState,
        action: PuckDockAction,
    ) -> bool {
        if charge_state == ControllerChargeState::Discharging {
            self.clear_dock_episode("discharging");
            return false;
        }
        action == PuckDockAction::PowerOff
            && info.controller_transport() == Some(ControllerTransport::Puck)
            && charge_state.is_external_power()
            && !self.dock_episode_handled
    }

    fn activity_after_failed_dock_attempt(&mut self, now: Instant) {
        if self.trigger == Some(ShutdownTrigger::PuckDock)
            && self.dock_failure_at.is_some_and(|failure| now > failure)
            && !self.dock_episode_handled
        {
            self.dock_episode_handled = true;
            self.retry_after = None;
            eprintln!(
                "level=info event=puck_dock_episode_handled reason=activity_after_failed_shutdown"
            );
        }
    }

    fn begin(&mut self, trigger: ShutdownTrigger) {
        self.phase = AutomaticShutdownPhase::PoweringOff;
        self.trigger = Some(trigger);
        eprintln!("level=info event=automatic_shutdown_started trigger={trigger:?}");
    }

    fn succeeded(&mut self, now: Instant, trigger: ShutdownTrigger) {
        self.phase = AutomaticShutdownPhase::Sleeping;
        self.trigger = Some(trigger);
        self.successful_shutdowns = self.successful_shutdowns.wrapping_add(1);
        self.last_success = Some(now);
        self.retry_after = None;
        self.dock_failure_at = None;
        if trigger == ShutdownTrigger::PuckDock {
            self.dock_episode_handled = true;
            eprintln!("level=info event=puck_dock_episode_handled reason=power_off_succeeded");
        }
        eprintln!("level=info event=automatic_shutdown_succeeded trigger={trigger:?}");
    }

    fn failed(&mut self, now: Instant, trigger: ShutdownTrigger, error: &str) {
        self.phase = AutomaticShutdownPhase::Degraded;
        self.trigger = Some(trigger);
        self.failures = self.failures.wrapping_add(1);
        self.retry_after = Some(now + AUTOMATIC_SHUTDOWN_RETRY_INTERVAL);
        self.dock_failure_at = (trigger == ShutdownTrigger::PuckDock).then_some(now);
        eprintln!(
            "level=warn event=automatic_shutdown_failed trigger={trigger:?} error={error:?} retry_ms={}",
            AUTOMATIC_SHUTDOWN_RETRY_INTERVAL.as_millis()
        );
    }

    fn retry_due(&self, now: Instant) -> bool {
        self.retry_after.is_none_or(|deadline| now >= deadline)
    }

    fn status(
        &self,
        config: &RuntimeConfig,
        idle_age: Option<Duration>,
        now: Instant,
    ) -> AutomaticShutdownStatus {
        AutomaticShutdownStatus {
            configured_timeout: config.idle_shutdown_timeout,
            puck_dock_action: config.puck_dock_action,
            puck_dock_episode_handled: self.dock_episode_handled,
            neutral_idle_age: idle_age,
            phase: self.phase,
            trigger: self.trigger,
            successful_shutdowns: self.successful_shutdowns,
            failures: self.failures,
            last_successful_shutdown_age: self
                .last_success
                .map(|last| now.saturating_duration_since(last)),
            retry_after: self
                .retry_after
                .map(|retry| retry.saturating_duration_since(now)),
        }
    }
}

struct ControllerCooldown {
    info: HidDeviceInfo,
    until: Instant,
}

fn binding_status_for_profile(profile: Option<&BindingProfile>) -> DesktopBindingsStatus {
    let Some(profile) = profile else {
        return DesktopBindingsStatus::default();
    };
    let configured_count = profile.configured_output_count();
    DesktopBindingsStatus {
        state: if configured_count == 0 {
            DesktopBindingsState::Disabled
        } else {
            DesktopBindingsState::PermissionRequired
        },
        active_profile_id: Some(profile.id.clone()),
        active_profile_name: Some(profile.name.clone()),
        configured_binding_count: configured_count,
        ..DesktopBindingsStatus::default()
    }
}

/// The profile wheel as the active loop sees it.
///
/// Wraps the pure state machine so the loop can ask one object every question
/// it has: what to hide from the game, what to hide from the desktop bindings,
/// and what the user just chose. A `None` picker is the feature switched off,
/// and every method then behaves as if the wheel does not exist.
struct PickerRuntime {
    /// Kept after the feature is switched off so a just-closed wheel's
    /// still-held controls keep draining; [`PickerRuntime::observe`] hands a
    /// disabled picker an empty roster, which can never arm.
    picker: Option<Picker>,
    enabled: bool,
}

impl PickerRuntime {
    fn new(config: Option<PickerConfig>) -> Self {
        Self {
            enabled: config.is_some(),
            picker: config.map(Picker::new),
        }
    }

    /// Replaces the configuration. Returns whether a wheel — open, or a hold
    /// partway toward one — was cancelled, which the caller must answer by
    /// dismissing the overlay.
    ///
    /// The picker itself latches whatever consumed controls are still held, so
    /// the press that was aimed at the wheel cannot leak into the game or the
    /// bindings engine; the caller keeps applying [`PickerRuntime::suppression`]
    /// as usual and the latch drains on release.
    fn set_config(&mut self, config: Option<PickerConfig>) -> bool {
        let was_active = self.picker.as_ref().is_some_and(Picker::owns_trigger);
        self.enabled = config.is_some();
        match (self.picker.as_mut(), config) {
            (Some(picker), Some(config)) => picker.set_config(config),
            (Some(picker), None) => {
                // Re-applying the current configuration closes the wheel and
                // latches the held controls without discarding the drain state.
                let config = *picker.config();
                picker.set_config(config);
            }
            (None, Some(config)) => self.picker = Some(Picker::new(config)),
            (None, None) => {}
        }
        was_active
    }

    fn is_open(&self) -> bool {
        self.picker.as_ref().is_some_and(Picker::is_open)
    }

    fn suppression(&self) -> Option<OutputSuppression> {
        self.picker.as_ref().and_then(Picker::suppression)
    }

    fn mask_trigger(&self, buttons: SteamButtons) -> SteamButtons {
        self.picker
            .as_ref()
            .map_or(buttons, |picker| picker.mask_trigger(buttons))
    }

    fn observe(
        &mut self,
        now: Duration,
        input: &PickerInput,
        roster: PickerRoster,
    ) -> PickerEvents {
        // A disabled picker still sees reports so its latch can drain, but an
        // empty roster keeps it from ever arming again.
        let roster = if self.enabled {
            roster
        } else {
            PickerRoster::default()
        };
        self.picker
            .as_mut()
            .map_or_else(PickerEvents::default, |picker| {
                picker.update(now, input, roster)
            })
    }

    /// Forces the wheel shut. Returns whether it had anything to close.
    fn close(&mut self) -> bool {
        self.picker.as_mut().is_some_and(Picker::close)
    }
}

struct DesktopBindingsRuntime {
    engine: Option<BindingEngine>,
    // Once authorized, the sink belongs to the active runtime session. Profile
    // changes may release outputs or remove the engine, but must not destroy
    // the sink: Enigo's macOS `Drop` can sleep for seconds after pad traffic.
    sink: Option<Box<dyn DesktopInputSink>>,
    // Status-only profile changes are allowed before the frontend has completed
    // the ordered macOS permission flow. Only Enable latches runtime activation.
    activation_requested: bool,
    last_snapshot: Option<DesktopInputSnapshot>,
    discard_pending_feedback: bool,
    status: DesktopBindingsStatus,
    status_dirty: bool,
}

impl DesktopBindingsRuntime {
    fn new(profile: Option<BindingProfile>) -> Self {
        let status = binding_status_for_profile(profile.as_ref());
        Self {
            engine: profile.map(BindingEngine::new),
            sink: None,
            activation_requested: false,
            last_snapshot: None,
            discard_pending_feedback: false,
            status,
            status_dirty: true,
        }
    }

    #[cfg(test)]
    fn with_sink(profile: BindingProfile, sink: Box<dyn DesktopInputSink>) -> Self {
        let mut status = binding_status_for_profile(Some(&profile));
        status.state = DesktopBindingsState::Ready;
        Self {
            engine: Some(BindingEngine::new(profile)),
            sink: Some(sink),
            activation_requested: true,
            last_snapshot: None,
            discard_pending_feedback: false,
            status,
            status_dirty: true,
        }
    }

    fn status(&self) -> DesktopBindingsStatus {
        let mut status = self.status.clone();
        status.held_output_count = self
            .engine
            .as_ref()
            .map_or(0, BindingEngine::held_output_count);
        status
    }

    fn take_status_update(&mut self) -> Option<DesktopBindingsStatus> {
        if !std::mem::take(&mut self.status_dirty) {
            return None;
        }
        Some(self.status())
    }

    fn held_output_count(&self) -> usize {
        self.engine
            .as_ref()
            .map_or(0, BindingEngine::held_output_count)
    }

    fn observe(&mut self, snapshot: DesktopInputSnapshot, now: Duration) -> PadFeedbackRequest {
        self.last_snapshot = Some(snapshot);
        let held_before = self.held_output_count();
        let (Some(engine), Some(sink)) = (self.engine.as_mut(), self.sink.as_mut()) else {
            return PadFeedbackRequest::NONE;
        };
        let feedback = match engine.observe_snapshot(snapshot, now, sink.as_mut()) {
            Ok(feedback) => {
                if self.status.state == DesktopBindingsState::Degraded {
                    self.status.state = DesktopBindingsState::Ready;
                    self.status.last_error = None;
                    self.status_dirty = true;
                }
                feedback
            }
            Err(error) => {
                self.fail(&error);
                PadFeedbackRequest::NONE
            }
        };
        if self.held_output_count() != held_before {
            self.status_dirty = true;
        }
        feedback
    }

    fn tick(&mut self, now: Duration) {
        let (Some(engine), Some(sink)) = (self.engine.as_mut(), self.sink.as_mut()) else {
            return;
        };
        if let Err(error) = engine.tick(now, sink.as_mut()) {
            self.fail(&error);
        }
    }

    fn needs_tick(&self) -> bool {
        self.sink.is_some() && self.engine.as_ref().is_some_and(BindingEngine::needs_tick)
    }

    fn take_discard_pending_feedback(&mut self) -> bool {
        std::mem::take(&mut self.discard_pending_feedback)
    }

    fn drop_sink(&mut self, reason: &'static str) {
        let Some(sink) = self.sink.take() else {
            return;
        };
        let started = Instant::now();
        drop(sink);
        let elapsed = started.elapsed();
        if elapsed >= SUPERVISOR_STALL_THRESHOLD {
            eprintln!(
                "level=warn event=desktop_sink_drop_stall reason={reason} elapsed_ms={}",
                elapsed.as_millis()
            );
        }
    }

    fn replace_profile(&mut self, profile: Option<BindingProfile>) -> Result<(), String> {
        let status = binding_status_for_profile(profile.as_ref());
        let result = if let Some(profile) = profile {
            if let (Some(engine), Some(sink)) = (self.engine.as_mut(), self.sink.as_mut()) {
                engine.replace_profile(profile, sink.as_mut())
            } else {
                self.engine = Some(BindingEngine::new(profile));
                Ok(())
            }
        } else {
            let result =
                if let (Some(engine), Some(sink)) = (self.engine.as_mut(), self.sink.as_mut()) {
                    engine.disconnect(sink.as_mut())
                } else {
                    Ok(())
                };
            self.engine = None;
            result
        };
        if self.status != status {
            self.status = status;
            self.status_dirty = true;
        }
        if let Err(error) = result {
            self.fail(&error);
            return Err(error);
        }
        if self.status.configured_binding_count == 0 {
            return Ok(());
        }
        if self.sink.is_none() && self.activation_requested {
            self.initialize_sink();
        } else if self.sink.is_some() {
            self.status.state = DesktopBindingsState::Ready;
            self.status.last_error = None;
        }
        if let (Some(snapshot), Some(engine), Some(sink)) =
            (self.last_snapshot, self.engine.as_mut(), self.sink.as_mut())
        {
            if let Err(error) = engine.observe_snapshot(snapshot, Duration::ZERO, sink.as_mut()) {
                self.fail(&error);
                return Err(error);
            }
        }
        Ok(())
    }

    fn enable(&mut self) -> Result<(), String> {
        self.activation_requested = true;
        if self.status.configured_binding_count == 0 {
            return Ok(());
        }
        if self.sink.is_none() {
            self.initialize_sink();
        }
        if self.sink.is_none() {
            return Err(self
                .status
                .last_error
                .clone()
                .unwrap_or_else(|| "desktop bindings are unavailable".to_owned()));
        }
        if let (Some(snapshot), Some(engine), Some(sink)) =
            (self.last_snapshot, self.engine.as_mut(), self.sink.as_mut())
        {
            if let Err(error) = engine.observe_snapshot(snapshot, Duration::ZERO, sink.as_mut()) {
                self.fail(&error);
                return Err(error);
            }
        }
        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), String> {
        let held_before = self.held_output_count();
        let result = if let (Some(engine), Some(sink)) = (self.engine.as_mut(), self.sink.as_mut())
        {
            engine.disconnect(sink.as_mut())
        } else {
            Ok(())
        };
        if let Err(error) = &result {
            self.fail(error);
        }
        self.last_snapshot = None;
        self.discard_pending_feedback = true;
        if self.held_output_count() != held_before {
            self.status_dirty = true;
        }
        result
    }

    fn shutdown(&mut self) -> Result<(), String> {
        let result = self.disconnect();
        // Drop can be slow on macOS, so do it on this worker before the
        // shutdown acknowledgement. The owner can then bound its wait without
        // ever blocking the supervisor or joining a permanently wedged drop.
        self.drop_sink("desktop_worker_shutdown");
        result
    }

    fn overflow(&mut self) {
        let _ = self.disconnect();
        self.status.state = DesktopBindingsState::Degraded;
        self.status.failures = self.status.failures.saturating_add(1);
        self.status.last_error = Some(
            "input transition mailbox overflowed; held inputs released and state rebaselined"
                .to_owned(),
        );
        self.status_dirty = true;
    }

    fn initialize_sink(&mut self) {
        match create_desktop_sink() {
            Ok(sink) => {
                self.sink = Some(sink);
                self.status.state = DesktopBindingsState::Ready;
                self.status.last_error = None;
                self.status_dirty = true;
            }
            Err(error) => {
                self.sink = None;
                self.status.state =
                    if error.contains("permission") || error.contains("Accessibility") {
                        DesktopBindingsState::PermissionRequired
                    } else {
                        DesktopBindingsState::Degraded
                    };
                self.status.last_error = Some(bounded_error(&error));
                self.status_dirty = true;
            }
        }
    }

    fn fail(&mut self, error: &str) {
        self.status.state = DesktopBindingsState::Degraded;
        self.status.failures = self.status.failures.saturating_add(1);
        self.status.last_error = Some(bounded_error(error));
        self.status_dirty = true;
        self.drop_sink("backend_failure");
        self.discard_pending_feedback = true;
    }
}

impl Drop for DesktopBindingsRuntime {
    fn drop(&mut self) {
        self.drop_sink("desktop_worker_exit");
    }
}

#[derive(Debug, Clone, Copy)]
struct DesktopWorkerSnapshot {
    snapshot: DesktopInputSnapshot,
    now: Duration,
    generation: u64,
    feedback_epoch: u64,
}

enum DesktopWorkerMessage {
    Snapshot(DesktopWorkerSnapshot),
    Overflow,
    ReplaceProfile {
        profile: Option<BindingProfile>,
        ack: Option<CommandAck>,
    },
    Enable {
        ack: Option<CommandAck>,
    },
    Disconnect(CommandAck),
    Shutdown(CommandAck),
}

impl DesktopWorkerMessage {
    fn reject(self, error: &str) {
        let ack = match self {
            Self::ReplaceProfile { ack, .. } | Self::Enable { ack } => ack,
            Self::Disconnect(ack) | Self::Shutdown(ack) => Some(ack),
            Self::Snapshot(_) | Self::Overflow => None,
        };
        if let Some(ack) = ack {
            let _ = ack.send(Err(error.to_owned()));
        }
    }

    const fn is_snapshot_or_overflow(&self) -> bool {
        matches!(self, Self::Snapshot(_) | Self::Overflow)
    }

    const fn is_safety_control(&self) -> bool {
        matches!(self, Self::Disconnect(_) | Self::Shutdown(_))
    }
}

/// Tracks whether two equal transition masks have established a baseline that
/// makes an intermediate analog-only sample replaceable.
#[derive(Debug, Default)]
struct StableTransitionRun {
    previous: Option<u8>,
    latest: Option<u8>,
}

impl StableTransitionRun {
    fn can_replace_latest(&self, transition_mask: Option<u8>) -> bool {
        transition_mask.is_some()
            && self.latest == transition_mask
            && self.previous == transition_mask
    }

    fn push(&mut self, transition_mask: Option<u8>) {
        self.previous = self.latest;
        self.latest = transition_mask;
    }

    fn reset(&mut self) {
        self.previous = None;
        self.latest = None;
    }

    fn reset_with_latest(&mut self, transition_mask: Option<u8>) {
        self.previous = None;
        self.latest = transition_mask;
    }
}

#[derive(Default)]
struct DesktopWorkerMailboxState {
    messages: VecDeque<DesktopWorkerMessage>,
    snapshot_count: usize,
    control_count: usize,
    transition_run: StableTransitionRun,
    generation: u64,
    feedback_epoch: u64,
    accepting: bool,
}

struct DesktopWorkerMailbox {
    state: Mutex<DesktopWorkerMailboxState>,
    wake: Condvar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopSnapshotPublish {
    Published,
    Overflowed,
    Closed,
}

impl Default for DesktopWorkerMailbox {
    fn default() -> Self {
        Self {
            state: Mutex::new(DesktopWorkerMailboxState {
                accepting: true,
                ..DesktopWorkerMailboxState::default()
            }),
            wake: Condvar::new(),
        }
    }
}

impl DesktopWorkerMailbox {
    // The supervisor is the sole producer. Snapshot runs may coalesce only
    // after preserving a baseline; controls reset that run and remain ordered
    // barriers. Overflow keeps controls, releases worker state, and retains the
    // newest snapshot as a non-emitting recovery baseline.
    fn publish_snapshot(
        &self,
        outputs: &DesktopWorkerOutputs,
        snapshot: DesktopInputSnapshot,
        now: Duration,
    ) -> DesktopSnapshotPublish {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.accepting {
            return DesktopSnapshotPublish::Closed;
        }
        let transition_mask = desktop_snapshot_transition_mask(snapshot);
        let result = if state
            .transition_run
            .can_replace_latest(Some(transition_mask))
        {
            let generation = state.generation;
            let feedback_epoch = state.feedback_epoch;
            let Some(DesktopWorkerMessage::Snapshot(latest)) = state.messages.back_mut() else {
                unreachable!("desktop snapshot coalescing state must describe the queue tail");
            };
            *latest = DesktopWorkerSnapshot {
                snapshot,
                now,
                generation,
                feedback_epoch,
            };
            DesktopSnapshotPublish::Published
        } else if state.snapshot_count == DESKTOP_INPUT_MAILBOX_CAPACITY {
            Self::reset_snapshots_for_overflow(&mut state);
            outputs.invalidate_feedback(state.feedback_epoch);
            state.messages.push_back(DesktopWorkerMessage::Overflow);
            Self::push_snapshot(&mut state, snapshot, now, transition_mask);
            DesktopSnapshotPublish::Overflowed
        } else {
            Self::push_snapshot(&mut state, snapshot, now, transition_mask);
            DesktopSnapshotPublish::Published
        };
        drop(state);
        self.wake.notify_one();
        result
    }

    fn publish_overflow(&self, outputs: &DesktopWorkerOutputs) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.accepting {
            return false;
        }
        Self::reset_snapshots_for_overflow(&mut state);
        outputs.invalidate_feedback(state.feedback_epoch);
        state.messages.push_back(DesktopWorkerMessage::Overflow);
        drop(state);
        self.wake.notify_one();
        true
    }

    fn push_control(
        &self,
        outputs: &DesktopWorkerOutputs,
        message: DesktopWorkerMessage,
        feedback_barrier: bool,
    ) -> Result<(), Box<DesktopWorkerMessage>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.accepting {
            return Err(Box::new(message));
        }
        if matches!(message, DesktopWorkerMessage::ReplaceProfile { .. })
            && matches!(
                state.messages.back(),
                Some(DesktopWorkerMessage::ReplaceProfile { .. })
            )
        {
            let Some(previous) = state.messages.pop_back() else {
                unreachable!("the queue tail was just matched")
            };
            state.messages.push_back(message);
            drop(state);
            previous.reject("desktop profile update superseded by a newer profile");
            self.wake.notify_one();
            return Ok(());
        }
        let reserved_capacity = DESKTOP_CONTROL_MAILBOX_CAPACITY - 1;
        let limit = if message.is_safety_control() {
            DESKTOP_CONTROL_MAILBOX_CAPACITY
        } else {
            reserved_capacity
        };
        if state.control_count >= limit {
            return Err(Box::new(message));
        }
        if feedback_barrier {
            state.feedback_epoch = state.feedback_epoch.wrapping_add(1);
            outputs.invalidate_feedback(state.feedback_epoch);
        }
        state.transition_run.reset();
        state.messages.push_back(message);
        state.control_count += 1;
        drop(state);
        self.wake.notify_one();
        Ok(())
    }

    fn take_batch(&self, timeout: Option<Duration>) -> VecDeque<DesktopWorkerMessage> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.messages.is_empty() && state.accepting {
            state = match timeout {
                Some(timeout) => {
                    let (returned, _) = self
                        .wake
                        .wait_timeout_while(state, timeout, |state| {
                            state.messages.is_empty() && state.accepting
                        })
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    returned
                }
                None => self
                    .wake
                    .wait_while(state, |state| state.messages.is_empty() && state.accepting)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            };
        }
        state.snapshot_count = 0;
        state.control_count = 0;
        state.transition_run.reset();
        std::mem::take(&mut state.messages)
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accepting = false;
        let pending = std::mem::take(&mut state.messages);
        state.snapshot_count = 0;
        state.control_count = 0;
        state.transition_run.reset();
        drop(state);
        for message in pending {
            message.reject("desktop-input worker stopped before processing the command");
        }
        self.wake.notify_all();
    }

    fn generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation
    }

    fn push_snapshot(
        state: &mut DesktopWorkerMailboxState,
        snapshot: DesktopInputSnapshot,
        now: Duration,
        transition_mask: u8,
    ) {
        state
            .messages
            .push_back(DesktopWorkerMessage::Snapshot(DesktopWorkerSnapshot {
                snapshot,
                now,
                generation: state.generation,
                feedback_epoch: state.feedback_epoch,
            }));
        state.snapshot_count += 1;
        state.transition_run.push(Some(transition_mask));
    }

    fn reset_snapshots_for_overflow(state: &mut DesktopWorkerMailboxState) {
        state
            .messages
            .retain(|message| !message.is_snapshot_or_overflow());
        state.snapshot_count = 0;
        state.transition_run.reset();
        state.generation = state.generation.wrapping_add(1);
        state.feedback_epoch = state.feedback_epoch.wrapping_add(1);
    }
}

fn desktop_snapshot_transition_mask(snapshot: DesktopInputSnapshot) -> u8 {
    desktop_transition_mask(
        snapshot.buttons,
        snapshot.left_pad.touched,
        snapshot.right_pad.touched,
    )
}

fn desktop_transition_mask(buttons: SteamButtons, left_touched: bool, right_touched: bool) -> u8 {
    let mut mask = bindable_mask(buttons);
    mask |= u8::from(left_touched) << 5;
    mask |= u8::from(right_touched) << 6;
    mask
}

#[derive(Debug, Clone, Copy)]
struct DesktopWorkerOutput {
    feedback: PadFeedbackRequest,
    discard_pending_feedback: bool,
}

impl Default for DesktopWorkerOutput {
    fn default() -> Self {
        Self {
            feedback: PadFeedbackRequest::NONE,
            discard_pending_feedback: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct DesktopWorkerOutputState {
    output: DesktopWorkerOutput,
    feedback_epoch: u64,
}

#[derive(Default)]
struct DesktopWorkerOutputs {
    state: Mutex<DesktopWorkerOutputState>,
}

impl DesktopWorkerOutputs {
    fn publish_feedback(&self, feedback_epoch: u64, feedback: PadFeedbackRequest) {
        if feedback == PadFeedbackRequest::NONE {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if feedback_epoch != state.feedback_epoch {
            return;
        }
        if feedback.left.is_some() {
            state.output.feedback.left = feedback.left;
        }
        if feedback.right.is_some() {
            state.output.feedback.right = feedback.right;
        }
    }

    fn discard_feedback(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.output.feedback = PadFeedbackRequest::NONE;
        state.output.discard_pending_feedback = true;
    }

    fn invalidate_feedback(&self, feedback_epoch: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.output.feedback = PadFeedbackRequest::NONE;
        state.output.discard_pending_feedback = true;
        state.feedback_epoch = feedback_epoch;
    }

    fn take(&self) -> DesktopWorkerOutput {
        std::mem::take(
            &mut self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .output,
        )
    }
}

// Owns BindingEngine and the non-Send desktop sink on one dedicated thread.
// Supervisor-facing snapshot publication never waits for desktop injection;
// disconnect and shutdown are acknowledged only after held outputs are released.
struct DesktopBindingsWorker {
    mailbox: Arc<DesktopWorkerMailbox>,
    outputs: Arc<DesktopWorkerOutputs>,
    status: Arc<Mutex<BridgeStatus>>,
    alive: Arc<AtomicBool>,
    started: Instant,
    handle: Option<JoinHandle<()>>,
}

impl DesktopBindingsWorker {
    fn spawn(profile: Option<BindingProfile>, status: Arc<Mutex<BridgeStatus>>) -> Self {
        Self::spawn_with_runtime(status, move || DesktopBindingsRuntime::new(profile))
    }

    fn spawn_with_runtime(
        status: Arc<Mutex<BridgeStatus>>,
        make_runtime: impl FnOnce() -> DesktopBindingsRuntime + Send + 'static,
    ) -> Self {
        let mailbox = Arc::new(DesktopWorkerMailbox::default());
        let worker_mailbox = Arc::clone(&mailbox);
        let outputs = Arc::new(DesktopWorkerOutputs::default());
        let worker_outputs = Arc::clone(&outputs);
        let worker_status = Arc::clone(&status);
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = Arc::clone(&alive);
        let started = Instant::now();
        let handle = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let runtime = make_runtime();
                run_desktop_bindings_worker(
                    runtime,
                    &worker_mailbox,
                    &worker_outputs,
                    &worker_status,
                    started,
                );
            }));
            if result.is_err() {
                publish_desktop_worker_failure(&worker_status, "desktop-input worker panicked");
                eprintln!("level=error event=desktop_input_worker_panicked");
            }
            worker_alive.store(false, Ordering::Release);
            worker_mailbox.close();
        });
        Self {
            mailbox,
            outputs,
            status,
            alive,
            started,
            handle: Some(handle),
        }
    }

    fn observe(&self, snapshot: DesktopInputSnapshot) {
        match self
            .mailbox
            .publish_snapshot(&self.outputs, snapshot, self.started.elapsed())
        {
            DesktopSnapshotPublish::Published | DesktopSnapshotPublish::Overflowed => {}
            DesktopSnapshotPublish::Closed => {
                self.outputs.discard_feedback();
                publish_desktop_worker_failure(&self.status, "desktop-input worker is unavailable");
            }
        }
    }

    fn overflow(&self) {
        if !self.mailbox.publish_overflow(&self.outputs) {
            self.outputs.discard_feedback();
            publish_desktop_worker_failure(&self.status, "desktop-input worker is unavailable");
        }
    }

    fn replace_profile(&self, profile: Option<BindingProfile>, ack: CommandAck) {
        self.enqueue_control(
            DesktopWorkerMessage::ReplaceProfile {
                profile,
                ack: Some(ack),
            },
            true,
        );
    }

    fn enable(&self, ack: CommandAck) {
        self.enqueue_control(DesktopWorkerMessage::Enable { ack: Some(ack) }, false);
    }

    fn enable_async(&self) {
        self.enqueue_control(DesktopWorkerMessage::Enable { ack: None }, false);
    }

    fn disconnect(&self) -> Result<(), String> {
        let (ack, receiver) = mpsc::channel();
        self.enqueue_control(DesktopWorkerMessage::Disconnect(ack), true);
        receiver
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| "desktop-input worker disconnect timed out".to_owned())?
    }

    fn take_output(&self) -> DesktopWorkerOutput {
        self.outputs.take()
    }

    fn status(&self) -> DesktopBindingsStatus {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .bindings
            .clone()
    }

    fn shutdown(&mut self) -> Result<(), String> {
        self.shutdown_with_timeout(COMMAND_TIMEOUT)
    }

    fn shutdown_with_timeout(&mut self, timeout: Duration) -> Result<(), String> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        let command_result = if self.alive.load(Ordering::Acquire) {
            let (ack, receiver) = mpsc::channel();
            if let Err(message) =
                self.mailbox
                    .push_control(&self.outputs, DesktopWorkerMessage::Shutdown(ack), true)
            {
                // A full mailbox means the worker is not draining. With no
                // Shutdown queued, a join could never be satisfied — detach,
                // exactly as the timeout path below does.
                self.outputs.discard_feedback();
                publish_desktop_worker_failure(&self.status, "desktop-input worker is unavailable");
                (*message).reject("desktop-input worker is unavailable");
                drop(handle);
                return Err("desktop-input worker control queue is full at shutdown".to_owned());
            }
            if let Ok(result) = receiver.recv_timeout(timeout) {
                result
            } else {
                // Rust cannot cancel a thread that is inside a third-party
                // platform call. Detach this final-shutdown worker rather
                // than defeating the timeout with an unconditional join;
                // the queued Shutdown still makes it exit if the call ever
                // returns, and no further work can reach it through `self`.
                drop(handle);
                return Err("desktop-input worker shutdown timed out".to_owned());
            }
        } else {
            Err("desktop-input worker stopped unexpectedly".to_owned())
        };
        let join_result = handle
            .join()
            .map_err(|_| "desktop-input worker panicked".to_owned());
        command_result.and(join_result)
    }

    fn enqueue_control(&self, message: DesktopWorkerMessage, feedback_barrier: bool) {
        if let Err(message) = self
            .mailbox
            .push_control(&self.outputs, message, feedback_barrier)
        {
            self.outputs.discard_feedback();
            publish_desktop_worker_failure(&self.status, "desktop-input worker is unavailable");
            (*message).reject("desktop-input worker is unavailable");
        }
    }
}

impl Drop for DesktopBindingsWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn run_desktop_bindings_worker(
    mut runtime: DesktopBindingsRuntime,
    mailbox: &DesktopWorkerMailbox,
    outputs: &DesktopWorkerOutputs,
    status: &Arc<Mutex<BridgeStatus>>,
    started: Instant,
) {
    if let Some(update) = runtime.take_status_update() {
        publish_desktop_binding_status(status, update);
    }
    let mut shutdown = false;
    let mut applied_generation = mailbox.generation();
    while !shutdown {
        let timeout = runtime.needs_tick().then_some(RUNTIME_POLL_INTERVAL);
        let messages = mailbox.take_batch(timeout);
        for message in messages {
            match message {
                DesktopWorkerMessage::Snapshot(snapshot) => {
                    let current_generation = mailbox.generation();
                    if snapshot.generation == current_generation {
                        let feedback = runtime.observe(snapshot.snapshot, snapshot.now);
                        outputs.publish_feedback(snapshot.feedback_epoch, feedback);
                    }
                    let current_generation = mailbox.generation();
                    apply_desktop_mailbox_overflow(
                        &mut runtime,
                        outputs,
                        &mut applied_generation,
                        current_generation,
                    );
                }
                DesktopWorkerMessage::Overflow => {
                    apply_desktop_mailbox_overflow(
                        &mut runtime,
                        outputs,
                        &mut applied_generation,
                        mailbox.generation(),
                    );
                }
                DesktopWorkerMessage::ReplaceProfile { profile, ack } => {
                    let result = runtime.replace_profile(profile);
                    if let Some(ack) = ack {
                        let _ = ack.send(result);
                    }
                }
                DesktopWorkerMessage::Enable { ack } => {
                    let result = runtime.enable();
                    if let Some(ack) = ack {
                        let _ = ack.send(result);
                    }
                }
                DesktopWorkerMessage::Disconnect(ack) => {
                    let result = runtime.disconnect();
                    let _ = ack.send(result);
                }
                DesktopWorkerMessage::Shutdown(ack) => {
                    let result = runtime.shutdown();
                    let _ = ack.send(result);
                    shutdown = true;
                }
            }
            if runtime.take_discard_pending_feedback() {
                outputs.discard_feedback();
            }
            if let Some(update) = runtime.take_status_update() {
                publish_desktop_binding_status(status, update);
            }
            if shutdown {
                break;
            }
        }
        if !shutdown && runtime.needs_tick() {
            runtime.tick(started.elapsed());
            if runtime.take_discard_pending_feedback() {
                outputs.discard_feedback();
            }
            if let Some(update) = runtime.take_status_update() {
                publish_desktop_binding_status(status, update);
            }
        }
    }
}

fn apply_desktop_mailbox_overflow(
    runtime: &mut DesktopBindingsRuntime,
    outputs: &DesktopWorkerOutputs,
    applied_generation: &mut u64,
    current_generation: u64,
) {
    if *applied_generation == current_generation {
        return;
    }
    runtime.overflow();
    outputs.discard_feedback();
    *applied_generation = current_generation;
    eprintln!(
        "level=warn event=desktop_input_worker_mailbox_overflow action=release_and_rebaseline"
    );
}

fn publish_desktop_binding_status(
    shared: &Arc<Mutex<BridgeStatus>>,
    bindings: DesktopBindingsStatus,
) {
    let mut status = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if status.bindings != bindings {
        status.bindings = bindings;
        status.revision = status.revision.wrapping_add(1);
    }
}

fn publish_desktop_worker_failure(shared: &Arc<Mutex<BridgeStatus>>, error: &str) {
    let mut bindings = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .bindings
        .clone();
    if bindings.state == DesktopBindingsState::Degraded
        && bindings.last_error.as_deref() == Some(error)
    {
        return;
    }
    bindings.state = DesktopBindingsState::Degraded;
    bindings.failures = bindings.failures.saturating_add(1);
    bindings.held_output_count = 0;
    bindings.last_error = Some(bounded_error(error));
    publish_desktop_binding_status(shared, bindings);
}

#[cfg(target_os = "macos")]
fn create_desktop_sink() -> Result<Box<dyn DesktopInputSink>, String> {
    desktop_bindings::MacOsDesktopInput::new()
        .map(|sink| Box::new(sink) as Box<dyn DesktopInputSink>)
}

#[cfg(not(target_os = "macos"))]
fn create_desktop_sink() -> Result<Box<dyn DesktopInputSink>, String> {
    Err("desktop bindings are only available on macOS".to_owned())
}

fn bounded_error(error: &str) -> String {
    const MAX_ERROR_CHARS: usize = 512;
    error.chars().take(MAX_ERROR_CHARS).collect()
}

struct Supervisor {
    config: RuntimeConfig,
    status: Arc<Mutex<BridgeStatus>>,
    desktop_bindings: DesktopBindingsWorker,
    commands: Receiver<RuntimeCommand>,
    /// A safety prerequisite that failed before the supervisor started. When
    /// present, hardware stays closed and Start requests are rejected.
    startup_blocker: Option<String>,
    desired_running: bool,
    /// System sleep is imminent or in progress: every hardware handle is
    /// closed and stays closed, whatever `desired_running` says.
    suspended: bool,
    /// Hardware discovery holds off until this instant after a system wake.
    wake_settle: Option<Instant>,
    shutdown_requested: bool,
    pending_stop_acks: Vec<CommandAck>,
    pending_shutdown_acks: Vec<CommandAck>,
    preferred_xiao_serial: Option<String>,
    controller_enumerator: Option<ControllerEnumerator>,
    controller_discovery: ControllerDiscoveryState<HidSession>,
    indexed_controller_discovery: IndexedControllerDiscoveryState,
    automatic_shutdown: AutomaticShutdownRuntime,
    controller_cooldown: Option<ControllerCooldown>,
    picker_events: PickerEventSink,
}

impl Supervisor {
    fn new(
        config: RuntimeConfig,
        status: Arc<Mutex<BridgeStatus>>,
        commands: Receiver<RuntimeCommand>,
        picker_events: PickerEventSink,
        startup_blocker: Option<String>,
    ) -> Self {
        let automatic_shutdown = AutomaticShutdownRuntime::new(&config);
        let desktop_bindings =
            DesktopBindingsWorker::spawn(config.binding_profile.clone(), Arc::clone(&status));
        Self {
            picker_events,
            config,
            status,
            desktop_bindings,
            commands,
            desired_running: startup_blocker.is_none(),
            startup_blocker,
            suspended: false,
            wake_settle: None,
            shutdown_requested: false,
            pending_stop_acks: Vec::new(),
            pending_shutdown_acks: Vec::new(),
            preferred_xiao_serial: None,
            controller_enumerator: None,
            controller_discovery: ControllerDiscoveryState::new(),
            indexed_controller_discovery: IndexedControllerDiscoveryState::new(),
            automatic_shutdown,
            controller_cooldown: None,
        }
    }

    #[allow(clippy::too_many_lines)] // The supervisor keeps endpoint ownership transitions linear.
    fn run(&mut self) {
        let mut retained_output = None;
        if let Some(error) = self.startup_blocker.clone() {
            self.transition(
                RuntimeState::Error,
                "Hardware safety monitor unavailable",
                Some(&error),
            );
        }
        loop {
            self.service_idle_commands();
            if self.shutdown_requested {
                drop(retained_output.take());
                self.clear_controller_discovery();
                self.clear_hardware_status();
                let desktop_result = self.desktop_bindings.shutdown();
                if let Err(error) = &desktop_result {
                    self.transition(
                        RuntimeState::Error,
                        "Desktop-input worker shutdown failed",
                        Some(error),
                    );
                } else {
                    self.transition(RuntimeState::Stopped, "Bridge stopped", None);
                }
                acknowledge_all_with_result(&mut self.pending_shutdown_acks, &desktop_result);
                acknowledge_all(&mut self.pending_stop_acks);
                break;
            }
            if !self.desired_running || self.suspended {
                drop(retained_output.take());
                self.clear_controller_discovery();
                if self.current_state() != RuntimeState::Error {
                    let detail = if self.suspended {
                        "Suspended for system sleep"
                    } else {
                        "Bridge stopped"
                    };
                    self.transition(RuntimeState::Stopped, detail, None);
                }
                acknowledge_all(&mut self.pending_stop_acks);
                self.wait_for_command();
                continue;
            }

            if let Some(until) = self.wake_settle {
                let now = Instant::now();
                if now < until {
                    self.wait_or_command(until.saturating_duration_since(now));
                    continue;
                }
                self.wake_settle = None;
            }

            if !matches!(
                self.current_state(),
                RuntimeState::Waiting | RuntimeState::Error
            ) {
                self.transition(
                    RuntimeState::Discovering,
                    "Looking for Steam Controller 2 and XIAO",
                    None,
                );
            }
            if retained_output.is_none() {
                match self.discover_output() {
                    Discovery::Ready(output) => retained_output = Some(output),
                    Discovery::Wait { detail, error } => {
                        self.clear_hardware_status();
                        self.transition(RuntimeState::Waiting, &detail, error.as_deref());
                        self.wait_or_command(DISCOVERY_INTERVAL);
                        continue;
                    }
                    Discovery::Error(message) => {
                        self.clear_hardware_status();
                        self.transition(RuntimeState::Error, &message, Some(&message));
                        self.wait_or_command(DISCOVERY_INTERVAL);
                        continue;
                    }
                }
            }

            let controller_discovery_started = Instant::now();
            let active = match self.discover_controller_source() {
                Discovery::Ready(active) => active,
                Discovery::Wait { detail, error } => {
                    self.clear_controller_status();
                    self.transition(RuntimeState::Waiting, &detail, error.as_deref());
                    if !service_waiting_output(retained_output.as_mut()) {
                        retained_output = None;
                        self.update_status(|status| {
                            status.xiao = XiaoStatus::default();
                        });
                    }
                    let delay =
                        controller_discovery_loop_delay(controller_discovery_started.elapsed());
                    if !delay.is_zero() {
                        self.wait_or_command(delay);
                    }
                    continue;
                }
                Discovery::Error(message) => {
                    self.clear_controller_status();
                    self.transition(RuntimeState::Error, &message, Some(&message));
                    retained_output = None;
                    self.wait_or_command(DISCOVERY_INTERVAL);
                    continue;
                }
            };

            self.transition(RuntimeState::Starting, "Starting bridge", None);
            let Some(output) = retained_output.take() else {
                continue;
            };
            match self.run_active(active, output) {
                Ok((ActiveExit::SourceLost, output, _)) => {
                    retained_output = Some(output);
                    self.clear_controller_discovery();
                    self.transition(
                        RuntimeState::Discovering,
                        "Controller stopped reporting; rediscovering active input source",
                        None,
                    );
                }
                Ok((ActiveExit::OutputLost(message), _, _)) => {
                    retained_output = None;
                    self.update_status(|status| {
                        status.xiao = XiaoStatus::default();
                    });
                    self.transition(RuntimeState::Waiting, &message, Some(&message));
                }
                Ok((ActiveExit::AutomaticShutdown { info, trigger }, output, _)) => {
                    retained_output = Some(output);
                    self.controller_cooldown = Some(ControllerCooldown {
                        info,
                        until: Instant::now() + POWER_OFF_COOLDOWN,
                    });
                    self.clear_controller_discovery();
                    self.clear_controller_status();
                    self.transition(
                        RuntimeState::Waiting,
                        &format!(
                            "Controller sleeping after {}; press Steam to wake",
                            match trigger {
                                ShutdownTrigger::IdleTimeout => "idle timeout",
                                ShutdownTrigger::PuckDock => "Puck placement",
                            }
                        ),
                        None,
                    );
                }
                Ok((ActiveExit::StoppedWithAck(ack), output, cleanup)) => {
                    acknowledge_after_hardware_release(
                        output,
                        || self.clear_controller_discovery(),
                        &ack,
                        cleanup,
                    );
                    self.clear_hardware_status();
                    self.desired_running = false;
                    self.transition(RuntimeState::Stopped, "Bridge stopped", None);
                }
                Ok((ActiveExit::SuspendedWithAck(ack), output, cleanup)) => {
                    acknowledge_after_hardware_release(
                        output,
                        || self.clear_controller_discovery(),
                        &ack,
                        cleanup,
                    );
                    self.clear_hardware_status();
                    self.transition(RuntimeState::Stopped, "Suspended for system sleep", None);
                }
                Ok((ActiveExit::ShutdownWithAck(ack), output, cleanup)) => {
                    acknowledge_after_hardware_release(
                        output,
                        || self.clear_controller_discovery(),
                        &ack,
                        cleanup,
                    );
                    self.clear_hardware_status();
                    self.shutdown_requested = true;
                }
                Err(message) => {
                    retained_output = None;
                    self.desired_running = false;
                    self.clear_hardware_status();
                    self.transition(RuntimeState::Error, &message, Some(&message));
                }
            }
        }
        if self.current_state() != RuntimeState::Error {
            self.transition(RuntimeState::Stopped, "Bridge stopped", None);
        }
    }

    fn discover_output(&mut self) -> Discovery<OutputSession> {
        if self.config.output != OutputSelection::Serial {
            return make_nonserial_output(&self.config.output).map_or_else(
                Discovery::Error,
                |output| {
                    self.update_status(|status| {
                        status.xiao = XiaoStatus::default();
                    });
                    Discovery::Ready(OutputSession { output, xiao: None })
                },
            );
        }

        let devices = match available_serial_devices() {
            Ok(devices) => devices,
            Err(error) => {
                return Discovery::Wait {
                    detail: "Cannot enumerate serial ports".to_owned(),
                    error: Some(error.to_string()),
                };
            }
        };
        let candidates: Vec<_> = match &self.config.serial {
            SerialSelection::AutoXiao => devices
                .into_iter()
                .filter(SerialDeviceInfo::is_xiao_bridge)
                .collect(),
            SerialSelection::Port(path) => devices
                .into_iter()
                .filter(|device| &device.path == path)
                .collect(),
        };
        if candidates.is_empty() {
            let detail = match &self.config.serial {
                SerialSelection::AutoXiao => {
                    "Waiting for XIAO Steam Controller Bridge CDC port".to_owned()
                }
                SerialSelection::Port(path) => format!("Waiting for XIAO serial port {path}"),
            };
            return Discovery::Wait {
                detail,
                error: None,
            };
        }

        let mut valid = Vec::new();
        let mut failures = Vec::new();
        for candidate in candidates {
            match SerialOutput::open(
                &candidate.path,
                self.config.baud_rate,
                self.config.serial_config,
            ) {
                Ok(output) => valid.push((candidate, output)),
                Err(error) => failures.push(format!("{}: {error}", candidate.path)),
            }
        }
        if valid.is_empty() {
            return Discovery::Wait {
                detail: "Waiting for a XIAO that completes the protocol-v1 Hello handshake"
                    .to_owned(),
                error: (!failures.is_empty()).then(|| failures.join("; ")),
            };
        }

        let selected_index = match choose_xiao_index(&valid, self.preferred_xiao_serial.as_deref())
        {
            Ok(index) => index,
            Err(message) => return Discovery::Error(message),
        };
        let (info, output) = valid.swap_remove(selected_index);
        self.preferred_xiao_serial.clone_from(&info.serial_number);
        self.update_status(|status| {
            status.xiao = XiaoStatus {
                path: Some(info.path.clone()),
                usb_serial: info.serial_number.clone(),
                handshake_complete: true,
            };
        });
        eprintln!(
            "level=info event=xiao_ready path={:?} usb_serial={} protocol=1",
            info.path,
            masked_serial(info.serial_number.as_deref())
        );
        Discovery::Ready(OutputSession {
            output: Box::new(output),
            xiao: Some(info),
        })
    }

    fn discover_controller_source(&mut self) -> Discovery<ActiveControllerSource> {
        match self.config.controller {
            ControllerSelection::Index(index) => {
                if self.indexed_controller_discovery.scan_due() {
                    let discovered = self
                        .controller_enumerator()
                        .and_then(ControllerEnumerator::enumerate_all)
                        .map_err(|error| error.to_string());
                    self.indexed_controller_discovery.refresh(index, discovered);
                }
                if let Some(error) = self.indexed_controller_discovery.scan_error() {
                    return Discovery::Wait {
                        detail: "Cannot enumerate Steam Controller HID collections".to_owned(),
                        error: Some(error.to_owned()),
                    };
                }
                let Some(info) = self.indexed_controller_discovery.info().cloned() else {
                    return Discovery::Wait {
                        detail: format!("Waiting for Steam Controller collection index {index}"),
                        error: None,
                    };
                };
                if !info.is_supported_controller_source() {
                    return Discovery::Error(format!(
                        "collection index {index} is not a supported Steam Controller 2 input; \
                         expected a 28de:1304 USB Puck ff00:0001 interface 2-5 or the \
                         28de:1303 Bluetooth ff00:0001 interface -1 collection"
                    ));
                }
                if self.source_on_cooldown(&info) {
                    return Discovery::Wait {
                        detail:
                            "Controller is finishing automatic shutdown; waiting for a fresh wake"
                                .to_owned(),
                        error: None,
                    };
                }
                match self
                    .controller_enumerator()
                    .and_then(|enumerator| enumerator.open(&info))
                {
                    Ok(mut session) => {
                        // Consume the synthetic open event here. The worker has
                        // already performed its initial suppression before it
                        // forwards any lifecycle or input event.
                        let _ = session.poll(Duration::ZERO);
                        self.update_source_discovered(&info, false);
                        self.indexed_controller_discovery.clear();
                        Discovery::Ready(ActiveControllerSource {
                            info,
                            session,
                            controller_seen: false,
                        })
                    }
                    Err(error) => Discovery::Wait {
                        detail: format!(
                            "Waiting to open Steam Controller collection index {index}"
                        ),
                        error: Some(ownership_guidance(&error)),
                    },
                }
            }
            ControllerSelection::AutoActive => self.discover_active_controller_source(),
        }
    }

    fn discover_active_controller_source(&mut self) -> Discovery<ActiveControllerSource> {
        if self.controller_discovery.scan_due() {
            let discovered = self
                .enumerate_controller_candidates()
                .map_err(|error| error.to_string());
            // Borrowed as a separate field so the open closure can reuse the
            // shared context while the discovery state is mutated.
            let enumerator = self.controller_enumerator.as_ref();
            self.controller_discovery
                .refresh(discovered, |_, info| match enumerator {
                    Some(enumerator) => enumerator.open(info).map_err(|error| {
                        format!(
                            "{}: {}",
                            controller_source_identity(info),
                            ownership_guidance(&error)
                        )
                    }),
                    None => Err("the HID context is unavailable".to_owned()),
                });
        }

        if self.controller_discovery.is_empty() {
            if let Some(error) = self.controller_discovery.scan_error() {
                return Discovery::Wait {
                    detail: "Cannot enumerate Steam Controller HID collections".to_owned(),
                    error: Some(error.to_owned()),
                };
            }
            if self.controller_discovery.supported_devices_seen() {
                return Discovery::Wait {
                    detail: "Steam Controller input found, but no collection can be opened"
                        .to_owned(),
                    error: self.controller_discovery.current_errors(&[]),
                };
            }
            return Discovery::Wait {
                detail: "Waiting for a Steam Controller 2 Puck or Bluetooth connection".to_owned(),
                error: None,
            };
        }

        let probe = self.controller_discovery.probe();
        match choose_unique_active(&probe.active_indices) {
            Ok(None) => Discovery::Wait {
                detail: "Steam Controller input found; waiting for valid controller state"
                    .to_owned(),
                error: self.controller_discovery.current_errors(&probe.failures),
            },
            Ok(Some(selected)) => {
                let selected_info = self.controller_discovery.candidate(selected).info.clone();
                if self.source_on_cooldown(&selected_info) {
                    return Discovery::Wait {
                        detail:
                            "Controller is finishing automatic shutdown; waiting for a fresh wake"
                                .to_owned(),
                        error: None,
                    };
                }
                let candidate = self.controller_discovery.select(selected);
                let info = candidate.info;
                self.update_source_discovered(&info, true);
                Discovery::Ready(ActiveControllerSource {
                    info,
                    session: candidate.session,
                    controller_seen: true,
                })
            }
            Err(active_indices) => {
                let global = self
                    .controller_enumerator()
                    .and_then(ControllerEnumerator::enumerate_all);
                let global_indices_available = match global {
                    Ok(devices) => self
                        .controller_discovery
                        .resolve_global_indices(&devices)
                        .is_ok(),
                    Err(_) => false,
                };
                let sources = active_indices
                    .iter()
                    .map(|index| {
                        let candidate = self.controller_discovery.candidate(*index);
                        if global_indices_available {
                            controller_source_description(
                                candidate.enumeration_index,
                                &candidate.info,
                            )
                        } else {
                            controller_source_identity(&candidate.info)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                Discovery::Error(format!(
                    "multiple active Steam Controller 2 input sources were detected: {sources}; \
                     run sc-probe list and restart with --index N"
                ))
            }
        }
    }

    /// Returns the shared HID context, building it on first use.
    ///
    /// One context serves filtered scans, full-inventory scans, and opening
    /// sessions. Constructing a context enumerates every collection in the
    /// system, so creating one per scan or per open attempt is what made idle
    /// discovery expensive.
    fn controller_enumerator(&mut self) -> Result<&mut ControllerEnumerator, DeviceError> {
        if self.controller_enumerator.is_none() {
            self.controller_enumerator = Some(ControllerEnumerator::new()?);
        }
        Ok(self
            .controller_enumerator
            .as_mut()
            .expect("controller enumerator was initialized"))
    }

    fn enumerate_controller_candidates(
        &mut self,
    ) -> Result<Vec<(usize, HidDeviceInfo)>, DeviceError> {
        self.controller_enumerator()
            .and_then(ControllerEnumerator::enumerate)
            .map(|devices| devices.into_iter().enumerate().collect())
    }

    fn clear_controller_discovery(&mut self) {
        self.controller_discovery.clear();
        self.indexed_controller_discovery.clear();
    }

    fn source_on_cooldown(&mut self, info: &HidDeviceInfo) -> bool {
        let now = Instant::now();
        if self
            .controller_cooldown
            .as_ref()
            .is_some_and(|cooldown| now >= cooldown.until)
        {
            self.controller_cooldown = None;
        }
        self.controller_cooldown.as_ref().is_some_and(|cooldown| {
            same_controller_collection(&cooldown.info, info) && now < cooldown.until
        })
    }

    #[allow(clippy::too_many_lines)] // Safety ordering is clearest in one linear ownership loop.
    fn run_active(
        &mut self,
        active: ActiveControllerSource,
        mut output: OutputSession,
    ) -> Result<(ActiveExit, OutputSession, Result<(), String>), String> {
        self.automatic_shutdown
            .source_selected(&active.info, &self.config);
        let initial_controller_seen = active.controller_seen;
        let mut engine = BridgeEngine::new(self.config.bridge, self.config.mapper)
            .map_err(|error| error.to_string())?;
        let mut recording = self.open_recording()?;
        let started = Instant::now();
        let dropped = Arc::new(AtomicU64::new(0));
        let mut worker =
            match HidWorker::spawn(active, self.config.lizard_mode, Arc::clone(&dropped)) {
                Ok(worker) => worker,
                Err(error) => {
                    let _ = engine.shutdown(&mut *output.output);
                    return Err(error);
                }
            };
        if let Err(error) = output.output.service() {
            let _ = engine.shutdown(&mut *output.output);
            worker.shutdown()?;
            return Err(format!(
                "XIAO service failed before input activation: {error}"
            ));
        }
        while output.output.take_feedback().is_some() {
            // Feedback received before the input worker exists is not a valid
            // post-reconnect lease. A continuing effect will be refreshed by
            // the XIAO within 25 ms.
        }
        let mut last_controller_state = Instant::now();
        let mut controller_state_seen = initial_controller_seen;
        let mut controller_connected = initial_controller_seen;
        let mut last_status = Instant::now()
            .checked_sub(STATUS_INTERVAL)
            .unwrap_or_else(Instant::now);
        let mut idle_activity = IdleActivityTracker::new(self.config.idle_shutdown_timeout);
        let mut latest_charge_state: Option<ControllerChargeState> = None;
        let mut last_charge_report = None;
        let mut pending_automatic_shutdown = None;
        self.desktop_bindings.enable_async();
        let mut picker = PickerRuntime::new(self.config.profile_picker);
        engine.connected();
        self.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
        self.automatic_shutdown.trigger = None;
        self.transition(RuntimeState::Running, "Bridge running", None);
        let automatic_status = self
            .automatic_shutdown
            .status(&self.config, None, Instant::now());
        let binding_status = self.desktop_bindings.status();
        self.update_status(|status| {
            status.source.connected = true;
            status.source.active = initial_controller_seen;
            status.controller.connected = initial_controller_seen;
            status.controller.last_state_age = initial_controller_seen.then_some(Duration::ZERO);
            status.lizard = worker.lizard_diagnostics();
            status.haptics = worker.haptics_diagnostics();
            status.bindings = binding_status;
            status.profile_picker = picker_status(&self.config, false);
            status.automatic_shutdown = automatic_status;
        });
        eprintln!(
            "level=info event=bridge_running input_transport={:?} input_interface={} \
             input_product={:?} input_serial={} xiao_path={:?} lizard_mode={:?}",
            worker.device_info().controller_transport(),
            worker.device_info().interface_number,
            worker.device_info().product,
            masked_serial(worker.device_info().serial_number.as_deref()),
            output.xiao.as_ref().map(|info| info.path.as_str()),
            self.config.lizard_mode
        );
        record_device_event(
            &mut recording,
            started,
            KIND_DEVICE_CONNECTED,
            Some(worker.device_info()),
        )?;

        let exit = 'active: loop {
            let mut iteration_timer = SupervisorIterationTimer::new("commands");
            let command_exit = self.service_active_commands(
                started.elapsed(),
                &mut idle_activity,
                &mut picker,
                &mut engine,
                &mut output,
                &worker,
            );
            iteration_timer.enter("worker_health");
            if let Some(command_exit) = command_exit {
                break command_exit;
            }
            if let Some(error) = worker.take_failure() {
                self.dismiss_picker(&mut picker);
                let _ = engine.shutdown(&mut *output.output);
                let _ = self.desktop_bindings.disconnect();
                worker.shutdown()?;
                self.clear_controller_status();
                return Err(error);
            }
            let mut direct_report = None;
            iteration_timer.enter("hid_wait");
            match worker.receiver.recv_timeout(RUNTIME_POLL_INTERVAL) {
                Ok(HidWorkerEvent::Connected(info)) => {
                    self.update_source_discovered(&info, false);
                }
                Ok(HidWorkerEvent::Disconnected) => {
                    let _ = engine.disconnected(&mut *output.output);
                    break ActiveExit::SourceLost;
                }
                Ok(HidWorkerEvent::StatusReport(report)) => {
                    direct_report = Some(report);
                }
                Ok(HidWorkerEvent::ReportReady) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break ActiveExit::SourceLost;
                }
            }
            iteration_timer.enter("mailbox");
            let batch = worker.take_report_batch();
            if batch.overflowed {
                self.desktop_bindings.overflow();
                worker.clear_pad_feedback();
                eprintln!(
                    "level=warn event=desktop_binding_mailbox_overflow action=release_and_rebaseline"
                );
            }
            iteration_timer.enter("controller_reports");
            for report in direct_report.into_iter().chain(batch.reports) {
                match process_report(
                    &report,
                    &mut engine,
                    &mut *output.output,
                    &mut recording,
                    started,
                    &mut idle_activity,
                ) {
                    Ok(ReportEffect::ControllerState {
                        meaningful_activity,
                        desktop_input,
                        picker_input,
                    }) => {
                        let now = started.elapsed();
                        let was_open = picker.is_open();
                        let events = picker.observe(now, &picker_input, self.config.picker_roster);
                        let tapped = events
                            .iter()
                            .any(|event| matches!(event, PickerEvent::TriggerTapped));
                        for event in events {
                            self.emit_picker_event(event);
                        }
                        // Refreshed every report, not just on an open/close
                        // edge: the wheel keeps withholding the button that
                        // closed it until the user lets go, so what is
                        // suppressed changes while the wheel is already shut.
                        engine.set_output_suppression(picker.suppression());
                        if picker.is_open() != was_open {
                            let picker_status = picker_status(&self.config, picker.is_open());
                            self.update_status(|status| status.profile_picker = picker_status);
                        }
                        if tapped {
                            // The hold swallowed the press to keep a Quick
                            // Access binding from firing. It turned out to be
                            // an ordinary press, so deliver it now as a tap:
                            // this synthesizes the down edge, and the real
                            // snapshot below is already the matching up edge.
                            self.desktop_bindings.observe(DesktopInputSnapshot {
                                buttons: profile_picker::with_trigger(desktop_input.buttons),
                                ..desktop_input
                            });
                        }
                        let desktop_input = DesktopInputSnapshot {
                            buttons: picker.mask_trigger(desktop_input.buttons),
                            ..desktop_input
                        };
                        self.desktop_bindings.observe(desktop_input);
                        last_controller_state = Instant::now();
                        controller_connected = true;
                        if meaningful_activity {
                            self.automatic_shutdown
                                .activity_after_failed_dock_attempt(Instant::now());
                        }
                        // Only the first state publishes eagerly, so the UI reacts
                        // without waiting for STATUS_INTERVAL. Doing this per report
                        // would clone the whole status and take the shared lock at
                        // controller report rate; the periodic block below keeps
                        // these same fields current afterwards.
                        if !controller_state_seen {
                            controller_state_seen = true;
                            self.update_status(|status| {
                                status.source.active = true;
                                status.controller.connected = true;
                                status.controller.last_state_age = Some(Duration::ZERO);
                            });
                        }
                    }
                    Ok(ReportEffect::Connected) => {
                        controller_connected = true;
                    }
                    Ok(ReportEffect::Battery {
                        percent,
                        charge_state,
                    }) => {
                        let charging_transition = latest_charge_state
                            .is_some_and(|previous| !previous.is_external_power())
                            && charge_state.is_external_power();
                        latest_charge_state = Some(charge_state);
                        last_charge_report = Some(Instant::now());
                        if charging_transition {
                            idle_activity.reset(started.elapsed());
                            eprintln!(
                                "level=info event=idle_shutdown_timer_reset reason=charging_transition"
                            );
                        }
                        let dock_event = self.automatic_shutdown.observe_charge_state(
                            worker.device_info(),
                            charge_state,
                            self.config.puck_dock_action,
                        );
                        self.update_status(|status| {
                            status.battery_percent = percent;
                            status.battery_charge_state = Some(charge_state);
                        });
                        if dock_event && self.automatic_shutdown.retry_due(Instant::now()) {
                            eprintln!(
                                "level=info event=puck_dock_detected charge_state={charge_state:?}"
                            );
                            pending_automatic_shutdown = Some(ShutdownTrigger::PuckDock);
                        }
                    }
                    Ok(ReportEffect::Disconnected) => {
                        let _ = engine.disconnected(&mut *output.output);
                        break 'active ActiveExit::SourceLost;
                    }
                    Ok(ReportEffect::None) => {}
                    Err(error) if is_output_error(&error) => {
                        break 'active ActiveExit::OutputLost(format!(
                            "XIAO output failed; waiting for reconnect: {error}"
                        ));
                    }
                    Err(error) => {
                        eprintln!("level=warn event=report_processing_failed error={error:?}");
                    }
                }
            }
            iteration_timer.enter("desktop_worker_output");
            let desktop_output = self.desktop_bindings.take_output();
            if desktop_output.discard_pending_feedback {
                worker.clear_pad_feedback();
            }
            worker.request_pad_feedback(desktop_output.feedback);
            let lost = dropped.swap(0, Ordering::AcqRel);
            if lost > 0 {
                engine.note_dropped_reports(lost);
            }
            iteration_timer.enter("output_service");
            let service_result = if should_tick_input_timeout(lost, worker.has_pending_report()) {
                engine
                    .tick(started.elapsed(), &mut *output.output)
                    .map(|_| ())
            } else {
                output
                    .output
                    .service()
                    .map_err(bridge_core::BridgeError::Output)
            };
            if let Err(error) = service_result {
                break ActiveExit::OutputLost(format!(
                    "XIAO service failed; waiting for reconnect: {error}"
                ));
            }
            iteration_timer.enter("output_feedback");
            while let Some(feedback) = output.output.take_feedback() {
                match feedback {
                    OutputFeedback::Rumble {
                        low_frequency,
                        high_frequency,
                    } => worker.set_rumble(low_frequency, high_frequency),
                }
            }
            iteration_timer.enter("automatic_shutdown");
            let now = Instant::now();
            let dock_retry_due = self.automatic_shutdown.trigger == Some(ShutdownTrigger::PuckDock)
                && self.automatic_shutdown.phase == AutomaticShutdownPhase::Degraded
                && !self.automatic_shutdown.dock_episode_handled
                && self.automatic_shutdown.retry_due(now)
                && worker.device_info().controller_transport() == Some(ControllerTransport::Puck)
                && latest_charge_state.is_some_and(ControllerChargeState::is_external_power)
                && last_charge_report.is_some_and(|reported| {
                    now.saturating_duration_since(reported) <= BATTERY_STATUS_FRESHNESS
                });
            let idle_shutdown_due = idle_activity.deadline_reached(started.elapsed())
                && idle_activity.is_neutral()
                && self.automatic_shutdown.retry_due(now);
            let automatic_trigger = pending_automatic_shutdown
                .take()
                .or_else(|| dock_retry_due.then_some(ShutdownTrigger::PuckDock))
                .or_else(|| idle_shutdown_due.then_some(ShutdownTrigger::IdleTimeout));
            if let Some(trigger) = automatic_trigger {
                if output.xiao.is_none() {
                    eprintln!(
                        "level=warn event=automatic_shutdown_skipped trigger={trigger:?} reason=xiao_not_ready"
                    );
                } else if trigger != ShutdownTrigger::IdleTimeout || idle_activity.is_neutral() {
                    self.automatic_shutdown.begin(trigger);
                    let powering_off = self.automatic_shutdown.status(
                        &self.config,
                        idle_activity.idle_age(started.elapsed()),
                        now,
                    );
                    self.update_status(|status| status.automatic_shutdown = powering_off);
                    match engine.shutdown(&mut *output.output) {
                        Ok(_) => match worker.power_off() {
                            Ok(()) => {
                                self.automatic_shutdown.succeeded(Instant::now(), trigger);
                                let automatic = self.automatic_shutdown.status(
                                    &self.config,
                                    idle_activity.idle_age(started.elapsed()),
                                    Instant::now(),
                                );
                                self.update_status(|status| {
                                    status.automatic_shutdown = automatic;
                                    status.last_error = None;
                                });
                                break ActiveExit::AutomaticShutdown {
                                    info: worker.device_info().clone(),
                                    trigger,
                                };
                            }
                            Err(error) => {
                                self.automatic_shutdown
                                    .failed(Instant::now(), trigger, &error);
                                let automatic = self.automatic_shutdown.status(
                                    &self.config,
                                    idle_activity.idle_age(started.elapsed()),
                                    Instant::now(),
                                );
                                self.update_status(|status| {
                                    status.automatic_shutdown = automatic;
                                    status.last_error = Some(format!(
                                        "automatic controller shutdown failed; gameplay continues: {error}"
                                    ));
                                });
                            }
                        },
                        Err(error) => {
                            break ActiveExit::OutputLost(format!(
                                "cannot neutralize XIAO before automatic controller shutdown: {error}"
                            ));
                        }
                    }
                }
            }
            if last_controller_state.elapsed() >= ACTIVE_SLOT_TIMEOUT {
                let _ = engine.disconnected(&mut *output.output);
                break ActiveExit::SourceLost;
            }
            iteration_timer.enter("status_update");
            if last_status.elapsed() >= STATUS_INTERVAL {
                let controller_age = last_controller_state.elapsed();
                let automatic = self.automatic_shutdown.status(
                    &self.config,
                    idle_activity.idle_age(started.elapsed()),
                    Instant::now(),
                );
                let binding_status = self.desktop_bindings.status();
                self.update_status(|status| {
                    status.bridge_metrics = engine.metrics();
                    status.output_diagnostics = output.output.diagnostics();
                    status.lizard = worker.lizard_diagnostics();
                    status.haptics = worker.haptics_diagnostics();
                    status.bindings = binding_status;
                    status.profile_picker = picker_status(&self.config, picker.is_open());
                    status.source.active =
                        controller_state_seen && controller_age < ACTIVE_SLOT_TIMEOUT;
                    status.controller.connected =
                        controller_connected && controller_age < ACTIVE_SLOT_TIMEOUT;
                    status.controller.last_state_age =
                        controller_connected.then_some(controller_age);
                    status.automatic_shutdown = automatic;
                });
                last_status = Instant::now();
            }
        };

        self.transition(RuntimeState::Stopping, "Neutralizing output", None);
        self.dismiss_picker(&mut picker);
        let neutral_result = engine.shutdown(&mut *output.output);
        let worker_result = worker.shutdown();
        let desktop_result = self.desktop_bindings.disconnect();
        idle_activity.pause();
        let automatic = self
            .automatic_shutdown
            .status(&self.config, None, Instant::now());
        let binding_status = self.desktop_bindings.status();
        self.update_status(|status| {
            status.bridge_metrics = engine.metrics();
            status.output_diagnostics = output.output.diagnostics();
            status.lizard = worker.lizard_diagnostics();
            status.haptics = worker.haptics_diagnostics();
            status.bindings = binding_status;
            status.automatic_shutdown = automatic;
        });
        self.clear_controller_status();
        let recording_result =
            record_device_event(&mut recording, started, KIND_DEVICE_DISCONNECTED, None);
        let neutral_result = neutral_result
            .map(|_| ())
            .map_err(|error| format!("cannot neutralize XIAO before HID release: {error}"));
        let required_cleanup =
            worker_result
                .and(recording_result)
                .and(if matches!(exit, ActiveExit::OutputLost(_)) {
                    Ok(())
                } else {
                    neutral_result
                });
        if exit.has_acknowledgement() {
            return Ok((exit, output, required_cleanup.and(desktop_result)));
        }
        required_cleanup?;
        if let Err(error) = desktop_result {
            eprintln!("level=warn event=desktop_input_worker_disconnect_failed error={error:?}");
        }
        Ok((exit, output, Ok(())))
    }

    fn open_recording(&self) -> Result<Option<RecordingWriter<File>>, String> {
        self.config
            .recording_path
            .as_ref()
            .map(|path| {
                File::create(path)
                    .map(RecordingWriter::new)
                    .map_err(|error| {
                        format!("cannot create recording '{}': {error}", path.display())
                    })
            })
            .transpose()
    }

    fn service_idle_commands(&mut self) {
        while let Ok(command) = self.commands.try_recv() {
            self.apply_idle_command(command);
        }
    }

    fn wait_for_command(&mut self) {
        if let Ok(command) = self.commands.recv_timeout(DISCOVERY_INTERVAL) {
            self.apply_idle_command(command);
        }
    }

    fn wait_or_command(&mut self, duration: Duration) {
        if let Ok(command) = self.commands.recv_timeout(duration) {
            self.apply_idle_command(command);
        }
    }

    fn apply_idle_command(&mut self, command: RuntimeCommand) {
        match command {
            RuntimeCommand::Start(ack) => {
                if let Some(error) = &self.startup_blocker {
                    let _ = ack.send(Err(error.clone()));
                } else {
                    self.desired_running = true;
                    let _ = ack.send(Ok(()));
                }
            }
            RuntimeCommand::Stop(ack) => {
                self.desired_running = false;
                self.transition(RuntimeState::Stopping, "Stopping bridge", None);
                self.clear_hardware_status();
                self.pending_stop_acks.push(ack);
            }
            RuntimeCommand::SuspendForSleep(ack) => {
                self.suspended = true;
                self.wake_settle = None;
                self.transition(RuntimeState::Stopping, "Suspending for system sleep", None);
                self.clear_hardware_status();
                // Acknowledged with the stop acks, after every handle is gone.
                self.pending_stop_acks.push(ack);
            }
            RuntimeCommand::ResumeFromWake(ack) => {
                if self.suspended {
                    self.suspended = false;
                    self.wake_settle = Some(Instant::now() + WAKE_SETTLE_DELAY);
                }
                let _ = ack.send(Ok(()));
            }
            RuntimeCommand::Shutdown(ack) => {
                self.desired_running = false;
                self.shutdown_requested = true;
                self.transition(RuntimeState::Stopping, "Stopping bridge", None);
                self.clear_hardware_status();
                self.pending_shutdown_acks.push(ack);
            }
            RuntimeCommand::SetIdleShutdown(timeout, ack) => {
                let result = validate_idle_shutdown_timeout(timeout);
                if result.is_ok() {
                    self.config.idle_shutdown_timeout = timeout;
                    self.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
                    self.update_status(|status| {
                        status.automatic_shutdown.configured_timeout = timeout;
                        status.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
                        status.automatic_shutdown.neutral_idle_age = None;
                    });
                }
                let _ = ack.send(result);
            }
            RuntimeCommand::SetPuckDockAction(action, ack) => {
                self.automatic_shutdown
                    .set_dock_action(action, &self.config);
                self.config.puck_dock_action = action;
                self.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
                let automatic = self
                    .automatic_shutdown
                    .status(&self.config, None, Instant::now());
                self.update_status(|status| status.automatic_shutdown = automatic);
                let _ = ack.send(Ok(()));
            }
            RuntimeCommand::SetBindingProfile(profile, ack) => {
                let profile = *profile;
                self.config.binding_profile.clone_from(&profile);
                self.desktop_bindings.replace_profile(profile, ack);
            }
            RuntimeCommand::EnableDesktopBindings(ack) => {
                self.desktop_bindings.enable(ack);
            }
            RuntimeCommand::SetPickerConfig(config, ack) => {
                self.config.profile_picker = config.map(PickerConfig::sanitized);
                let picker_status = picker_status(&self.config, false);
                self.update_status(|status| status.profile_picker = picker_status);
                let _ = ack.send(Ok(()));
            }
            RuntimeCommand::SetPickerRoster(roster, ack) => {
                self.config.picker_roster = roster;
                let picker_status = picker_status(&self.config, false);
                self.update_status(|status| status.profile_picker = picker_status);
                let _ = ack.send(Ok(()));
            }
        }
    }

    // Commands act on the whole active session, and bundling its parts into a
    // struct only to unpack them again here would hide which ones each command
    // actually touches.
    #[allow(clippy::too_many_arguments)]
    fn service_active_commands(
        &mut self,
        now: Duration,
        idle_activity: &mut IdleActivityTracker,
        picker: &mut PickerRuntime,
        engine: &mut BridgeEngine,
        output: &mut OutputSession,
        worker: &HidWorker,
    ) -> Option<ActiveExit> {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                RuntimeCommand::Start(ack) => {
                    if let Some(error) = &self.startup_blocker {
                        let _ = ack.send(Err(error.clone()));
                    } else {
                        let _ = ack.send(Ok(()));
                    }
                }
                RuntimeCommand::Stop(ack) => {
                    self.desired_running = false;
                    // The active loop acknowledges after its neutral-before-release cleanup.
                    return Some(ActiveExit::StoppedWithAck(ack));
                }
                RuntimeCommand::SuspendForSleep(ack) => {
                    self.suspended = true;
                    self.wake_settle = None;
                    // Same cleanup as a stop: the device is parked at neutral
                    // and every handle is closed before the ack lets the
                    // caller's sleep handler return.
                    return Some(ActiveExit::SuspendedWithAck(ack));
                }
                RuntimeCommand::ResumeFromWake(ack) => {
                    // Already awake and running; nothing to resume.
                    let _ = ack.send(Ok(()));
                }
                RuntimeCommand::Shutdown(ack) => {
                    self.desired_running = false;
                    self.shutdown_requested = true;
                    return Some(ActiveExit::ShutdownWithAck(ack));
                }
                RuntimeCommand::SetIdleShutdown(timeout, ack) => {
                    let result = validate_idle_shutdown_timeout(timeout);
                    if result.is_ok() {
                        self.config.idle_shutdown_timeout = timeout;
                        idle_activity.set_timeout(timeout, now);
                        self.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
                        eprintln!(
                            "level=info event=idle_shutdown_setting_changed timeout_secs={:?}",
                            timeout.map(|value| value.as_secs())
                        );
                    }
                    let _ = ack.send(result);
                }
                RuntimeCommand::SetPuckDockAction(action, ack) => {
                    self.automatic_shutdown
                        .set_dock_action(action, &self.config);
                    self.config.puck_dock_action = action;
                    self.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
                    eprintln!("level=info event=puck_dock_action_changed action={action:?}");
                    let _ = ack.send(Ok(()));
                }
                RuntimeCommand::SetBindingProfile(profile, ack) => {
                    neutralize_before_desktop_work(engine, output);
                    worker.clear_pad_feedback();
                    let profile = *profile;
                    self.config.binding_profile.clone_from(&profile);
                    self.desktop_bindings.replace_profile(profile, ack);
                }
                RuntimeCommand::EnableDesktopBindings(ack) => {
                    neutralize_before_desktop_work(engine, output);
                    self.desktop_bindings.enable(ack);
                }
                RuntimeCommand::SetPickerConfig(config, ack) => {
                    self.config.profile_picker = config.map(PickerConfig::sanitized);
                    // A reconfigured wheel is a closed wheel — and a cancelled
                    // hold counts too, or the overlay spawned at `Preparing`
                    // would outlive the hold it was spawned for. The picker
                    // latches whatever is still held, so the suppression it
                    // reports (not a blanket clear) is what hands the game its
                    // controls back without leaking the in-flight press.
                    if picker.set_config(self.config.profile_picker) {
                        engine.set_output_suppression(picker.suppression());
                        self.emit_picker_event(PickerEvent::Dismissed);
                    }
                    let picker_status = picker_status(&self.config, false);
                    self.update_status(|status| status.profile_picker = picker_status);
                    eprintln!(
                        "level=info event=profile_picker_configured enabled={}",
                        self.config.profile_picker.is_some()
                    );
                    let _ = ack.send(Ok(()));
                }
                RuntimeCommand::SetPickerRoster(roster, ack) => {
                    self.config.picker_roster = roster;
                    let picker_status = picker_status(&self.config, picker.is_open());
                    self.update_status(|status| status.profile_picker = picker_status);
                    let _ = ack.send(Ok(()));
                }
            }
        }
        None
    }

    fn emit_picker_event(&self, event: PickerEvent) {
        (self.picker_events)(event);
    }

    /// Closes the wheel and tells the frontend, for a controller that went away
    /// or a session that is ending. Suppression dies with the engine.
    fn dismiss_picker(&self, picker: &mut PickerRuntime) {
        if picker.close() {
            self.emit_picker_event(PickerEvent::Dismissed);
            let picker_status = picker_status(&self.config, false);
            self.update_status(|status| status.profile_picker = picker_status);
        }
    }

    fn transition(&self, state: RuntimeState, detail: &str, error: Option<&str>) {
        self.update_status(|status| {
            status.state = state;
            detail.clone_into(&mut status.detail);
            if let Some(error) = error {
                status.last_error = Some(error.to_owned());
            } else if matches!(state, RuntimeState::Running | RuntimeState::Stopped) {
                status.last_error = None;
            }
        });
    }

    fn current_state(&self) -> RuntimeState {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state
    }

    fn update_source_discovered(&self, info: &HidDeviceInfo, active: bool) {
        self.update_status(|status| {
            status.source = ControllerSourceStatus {
                identity: Some(info.clone()),
                transport: info.controller_transport(),
                connected: true,
                active,
            };
        });
    }

    fn clear_controller_status(&self) {
        self.update_status(|status| {
            status.source = ControllerSourceStatus::default();
            status.controller = ControllerStatus::default();
            status.battery_percent = None;
            status.battery_charge_state = None;
            status.lizard = LizardStatus::default();
        });
    }

    fn clear_hardware_status(&self) {
        self.update_status(|status| {
            status.source = ControllerSourceStatus::default();
            status.controller = ControllerStatus::default();
            status.xiao = XiaoStatus::default();
            status.battery_percent = None;
            status.battery_charge_state = None;
            status.lizard = LizardStatus::default();
        });
    }

    fn update_status(&self, update: impl FnOnce(&mut BridgeStatus)) -> bool {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = status.clone();
        update(&mut status);
        let changed = status.state != previous.state
            || status.detail != previous.detail
            || status.source != previous.source
            || status.controller != previous.controller
            || status.xiao != previous.xiao
            || status.battery_percent != previous.battery_percent
            || status.battery_charge_state != previous.battery_charge_state
            || status.lizard != previous.lizard
            || status.haptics != previous.haptics
            || status.bindings != previous.bindings
            || status.profile_picker != previous.profile_picker
            || status.automatic_shutdown != previous.automatic_shutdown
            || status.bridge_metrics != previous.bridge_metrics
            || status.output_diagnostics != previous.output_diagnostics
            || status.last_error != previous.last_error;
        if changed {
            status.revision = status.revision.wrapping_add(1);
        }
        changed
    }
}

trait ControllerProbeSession {
    fn poll_for_discovery(&mut self, timeout: Duration) -> Result<Option<DeviceEvent>, String>;
}

impl ControllerProbeSession for HidSession {
    fn poll_for_discovery(&mut self, timeout: Duration) -> Result<Option<DeviceEvent>, String> {
        self.poll(timeout).map_err(|error| error.to_string())
    }
}

struct ControllerCandidate<S> {
    enumeration_index: usize,
    info: HidDeviceInfo,
    session: S,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ControllerReconcileMetrics {
    opened: usize,
    reused: usize,
    removed: usize,
    failures: usize,
}

struct ControllerProbe {
    active_indices: Vec<usize>,
    failures: Vec<String>,
}

struct IndexedControllerDiscoveryState {
    info: Option<HidDeviceInfo>,
    next_scan: Instant,
    stable_scan_interval: Duration,
    scan_error: Option<String>,
}

impl IndexedControllerDiscoveryState {
    fn new() -> Self {
        Self {
            info: None,
            next_scan: Instant::now(),
            stable_scan_interval: MIN_STABLE_CONTROLLER_SCAN_INTERVAL,
            scan_error: None,
        }
    }

    fn scan_due(&self) -> bool {
        Instant::now() >= self.next_scan
    }

    fn refresh(&mut self, index: usize, discovered: Result<Vec<HidDeviceInfo>, String>) {
        let previous = self.info.take();
        match discovered {
            Ok(devices) => {
                self.info = devices.get(index).cloned();
                self.scan_error = None;
            }
            Err(error) => {
                self.info = None;
                self.scan_error = Some(error);
            }
        }
        let unchanged = previous
            .as_ref()
            .zip(self.info.as_ref())
            .is_some_and(|(previous, current)| same_controller_collection(previous, current));
        self.stable_scan_interval = if unchanged {
            next_stable_controller_scan_interval(self.stable_scan_interval)
        } else {
            MIN_STABLE_CONTROLLER_SCAN_INTERVAL
        };
        self.next_scan = Instant::now()
            + controller_inventory_scan_interval(self.info.is_some(), self.stable_scan_interval);
    }

    fn clear(&mut self) {
        self.info = None;
        self.next_scan = Instant::now();
        self.stable_scan_interval = MIN_STABLE_CONTROLLER_SCAN_INTERVAL;
        self.scan_error = None;
    }

    fn info(&self) -> Option<&HidDeviceInfo> {
        self.info.as_ref()
    }

    fn scan_error(&self) -> Option<&str> {
        self.scan_error.as_deref()
    }
}

// Keep inactive collections open until the HID inventory changes. Reopening all
// Puck slots for every probe creates native reader threads repeatedly and, on
// macOS, leaves IOHID-owned report buffers retained by the main run loop.
struct ControllerDiscoveryState<S> {
    candidates: Vec<ControllerCandidate<S>>,
    next_scan: Instant,
    stable_scan_interval: Duration,
    supported_devices_seen: bool,
    open_failures: Vec<String>,
    scan_error: Option<String>,
}

impl<S> ControllerDiscoveryState<S> {
    fn new() -> Self {
        Self {
            candidates: Vec::new(),
            next_scan: Instant::now(),
            stable_scan_interval: MIN_STABLE_CONTROLLER_SCAN_INTERVAL,
            supported_devices_seen: false,
            open_failures: Vec::new(),
            scan_error: None,
        }
    }

    fn scan_due(&self) -> bool {
        Instant::now() >= self.next_scan
    }

    fn refresh(
        &mut self,
        discovered: Result<Vec<(usize, HidDeviceInfo)>, String>,
        mut open: impl FnMut(usize, &HidDeviceInfo) -> Result<S, String>,
    ) -> ControllerReconcileMetrics {
        let Ok(discovered) = discovered else {
            self.scan_error = discovered.err();
            self.stable_scan_interval = MIN_STABLE_CONTROLLER_SCAN_INTERVAL;
            self.next_scan = Instant::now()
                + controller_inventory_scan_interval(
                    !self.candidates.is_empty(),
                    self.stable_scan_interval,
                );
            return ControllerReconcileMetrics::default();
        };

        self.scan_error = None;
        self.supported_devices_seen = !discovered.is_empty();
        self.open_failures.clear();

        let old_count = self.candidates.len();
        let mut existing: Vec<_> = self.candidates.drain(..).map(Some).collect();
        let mut reconciled = Vec::with_capacity(discovered.len());
        let mut metrics = ControllerReconcileMetrics::default();

        for (enumeration_index, info) in discovered {
            let existing_index = existing.iter().position(|candidate| {
                candidate
                    .as_ref()
                    .is_some_and(|candidate| same_controller_collection(&candidate.info, &info))
            });
            if let Some(existing_index) = existing_index {
                let mut candidate = existing[existing_index]
                    .take()
                    .expect("matched candidate exists");
                candidate.enumeration_index = enumeration_index;
                candidate.info = info;
                reconciled.push(candidate);
                metrics.reused += 1;
                continue;
            }

            match open(enumeration_index, &info) {
                Ok(session) => {
                    reconciled.push(ControllerCandidate {
                        enumeration_index,
                        info,
                        session,
                    });
                    metrics.opened += 1;
                }
                Err(error) => {
                    self.open_failures.push(error);
                    metrics.failures += 1;
                }
            }
        }

        metrics.removed = old_count.saturating_sub(metrics.reused);
        self.candidates = reconciled;
        let inventory_changed = metrics.opened > 0 || metrics.removed > 0 || metrics.failures > 0;
        self.stable_scan_interval = if inventory_changed {
            MIN_STABLE_CONTROLLER_SCAN_INTERVAL
        } else {
            next_stable_controller_scan_interval(self.stable_scan_interval)
        };
        self.next_scan = Instant::now()
            + controller_inventory_scan_interval(
                !self.candidates.is_empty(),
                self.stable_scan_interval,
            );
        metrics
    }

    fn clear(&mut self) {
        self.candidates.clear();
        self.next_scan = Instant::now();
        self.stable_scan_interval = MIN_STABLE_CONTROLLER_SCAN_INTERVAL;
        self.supported_devices_seen = false;
        self.open_failures.clear();
        self.scan_error = None;
    }

    fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    fn supported_devices_seen(&self) -> bool {
        self.supported_devices_seen
    }

    fn scan_error(&self) -> Option<&str> {
        self.scan_error.as_deref()
    }

    fn current_errors(&self, probe_failures: &[String]) -> Option<String> {
        let errors = self
            .scan_error
            .iter()
            .chain(&self.open_failures)
            .chain(probe_failures)
            .map(String::as_str)
            .collect::<Vec<_>>();
        (!errors.is_empty()).then(|| errors.join("; "))
    }

    fn candidate(&self, index: usize) -> &ControllerCandidate<S> {
        &self.candidates[index]
    }

    fn resolve_global_indices(&mut self, devices: &[HidDeviceInfo]) -> Result<(), String> {
        let resolved = self
            .candidates
            .iter()
            .map(|candidate| {
                devices
                    .iter()
                    .position(|info| same_controller_collection(&candidate.info, info))
                    .ok_or_else(|| {
                        format!(
                            "cannot resolve the global index for {}",
                            controller_source_identity(&candidate.info)
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (candidate, index) in self.candidates.iter_mut().zip(resolved) {
            candidate.enumeration_index = index;
        }
        Ok(())
    }

    fn select(&mut self, index: usize) -> ControllerCandidate<S> {
        let selected = self.candidates.swap_remove(index);
        self.clear();
        selected
    }
}

impl<S: ControllerProbeSession> ControllerDiscoveryState<S> {
    fn probe(&mut self) -> ControllerProbe {
        let mut decoder = SteamControllerDecoder::new();
        let mut active_indices = Vec::new();
        let mut failures = Vec::new();
        for (index, candidate) in self.candidates.iter_mut().enumerate() {
            for _ in 0..MAX_DISCOVERY_REPORTS_PER_CANDIDATE {
                match candidate.session.poll_for_discovery(Duration::ZERO) {
                    Ok(Some(DeviceEvent::Report(report)))
                        if is_valid_controller_state(&mut decoder, &report) =>
                    {
                        active_indices.push(index);
                        break;
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(error) => {
                        failures.push(format!(
                            "{}: {error}",
                            controller_source_identity(&candidate.info)
                        ));
                        break;
                    }
                }
            }
        }
        ControllerProbe {
            active_indices,
            failures,
        }
    }
}

fn controller_inventory_scan_interval(
    has_open_candidates: bool,
    stable_scan_interval: Duration,
) -> Duration {
    if has_open_candidates {
        stable_scan_interval
    } else {
        DISCOVERY_INTERVAL
    }
}

fn next_stable_controller_scan_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .min(MAX_STABLE_CONTROLLER_SCAN_INTERVAL)
}

fn controller_discovery_loop_delay(elapsed: Duration) -> Duration {
    DISCOVERY_INTERVAL.saturating_sub(elapsed)
}

fn same_controller_collection(left: &HidDeviceInfo, right: &HidDeviceInfo) -> bool {
    let same_stable_serial = left
        .serial_number
        .as_deref()
        .filter(|value| !value.is_empty())
        .zip(
            right
                .serial_number
                .as_deref()
                .filter(|value| !value.is_empty()),
        )
        .is_some_and(|(left, right)| left == right);
    (left.path == right.path || same_stable_serial)
        && left.vendor_id == right.vendor_id
        && left.product_id == right.product_id
        && left.usage_page == right.usage_page
        && left.usage == right.usage
        && left.interface_number == right.interface_number
}

enum Discovery<T> {
    Ready(T),
    Wait {
        detail: String,
        error: Option<String>,
    },
    Error(String),
}

struct ActiveControllerSource {
    info: HidDeviceInfo,
    session: HidSession,
    controller_seen: bool,
}

struct OutputSession {
    output: Box<dyn GamepadOutput>,
    xiao: Option<SerialDeviceInfo>,
}

/// Parks the gamepad output at neutral before desktop-input reconfiguration.
///
/// Constructing the desktop-input sink is synchronous, and Enigo's macOS
/// destructor can deliberately sleep after sustained event traffic. Those
/// operations now run on the dedicated desktop-input worker, so they cannot
/// block the supervisor's XIAO deadline. Retaining this neutral transition also
/// makes reconfiguration safe if worker ownership changes again later.
///
/// The next controller report restores the real state, and `reset` keeps the
/// unchanged-output dedupe consistent so that restore is not skipped.
fn neutralize_before_desktop_work(engine: &mut BridgeEngine, output: &mut OutputSession) {
    if let Err(error) = engine.reset(&mut *output.output) {
        // Worth knowing about, but not worth failing the command over: the
        // caller's own error handling covers a genuinely dead link.
        eprintln!("level=warn event=neutral_before_desktop_work_failed error={error:?}");
    }
}

fn service_waiting_output(output: Option<&mut OutputSession>) -> bool {
    output.is_some_and(|output| match output.output.service() {
        Ok(()) => {
            while output.output.take_feedback().is_some() {}
            true
        }
        Err(error) => {
            eprintln!("level=warn event=xiao_lost phase=waiting error={error:?} action=rediscover");
            false
        }
    })
}

enum ActiveExit {
    SourceLost,
    OutputLost(String),
    AutomaticShutdown {
        info: HidDeviceInfo,
        trigger: ShutdownTrigger,
    },
    StoppedWithAck(CommandAck),
    ShutdownWithAck(CommandAck),
    SuspendedWithAck(CommandAck),
}

impl ActiveExit {
    const fn has_acknowledgement(&self) -> bool {
        matches!(
            self,
            Self::StoppedWithAck(_) | Self::ShutdownWithAck(_) | Self::SuspendedWithAck(_)
        )
    }
}

/// Enforces the public command contract: no acknowledgement can become
/// observable until both the output (and therefore the serial port) and every
/// controller-discovery session have been dropped.
fn acknowledge_after_hardware_release(
    output: OutputSession,
    release_controller_sessions: impl FnOnce(),
    acknowledgement: &CommandAck,
    result: Result<(), String>,
) {
    drop(output);
    release_controller_sessions();
    let _ = acknowledgement.send(result);
}

fn make_nonserial_output(selection: &OutputSelection) -> Result<Box<dyn GamepadOutput>, String> {
    match selection {
        OutputSelection::Serial => Err("serial output requires XIAO discovery".to_owned()),
        OutputSelection::Dump(format) => Ok(Box::new(DumpOutput::new(io::stdout(), *format))),
        OutputSelection::File(path) => FileOutput::create(path)
            .map(|output| Box::new(output) as Box<dyn GamepadOutput>)
            .map_err(|error| error.to_string()),
        OutputSelection::Mock => Ok(Box::new(MockOutput::default())),
    }
}

fn choose_xiao_index<T>(
    valid: &[(SerialDeviceInfo, T)],
    preferred_serial: Option<&str>,
) -> Result<usize, String> {
    if valid.len() == 1 {
        return Ok(0);
    }
    if let Some(preferred) = preferred_serial {
        let preferred_matches: Vec<_> = valid
            .iter()
            .enumerate()
            .filter(|(_, (info, _))| info.serial_number.as_deref() == Some(preferred))
            .map(|(index, _)| index)
            .collect();
        if preferred_matches.len() == 1 {
            return Ok(preferred_matches[0]);
        }
    }
    Err(xiao_ambiguity_message(valid))
}

fn xiao_ambiguity_message<T>(valid: &[(SerialDeviceInfo, T)]) -> String {
    let ports = valid
        .iter()
        .map(|(info, _)| {
            format!(
                "{} (serial {})",
                info.path,
                masked_serial(info.serial_number.as_deref())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("multiple valid XIAO bridges found: {ports}; restart with --port PATH")
}

fn choose_unique_active(active_indices: &[usize]) -> Result<Option<usize>, Vec<usize>> {
    match active_indices {
        [] => Ok(None),
        [selected] => Ok(Some(*selected)),
        multiple => Err(multiple.to_vec()),
    }
}

fn controller_source_description(enumeration_index: usize, info: &HidDeviceInfo) -> String {
    format!(
        "index {enumeration_index} {}",
        controller_source_identity(info)
    )
}

fn controller_source_identity(info: &HidDeviceInfo) -> String {
    let transport = info
        .controller_transport()
        .map_or_else(|| "Unknown".to_owned(), |value| value.to_string());
    format!(
        "{transport} product {:?} serial {} interface {}",
        info.product.as_deref().unwrap_or("<unknown>"),
        masked_serial(info.serial_number.as_deref()),
        info.interface_number
    )
}

fn acknowledge_all(acks: &mut Vec<CommandAck>) {
    for ack in acks.drain(..) {
        let _ = ack.send(Ok(()));
    }
}

fn acknowledge_all_with_result(acks: &mut Vec<CommandAck>, result: &Result<(), String>) {
    for ack in acks.drain(..) {
        let _ = ack.send(result.clone());
    }
}

fn ownership_guidance(error: &DeviceError) -> String {
    format!(
        "{error}. Fully quit Steam and other controller tools; if Steam's ipcserver remains, \
         stop its LaunchAgent manually"
    )
}

fn is_valid_controller_state(decoder: &mut SteamControllerDecoder, report: &RawHidReport) -> bool {
    is_latest_state_report(report.report_id)
        && matches!(
            decoder.decode(report.report_id, &report.data),
            Ok(DecodedReport::ControllerState(_))
        )
}

fn is_latest_state_report(report_id: u8) -> bool {
    matches!(report_id, INPUT_REPORT_ID | EXTENDED_INPUT_REPORT_ID)
}

#[derive(Debug)]
enum ReportEffect {
    ControllerState {
        meaningful_activity: bool,
        desktop_input: DesktopInputSnapshot,
        picker_input: PickerInput,
    },
    Connected,
    Battery {
        percent: Option<u8>,
        charge_state: ControllerChargeState,
    },
    Disconnected,
    None,
}

fn process_report(
    report: &RawHidReport,
    engine: &mut BridgeEngine,
    output: &mut dyn GamepadOutput,
    recording: &mut Option<RecordingWriter<File>>,
    started: Instant,
    idle_activity: &mut IdleActivityTracker,
) -> Result<ReportEffect, String> {
    let timestamp = elapsed_us(started);
    record_lazy(recording, || {
        RecordingEvent::raw_hid_with_metadata(
            timestamp,
            report.report_id,
            &report.data,
            Some(&report.source_device_id),
            Some(&report.transport),
            report.dropped_reports,
        )
    })?;
    match engine.process_report(report.report_id, &report.data, started.elapsed(), output) {
        Ok(ProcessOutcome::State {
            source,
            mapped,
            unsuppressed,
            ..
        }) => {
            // Activity is judged on the unsuppressed state: steering the
            // profile wheel pins `mapped` at neutral, and must not read as
            // an idle controller to the automatic-shutdown clock.
            let meaningful_activity =
                idle_activity.observe(started.elapsed(), &source, &unsuppressed);
            record_lazy(recording, || {
                RecordingEvent::decoded_steam_state(timestamp, &source)
            })?;
            record_lazy(recording, || {
                RecordingEvent::mapped_gamepad_state(timestamp, &mapped)
            })?;
            Ok(ReportEffect::ControllerState {
                meaningful_activity,
                desktop_input: DesktopInputSnapshot {
                    buttons: source.buttons,
                    left_pad: PadSample {
                        x: source.left_pad_x,
                        y: source.left_pad_y,
                        pressure: source.left_pad_pressure,
                        touched: source.left_pad_touched,
                        pressed: source.left_pad_pressed,
                    },
                    right_pad: PadSample {
                        x: source.right_pad_x,
                        y: source.right_pad_y,
                        pressure: source.right_pad_pressure,
                        touched: source.right_pad_touched,
                        pressed: source.right_pad_pressed,
                    },
                },
                picker_input: PickerInput {
                    buttons: source.buttons,
                    left_stick: (source.left_stick_x, source.left_stick_y),
                    right_stick: (source.right_stick_x, source.right_stick_y),
                },
            })
        }
        Ok(ProcessOutcome::Status(DecodedReport::Battery { status, .. })) => {
            Ok(ReportEffect::Battery {
                percent: valid_battery_percent(status.percent),
                charge_state: ControllerChargeState::from_raw(status.charge_state),
            })
        }
        Ok(ProcessOutcome::Status(DecodedReport::Connection(ConnectionState::Disconnected))) => {
            Ok(ReportEffect::Disconnected)
        }
        Ok(ProcessOutcome::Status(DecodedReport::Connection(ConnectionState::Connected))) => {
            Ok(ReportEffect::Connected)
        }
        Ok(_) => Ok(ReportEffect::None),
        Err(bridge_core::BridgeError::Decode(error)) => {
            eprintln!("level=warn event=decode_failure error={error:?}");
            Ok(ReportEffect::None)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn is_output_error(message: &str) -> bool {
    message.contains("output failed") || message.contains("serial") || message.contains("transport")
}

fn valid_battery_percent(percent: u8) -> Option<u8> {
    (percent <= 100).then_some(percent)
}

fn record_lazy(
    writer: &mut Option<RecordingWriter<File>>,
    make_event: impl FnOnce() -> Result<RecordingEvent, RecordingError>,
) -> Result<(), String> {
    let Some(writer) = writer else {
        return Ok(());
    };
    let event = make_event().map_err(|error| error.to_string())?;
    writer
        .write_event(&event)
        .map_err(|error| error.to_string())
}

fn record_device_event(
    writer: &mut Option<RecordingWriter<File>>,
    started: Instant,
    kind: &str,
    info: Option<&HidDeviceInfo>,
) -> Result<(), String> {
    record_lazy(writer, || {
        let payload = info.map_or_else(
            || json!({}),
            |info| {
                json!({
                    "id": info.id,
                    "path": info.path,
                    "vendor_id": info.vendor_id,
                    "product_id": info.product_id,
                    "usage_page": info.usage_page,
                    "usage": info.usage,
                    "interface_number": info.interface_number,
                    "transport": info.transport,
                    "product": info.product,
                    "manufacturer": info.manufacturer,
                })
            },
        );
        Ok(RecordingEvent::new(elapsed_us(started), kind, payload))
    })
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn should_tick_input_timeout(replaced_reports: u64, has_pending_report: bool) -> bool {
    replaced_reports == 0 && !has_pending_report
}

#[derive(Debug, Default)]
struct SharedLizardMetrics {
    active: AtomicBool,
    refreshes: AtomicU64,
    failures: AtomicU64,
    last_refresh_millis: AtomicU64,
}

impl SharedLizardMetrics {
    fn record_success(&self, now: Duration) {
        self.active.store(true, Ordering::Release);
        self.refreshes.fetch_add(1, Ordering::Relaxed);
        let millis = u64::try_from(now.as_millis())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        self.last_refresh_millis.store(millis, Ordering::Release);
    }

    fn record_failure(&self) {
        self.active.store(false, Ordering::Release);
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    fn record_disconnected(&self) {
        self.active.store(false, Ordering::Release);
        self.last_refresh_millis.store(0, Ordering::Release);
    }

    fn snapshot(&self, now: Duration) -> LizardStatus {
        let last_refresh_millis = self.last_refresh_millis.load(Ordering::Acquire);
        LizardStatus {
            suppressed: self.active.load(Ordering::Acquire),
            refreshes: self.refreshes.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            last_refresh_age: (last_refresh_millis > 0)
                .then(|| now.saturating_sub(Duration::from_millis(last_refresh_millis - 1))),
        }
    }
}

struct LizardSupervisor {
    mode: LizardMode,
    heartbeat: LizardModeHeartbeat,
    metrics: Arc<SharedLizardMetrics>,
}

impl LizardSupervisor {
    fn new(mode: LizardMode, metrics: Arc<SharedLizardMetrics>) -> Self {
        Self {
            mode,
            heartbeat: LizardModeHeartbeat::new(),
            metrics,
        }
    }

    fn connected(&mut self, now: Duration, session: &HidSession) -> Result<(), DeviceError> {
        self.heartbeat.connected();
        if self.mode == LizardMode::Suppress {
            self.refresh(now, session)?;
        }
        Ok(())
    }

    fn service(&mut self, now: Duration, session: &HidSession) -> Result<(), DeviceError> {
        if self.mode == LizardMode::Suppress && self.heartbeat.refresh_due(now) {
            self.refresh(now, session)?;
        }
        Ok(())
    }

    fn refresh(&mut self, now: Duration, session: &HidSession) -> Result<(), DeviceError> {
        if let Err(error) = session.suppress_lizard_mode() {
            self.metrics.record_failure();
            return Err(error);
        }
        self.heartbeat.refreshed(now);
        self.metrics.record_success(now);
        Ok(())
    }

    fn disconnected(&mut self) {
        self.heartbeat.disconnected();
        self.metrics.record_disconnected();
    }
}

#[derive(Debug, Default)]
struct SharedHapticsMetrics {
    active: AtomicBool,
    degraded: AtomicBool,
    pad_degraded: AtomicBool,
    commands_received: AtomicU64,
    writes: AtomicU64,
    refreshes: AtomicU64,
    coalesced_commands: AtomicU64,
    failures: AtomicU64,
    last_command_millis: AtomicU64,
    pad_feedback_ticks: AtomicU64,
    pad_feedback_coalesced: AtomicU64,
    pad_feedback_failures: AtomicU64,
    last_pad_feedback_millis: AtomicU64,
    pad_feedback_last_error: Mutex<Option<String>>,
}

impl SharedHapticsMetrics {
    fn record_command(&self, now: Duration, coalesced: bool) {
        self.commands_received.fetch_add(1, Ordering::Relaxed);
        if coalesced {
            self.coalesced_commands.fetch_add(1, Ordering::Relaxed);
        }
        let millis = u64::try_from(now.as_millis())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        self.last_command_millis.store(millis, Ordering::Release);
    }

    fn record_success(&self, active: bool, refresh: bool) {
        self.active.store(active, Ordering::Release);
        self.degraded.store(false, Ordering::Release);
        self.writes.fetch_add(1, Ordering::Relaxed);
        if refresh {
            self.refreshes.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_failure(&self) {
        self.active.store(false, Ordering::Release);
        self.degraded.store(true, Ordering::Release);
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    fn record_disconnected(&self) {
        self.active.store(false, Ordering::Release);
        self.degraded.store(false, Ordering::Release);
        self.pad_degraded.store(false, Ordering::Release);
        self.last_command_millis.store(0, Ordering::Release);
        self.last_pad_feedback_millis.store(0, Ordering::Release);
        *self
            .pad_feedback_last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn record_pad_coalesced(&self, count: u64) {
        if count > 0 {
            self.pad_feedback_coalesced
                .fetch_add(count, Ordering::Relaxed);
        }
    }

    fn record_pad_success(&self, now: Duration) {
        self.pad_degraded.store(false, Ordering::Release);
        self.pad_feedback_ticks.fetch_add(1, Ordering::Relaxed);
        let millis = u64::try_from(now.as_millis())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        self.last_pad_feedback_millis
            .store(millis, Ordering::Release);
        *self
            .pad_feedback_last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn record_pad_failure(&self, error: &str) {
        self.pad_degraded.store(true, Ordering::Release);
        self.pad_feedback_failures.fetch_add(1, Ordering::Relaxed);
        *self
            .pad_feedback_last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(bounded_error(error));
    }

    fn snapshot(&self, now: Duration) -> HapticsStatus {
        let last_command_millis = self.last_command_millis.load(Ordering::Acquire);
        let last_pad_feedback_millis = self.last_pad_feedback_millis.load(Ordering::Acquire);
        let state =
            if self.degraded.load(Ordering::Acquire) || self.pad_degraded.load(Ordering::Acquire) {
                HapticsState::Degraded
            } else if self.active.load(Ordering::Acquire) {
                HapticsState::Active
            } else {
                HapticsState::Idle
            };
        HapticsStatus {
            state,
            commands_received: self.commands_received.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            refreshes: self.refreshes.load(Ordering::Relaxed),
            coalesced_commands: self.coalesced_commands.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            last_command_age: (last_command_millis > 0)
                .then(|| now.saturating_sub(Duration::from_millis(last_command_millis - 1))),
            pad_feedback_ticks: self.pad_feedback_ticks.load(Ordering::Relaxed),
            pad_feedback_coalesced: self.pad_feedback_coalesced.load(Ordering::Relaxed),
            pad_feedback_failures: self.pad_feedback_failures.load(Ordering::Relaxed),
            last_pad_feedback_age: (last_pad_feedback_millis > 0)
                .then(|| now.saturating_sub(Duration::from_millis(last_pad_feedback_millis - 1))),
            pad_feedback_last_error: self
                .pad_feedback_last_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct RumbleCommand {
    low_frequency: u16,
    high_frequency: u16,
}

impl RumbleCommand {
    const fn is_active(self) -> bool {
        self.low_frequency != 0 || self.high_frequency != 0
    }
}

#[derive(Debug, Default)]
struct LatestRumbleSlot {
    command: Mutex<Option<RumbleCommand>>,
}

impl LatestRumbleSlot {
    fn publish(&self, command: RumbleCommand) -> bool {
        self.command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(command)
            .is_some()
    }

    fn take(&self) -> Option<RumbleCommand> {
        self.command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn clear(&self) {
        self.command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PadFeedbackCommand {
    side: PadHapticSide,
    gain: PadHapticGain,
}

#[derive(Debug, Default)]
struct PendingPadFeedbackState {
    left_gain: Option<PadHapticGain>,
    right_gain: Option<PadHapticGain>,
}

#[derive(Debug, Default)]
struct PendingPadFeedback {
    state: Mutex<PendingPadFeedbackState>,
}

impl PendingPadFeedback {
    fn publish(&self, request: PadFeedbackRequest) -> u64 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut coalesced = 0;
        if let Some(strength) = request.left {
            coalesced += u64::from(state.left_gain.replace(strength.haptic_gain()).is_some());
        }
        if let Some(strength) = request.right {
            coalesced += u64::from(state.right_gain.replace(strength.haptic_gain()).is_some());
        }
        coalesced
    }

    fn take(&self) -> Vec<PadFeedbackCommand> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let left = state.left_gain.take();
        let right = state.right_gain.take();
        match (left, right) {
            (Some(left), Some(right)) if left == right => vec![PadFeedbackCommand {
                side: PadHapticSide::Both,
                gain: left,
            }],
            (Some(left), Some(right)) => vec![
                PadFeedbackCommand {
                    side: PadHapticSide::Left,
                    gain: left,
                },
                PadFeedbackCommand {
                    side: PadHapticSide::Right,
                    gain: right,
                },
            ],
            (Some(gain), None) => vec![PadFeedbackCommand {
                side: PadHapticSide::Left,
                gain,
            }],
            (None, Some(gain)) => vec![PadFeedbackCommand {
                side: PadHapticSide::Right,
                gain,
            }],
            (None, None) => Vec::new(),
        }
    }

    fn clear(&self) {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            PendingPadFeedbackState::default();
    }
}

trait RumbleWriter {
    fn write_rumble(&self, low_frequency: u16, high_frequency: u16) -> Result<(), String>;
}

impl RumbleWriter for HidSession {
    fn write_rumble(&self, low_frequency: u16, high_frequency: u16) -> Result<(), String> {
        self.set_rumble(low_frequency, high_frequency)
            .map_err(|error| error.to_string())
    }
}

trait PadFeedbackWriter {
    fn write_pad_feedback(&self, side: PadHapticSide, gain: PadHapticGain) -> Result<(), String>;
}

impl PadFeedbackWriter for HidSession {
    fn write_pad_feedback(&self, side: PadHapticSide, gain: PadHapticGain) -> Result<(), String> {
        self.pad_haptic_tick(side, gain)
            .map_err(|error| error.to_string())
    }
}

struct PadFeedbackSupervisor {
    connected: bool,
    retry_after: Option<Duration>,
    metrics: Arc<SharedHapticsMetrics>,
}

impl PadFeedbackSupervisor {
    fn new(metrics: Arc<SharedHapticsMetrics>) -> Self {
        Self {
            connected: false,
            retry_after: None,
            metrics,
        }
    }

    fn connected(&mut self) {
        self.connected = true;
        self.retry_after = None;
    }

    fn disconnected(&mut self) {
        self.connected = false;
        self.retry_after = None;
    }

    fn service(
        &mut self,
        now: Duration,
        session: &impl PadFeedbackWriter,
        commands: Vec<PadFeedbackCommand>,
    ) {
        if !self.connected
            || self
                .retry_after
                .is_some_and(|retry_after| now < retry_after)
        {
            return;
        }
        for command in commands {
            match session.write_pad_feedback(command.side, command.gain) {
                Ok(()) => self.metrics.record_pad_success(now),
                Err(error) => {
                    self.retry_after = Some(now.saturating_add(PAD_FEEDBACK_RETRY_INTERVAL));
                    self.metrics.record_pad_failure(&error);
                    eprintln!(
                        "level=warn event=pad_feedback_write_failed error={error:?} retry_ms={}",
                        PAD_FEEDBACK_RETRY_INTERVAL.as_millis()
                    );
                    break;
                }
            }
        }
    }
}

struct HapticsSupervisor {
    connected: bool,
    desired: RumbleCommand,
    lease_received: Option<Duration>,
    last_write: Option<Duration>,
    retry_after: Option<Duration>,
    metrics: Arc<SharedHapticsMetrics>,
}

impl HapticsSupervisor {
    fn new(metrics: Arc<SharedHapticsMetrics>) -> Self {
        Self {
            connected: false,
            desired: RumbleCommand::default(),
            lease_received: None,
            last_write: None,
            retry_after: None,
            metrics,
        }
    }

    fn connected(&mut self, now: Duration, session: &impl RumbleWriter) {
        self.connected = true;
        self.desired = RumbleCommand::default();
        self.lease_received = None;
        self.last_write = None;
        self.retry_after = None;
        self.write(now, session, self.desired, false);
    }

    fn command(&mut self, now: Duration, session: &impl RumbleWriter, command: RumbleCommand) {
        let changed = command != self.desired;
        self.desired = command;
        if command.is_active() {
            self.lease_received = Some(now);
            if changed && self.retry_due(now) {
                self.write(now, session, command, false);
            }
        } else {
            self.lease_received = None;
            if changed
                || self.metrics.active.load(Ordering::Acquire)
                || self.metrics.degraded.load(Ordering::Acquire)
            {
                self.write(now, session, command, false);
            }
        }
    }

    fn service(&mut self, now: Duration, session: &impl RumbleWriter) {
        if self.desired.is_active()
            && self
                .lease_received
                .is_some_and(|received| now.saturating_sub(received) >= RUMBLE_LEASE_TIMEOUT)
        {
            self.desired = RumbleCommand::default();
            self.lease_received = None;
            self.write(now, session, self.desired, false);
            return;
        }
        if !self.desired.is_active() {
            return;
        }
        if self.metrics.degraded.load(Ordering::Acquire) {
            if self.retry_due(now) {
                self.write(now, session, self.desired, false);
            }
            return;
        }
        if self
            .last_write
            .is_some_and(|written| now.saturating_sub(written) >= RUMBLE_REFRESH_INTERVAL)
        {
            self.write(now, session, self.desired, true);
        }
    }

    fn shutdown(&mut self, now: Duration, session: &impl RumbleWriter) {
        self.desired = RumbleCommand::default();
        self.lease_received = None;
        if self.connected {
            self.write(now, session, self.desired, false);
            self.connected = false;
        }
    }

    fn disconnected(&mut self) {
        self.connected = false;
        self.desired = RumbleCommand::default();
        self.lease_received = None;
        self.last_write = None;
        self.retry_after = None;
        self.metrics.record_disconnected();
    }

    fn retry_due(&self, now: Duration) -> bool {
        self.retry_after
            .is_none_or(|retry_after| now >= retry_after)
    }

    fn write(
        &mut self,
        now: Duration,
        session: &impl RumbleWriter,
        command: RumbleCommand,
        refresh: bool,
    ) {
        match session.write_rumble(command.low_frequency, command.high_frequency) {
            Ok(()) => {
                self.last_write = Some(now);
                self.retry_after = None;
                self.metrics.record_success(command.is_active(), refresh);
            }
            Err(error) => {
                self.retry_after = Some(now.saturating_add(RUMBLE_RETRY_INTERVAL));
                self.metrics.record_failure();
                eprintln!(
                    "level=warn event=rumble_write_failed error={error:?} retry_ms={}",
                    RUMBLE_RETRY_INTERVAL.as_millis()
                );
            }
        }
    }
}

#[derive(Debug)]
enum HidWorkerEvent {
    Connected(HidDeviceInfo),
    Disconnected,
    StatusReport(RawHidReport),
    ReportReady,
}

enum HidWorkerControl {
    PowerOff(CommandAck),
}

struct PowerOffSequence {
    ack: Option<CommandAck>,
    attempts: u8,
    successes: u8,
    last_error: Option<String>,
    next_write: Duration,
    disconnected_after_success: bool,
}

trait PowerOffWriter {
    fn write_power_off(&self) -> Result<(), String>;
}

impl PowerOffWriter for HidSession {
    fn write_power_off(&self) -> Result<(), String> {
        self.power_off().map_err(|error| error.to_string())
    }
}

impl PowerOffSequence {
    fn new(ack: CommandAck, now: Duration) -> Self {
        Self {
            ack: Some(ack),
            attempts: 0,
            successes: 0,
            last_error: None,
            next_write: now,
            disconnected_after_success: false,
        }
    }

    fn service(
        &mut self,
        now: Duration,
        session: &impl PowerOffWriter,
    ) -> Option<Result<(), String>> {
        if self.disconnected_after_success {
            return Some(Ok(()));
        }
        if now < self.next_write {
            return None;
        }
        self.attempts = self.attempts.saturating_add(1);
        match session.write_power_off() {
            Ok(()) => {
                self.successes = self.successes.saturating_add(1);
                eprintln!(
                    "level=info event=controller_power_off_write attempt={} total={} result=success",
                    self.attempts, POWER_OFF_BURST_WRITES
                );
            }
            Err(error) => {
                self.last_error = Some(error.clone());
                eprintln!(
                    "level=warn event=controller_power_off_write attempt={} total={} result=failure error={error:?}",
                    self.attempts, POWER_OFF_BURST_WRITES
                );
            }
        }
        if self.attempts >= POWER_OFF_BURST_WRITES {
            if self.successes > 0 {
                Some(Ok(()))
            } else {
                Some(Err(self.last_error.clone().unwrap_or_else(|| {
                    "all controller power-off writes failed".to_owned()
                })))
            }
        } else {
            self.next_write = now.saturating_add(POWER_OFF_BURST_INTERVAL);
            None
        }
    }

    fn note_disconnected(&mut self) {
        if self.successes > 0 {
            self.disconnected_after_success = true;
        }
    }

    fn finish(&mut self, result: Result<(), String>) {
        if let Some(ack) = self.ack.take() {
            let _ = ack.send(result);
        }
    }
}

#[derive(Debug, Default)]
struct TransitionMailboxState {
    reports: VecDeque<RawHidReport>,
    transition_run: StableTransitionRun,
    notification_pending: bool,
    overflowed: bool,
}

#[derive(Debug, Default)]
struct TransitionReportMailbox {
    state: Mutex<TransitionMailboxState>,
}

#[derive(Debug, Default)]
struct TransitionReportBatch {
    reports: VecDeque<RawHidReport>,
    overflowed: bool,
}

impl TransitionReportMailbox {
    fn publish(&self, report: RawHidReport, dropped: &AtomicU64) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let transition_mask = raw_desktop_transition_mask(&report);
        if state.transition_run.can_replace_latest(transition_mask) {
            let _ = state.reports.pop_back();
            state.reports.push_back(report);
            dropped.fetch_add(1, Ordering::Relaxed);
        } else if state.reports.len() == INPUT_MAILBOX_CAPACITY {
            dropped.fetch_add(state.reports.len() as u64, Ordering::Relaxed);
            state.reports.clear();
            state.reports.push_back(report);
            state.transition_run.reset_with_latest(transition_mask);
            state.overflowed = true;
        } else {
            state.reports.push_back(report);
            state.transition_run.push(transition_mask);
        }
        let needs_notification = !state.notification_pending;
        state.notification_pending = true;
        needs_notification
    }

    fn take_all(&self) -> TransitionReportBatch {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.notification_pending = false;
        state.transition_run.reset();
        TransitionReportBatch {
            reports: std::mem::take(&mut state.reports),
            overflowed: std::mem::take(&mut state.overflowed),
        }
    }

    fn has_pending(&self) -> bool {
        !self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reports
            .is_empty()
    }

    fn clear(&self, dropped: &AtomicU64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.reports.is_empty() {
            dropped.fetch_add(state.reports.len() as u64, Ordering::Relaxed);
            state.reports.clear();
        }
        state.notification_pending = false;
        state.overflowed = false;
        state.transition_run.reset();
    }
}

fn raw_desktop_transition_mask(report: &RawHidReport) -> Option<u8> {
    let valid_size = match report.report_id {
        INPUT_REPORT_ID => report.data.len() == INPUT_REPORT_SIZE,
        EXTENDED_INPUT_REPORT_ID => report.data.len() == EXTENDED_INPUT_REPORT_SIZE,
        _ => false,
    };
    if !valid_size {
        return None;
    }
    let buttons = steam_controller_protocol::SteamButtons(u32::from_le_bytes([
        report.data[2],
        report.data[3],
        report.data[4],
        report.data[5],
    ]));
    Some(desktop_transition_mask(
        buttons,
        buttons.contains(SteamButton::LeftPadTouch),
        buttons.contains(SteamButton::RightPadTouch),
    ))
}

struct HidWorker {
    receiver: Receiver<HidWorkerEvent>,
    failure_receiver: Receiver<String>,
    report_mailbox: Arc<TransitionReportMailbox>,
    latest_rumble: Arc<LatestRumbleSlot>,
    pending_pad_feedback: Arc<PendingPadFeedback>,
    control_sender: mpsc::Sender<HidWorkerControl>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    started: Instant,
    lizard_metrics: Arc<SharedLizardMetrics>,
    haptics_metrics: Arc<SharedHapticsMetrics>,
    info: HidDeviceInfo,
}

impl HidWorker {
    #[allow(clippy::too_many_lines)] // Keep HID, lizard, and rumble safety ordering linear.
    fn spawn(
        active: ActiveControllerSource,
        lizard_mode: LizardMode,
        dropped: Arc<AtomicU64>,
    ) -> Result<Self, String> {
        let ActiveControllerSource {
            info, mut session, ..
        } = active;
        let (sender, receiver) = mpsc::sync_channel(64);
        let (failure_sender, failure_receiver) = mpsc::channel();
        let report_mailbox = Arc::new(TransitionReportMailbox::default());
        let worker_latest_report = Arc::clone(&report_mailbox);
        let latest_rumble = Arc::new(LatestRumbleSlot::default());
        let worker_latest_rumble = Arc::clone(&latest_rumble);
        let pending_pad_feedback = Arc::new(PendingPadFeedback::default());
        let worker_pending_pad_feedback = Arc::clone(&pending_pad_feedback);
        let (control_sender, control_receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let lizard_metrics = Arc::new(SharedLizardMetrics::default());
        let worker_lizard_metrics = Arc::clone(&lizard_metrics);
        let haptics_metrics = Arc::new(SharedHapticsMetrics::default());
        let worker_haptics_metrics = Arc::clone(&haptics_metrics);
        let worker_started = Instant::now();

        let mut initial_lizard =
            LizardSupervisor::new(lizard_mode, Arc::clone(&worker_lizard_metrics));
        initial_lizard
            .connected(worker_started.elapsed(), &session)
            .map_err(|error| {
                format!(
                    "lizard-mode suppression failed before input was accepted; \
                     XIAO was not activated: {error}"
                )
            })?;

        let worker_info = info.clone();
        let handle = thread::spawn(move || {
            let mut lizard = initial_lizard;
            let mut haptics = HapticsSupervisor::new(worker_haptics_metrics);
            haptics.connected(worker_started.elapsed(), &session);
            let mut pad_feedback = PadFeedbackSupervisor::new(Arc::clone(&haptics.metrics));
            pad_feedback.connected();
            let mut accepting_input = true;
            let mut power_off_sequence: Option<PowerOffSequence> = None;
            while !worker_stop.load(Ordering::Acquire) {
                if power_off_sequence.is_none() {
                    if let Ok(HidWorkerControl::PowerOff(ack)) = control_receiver.try_recv() {
                        accepting_input = false;
                        worker_latest_report.clear(&dropped);
                        worker_latest_rumble.clear();
                        worker_pending_pad_feedback.clear();
                        haptics.shutdown(worker_started.elapsed(), &session);
                        pad_feedback.disconnected();
                        lizard.disconnected();
                        power_off_sequence =
                            Some(PowerOffSequence::new(ack, worker_started.elapsed()));
                    }
                }
                if let Some(sequence) = power_off_sequence.as_mut() {
                    if let Some(result) = sequence.service(worker_started.elapsed(), &session) {
                        let failed = result.is_err();
                        sequence.finish(result);
                        power_off_sequence = None;
                        if failed {
                            accepting_input = true;
                            haptics.connected(worker_started.elapsed(), &session);
                            pad_feedback.connected();
                            if let Err(error) = lizard.connected(worker_started.elapsed(), &session)
                            {
                                worker_latest_report.clear(&dropped);
                                let _ = failure_sender.send(format!(
                                    "lizard-mode suppression could not resume after a failed \
                                     power-off attempt: {error}"
                                ));
                                break;
                            }
                        }
                    }
                }
                if accepting_input {
                    if let Err(error) = lizard.service(worker_started.elapsed(), &session) {
                        worker_latest_report.clear(&dropped);
                        let _ = failure_sender.send(format!(
                            "lizard-mode refresh failed; XIAO was neutralized and input stopped: {error}"
                        ));
                        break;
                    }
                }
                if accepting_input {
                    if let Some(command) = worker_latest_rumble.take() {
                        haptics.command(worker_started.elapsed(), &session, command);
                    }
                }
                if accepting_input {
                    haptics.service(worker_started.elapsed(), &session);
                    pad_feedback.service(
                        worker_started.elapsed(),
                        &session,
                        worker_pending_pad_feedback.take(),
                    );
                }
                match session.poll(RUNTIME_POLL_INTERVAL) {
                    Ok(Some(DeviceEvent::Connected(info))) => {
                        if !accepting_input {
                            continue;
                        }
                        if let Err(error) = lizard.connected(worker_started.elapsed(), &session) {
                            worker_latest_report.clear(&dropped);
                            let _ = failure_sender.send(format!(
                                "lizard-mode suppression failed after reconnect: {error}"
                            ));
                            break;
                        }
                        haptics.connected(worker_started.elapsed(), &session);
                        worker_pending_pad_feedback.clear();
                        pad_feedback.connected();
                        if !send_worker_event(
                            &sender,
                            HidWorkerEvent::Connected(info),
                            &worker_stop,
                        ) {
                            break;
                        }
                    }
                    Ok(Some(DeviceEvent::Disconnected)) => {
                        if let Some(sequence) = power_off_sequence.as_mut() {
                            sequence.note_disconnected();
                            continue;
                        }
                        if !accepting_input {
                            continue;
                        }
                        haptics.disconnected();
                        pad_feedback.disconnected();
                        worker_latest_rumble.clear();
                        worker_pending_pad_feedback.clear();
                        lizard.disconnected();
                        worker_latest_report.clear(&dropped);
                        if !send_worker_event(&sender, HidWorkerEvent::Disconnected, &worker_stop) {
                            break;
                        }
                    }
                    Ok(Some(DeviceEvent::Report(report))) => {
                        if !accepting_input {
                            continue;
                        }
                        let published = if is_latest_state_report(report.report_id) {
                            publish_report(&sender, &worker_latest_report, report, &dropped)
                        } else {
                            send_worker_event(
                                &sender,
                                HidWorkerEvent::StatusReport(report),
                                &worker_stop,
                            )
                        };
                        if !published {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        haptics.disconnected();
                        pad_feedback.disconnected();
                        worker_latest_rumble.clear();
                        worker_pending_pad_feedback.clear();
                        lizard.disconnected();
                        worker_latest_report.clear(&dropped);
                        let _ = failure_sender.send(format!("HID worker failed: {error}"));
                        break;
                    }
                }
            }
            while !worker_stop.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(1));
            }
            worker_latest_report.clear(&dropped);
            worker_latest_rumble.clear();
            worker_pending_pad_feedback.clear();
            pad_feedback.disconnected();
            haptics.shutdown(worker_started.elapsed(), &session);
            lizard.disconnected();
        });
        Ok(Self {
            receiver,
            failure_receiver,
            report_mailbox,
            latest_rumble,
            pending_pad_feedback,
            control_sender,
            stop,
            handle: Some(handle),
            started: worker_started,
            lizard_metrics,
            haptics_metrics,
            info: worker_info,
        })
    }

    fn device_info(&self) -> &HidDeviceInfo {
        &self.info
    }

    fn take_failure(&self) -> Option<String> {
        self.failure_receiver.try_recv().ok()
    }

    fn lizard_diagnostics(&self) -> LizardStatus {
        self.lizard_metrics.snapshot(self.started.elapsed())
    }

    fn haptics_diagnostics(&self) -> HapticsStatus {
        self.haptics_metrics.snapshot(self.started.elapsed())
    }

    fn set_rumble(&self, low_frequency: u16, high_frequency: u16) {
        let command = RumbleCommand {
            low_frequency,
            high_frequency,
        };
        let coalesced = self.latest_rumble.publish(command);
        self.haptics_metrics
            .record_command(self.started.elapsed(), coalesced);
    }

    fn request_pad_feedback(&self, request: PadFeedbackRequest) {
        if request == PadFeedbackRequest::NONE {
            return;
        }
        let coalesced = self.pending_pad_feedback.publish(request);
        self.haptics_metrics.record_pad_coalesced(coalesced);
    }

    fn clear_pad_feedback(&self) {
        self.pending_pad_feedback.clear();
    }

    fn power_off(&self) -> Result<(), String> {
        let (ack_sender, ack_receiver) = mpsc::channel();
        self.control_sender
            .send(HidWorkerControl::PowerOff(ack_sender))
            .map_err(|_| "HID worker stopped before controller power-off could start".to_owned())?;
        ack_receiver
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| "controller power-off sequence timed out".to_owned())?
    }

    fn take_report_batch(&self) -> TransitionReportBatch {
        self.report_mailbox.take_all()
    }

    fn has_pending_report(&self) -> bool {
        self.report_mailbox.has_pending()
    }

    fn shutdown(&mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| "HID worker panicked".to_owned())?;
        }
        Ok(())
    }
}

impl Drop for HidWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn send_worker_event(
    sender: &SyncSender<HidWorkerEvent>,
    mut event: HidWorkerEvent,
    stop: &AtomicBool,
) -> bool {
    loop {
        match sender.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                if stop.load(Ordering::Acquire) {
                    return false;
                }
                event = returned;
                thread::sleep(Duration::from_millis(1));
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

fn publish_report(
    sender: &SyncSender<HidWorkerEvent>,
    latest_report: &TransitionReportMailbox,
    report: RawHidReport,
    dropped: &AtomicU64,
) -> bool {
    if !latest_report.publish(report, dropped) {
        return true;
    }
    match sender.try_send(HidWorkerEvent::ReportReady) {
        Ok(()) | Err(TrySendError::Full(_)) => true,
        Err(TrySendError::Disconnected(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervisor_timer_reports_the_slowest_phase_after_a_stall() {
        let started = Instant::now();
        let mut timer = SupervisorIterationTimer::new_at("commands", started);
        timer.enter_at("hid_wait", started + Duration::from_millis(4));
        timer.enter_at("controller_reports", started + Duration::from_millis(62));

        let stall = timer
            .take_stall_at(started + Duration::from_millis(70))
            .unwrap();
        assert_eq!(stall.elapsed, Duration::from_millis(70));
        assert_eq!(stall.phase, "hid_wait");
        assert_eq!(stall.phase_elapsed, Duration::from_millis(58));
        assert!(timer
            .take_stall_at(started + Duration::from_millis(80))
            .is_none());
    }

    #[test]
    fn supervisor_timer_ignores_iterations_below_the_warning_threshold() {
        let started = Instant::now();
        let mut timer = SupervisorIterationTimer::new_at("commands", started);
        timer.enter_at("hid_wait", started + Duration::from_millis(10));
        let below_threshold = SUPERVISOR_STALL_THRESHOLD.saturating_sub(Duration::from_millis(1));

        assert!(timer.take_stall_at(started + below_threshold).is_none());
    }

    #[test]
    fn desktop_worker_mailbox_coalesces_motion_without_dropping_edges() {
        let mailbox = DesktopWorkerMailbox::default();
        let outputs = DesktopWorkerOutputs::default();
        let neutral = DesktopInputSnapshot::buttons_only(SteamButtons::default());
        let touched = |x| DesktopInputSnapshot {
            right_pad: PadSample {
                x,
                touched: true,
                ..PadSample::NEUTRAL
            },
            ..neutral
        };
        let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);

        assert_eq!(
            mailbox.publish_snapshot(&outputs, neutral, Duration::ZERO),
            DesktopSnapshotPublish::Published
        );
        assert_eq!(
            mailbox.publish_snapshot(&outputs, touched(0), Duration::from_millis(1)),
            DesktopSnapshotPublish::Published
        );
        assert_eq!(
            mailbox.publish_snapshot(&outputs, touched(10), Duration::from_millis(2)),
            DesktopSnapshotPublish::Published
        );
        assert_eq!(
            mailbox.publish_snapshot(&outputs, touched(20), Duration::from_millis(3)),
            DesktopSnapshotPublish::Published
        );
        assert_eq!(
            mailbox.publish_snapshot(
                &outputs,
                DesktopInputSnapshot {
                    buttons: r4,
                    ..touched(20)
                },
                Duration::from_millis(4),
            ),
            DesktopSnapshotPublish::Published
        );
        assert_eq!(
            mailbox.publish_snapshot(&outputs, touched(20), Duration::from_millis(5)),
            DesktopSnapshotPublish::Published
        );

        let messages = mailbox.take_batch(Some(Duration::ZERO));
        let snapshots = messages
            .into_iter()
            .map(|message| match message {
                DesktopWorkerMessage::Snapshot(snapshot) => snapshot,
                _ => panic!("expected only desktop snapshots"),
            })
            .collect::<Vec<_>>();
        assert_eq!(snapshots.len(), 5);
        assert_eq!(snapshots[0].snapshot, neutral);
        assert_eq!(snapshots[1].snapshot, touched(0));
        assert_eq!(snapshots[2].snapshot, touched(20));
        assert_eq!(snapshots[2].now, Duration::from_millis(3));
        assert_eq!(snapshots[3].snapshot.buttons, r4);
        assert_eq!(snapshots[4].snapshot.buttons, SteamButtons::default());
    }

    #[test]
    fn desktop_worker_mailbox_overflow_keeps_control_barriers_and_latest_state() {
        let mailbox = DesktopWorkerMailbox::default();
        let outputs = DesktopWorkerOutputs::default();
        let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);
        for index in 0..DESKTOP_INPUT_MAILBOX_CAPACITY {
            let buttons = if index % 2 == 0 {
                SteamButtons::default()
            } else {
                r4
            };
            assert_eq!(
                mailbox.publish_snapshot(
                    &outputs,
                    DesktopInputSnapshot::buttons_only(buttons),
                    Duration::from_millis(index as u64),
                ),
                DesktopSnapshotPublish::Published
            );
        }
        assert!(mailbox
            .push_control(
                &outputs,
                DesktopWorkerMessage::ReplaceProfile {
                    profile: None,
                    ack: None,
                },
                true,
            )
            .is_ok());
        let latest = DesktopInputSnapshot::buttons_only(r4);
        assert_eq!(
            mailbox.publish_snapshot(&outputs, latest, Duration::from_secs(1)),
            DesktopSnapshotPublish::Overflowed
        );

        let mut messages = mailbox.take_batch(Some(Duration::ZERO));
        assert!(matches!(
            messages.pop_front(),
            Some(DesktopWorkerMessage::ReplaceProfile { .. })
        ));
        assert!(matches!(
            messages.pop_front(),
            Some(DesktopWorkerMessage::Overflow)
        ));
        let Some(DesktopWorkerMessage::Snapshot(snapshot)) = messages.pop_front() else {
            panic!("overflow must retain the latest desktop snapshot");
        };
        assert_eq!(snapshot.snapshot, latest);
        assert_eq!(snapshot.generation, 1);
        assert!(messages.is_empty());
    }

    #[test]
    fn desktop_worker_control_mailbox_is_bounded_with_a_reserved_safety_slot() {
        let mailbox = DesktopWorkerMailbox::default();
        let outputs = DesktopWorkerOutputs::default();
        for _ in 0..(DESKTOP_CONTROL_MAILBOX_CAPACITY - 1) {
            assert!(mailbox
                .push_control(&outputs, DesktopWorkerMessage::Enable { ack: None }, false,)
                .is_ok());
        }
        assert!(mailbox
            .push_control(&outputs, DesktopWorkerMessage::Enable { ack: None }, false,)
            .is_err());

        let (disconnect_ack, _disconnect_receiver) = mpsc::channel();
        assert!(mailbox
            .push_control(
                &outputs,
                DesktopWorkerMessage::Disconnect(disconnect_ack),
                true,
            )
            .is_ok());
        let (shutdown_ack, _shutdown_receiver) = mpsc::channel();
        assert!(mailbox
            .push_control(&outputs, DesktopWorkerMessage::Shutdown(shutdown_ack), true,)
            .is_err());
        assert_eq!(
            mailbox.take_batch(Some(Duration::ZERO)).len(),
            DESKTOP_CONTROL_MAILBOX_CAPACITY
        );
    }

    #[test]
    fn desktop_worker_barriers_discard_staged_pad_feedback() {
        let outputs = DesktopWorkerOutputs::default();
        outputs.publish_feedback(
            0,
            PadFeedbackRequest {
                left: Some(desktop_bindings::PadFeedbackStrength::Low),
                right: Some(desktop_bindings::PadFeedbackStrength::High),
            },
        );

        outputs.invalidate_feedback(1);
        outputs.publish_feedback(
            0,
            PadFeedbackRequest {
                left: Some(desktop_bindings::PadFeedbackStrength::Medium),
                right: None,
            },
        );

        let output = outputs.take();
        assert_eq!(output.feedback, PadFeedbackRequest::NONE);
        assert!(output.discard_pending_feedback);
        outputs.publish_feedback(
            1,
            PadFeedbackRequest {
                left: Some(desktop_bindings::PadFeedbackStrength::Medium),
                right: None,
            },
        );
        let recovered = outputs.take();
        assert_eq!(
            recovered.feedback.left,
            Some(desktop_bindings::PadFeedbackStrength::Medium)
        );
        assert!(!recovered.discard_pending_feedback);
        assert_eq!(outputs.take().feedback, PadFeedbackRequest::NONE);
    }

    #[test]
    fn desktop_worker_mailbox_waits_indefinitely_until_work_arrives() {
        let mailbox = Arc::new(DesktopWorkerMailbox::default());
        let worker_mailbox = Arc::clone(&mailbox);
        let outputs = DesktopWorkerOutputs::default();
        let (started, started_receiver) = mpsc::channel();
        let (completed, completed_receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            started.send(()).unwrap();
            let messages = worker_mailbox.take_batch(None);
            completed.send(messages.len()).unwrap();
        });
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        thread::sleep(RUNTIME_POLL_INTERVAL * 3);
        assert!(matches!(
            completed_receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        assert!(mailbox
            .push_control(&outputs, DesktopWorkerMessage::Enable { ack: None }, false)
            .is_ok());
        assert_eq!(
            completed_receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            1
        );
        handle.join().unwrap();
    }

    struct FakeDiscoverySession {
        events: std::collections::VecDeque<Result<Option<DeviceEvent>, String>>,
        timeouts: Arc<Mutex<Vec<Duration>>>,
    }

    impl FakeDiscoverySession {
        fn idle(timeouts: Arc<Mutex<Vec<Duration>>>) -> Self {
            Self {
                events: std::collections::VecDeque::new(),
                timeouts,
            }
        }

        fn with_report(report: RawHidReport, timeouts: Arc<Mutex<Vec<Duration>>>) -> Self {
            Self {
                events: [Ok(Some(DeviceEvent::Report(report)))].into(),
                timeouts,
            }
        }

        fn with_error(error: &str, timeouts: Arc<Mutex<Vec<Duration>>>) -> Self {
            Self {
                events: [Err(error.to_owned())].into(),
                timeouts,
            }
        }

        fn with_events(
            events: Vec<Result<Option<DeviceEvent>, String>>,
            timeouts: Arc<Mutex<Vec<Duration>>>,
        ) -> Self {
            Self {
                events: events.into(),
                timeouts,
            }
        }
    }

    impl ControllerProbeSession for FakeDiscoverySession {
        fn poll_for_discovery(&mut self, timeout: Duration) -> Result<Option<DeviceEvent>, String> {
            self.timeouts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(timeout);
            self.events.pop_front().unwrap_or(Ok(None))
        }
    }

    #[derive(Default)]
    struct FakeRumbleWriter {
        fail: AtomicBool,
        writes: Mutex<Vec<(u16, u16)>>,
    }

    impl RumbleWriter for FakeRumbleWriter {
        fn write_rumble(&self, low_frequency: u16, high_frequency: u16) -> Result<(), String> {
            if self.fail.load(Ordering::Acquire) {
                return Err("injected rumble failure".to_owned());
            }
            self.writes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((low_frequency, high_frequency));
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakePadFeedbackWriter {
        fail: AtomicBool,
        writes: Mutex<Vec<(PadHapticSide, PadHapticGain)>>,
    }

    impl PadFeedbackWriter for FakePadFeedbackWriter {
        fn write_pad_feedback(
            &self,
            side: PadHapticSide,
            gain: PadHapticGain,
        ) -> Result<(), String> {
            if self.fail.load(Ordering::Acquire) {
                return Err("injected pad feedback failure".to_owned());
            }
            self.writes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((side, gain));
            Ok(())
        }
    }

    struct FakePowerOffWriter {
        results: Mutex<std::collections::VecDeque<Result<(), String>>>,
        writes: AtomicU64,
    }

    impl FakePowerOffWriter {
        fn new(results: impl IntoIterator<Item = Result<(), String>>) -> Self {
            Self {
                results: Mutex::new(results.into_iter().collect()),
                writes: AtomicU64::new(0),
            }
        }
    }

    impl PowerOffWriter for FakePowerOffWriter {
        fn write_power_off(&self) -> Result<(), String> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            self.results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or(Ok(()))
        }
    }

    fn serial_info(path: &str, serial: &str) -> SerialDeviceInfo {
        SerialDeviceInfo {
            path: path.to_owned(),
            vendor_id: Some(bridge_output::XIAO_USB_VENDOR_ID),
            product_id: Some(bridge_output::XIAO_USB_PRODUCT_ID),
            serial_number: Some(serial.to_owned()),
            manufacturer: Some(bridge_output::XIAO_USB_MANUFACTURER.to_owned()),
            product: Some(bridge_output::XIAO_USB_PRODUCT.to_owned()),
        }
    }

    fn controller_info(product_id: u16, interface_number: i32, transport: &str) -> HidDeviceInfo {
        HidDeviceInfo {
            id: format!("{transport}-{interface_number}"),
            path: format!("{transport}-{interface_number}"),
            vendor_id: steam_controller_device::PROTEUS_VENDOR_ID,
            product_id,
            usage_page: steam_controller_device::STEAM_USAGE_PAGE,
            usage: steam_controller_device::STEAM_CONTROLLER_USAGE,
            interface_number,
            serial_number: Some("redacted".to_owned()),
            manufacturer: Some("Valve Corporation".to_owned()),
            product: Some(if transport == "Bluetooth" {
                "Steam Ctrl (BT)".to_owned()
            } else {
                "Steam Controller Puck".to_owned()
            }),
            transport: transport.to_owned(),
        }
    }

    fn controller_state_report(source: &str) -> RawHidReport {
        let mut data = vec![0; steam_controller_protocol::INPUT_REPORT_SIZE];
        data[0] = INPUT_REPORT_ID;
        RawHidReport {
            timestamp: Duration::ZERO,
            report_id: INPUT_REPORT_ID,
            data,
            source_device_id: source.to_owned(),
            transport: "USB".to_owned(),
            dropped_reports: 0,
        }
    }

    #[test]
    fn runtime_defaults_to_zero_configuration_serial_bridge() {
        let config = RuntimeConfig::default();
        assert_eq!(config.controller, ControllerSelection::AutoActive);
        assert_eq!(config.serial, SerialSelection::AutoXiao);
        assert_eq!(config.output, OutputSelection::Serial);
        assert_eq!(config.lizard_mode, LizardMode::Suppress);
        assert_eq!(config.idle_shutdown_timeout, Some(Duration::from_mins(15)));
        assert_eq!(config.puck_dock_action, PuckDockAction::LeaveOn);
        assert!(config.binding_profile.is_none());
    }

    #[test]
    fn runtime_timeout_updates_enforce_the_documented_minimum_and_maximum() {
        assert!(validate_idle_shutdown_timeout(None).is_ok());
        assert!(validate_idle_shutdown_timeout(Some(Duration::from_secs(59))).is_err());
        assert!(validate_idle_shutdown_timeout(Some(Duration::from_mins(1))).is_ok());
        assert!(validate_idle_shutdown_timeout(Some(Duration::from_hours(24))).is_ok());
        assert!(validate_idle_shutdown_timeout(Some(
            Duration::from_hours(24) + Duration::from_secs(1)
        ))
        .is_err());
    }

    #[test]
    fn invalid_battery_values_remain_unknown() {
        assert_eq!(valid_battery_percent(0), Some(0));
        assert_eq!(valid_battery_percent(100), Some(100));
        assert_eq!(valid_battery_percent(101), None);
        assert_eq!(valid_battery_percent(u8::MAX), None);
    }

    #[test]
    fn disabled_recording_does_not_construct_events() {
        let mut writer = None;
        let constructed = std::cell::Cell::new(false);
        record_lazy(&mut writer, || {
            constructed.set(true);
            Ok(RecordingEvent::new(0, recording::KIND_RAW_HID, json!({})))
        })
        .unwrap();
        assert!(!constructed.get());
    }

    #[test]
    fn transition_mailbox_coalesces_analog_reports_but_preserves_button_edges() {
        let mailbox = TransitionReportMailbox::default();
        let dropped = AtomicU64::new(0);
        let report = |sequence: u8, buttons: u32| {
            let mut data = vec![0; steam_controller_protocol::INPUT_REPORT_SIZE];
            data[0] = INPUT_REPORT_ID;
            data[1] = sequence;
            data[2..6].copy_from_slice(&buttons.to_le_bytes());
            RawHidReport {
                timestamp: Duration::ZERO,
                report_id: INPUT_REPORT_ID,
                data,
                source_device_id: "mailbox".to_owned(),
                transport: "USB".to_owned(),
                dropped_reports: 0,
            }
        };
        let r4 = 1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8;
        assert!(mailbox.publish(report(1, 0), &dropped));
        assert!(!mailbox.publish(report(2, 0), &dropped));
        assert!(!mailbox.publish(report(6, 0), &dropped));
        assert!(!mailbox.publish(report(3, r4), &dropped));
        assert!(!mailbox.publish(report(4, r4), &dropped));
        assert!(!mailbox.publish(report(7, r4), &dropped));
        assert!(!mailbox.publish(report(5, 0), &dropped));
        let batch = mailbox.take_all();
        assert!(!batch.overflowed);
        assert_eq!(
            batch
                .reports
                .iter()
                .map(|report| report.data[1])
                .collect::<Vec<_>>(),
            vec![1, 6, 3, 7, 5]
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn transition_mailbox_preserves_pad_touch_baseline_and_latest_coordinates() {
        let mailbox = TransitionReportMailbox::default();
        let dropped = AtomicU64::new(0);
        let report = |sequence: u8, touched: bool, x: i16| {
            let mut data = vec![0; INPUT_REPORT_SIZE];
            data[0] = INPUT_REPORT_ID;
            data[1] = sequence;
            let buttons = if touched {
                1_u32 << SteamButton::RightPadTouch as u8
            } else {
                0
            };
            data[2..6].copy_from_slice(&buttons.to_le_bytes());
            data[24..26].copy_from_slice(&x.to_le_bytes());
            RawHidReport {
                timestamp: Duration::ZERO,
                report_id: INPUT_REPORT_ID,
                data,
                source_device_id: "mailbox".to_owned(),
                transport: "USB".to_owned(),
                dropped_reports: 0,
            }
        };
        assert!(mailbox.publish(report(1, false, 0), &dropped));
        assert!(!mailbox.publish(report(2, true, 100), &dropped));
        assert!(!mailbox.publish(report(3, true, 200), &dropped));
        assert!(!mailbox.publish(report(4, true, 300), &dropped));
        assert!(!mailbox.publish(report(5, false, 0), &dropped));
        let batch = mailbox.take_all();
        assert_eq!(
            batch
                .reports
                .iter()
                .map(|report| report.data[1])
                .collect::<Vec<_>>(),
            vec![1, 2, 4, 5]
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn transition_mailbox_overflow_retains_newest_as_recovery_baseline() {
        let mailbox = TransitionReportMailbox::default();
        let dropped = AtomicU64::new(0);
        let capacity = u8::try_from(INPUT_MAILBOX_CAPACITY).unwrap();
        for sequence in 0..=capacity {
            let mut data = vec![0; steam_controller_protocol::INPUT_REPORT_SIZE];
            data[0] = INPUT_REPORT_ID;
            data[1] = sequence;
            let buttons = if sequence % 2 == 0 {
                0
            } else {
                1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8
            };
            data[2..6].copy_from_slice(&buttons.to_le_bytes());
            let _ = mailbox.publish(
                RawHidReport {
                    timestamp: Duration::ZERO,
                    report_id: INPUT_REPORT_ID,
                    data,
                    source_device_id: "mailbox".to_owned(),
                    transport: "USB".to_owned(),
                    dropped_reports: 0,
                },
                &dropped,
            );
        }
        let batch = mailbox.take_all();
        assert!(batch.overflowed);
        assert_eq!(batch.reports.len(), 1);
        assert_eq!(batch.reports[0].data[1], capacity);
        assert_eq!(
            dropped.load(Ordering::Relaxed),
            INPUT_MAILBOX_CAPACITY as u64
        );
    }

    #[derive(Clone)]
    struct SharedDesktopSink(Arc<Mutex<Vec<String>>>);

    impl DesktopInputSink for SharedDesktopSink {
        fn key(&mut self, key: desktop_bindings::KeyboardKey, pressed: bool) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("key:{key:?}:{pressed}"));
            Ok(())
        }

        fn modifier(
            &mut self,
            modifier: desktop_bindings::Modifier,
            pressed: bool,
        ) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("modifier:{modifier:?}:{pressed}"));
            Ok(())
        }

        fn mouse_button(
            &mut self,
            button: desktop_bindings::MouseButton,
            pressed: bool,
        ) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("mouse:{button:?}:{pressed}"));
            Ok(())
        }

        fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.0.lock().unwrap().push(format!("move:{x}:{y}"));
            Ok(())
        }

        fn scroll(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.0.lock().unwrap().push(format!("scroll:{x}:{y}"));
            Ok(())
        }
    }

    struct DropTrackedDesktopSink {
        inner: SharedDesktopSink,
        drops: Arc<AtomicU64>,
    }

    impl DropTrackedDesktopSink {
        fn new(events: Arc<Mutex<Vec<String>>>, drops: Arc<AtomicU64>) -> Self {
            Self {
                inner: SharedDesktopSink(events),
                drops,
            }
        }
    }

    impl Drop for DropTrackedDesktopSink {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl DesktopInputSink for DropTrackedDesktopSink {
        fn key(&mut self, key: desktop_bindings::KeyboardKey, pressed: bool) -> Result<(), String> {
            self.inner.key(key, pressed)
        }

        fn modifier(
            &mut self,
            modifier: desktop_bindings::Modifier,
            pressed: bool,
        ) -> Result<(), String> {
            self.inner.modifier(modifier, pressed)
        }

        fn mouse_button(
            &mut self,
            button: desktop_bindings::MouseButton,
            pressed: bool,
        ) -> Result<(), String> {
            self.inner.mouse_button(button, pressed)
        }

        fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.inner.mouse_move(x, y)
        }

        fn scroll(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.inner.scroll(x, y)
        }
    }

    struct BlockingMotionSink {
        inner: SharedDesktopSink,
        entered: Option<mpsc::Sender<()>>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl DesktopInputSink for BlockingMotionSink {
        fn key(&mut self, key: desktop_bindings::KeyboardKey, pressed: bool) -> Result<(), String> {
            self.inner.key(key, pressed)
        }

        fn modifier(
            &mut self,
            modifier: desktop_bindings::Modifier,
            pressed: bool,
        ) -> Result<(), String> {
            self.inner.modifier(modifier, pressed)
        }

        fn mouse_button(
            &mut self,
            button: desktop_bindings::MouseButton,
            pressed: bool,
        ) -> Result<(), String> {
            self.inner.mouse_button(button, pressed)
        }

        fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.inner.mouse_move(x, y)?;
            if let Some(entered) = self.entered.take() {
                let _ = entered.send(());
                let (released, wake) = &*self.gate;
                let mut released = released
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while !*released {
                    released = wake
                        .wait(released)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
            Ok(())
        }

        fn scroll(&mut self, x: i32, y: i32) -> Result<(), String> {
            self.inner.scroll(x, y)
        }
    }

    struct LatencyProbeSink {
        starts: Arc<Mutex<VecDeque<Instant>>>,
        samples: SyncSender<Duration>,
    }

    impl DesktopInputSink for LatencyProbeSink {
        fn key(
            &mut self,
            _key: desktop_bindings::KeyboardKey,
            _pressed: bool,
        ) -> Result<(), String> {
            let started = self
                .starts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .ok_or_else(|| "latency probe received an unexpected key event".to_owned())?;
            self.samples
                .send(started.elapsed())
                .map_err(|_| "latency probe receiver disconnected".to_owned())
        }

        fn modifier(
            &mut self,
            _modifier: desktop_bindings::Modifier,
            _pressed: bool,
        ) -> Result<(), String> {
            Ok(())
        }

        fn mouse_button(
            &mut self,
            _button: desktop_bindings::MouseButton,
            _pressed: bool,
        ) -> Result<(), String> {
            Ok(())
        }

        fn mouse_move(&mut self, _x: i32, _y: i32) -> Result<(), String> {
            Ok(())
        }

        fn scroll(&mut self, _x: i32, _y: i32) -> Result<(), String> {
            Ok(())
        }
    }

    struct FailingMotionSink;

    impl DesktopInputSink for FailingMotionSink {
        fn key(
            &mut self,
            _key: desktop_bindings::KeyboardKey,
            _pressed: bool,
        ) -> Result<(), String> {
            Ok(())
        }

        fn modifier(
            &mut self,
            _modifier: desktop_bindings::Modifier,
            _pressed: bool,
        ) -> Result<(), String> {
            Ok(())
        }

        fn mouse_button(
            &mut self,
            _button: desktop_bindings::MouseButton,
            _pressed: bool,
        ) -> Result<(), String> {
            Ok(())
        }

        fn mouse_move(&mut self, _x: i32, _y: i32) -> Result<(), String> {
            Err("desktop permission revoked".to_owned())
        }

        fn scroll(&mut self, _x: i32, _y: i32) -> Result<(), String> {
            Err("desktop permission revoked".to_owned())
        }
    }

    fn desktop_snapshot(buttons: steam_controller_protocol::SteamButtons) -> DesktopInputSnapshot {
        DesktopInputSnapshot::buttons_only(buttons)
    }

    #[test]
    fn blocked_desktop_sink_does_not_block_the_supervisor_facing_publisher() {
        let mut profile = BindingProfile::default();
        profile.pads.right_mouse.enabled = true;
        let status = Arc::new(Mutex::new(BridgeStatus::default()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let worker_events = Arc::clone(&events);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_gate = Arc::clone(&gate);
        let (entered, entered_receiver) = mpsc::channel();
        let worker_profile = profile.clone();
        let mut worker =
            DesktopBindingsWorker::spawn_with_runtime(Arc::clone(&status), move || {
                DesktopBindingsRuntime::with_sink(
                    worker_profile,
                    Box::new(BlockingMotionSink {
                        inner: SharedDesktopSink(worker_events),
                        entered: Some(entered),
                        gate: worker_gate,
                    }),
                )
            });
        let right_pad = |x| DesktopInputSnapshot {
            right_pad: PadSample {
                x,
                touched: true,
                ..PadSample::NEUTRAL
            },
            ..DesktopInputSnapshot::buttons_only(SteamButtons::default())
        };
        worker.observe(DesktopInputSnapshot::buttons_only(SteamButtons::default()));
        worker.observe(right_pad(0));
        worker.observe(right_pad(512));
        entered_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        let (ack, ack_receiver) = mpsc::channel();
        let publish_started = Instant::now();
        for x in 513..=768 {
            worker.observe(right_pad(x));
        }
        worker.replace_profile(Some(profile), ack);
        let publish_elapsed = publish_started.elapsed();
        assert!(matches!(
            ack_receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        let (released, wake) = &*gate;
        *released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        wake.notify_all();
        let command_result = ack_receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        let shutdown_result = worker.shutdown();

        assert!(
            publish_elapsed < SUPERVISOR_STALL_THRESHOLD,
            "desktop publisher took {publish_elapsed:?} while the sink was blocked"
        );
        command_result.unwrap();
        shutdown_result.unwrap();
        assert!(!events.lock().unwrap().is_empty());
    }

    #[test]
    fn blocked_desktop_sink_cannot_defeat_the_shutdown_timeout() {
        let mut profile = BindingProfile::default();
        profile.pads.right_mouse.enabled = true;
        let status = Arc::new(Mutex::new(BridgeStatus::default()));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_gate = Arc::clone(&gate);
        let (entered, entered_receiver) = mpsc::channel();
        let mut worker =
            DesktopBindingsWorker::spawn_with_runtime(Arc::clone(&status), move || {
                DesktopBindingsRuntime::with_sink(
                    profile,
                    Box::new(BlockingMotionSink {
                        inner: SharedDesktopSink(Arc::new(Mutex::new(Vec::new()))),
                        entered: Some(entered),
                        gate: worker_gate,
                    }),
                )
            });
        let alive = Arc::clone(&worker.alive);
        let right_pad = |x| DesktopInputSnapshot {
            right_pad: PadSample {
                x,
                touched: true,
                ..PadSample::NEUTRAL
            },
            ..DesktopInputSnapshot::buttons_only(SteamButtons::default())
        };
        worker.observe(DesktopInputSnapshot::buttons_only(SteamButtons::default()));
        worker.observe(right_pad(0));
        worker.observe(right_pad(512));
        entered_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        let started = Instant::now();
        assert_eq!(
            worker.shutdown_with_timeout(Duration::from_millis(20)),
            Err("desktop-input worker shutdown timed out".to_owned())
        );
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "the timeout was followed by a blocking join"
        );

        let (released, wake) = &*gate;
        *released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        wake.notify_all();
        let deadline = Instant::now() + Duration::from_secs(1);
        while alive.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(!alive.load(Ordering::Acquire));
    }

    #[test]
    fn desktop_worker_input_latency_stays_within_budget() {
        const SAMPLE_COUNT: usize = 64;

        fn latency_profile() -> BindingProfile {
            let mut profile = BindingProfile::default();
            profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
                key: desktop_bindings::KeyboardKey::F5,
                modifiers: std::collections::BTreeSet::new(),
            });
            profile
        }

        fn percentile(samples: &mut [Duration], percentile: usize) -> Duration {
            samples.sort_unstable();
            samples[(samples.len() - 1) * percentile / 100]
        }

        fn push_start(starts: &Mutex<VecDeque<Instant>>) {
            starts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push_back(Instant::now());
        }

        let direct_starts = Arc::new(Mutex::new(VecDeque::new()));
        let (direct_sender, direct_receiver) = mpsc::sync_channel(1);
        let mut direct = DesktopBindingsRuntime::with_sink(
            latency_profile(),
            Box::new(LatencyProbeSink {
                starts: Arc::clone(&direct_starts),
                samples: direct_sender,
            }),
        );
        let neutral = SteamButtons::default();
        let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);
        let _ = direct.observe(desktop_snapshot(neutral), Duration::ZERO);
        let mut direct_samples = Vec::with_capacity(SAMPLE_COUNT);
        for index in 0..SAMPLE_COUNT {
            push_start(&direct_starts);
            let buttons = if index % 2 == 0 { r4 } else { neutral };
            let _ = direct.observe(
                desktop_snapshot(buttons),
                Duration::from_micros(u64::try_from(index + 1).unwrap()),
            );
            direct_samples.push(direct_receiver.recv().unwrap());
        }

        let status = Arc::new(Mutex::new(BridgeStatus::default()));
        let worker_starts = Arc::new(Mutex::new(VecDeque::new()));
        let worker_sink_starts = Arc::clone(&worker_starts);
        let (worker_sender, worker_receiver) = mpsc::sync_channel(1);
        let mut worker =
            DesktopBindingsWorker::spawn_with_runtime(Arc::clone(&status), move || {
                DesktopBindingsRuntime::with_sink(
                    latency_profile(),
                    Box::new(LatencyProbeSink {
                        starts: worker_sink_starts,
                        samples: worker_sender,
                    }),
                )
            });
        worker.observe(desktop_snapshot(neutral));
        let (barrier, barrier_receiver) = mpsc::channel();
        worker.replace_profile(Some(latency_profile()), barrier);
        barrier_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();

        let mut worker_samples = Vec::with_capacity(SAMPLE_COUNT);
        for index in 0..SAMPLE_COUNT {
            // Give the worker time to return to its Condvar wait so this covers
            // the wake-up path instead of measuring only an already-running loop.
            thread::sleep(Duration::from_millis(1));
            push_start(&worker_starts);
            let buttons = if index % 2 == 0 { r4 } else { neutral };
            worker.observe(desktop_snapshot(buttons));
            worker_samples.push(
                worker_receiver
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap(),
            );
        }
        worker.shutdown().unwrap();

        let direct_p50 = percentile(&mut direct_samples, 50);
        let worker_p50 = percentile(&mut worker_samples, 50);
        let worker_p95 = percentile(&mut worker_samples, 95);
        let worker_max = *worker_samples.last().unwrap();
        eprintln!(
            "desktop worker input latency: direct_p50_us={} worker_p50_us={} worker_p95_us={} worker_max_us={}",
            direct_p50.as_micros(),
            worker_p50.as_micros(),
            worker_p95.as_micros(),
            worker_max.as_micros()
        );

        assert!(
            worker_p95 < RUNTIME_POLL_INTERVAL,
            "desktop worker p95 input latency {worker_p95:?} exceeded the {RUNTIME_POLL_INTERVAL:?} runtime tick; direct p50 was {direct_p50:?} and worker p50 was {worker_p50:?}"
        );
        assert!(
            worker_max < SUPERVISOR_STALL_THRESHOLD,
            "desktop worker max input latency {worker_max:?} exceeded the {SUPERVISOR_STALL_THRESHOLD:?} stall threshold"
        );
    }

    #[test]
    fn desktop_worker_disconnect_acknowledges_held_output_release() {
        let mut profile = BindingProfile::default();
        profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        let status = Arc::new(Mutex::new(BridgeStatus::default()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let worker_events = Arc::clone(&events);
        let worker_profile = profile.clone();
        let mut worker =
            DesktopBindingsWorker::spawn_with_runtime(Arc::clone(&status), move || {
                DesktopBindingsRuntime::with_sink(
                    worker_profile,
                    Box::new(SharedDesktopSink(worker_events)),
                )
            });
        let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);
        worker.observe(desktop_snapshot(SteamButtons::default()));
        worker.observe(desktop_snapshot(r4));

        // An identical replacement is an ordered no-op and therefore a useful
        // barrier proving that both snapshots were processed first.
        let (ack, receiver) = mpsc::channel::<Result<(), String>>();
        worker.replace_profile(Some(profile), ack);
        receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert_eq!(*events.lock().unwrap(), ["key:F5:true".to_owned()]);
        assert_eq!(worker.status().held_output_count, 1);

        worker.disconnect().unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
        );
        assert_eq!(worker.status().held_output_count, 0);
        worker.shutdown().unwrap();
    }

    #[test]
    fn desktop_worker_overflow_releases_and_rebaselines_before_recovery() {
        let mut profile = BindingProfile::default();
        profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        let status = Arc::new(Mutex::new(BridgeStatus::default()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let worker_events = Arc::clone(&events);
        let worker_profile = profile.clone();
        let mut worker =
            DesktopBindingsWorker::spawn_with_runtime(Arc::clone(&status), move || {
                DesktopBindingsRuntime::with_sink(
                    worker_profile,
                    Box::new(SharedDesktopSink(worker_events)),
                )
            });
        let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);
        worker.observe(desktop_snapshot(SteamButtons::default()));
        worker.observe(desktop_snapshot(r4));
        let (first_ack, first_receiver) = mpsc::channel();
        worker.replace_profile(Some(profile.clone()), first_ack);
        first_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();

        worker.overflow();
        worker.observe(desktop_snapshot(r4));
        worker.observe(desktop_snapshot(SteamButtons::default()));
        worker.observe(desktop_snapshot(r4));
        let (second_ack, second_receiver) = mpsc::channel();
        worker.replace_profile(Some(profile), second_ack);
        second_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();

        assert_eq!(
            *events.lock().unwrap(),
            [
                "key:F5:true".to_owned(),
                "key:F5:false".to_owned(),
                "key:F5:true".to_owned(),
            ]
        );
        assert_eq!(worker.status().held_output_count, 1);
        worker.shutdown().unwrap();
        assert_eq!(events.lock().unwrap().last().unwrap(), "key:F5:false");
    }

    /// Replays the glue `run_active` puts between a report and the profile
    /// wheel, so these tests cover the wiring and not just the state machine.
    struct PickerHarness {
        picker: PickerRuntime,
        bindings: DesktopBindingsRuntime,
        engine: BridgeEngine,
        output: MockOutput,
        idle: IdleActivityTracker,
        started: Instant,
        events: Vec<PickerEvent>,
    }

    impl PickerHarness {
        fn new(profile: BindingProfile, sink: Box<dyn DesktopInputSink>) -> Self {
            let mut engine =
                BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
            engine.connected();
            Self {
                picker: PickerRuntime::new(Some(PickerConfig::default())),
                bindings: DesktopBindingsRuntime::with_sink(profile, sink),
                engine,
                output: MockOutput::default(),
                idle: IdleActivityTracker::new(None),
                started: Instant::now(),
                events: Vec::new(),
            }
        }

        fn feed(&mut self, now: Duration, report: &RawHidReport, roster: PickerRoster) {
            let effect = process_report(
                report,
                &mut self.engine,
                &mut self.output,
                &mut None,
                self.started,
                &mut self.idle,
            )
            .unwrap();
            let ReportEffect::ControllerState {
                desktop_input,
                picker_input,
                ..
            } = effect
            else {
                return;
            };
            let events = self.picker.observe(now, &picker_input, roster);
            let tapped = events
                .iter()
                .any(|event| matches!(event, PickerEvent::TriggerTapped));
            self.events.extend(events);
            // Every report, matching `run_active`: what is suppressed keeps
            // changing after the wheel has already closed, while the button that
            // closed it is still held.
            self.engine
                .set_output_suppression(self.picker.suppression());
            if tapped {
                let _ = self.bindings.observe(
                    DesktopInputSnapshot {
                        buttons: profile_picker::with_trigger(desktop_input.buttons),
                        ..desktop_input
                    },
                    now,
                );
            }
            let _ = self.bindings.observe(
                DesktopInputSnapshot {
                    buttons: self.picker.mask_trigger(desktop_input.buttons),
                    ..desktop_input
                },
                now,
            );
        }
    }

    fn quick_access_profile() -> BindingProfile {
        let mut profile = BindingProfile::default();
        profile.bindings.quick_access = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        profile
    }

    fn picker_report(
        sequence: u8,
        buttons: &[SteamButton],
        right_stick: (i16, i16),
    ) -> RawHidReport {
        let mut data = vec![0; INPUT_REPORT_SIZE];
        data[0] = INPUT_REPORT_ID;
        data[1] = sequence;
        let mask = buttons
            .iter()
            .fold(0_u32, |mask, button| mask | 1 << *button as u8);
        data[2..6].copy_from_slice(&mask.to_le_bytes());
        data[14..16].copy_from_slice(&right_stick.0.to_le_bytes());
        data[16..18].copy_from_slice(&right_stick.1.to_le_bytes());
        RawHidReport {
            timestamp: Duration::ZERO,
            report_id: INPUT_REPORT_ID,
            data,
            source_device_id: "picker-test".to_owned(),
            transport: "USB".to_owned(),
            dropped_reports: 0,
        }
    }

    const TEST_ROSTER: PickerRoster = PickerRoster {
        len: 4,
        active: Some(0),
        revision: 0,
    };

    #[derive(Clone, Default)]
    struct SharedOutput(Arc<Mutex<Vec<gamepad_state::GamepadState>>>);

    impl GamepadOutput for SharedOutput {
        fn send_state(
            &mut self,
            state: &gamepad_state::GamepadState,
        ) -> Result<(), bridge_output::OutputError> {
            self.0.lock().unwrap().push(*state);
            Ok(())
        }
    }

    struct DropOrderOutput(Arc<Mutex<Vec<&'static str>>>);

    impl Drop for DropOrderOutput {
        fn drop(&mut self) {
            self.0.lock().unwrap().push("output");
        }
    }

    impl GamepadOutput for DropOrderOutput {
        fn send_state(
            &mut self,
            _state: &gamepad_state::GamepadState,
        ) -> Result<(), bridge_output::OutputError> {
            Ok(())
        }
    }

    #[test]
    fn hardware_release_finishes_before_command_acknowledgement() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let output = OutputSession {
            output: Box::new(DropOrderOutput(Arc::clone(&order))),
            xiao: None,
        };
        let release_order = Arc::clone(&order);
        let ack_order = Arc::clone(&order);
        let (ack, receiver) = mpsc::channel::<Result<(), String>>();
        let observer = thread::spawn(move || {
            receiver.recv().unwrap().unwrap();
            ack_order.lock().unwrap().push("ack");
        });

        acknowledge_after_hardware_release(
            output,
            move || release_order.lock().unwrap().push("controllers"),
            &ack,
            Ok(()),
        );
        observer.join().unwrap();
        assert_eq!(*order.lock().unwrap(), ["output", "controllers", "ack"]);
    }

    #[test]
    fn a_slow_desktop_operation_is_preceded_by_neutral_on_the_wire() {
        // Regression: constructing the desktop-input sink, or destroying it
        // after a backend failure, can block this thread beyond the firmware's
        // 100 ms controller-data watchdog. Healthy profile switches retain the
        // sink, but neutral remains the safety boundary around lifecycle work.
        let states = Arc::new(Mutex::new(Vec::new()));
        let mut session = OutputSession {
            output: Box::new(SharedOutput(Arc::clone(&states))),
            xiao: None,
        };
        let mut engine =
            BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
        engine.connected();
        let mut idle = IdleActivityTracker::new(None);
        let started = Instant::now();
        let held = [SteamButton::A];

        let report = picker_report(1, &held, (0, 32_767));
        process_report(
            &report,
            &mut engine,
            &mut *session.output,
            &mut None,
            started,
            &mut idle,
        )
        .unwrap();
        let active = *states.lock().unwrap().last().unwrap();
        assert_ne!(active, gamepad_state::GamepadState::NEUTRAL);

        neutralize_before_desktop_work(&mut engine, &mut session);
        assert_eq!(
            states.lock().unwrap().last(),
            Some(&gamepad_state::GamepadState::NEUTRAL),
            "the device must be parked before the thread blocks"
        );

        // The controller has not moved, so the unchanged-output dedupe must
        // still know the wire is at neutral and resend the real state.
        process_report(
            &picker_report(2, &held, (0, 32_767)),
            &mut engine,
            &mut *session.output,
            &mut None,
            started,
            &mut idle,
        )
        .unwrap();
        assert_eq!(
            states.lock().unwrap().last(),
            Some(&active),
            "the real state must come back after the operation"
        );
    }

    #[test]
    fn holding_quick_access_opens_the_wheel_without_firing_its_binding() {
        let keys = Arc::new(Mutex::new(Vec::new()));
        let mut harness = PickerHarness::new(
            quick_access_profile(),
            Box::new(SharedDesktopSink(Arc::clone(&keys))),
        );
        harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
        harness.feed(
            Duration::from_millis(10),
            &picker_report(2, &[SteamButton::QuickAccess], (0, 0)),
            TEST_ROSTER,
        );
        // Arming already hides the press, so the F5 chord never fires.
        assert!(keys.lock().unwrap().is_empty());

        harness.feed(
            Duration::from_millis(2_010),
            &picker_report(3, &[SteamButton::QuickAccess], (0, 0)),
            TEST_ROSTER,
        );
        assert_eq!(
            harness.events,
            vec![PickerEvent::Opened {
                selected: 0,
                page: 0,
                roster_revision: 0,
            }]
        );
        assert!(keys.lock().unwrap().is_empty());
        assert!(harness.engine.output_suppression().is_some());
    }

    #[test]
    fn an_open_wheel_hides_its_controls_from_the_game_and_gives_them_back() {
        let keys = Arc::new(Mutex::new(Vec::new()));
        let mut harness = PickerHarness::new(
            quick_access_profile(),
            Box::new(SharedDesktopSink(Arc::clone(&keys))),
        );
        let held = [SteamButton::QuickAccess];
        harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
        harness.feed(
            Duration::from_millis(10),
            &picker_report(2, &held, (0, 0)),
            TEST_ROSTER,
        );
        harness.feed(
            Duration::from_millis(2_010),
            &picker_report(3, &held, (0, 0)),
            TEST_ROSTER,
        );
        assert!(harness.picker.is_open());

        // Steering the wheel must not also steer the game.
        harness.feed(
            Duration::from_millis(2_100),
            &picker_report(4, &held, (0, 32_767)),
            TEST_ROSTER,
        );
        let hidden = *harness.output.states.last().unwrap();
        assert_eq!((hidden.right_x, hidden.right_y), (0.0, 0.0));
        assert!(!hidden.buttons.contains(gamepad_state::Button::Extra3));

        // A commits and the wheel closes, but A is still physically down.
        harness.feed(
            Duration::from_millis(2_200),
            &picker_report(5, &[SteamButton::A], (0, 32_767)),
            TEST_ROSTER,
        );
        assert!(!harness.picker.is_open());
        assert_eq!(
            harness.events.last(),
            Some(&PickerEvent::Commit {
                index: 0,
                roster_revision: 0,
            })
        );

        // The sticks come back immediately -- the user can play again -- but the
        // press that closed the wheel must not reach the game just because it is
        // still held. Regression: it used to, on this very report.
        harness.feed(
            Duration::from_millis(2_250),
            &picker_report(6, &[SteamButton::A], (0, 32_767)),
            TEST_ROSTER,
        );
        let after_commit = *harness.output.states.last().unwrap();
        assert!(after_commit.right_y > 0.0, "the game is playable again");
        assert!(
            !after_commit.buttons.contains(gamepad_state::Button::South),
            "the commit press must not leak into the game"
        );

        // Released, so a later, deliberate press does reach the game.
        harness.feed(
            Duration::from_millis(2_300),
            &picker_report(7, &[], (0, 32_767)),
            TEST_ROSTER,
        );
        assert!(harness.engine.output_suppression().is_none());
        harness.feed(
            Duration::from_millis(2_400),
            &picker_report(8, &[SteamButton::A], (0, 32_767)),
            TEST_ROSTER,
        );
        let deliberate = *harness.output.states.last().unwrap();
        assert!(deliberate.buttons.contains(gamepad_state::Button::South));
    }

    #[test]
    fn a_quick_access_tap_still_fires_its_desktop_binding() {
        let keys = Arc::new(Mutex::new(Vec::new()));
        let mut harness = PickerHarness::new(
            quick_access_profile(),
            Box::new(SharedDesktopSink(Arc::clone(&keys))),
        );
        harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
        harness.feed(
            Duration::from_millis(10),
            &picker_report(2, &[SteamButton::QuickAccess], (0, 0)),
            TEST_ROSTER,
        );
        harness.feed(
            Duration::from_millis(500),
            &picker_report(3, &[], (0, 0)),
            TEST_ROSTER,
        );
        assert_eq!(harness.events, vec![PickerEvent::TriggerTapped]);
        assert_eq!(
            *keys.lock().unwrap(),
            ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
        );
    }

    #[test]
    fn dismissing_with_a_second_quick_access_press_does_not_fire_its_binding() {
        // Regression: the dismissing press returns the picker to Idle on the
        // very report that carries the down edge. Without the latch-aware mask
        // the bindings engine saw that edge as a fresh press and fired the
        // binding the wheel exists to protect.
        let keys = Arc::new(Mutex::new(Vec::new()));
        let mut harness = PickerHarness::new(
            quick_access_profile(),
            Box::new(SharedDesktopSink(Arc::clone(&keys))),
        );
        let held = [SteamButton::QuickAccess];
        harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
        harness.feed(
            Duration::from_millis(10),
            &picker_report(2, &held, (0, 0)),
            TEST_ROSTER,
        );
        harness.feed(
            Duration::from_millis(2_010),
            &picker_report(3, &held, (0, 0)),
            TEST_ROSTER,
        );
        assert!(harness.picker.is_open());

        // Release, then press Quick Access again to cancel.
        harness.feed(
            Duration::from_millis(2_100),
            &picker_report(4, &[], (0, 0)),
            TEST_ROSTER,
        );
        harness.feed(
            Duration::from_millis(2_200),
            &picker_report(5, &held, (0, 0)),
            TEST_ROSTER,
        );
        assert_eq!(harness.events.last(), Some(&PickerEvent::Dismissed));
        assert!(
            keys.lock().unwrap().is_empty(),
            "cancelling the wheel must not fire the Quick Access binding"
        );

        // Still held: still nothing. Released: still nothing.
        harness.feed(
            Duration::from_millis(2_300),
            &picker_report(6, &held, (0, 0)),
            TEST_ROSTER,
        );
        harness.feed(
            Duration::from_millis(2_400),
            &picker_report(7, &[], (0, 0)),
            TEST_ROSTER,
        );
        assert!(keys.lock().unwrap().is_empty());
        assert!(harness.engine.output_suppression().is_none());

        // A later deliberate tap fires the binding as normal.
        harness.feed(
            Duration::from_secs(3),
            &picker_report(8, &held, (0, 0)),
            TEST_ROSTER,
        );
        harness.feed(
            Duration::from_millis(3_100),
            &picker_report(9, &[], (0, 0)),
            TEST_ROSTER,
        );
        assert_eq!(
            *keys.lock().unwrap(),
            ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
        );
    }

    #[test]
    fn a_config_change_mid_hold_cancels_the_wheel_and_swallows_the_press() {
        // Past the halfway mark the overlay child is already running, so the
        // caller must be told the hold is off (it answers with `Dismissed`),
        // and the withheld press must not become a fresh edge for the
        // bindings engine.
        let keys = Arc::new(Mutex::new(Vec::new()));
        let mut harness = PickerHarness::new(
            quick_access_profile(),
            Box::new(SharedDesktopSink(Arc::clone(&keys))),
        );
        let held = [SteamButton::QuickAccess];
        harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
        harness.feed(
            Duration::from_millis(10),
            &picker_report(2, &held, (0, 0)),
            TEST_ROSTER,
        );
        harness.feed(
            Duration::from_millis(1_200),
            &picker_report(3, &held, (0, 0)),
            TEST_ROSTER,
        );
        assert_eq!(harness.events, vec![PickerEvent::Preparing]);

        assert!(
            harness.picker.set_config(Some(PickerConfig {
                hold: Duration::from_secs(3),
                ..PickerConfig::default()
            })),
            "a cancelled hold must be reported so the overlay child is stopped"
        );

        // The press stays swallowed while held, and its release is not a tap.
        harness.feed(
            Duration::from_millis(1_300),
            &picker_report(4, &held, (0, 0)),
            TEST_ROSTER,
        );
        harness.feed(
            Duration::from_millis(1_400),
            &picker_report(5, &[], (0, 0)),
            TEST_ROSTER,
        );
        assert_eq!(harness.events, vec![PickerEvent::Preparing]);
        assert!(keys.lock().unwrap().is_empty());
        assert!(harness.engine.output_suppression().is_none());
    }

    #[test]
    fn disabling_the_wheel_while_open_keeps_held_controls_latched() {
        // Switching the feature off is not allowed to hand the game or the
        // bindings engine the buttons that were operating the wheel.
        let keys = Arc::new(Mutex::new(Vec::new()));
        let mut harness = PickerHarness::new(
            quick_access_profile(),
            Box::new(SharedDesktopSink(Arc::clone(&keys))),
        );
        let held = [SteamButton::QuickAccess];
        harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
        harness.feed(
            Duration::from_millis(10),
            &picker_report(2, &held, (0, 0)),
            TEST_ROSTER,
        );
        harness.feed(
            Duration::from_millis(2_010),
            &picker_report(3, &held, (0, 0)),
            TEST_ROSTER,
        );
        assert!(harness.picker.is_open());

        assert!(harness.picker.set_config(None));
        harness.feed(
            Duration::from_millis(2_100),
            &picker_report(4, &held, (0, 0)),
            TEST_ROSTER,
        );
        assert!(
            keys.lock().unwrap().is_empty(),
            "the held trigger must not become a fresh press when the wheel is disabled"
        );
        assert!(
            harness.engine.output_suppression().is_some(),
            "the held trigger stays withheld from the game until released"
        );

        // Released: everything drains. A fresh press is an ordinary binding
        // press again, with no wheel to intercept it.
        harness.feed(
            Duration::from_millis(2_200),
            &picker_report(5, &[], (0, 0)),
            TEST_ROSTER,
        );
        assert!(harness.engine.output_suppression().is_none());
        let events_before_press = harness.events.len();
        harness.feed(
            Duration::from_secs(3),
            &picker_report(6, &held, (0, 0)),
            TEST_ROSTER,
        );
        assert_eq!(*keys.lock().unwrap(), ["key:F5:true".to_owned()]);
        assert_eq!(
            harness.events.len(),
            events_before_press,
            "a disabled wheel must not arm or open again"
        );
    }

    #[test]
    fn a_disabled_wheel_leaves_quick_access_entirely_alone() {
        let keys = Arc::new(Mutex::new(Vec::new()));
        let mut harness = PickerHarness::new(
            quick_access_profile(),
            Box::new(SharedDesktopSink(Arc::clone(&keys))),
        );
        harness.picker = PickerRuntime::new(None);
        harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
        harness.feed(
            Duration::from_millis(10),
            &picker_report(2, &[SteamButton::QuickAccess], (0, 0)),
            TEST_ROSTER,
        );
        // The binding fires on the press edge, exactly as before the wheel existed.
        assert_eq!(*keys.lock().unwrap(), ["key:F5:true".to_owned()]);
        harness.feed(
            Duration::from_secs(5),
            &picker_report(3, &[SteamButton::QuickAccess], (0, 0)),
            TEST_ROSTER,
        );
        assert!(harness.events.is_empty());
        assert!(harness.engine.output_suppression().is_none());
    }

    #[test]
    fn closing_the_wheel_for_a_lost_controller_reports_it_once() {
        let keys = Arc::new(Mutex::new(Vec::new()));
        let mut harness = PickerHarness::new(
            quick_access_profile(),
            Box::new(SharedDesktopSink(Arc::clone(&keys))),
        );
        let held = [SteamButton::QuickAccess];
        harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
        harness.feed(
            Duration::from_millis(10),
            &picker_report(2, &held, (0, 0)),
            TEST_ROSTER,
        );
        harness.feed(
            Duration::from_millis(2_010),
            &picker_report(3, &held, (0, 0)),
            TEST_ROSTER,
        );
        assert!(harness.picker.is_open());
        assert!(harness.picker.close());
        assert!(!harness.picker.close());
    }

    #[test]
    fn runtime_binding_observation_does_not_change_gamepad_output() {
        let report = |sequence: u8, buttons: u32| {
            let mut data = vec![0; INPUT_REPORT_SIZE];
            data[0] = INPUT_REPORT_ID;
            data[1] = sequence;
            data[2..6].copy_from_slice(&buttons.to_le_bytes());
            RawHidReport {
                timestamp: Duration::ZERO,
                report_id: INPUT_REPORT_ID,
                data,
                source_device_id: "runtime-test".to_owned(),
                transport: "USB".to_owned(),
                dropped_reports: 0,
            }
        };
        let r4 = 1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8;
        let reports = [report(1, 0), report(2, r4), report(3, 0)];

        let mut profile = BindingProfile::default();
        profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut bindings = DesktopBindingsRuntime::with_sink(
            profile,
            Box::new(SharedDesktopSink(Arc::clone(&events))),
        );
        let mut bound_engine =
            BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
        let mut control_engine =
            BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
        bound_engine.connected();
        control_engine.connected();
        let mut bound_output = MockOutput::default();
        let mut control_output = MockOutput::default();
        let mut bound_idle = IdleActivityTracker::new(None);
        let mut control_idle = IdleActivityTracker::new(None);
        let started = Instant::now();
        for report in &reports {
            let effect = process_report(
                report,
                &mut bound_engine,
                &mut bound_output,
                &mut None,
                started,
                &mut bound_idle,
            )
            .unwrap();
            if let ReportEffect::ControllerState { desktop_input, .. } = effect {
                let _ = bindings.observe(desktop_input, started.elapsed());
            }
            let _ = process_report(
                report,
                &mut control_engine,
                &mut control_output,
                &mut None,
                started,
                &mut control_idle,
            )
            .unwrap();
        }
        assert_eq!(bound_output.states, control_output.states);
        assert_eq!(
            *events.lock().unwrap(),
            ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
        );
    }

    #[test]
    fn runtime_pad_observation_emits_mouse_and_feedback_without_changing_gamepad_output() {
        let report = |sequence: u8, touched: bool, x: i16| {
            let mut data = vec![0; INPUT_REPORT_SIZE];
            data[0] = INPUT_REPORT_ID;
            data[1] = sequence;
            let buttons = if touched {
                1_u32 << SteamButton::RightPadTouch as u8
            } else {
                0
            };
            data[2..6].copy_from_slice(&buttons.to_le_bytes());
            data[24..26].copy_from_slice(&x.to_le_bytes());
            RawHidReport {
                timestamp: Duration::ZERO,
                report_id: INPUT_REPORT_ID,
                data,
                source_device_id: "runtime-pad-test".to_owned(),
                transport: "USB".to_owned(),
                dropped_reports: 0,
            }
        };
        let reports = [
            report(1, false, 0),
            report(2, true, 0),
            report(3, true, 768),
        ];
        let mut profile = BindingProfile::default();
        profile.pads.right_mouse.enabled = true;
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut bindings = DesktopBindingsRuntime::with_sink(
            profile,
            Box::new(SharedDesktopSink(Arc::clone(&events))),
        );
        let mut bound_engine =
            BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
        let mut control_engine =
            BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
        bound_engine.connected();
        control_engine.connected();
        let mut bound_output = MockOutput::default();
        let mut control_output = MockOutput::default();
        let mut bound_idle = IdleActivityTracker::new(None);
        let mut control_idle = IdleActivityTracker::new(None);
        let started = Instant::now();
        let mut feedback = PadFeedbackRequest::NONE;
        for (index, report) in reports.iter().enumerate() {
            let effect = process_report(
                report,
                &mut bound_engine,
                &mut bound_output,
                &mut None,
                started,
                &mut bound_idle,
            )
            .unwrap();
            if let ReportEffect::ControllerState { desktop_input, .. } = effect {
                feedback = bindings.observe(
                    desktop_input,
                    Duration::from_millis(u64::try_from(index * 20).unwrap()),
                );
            }
            let _ = process_report(
                report,
                &mut control_engine,
                &mut control_output,
                &mut None,
                started,
                &mut control_idle,
            )
            .unwrap();
        }
        assert_eq!(bound_output.states, control_output.states);
        assert_eq!(*events.lock().unwrap(), ["move:12:0".to_owned()]);
        assert_eq!(
            feedback.right,
            Some(desktop_bindings::PadFeedbackStrength::Medium)
        );
    }

    #[test]
    fn runtime_tick_advances_scroll_momentum_without_more_hid_reports() {
        let mut profile = BindingProfile::default();
        profile.pads.left_scroll.enabled = true;
        profile.pads.left_scroll.feedback.enabled = false;
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut bindings = DesktopBindingsRuntime::with_sink(
            profile,
            Box::new(SharedDesktopSink(Arc::clone(&events))),
        );
        let snapshot = |x, touched| DesktopInputSnapshot {
            left_pad: PadSample {
                x,
                touched,
                ..PadSample::default()
            },
            ..desktop_snapshot(steam_controller_protocol::SteamButtons::default())
        };

        assert!(!bindings.needs_tick());
        let _ = bindings.observe(snapshot(0, false), Duration::ZERO);
        let _ = bindings.observe(snapshot(0, true), Duration::from_millis(1));
        let _ = bindings.observe(snapshot(768, true), Duration::from_millis(21));
        assert!(!bindings.needs_tick());
        let _ = bindings.observe(snapshot(0, false), Duration::from_millis(22));
        assert!(bindings.needs_tick());
        bindings.tick(Duration::from_millis(72));

        assert_eq!(
            *events.lock().unwrap(),
            ["scroll:12:0".to_owned(), "scroll:10:0".to_owned()]
        );
    }

    #[test]
    fn desktop_motion_failure_requests_pending_feedback_discard() {
        let mut profile = BindingProfile::default();
        profile.pads.right_mouse.enabled = true;
        let mut bindings = DesktopBindingsRuntime::with_sink(profile, Box::new(FailingMotionSink));
        let snapshot = |x, touched| DesktopInputSnapshot {
            right_pad: PadSample {
                x,
                touched,
                ..PadSample::default()
            },
            ..desktop_snapshot(steam_controller_protocol::SteamButtons::default())
        };

        let _ = bindings.observe(snapshot(0, false), Duration::ZERO);
        let _ = bindings.observe(snapshot(0, true), Duration::from_millis(1));
        assert_eq!(
            bindings.observe(snapshot(224, true), Duration::from_millis(20)),
            PadFeedbackRequest::NONE
        );
        assert_eq!(bindings.status().state, DesktopBindingsState::Degraded);
        assert!(bindings.take_discard_pending_feedback());
        assert!(!bindings.take_discard_pending_feedback());
    }

    #[test]
    fn desktop_status_is_published_only_when_semantics_change() {
        let mut profile = BindingProfile::default();
        profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        let mut bindings = DesktopBindingsRuntime::with_sink(
            profile,
            Box::new(SharedDesktopSink(Arc::new(Mutex::new(Vec::new())))),
        );
        assert!(bindings.take_status_update().is_some());
        assert!(bindings.take_status_update().is_none());

        let neutral = SteamButtons::default();
        let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);
        let _ = bindings.observe(desktop_snapshot(neutral), Duration::ZERO);
        assert!(bindings.take_status_update().is_none());
        let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(1));
        assert_eq!(bindings.take_status_update().unwrap().held_output_count, 1);
        assert!(bindings.take_status_update().is_none());
    }

    #[test]
    fn replacing_profile_keeps_an_existing_authorized_sink_ready() {
        let mut first = BindingProfile::default();
        first.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut bindings = DesktopBindingsRuntime::with_sink(
            first,
            Box::new(SharedDesktopSink(Arc::clone(&events))),
        );
        let mut second = BindingProfile {
            name: "Second".to_owned(),
            ..BindingProfile::default()
        };
        second.bindings.r5 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F9,
            modifiers: std::collections::BTreeSet::new(),
        });

        bindings.replace_profile(Some(second)).unwrap();

        let status = bindings.status();
        assert_eq!(status.state, DesktopBindingsState::Ready);
        assert_eq!(status.configured_binding_count, 1);
        assert!(status.last_error.is_none());
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn switching_through_an_unbound_profile_reuses_the_authorized_sink() {
        let mut first = BindingProfile::default();
        first.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let drops = Arc::new(AtomicU64::new(0));
        let mut bindings = DesktopBindingsRuntime::with_sink(
            first,
            Box::new(DropTrackedDesktopSink::new(
                Arc::clone(&events),
                Arc::clone(&drops),
            )),
        );
        let neutral = steam_controller_protocol::SteamButtons::default();
        let r4 = steam_controller_protocol::SteamButtons(
            1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8,
        );
        let _ = bindings.observe(desktop_snapshot(neutral), Duration::ZERO);
        let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(1));

        bindings
            .replace_profile(Some(BindingProfile::default()))
            .unwrap();

        let status = bindings.status();
        assert_eq!(status.state, DesktopBindingsState::Disabled);
        assert_eq!(status.configured_binding_count, 0);
        assert_eq!(status.held_output_count, 0);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        assert_eq!(
            *events.lock().unwrap(),
            ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
        );

        let mut rebound = BindingProfile {
            name: "Rebound".to_owned(),
            ..BindingProfile::default()
        };
        rebound.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F9,
            modifiers: std::collections::BTreeSet::new(),
        });
        bindings.replace_profile(Some(rebound)).unwrap();
        assert_eq!(bindings.status().state, DesktopBindingsState::Ready);
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        // The control stayed held through both switches. Reusing the sink must
        // not turn that into a synthetic press in the replacement profile.
        let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(2));
        assert_eq!(events.lock().unwrap().len(), 2);
        let _ = bindings.observe(desktop_snapshot(neutral), Duration::from_millis(3));
        let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(4));
        assert_eq!(
            *events.lock().unwrap(),
            [
                "key:F5:true".to_owned(),
                "key:F5:false".to_owned(),
                "key:F9:true".to_owned(),
            ]
        );

        drop(bindings);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn clearing_a_profile_retains_the_authorized_sink_for_later_reuse() {
        let mut first = BindingProfile::default();
        first.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let drops = Arc::new(AtomicU64::new(0));
        let mut bindings = DesktopBindingsRuntime::with_sink(
            first,
            Box::new(DropTrackedDesktopSink::new(
                Arc::clone(&events),
                Arc::clone(&drops),
            )),
        );

        bindings.replace_profile(None).unwrap();
        assert_eq!(bindings.status(), DesktopBindingsStatus::default());
        assert!(bindings.engine.is_none());
        assert!(bindings.sink.is_some());
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        let mut replacement = BindingProfile::default();
        replacement.bindings.r5 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F9,
            modifiers: std::collections::BTreeSet::new(),
        });
        bindings.replace_profile(Some(replacement)).unwrap();
        assert_eq!(bindings.status().state, DesktopBindingsState::Ready);
        assert!(bindings.engine.is_some());
        assert!(bindings.sink.is_some());
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        assert!(events.lock().unwrap().is_empty());

        drop(bindings);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn idle_profile_replacement_does_not_initialize_desktop_input() {
        let mut profile = BindingProfile::default();
        profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        let mut bindings = DesktopBindingsRuntime::new(None);

        bindings.replace_profile(Some(profile)).unwrap();

        assert!(!bindings.activation_requested);
        assert!(bindings.sink.is_none());
        assert_eq!(
            bindings.status().state,
            DesktopBindingsState::PermissionRequired
        );
    }

    #[test]
    fn enabling_an_already_ready_sink_preserves_held_output_state() {
        let mut profile = BindingProfile::default();
        profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut bindings = DesktopBindingsRuntime::with_sink(
            profile,
            Box::new(SharedDesktopSink(Arc::clone(&events))),
        );
        let r4 = steam_controller_protocol::SteamButtons(
            1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8,
        );
        let _ = bindings.observe(
            desktop_snapshot(steam_controller_protocol::SteamButtons::default()),
            Duration::ZERO,
        );
        let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(1));

        bindings.enable().unwrap();
        let _ = bindings.observe(
            desktop_snapshot(steam_controller_protocol::SteamButtons::default()),
            Duration::from_millis(2),
        );

        assert_eq!(
            *events.lock().unwrap(),
            ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
        );
    }

    #[test]
    fn mailbox_overflow_recovers_after_a_non_emitting_baseline() {
        let mut profile = BindingProfile::default();
        profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut bindings = DesktopBindingsRuntime::with_sink(
            profile,
            Box::new(SharedDesktopSink(Arc::clone(&events))),
        );
        let neutral = steam_controller_protocol::SteamButtons::default();
        let r4 = steam_controller_protocol::SteamButtons(
            1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8,
        );
        let _ = bindings.observe(desktop_snapshot(neutral), Duration::ZERO);
        let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(1));

        bindings.overflow();
        assert_eq!(bindings.status().state, DesktopBindingsState::Degraded);
        let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(2));
        assert_eq!(bindings.status().state, DesktopBindingsState::Ready);
        let _ = bindings.observe(desktop_snapshot(neutral), Duration::from_millis(3));
        let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(4));

        assert_eq!(
            *events.lock().unwrap(),
            [
                "key:F5:true".to_owned(),
                "key:F5:false".to_owned(),
                "key:F5:true".to_owned(),
            ]
        );
        assert_eq!(bindings.status().failures, 1);
    }

    #[test]
    fn latest_rumble_slot_coalesces_to_one_command() {
        let slot = LatestRumbleSlot::default();
        assert!(!slot.publish(RumbleCommand {
            low_frequency: 1,
            high_frequency: 2,
        }));
        assert!(slot.publish(RumbleCommand {
            low_frequency: 3,
            high_frequency: 4,
        }));
        assert_eq!(
            slot.take(),
            Some(RumbleCommand {
                low_frequency: 3,
                high_frequency: 4,
            })
        );
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn pending_pad_feedback_coalesces_sides_and_preserves_strength() {
        let pending = PendingPadFeedback::default();
        assert_eq!(
            pending.publish(PadFeedbackRequest {
                left: Some(desktop_bindings::PadFeedbackStrength::Medium),
                right: None,
            }),
            0
        );
        assert_eq!(
            pending.publish(PadFeedbackRequest {
                left: Some(desktop_bindings::PadFeedbackStrength::Medium),
                right: Some(desktop_bindings::PadFeedbackStrength::Medium),
            }),
            1
        );
        assert_eq!(
            pending.take(),
            vec![PadFeedbackCommand {
                side: PadHapticSide::Both,
                gain: PadHapticGain::Medium,
            }]
        );

        let _ = pending.publish(PadFeedbackRequest {
            left: Some(desktop_bindings::PadFeedbackStrength::Low),
            right: Some(desktop_bindings::PadFeedbackStrength::High),
        });
        assert_eq!(
            pending.take(),
            vec![
                PadFeedbackCommand {
                    side: PadHapticSide::Left,
                    gain: PadHapticGain::Low,
                },
                PadFeedbackCommand {
                    side: PadHapticSide::Right,
                    gain: PadHapticGain::High,
                },
            ]
        );
        pending.clear();
        assert!(pending.take().is_empty());
    }

    #[test]
    fn pad_feedback_failure_backs_off_without_changing_rumble_state() {
        let metrics = Arc::new(SharedHapticsMetrics::default());
        let writer = FakePadFeedbackWriter::default();
        let mut feedback = PadFeedbackSupervisor::new(Arc::clone(&metrics));
        feedback.connected();
        writer.fail.store(true, Ordering::Release);
        feedback.service(
            Duration::from_millis(10),
            &writer,
            vec![PadFeedbackCommand {
                side: PadHapticSide::Right,
                gain: PadHapticGain::Medium,
            }],
        );
        let failed = metrics.snapshot(Duration::from_millis(10));
        assert_eq!(failed.state, HapticsState::Degraded);
        assert_eq!(failed.pad_feedback_failures, 1);
        assert!(failed.pad_feedback_last_error.is_some());

        writer.fail.store(false, Ordering::Release);
        feedback.service(
            Duration::from_millis(509),
            &writer,
            vec![PadFeedbackCommand {
                side: PadHapticSide::Right,
                gain: PadHapticGain::Medium,
            }],
        );
        assert!(writer.writes.lock().unwrap().is_empty());
        feedback.service(
            Duration::from_millis(510),
            &writer,
            vec![PadFeedbackCommand {
                side: PadHapticSide::Right,
                gain: PadHapticGain::Medium,
            }],
        );
        let recovered = metrics.snapshot(Duration::from_millis(510));
        assert_eq!(recovered.state, HapticsState::Idle);
        assert_eq!(recovered.pad_feedback_ticks, 1);
        assert!(recovered.pad_feedback_last_error.is_none());
    }

    #[test]
    fn haptics_refreshes_expires_and_recovers_after_backoff() {
        let metrics = Arc::new(SharedHapticsMetrics::default());
        let writer = FakeRumbleWriter::default();
        let mut haptics = HapticsSupervisor::new(Arc::clone(&metrics));

        haptics.connected(Duration::ZERO, &writer);
        haptics.command(
            Duration::from_millis(1),
            &writer,
            RumbleCommand {
                low_frequency: 0x1234,
                high_frequency: 0xabcd,
            },
        );
        haptics.service(Duration::from_millis(40), &writer);
        assert_eq!(metrics.snapshot(Duration::from_millis(40)).refreshes, 0);
        haptics.service(Duration::from_millis(41), &writer);
        assert_eq!(metrics.snapshot(Duration::from_millis(41)).refreshes, 1);

        haptics.command(
            Duration::from_millis(50),
            &writer,
            RumbleCommand {
                low_frequency: 0x1234,
                high_frequency: 0xabcd,
            },
        );
        haptics.service(Duration::from_millis(150), &writer);
        assert_eq!(
            writer
                .writes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .last()
                .copied(),
            Some((0, 0))
        );
        assert_eq!(
            metrics.snapshot(Duration::from_millis(150)).state,
            HapticsState::Idle
        );

        writer.fail.store(true, Ordering::Release);
        haptics.command(
            Duration::from_millis(200),
            &writer,
            RumbleCommand {
                low_frequency: 1,
                high_frequency: 2,
            },
        );
        assert_eq!(
            metrics.snapshot(Duration::from_millis(200)).state,
            HapticsState::Degraded
        );
        writer.fail.store(false, Ordering::Release);
        for now in [250, 340, 430, 520, 610, 699] {
            haptics.command(
                Duration::from_millis(now),
                &writer,
                RumbleCommand {
                    low_frequency: 1,
                    high_frequency: 2,
                },
            );
            haptics.service(Duration::from_millis(now), &writer);
        }
        assert_eq!(
            metrics.snapshot(Duration::from_millis(699)).state,
            HapticsState::Degraded
        );
        haptics.command(
            Duration::from_millis(700),
            &writer,
            RumbleCommand {
                low_frequency: 1,
                high_frequency: 2,
            },
        );
        haptics.service(Duration::from_millis(700), &writer);
        let recovered = metrics.snapshot(Duration::from_millis(700));
        assert_eq!(recovered.state, HapticsState::Active);
        assert_eq!(recovered.failures, 1);
    }

    #[test]
    fn active_source_selection_is_order_independent_and_rejects_ambiguity() {
        assert_eq!(choose_unique_active(&[]), Ok(None));
        assert_eq!(choose_unique_active(&[3]), Ok(Some(3)));
        assert_eq!(choose_unique_active(&[0]), Ok(Some(0)));
        assert_eq!(choose_unique_active(&[1, 3]), Err(vec![1, 3]));
    }

    #[test]
    fn controller_inventory_scans_quickly_only_until_candidates_are_open() {
        assert_eq!(
            controller_inventory_scan_interval(false, MAX_STABLE_CONTROLLER_SCAN_INTERVAL,),
            DISCOVERY_INTERVAL
        );
        assert_eq!(
            controller_inventory_scan_interval(true, MIN_STABLE_CONTROLLER_SCAN_INTERVAL,),
            MIN_STABLE_CONTROLLER_SCAN_INTERVAL
        );
        assert_eq!(
            next_stable_controller_scan_interval(MIN_STABLE_CONTROLLER_SCAN_INTERVAL),
            Duration::from_secs(4)
        );
        assert_eq!(
            next_stable_controller_scan_interval(Duration::from_secs(8)),
            MAX_STABLE_CONTROLLER_SCAN_INTERVAL
        );
        assert_eq!(
            next_stable_controller_scan_interval(MAX_STABLE_CONTROLLER_SCAN_INTERVAL),
            MAX_STABLE_CONTROLLER_SCAN_INTERVAL
        );
    }

    #[test]
    fn indexed_controller_discovery_caches_the_global_selection_between_scans() {
        let selected = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
            "USB",
        );
        let mut unrelated = selected.clone();
        unrelated.id = "unrelated".to_owned();
        unrelated.path = "unrelated".to_owned();
        unrelated.vendor_id = 0x1234;
        unrelated.product_id = 0x5678;
        let mut global = vec![unrelated; 44];
        global[43] = selected.clone();

        let mut discovery = IndexedControllerDiscoveryState::new();
        discovery.refresh(43, Ok(global.clone()));

        assert_eq!(discovery.info(), Some(&selected));
        assert_eq!(discovery.scan_error(), None);
        assert_eq!(
            discovery.stable_scan_interval,
            MIN_STABLE_CONTROLLER_SCAN_INTERVAL
        );
        assert!(!discovery.scan_due());
        assert!(
            discovery
                .next_scan
                .saturating_duration_since(Instant::now())
                > Duration::from_secs(1)
        );

        discovery.refresh(43, Ok(global));
        assert_eq!(discovery.stable_scan_interval, Duration::from_secs(4));
    }

    #[test]
    fn controller_discovery_loop_keeps_nonblocking_probes_at_two_hertz() {
        assert_eq!(
            controller_discovery_loop_delay(Duration::ZERO),
            DISCOVERY_INTERVAL
        );
        assert_eq!(
            controller_discovery_loop_delay(DISCOVERY_INTERVAL + Duration::from_millis(1)),
            Duration::ZERO
        );
        assert_eq!(
            controller_discovery_loop_delay(Duration::from_millis(100)),
            Duration::from_millis(400)
        );
    }

    #[test]
    fn idle_controller_discovery_reuses_sessions_across_scans_and_index_reordering() {
        let first = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
            "USB",
        );
        let second = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE + 1,
            "USB",
        );
        let mut discovery = ControllerDiscoveryState::new();
        let mut next_session = 0;
        let first_refresh = discovery.refresh(
            Ok(vec![(40, first.clone()), (41, second.clone())]),
            |_, _| {
                next_session += 1;
                Ok(next_session)
            },
        );
        assert_eq!(
            first_refresh,
            ControllerReconcileMetrics {
                opened: 2,
                ..ControllerReconcileMetrics::default()
            }
        );

        let second_refresh = discovery.refresh(
            Ok(vec![(12, second.clone()), (13, first.clone())]),
            |_, _| {
                next_session += 1;
                Ok(next_session)
            },
        );
        assert_eq!(
            second_refresh,
            ControllerReconcileMetrics {
                reused: 2,
                ..ControllerReconcileMetrics::default()
            }
        );
        assert_eq!(next_session, 2);
        assert_eq!(discovery.stable_scan_interval, Duration::from_secs(4));
        assert_eq!(discovery.candidate(0).enumeration_index, 12);
        assert_eq!(discovery.candidate(0).session, 2);
        assert_eq!(discovery.candidate(1).enumeration_index, 13);
        assert_eq!(discovery.candidate(1).session, 1);
    }

    #[test]
    fn controller_discovery_reconciles_arrival_removal_and_changed_paths() {
        let first = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
            "USB",
        );
        let second = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE + 1,
            "USB",
        );
        let bluetooth = controller_info(
            steam_controller_device::STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID,
            steam_controller_device::BLUETOOTH_CONTROLLER_INTERFACE,
            "Bluetooth",
        );
        let mut discovery = ControllerDiscoveryState::new();
        let mut next_session = 0;
        discovery.refresh(Ok(vec![(40, first), (41, second.clone())]), |_, _| {
            next_session += 1;
            Ok(next_session)
        });

        let mut moved_second = second;
        moved_second.id = "new-device-service-id".to_owned();
        moved_second.path = "new-device-service-id".to_owned();
        let refresh = discovery.refresh(Ok(vec![(8, moved_second), (58, bluetooth)]), |_, _| {
            next_session += 1;
            Ok(next_session)
        });
        assert_eq!(
            refresh,
            ControllerReconcileMetrics {
                opened: 1,
                reused: 1,
                removed: 1,
                ..ControllerReconcileMetrics::default()
            }
        );
        assert_eq!(next_session, 3);
        assert_eq!(discovery.candidate(0).session, 2);
        assert_eq!(discovery.candidate(1).session, 3);
    }

    #[test]
    fn ambiguity_indices_are_resolved_against_the_global_hid_list() {
        let puck = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
            "USB",
        );
        let bluetooth = controller_info(
            steam_controller_device::STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID,
            steam_controller_device::BLUETOOTH_CONTROLLER_INTERFACE,
            "Bluetooth",
        );
        let mut discovery = ControllerDiscoveryState::new();
        discovery.refresh(
            Ok(vec![(0, puck.clone()), (1, bluetooth.clone())]),
            |index, _| Ok(index),
        );

        let mut unrelated = puck.clone();
        unrelated.id = "unrelated".to_owned();
        unrelated.path = "unrelated".to_owned();
        unrelated.vendor_id = 0x1234;
        unrelated.product_id = 0x5678;
        let mut global = vec![unrelated.clone(); 7];
        global[3] = bluetooth;
        global[6] = puck;
        discovery.resolve_global_indices(&global).unwrap();

        assert_eq!(discovery.candidate(0).enumeration_index, 6);
        assert_eq!(discovery.candidate(1).enumeration_index, 3);
    }

    #[test]
    fn failed_global_index_resolution_does_not_partially_mutate_candidates() {
        let puck = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
            "USB",
        );
        let bluetooth = controller_info(
            steam_controller_device::STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID,
            steam_controller_device::BLUETOOTH_CONTROLLER_INTERFACE,
            "Bluetooth",
        );
        let mut discovery = ControllerDiscoveryState::new();
        discovery.refresh(Ok(vec![(0, puck.clone()), (1, bluetooth)]), |index, _| {
            Ok(index)
        });

        let mut unrelated = puck.clone();
        unrelated.id = "unrelated".to_owned();
        unrelated.path = "unrelated".to_owned();
        unrelated.vendor_id = 0x1234;
        unrelated.product_id = 0x5678;
        let mut incomplete_global = vec![unrelated; 7];
        incomplete_global[6] = puck;

        assert!(discovery
            .resolve_global_indices(&incomplete_global)
            .is_err());
        assert_eq!(discovery.candidate(0).enumeration_index, 0);
        assert_eq!(discovery.candidate(1).enumeration_index, 1);
    }

    #[test]
    fn discovery_probe_uses_nonblocking_reads_for_idle_candidates() {
        let timeouts = Arc::new(Mutex::new(Vec::new()));
        let mut discovery = ControllerDiscoveryState::new();
        let candidates = (0..4)
            .map(|offset| {
                (
                    40 + usize::try_from(offset).unwrap(),
                    controller_info(
                        steam_controller_device::PROTEUS_PRODUCT_ID,
                        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE + offset,
                        "USB",
                    ),
                )
            })
            .collect();
        discovery.refresh(Ok(candidates), |_, info| {
            if info.interface_number == steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE + 2 {
                Ok(FakeDiscoverySession::with_report(
                    controller_state_report(&info.id),
                    Arc::clone(&timeouts),
                ))
            } else {
                Ok(FakeDiscoverySession::idle(Arc::clone(&timeouts)))
            }
        });

        let probe = discovery.probe();
        assert_eq!(probe.active_indices, vec![2]);
        assert!(probe.failures.is_empty());
        assert_eq!(
            *timeouts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![Duration::ZERO; 4]
        );
    }

    #[test]
    fn discovery_probe_drains_a_bounded_prefix_to_find_fresh_state() {
        let timeouts = Arc::new(Mutex::new(Vec::new()));
        let puck = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
            "USB",
        );
        let connected = DeviceEvent::Connected(puck.clone());
        let state = DeviceEvent::Report(controller_state_report(&puck.id));
        let mut discovery = ControllerDiscoveryState::new();
        discovery.refresh(Ok(vec![(40, puck)]), |_, _| {
            Ok(FakeDiscoverySession::with_events(
                vec![Ok(Some(connected.clone())), Ok(Some(state.clone()))],
                Arc::clone(&timeouts),
            ))
        });

        let probe = discovery.probe();
        assert_eq!(probe.active_indices, vec![0]);
        assert_eq!(
            *timeouts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![Duration::ZERO; 2]
        );
    }

    #[test]
    fn discovery_probe_never_drains_more_than_the_fixed_limit() {
        let timeouts = Arc::new(Mutex::new(Vec::new()));
        let puck = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
            "USB",
        );
        let mut events = (0..MAX_DISCOVERY_REPORTS_PER_CANDIDATE + 3)
            .map(|_| Ok(Some(DeviceEvent::Connected(puck.clone()))))
            .collect::<Vec<_>>();
        let mut discovery = ControllerDiscoveryState::new();
        discovery.refresh(Ok(vec![(40, puck)]), |_, _| {
            Ok(FakeDiscoverySession::with_events(
                std::mem::take(&mut events),
                Arc::clone(&timeouts),
            ))
        });

        let probe = discovery.probe();
        assert!(probe.active_indices.is_empty());
        assert_eq!(
            timeouts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            MAX_DISCOVERY_REPORTS_PER_CANDIDATE
        );
    }

    #[test]
    fn discovery_probe_failures_use_identity_not_filtered_indices() {
        let timeouts = Arc::new(Mutex::new(Vec::new()));
        let puck = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
            "USB",
        );
        let mut discovery = ControllerDiscoveryState::new();
        discovery.refresh(Ok(vec![(2, puck)]), |_, _| {
            Ok(FakeDiscoverySession::with_error(
                "injected read failure",
                Arc::clone(&timeouts),
            ))
        });

        let probe = discovery.probe();
        assert_eq!(probe.failures.len(), 1);
        assert!(probe.failures[0].starts_with("Puck product"));
        assert!(probe.failures[0].contains("injected read failure"));
        assert!(!probe.failures[0].contains("index 2"));
    }

    #[test]
    fn ambiguity_descriptions_retain_global_indices_and_transports() {
        let puck = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
            "USB",
        );
        let bluetooth = controller_info(
            steam_controller_device::STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID,
            steam_controller_device::BLUETOOTH_CONTROLLER_INTERFACE,
            "Bluetooth",
        );
        let puck_description = controller_source_description(43, &puck);
        let bluetooth_description = controller_source_description(58, &bluetooth);
        assert!(puck_description.contains("index 43 Puck"));
        assert!(bluetooth_description.contains("index 58 Bluetooth"));
        assert!(bluetooth_description.contains("interface -1"));
    }

    #[test]
    fn only_complete_controller_states_mark_a_slot_active() {
        let report = |report_id, data: Vec<u8>| RawHidReport {
            timestamp: Duration::ZERO,
            report_id,
            data,
            source_device_id: "slot".to_owned(),
            transport: "USB".to_owned(),
            dropped_reports: 0,
        };
        let mut decoder = SteamControllerDecoder::new();
        let mut state = vec![0; steam_controller_protocol::INPUT_REPORT_SIZE];
        state[0] = INPUT_REPORT_ID;
        assert!(is_valid_controller_state(
            &mut decoder,
            &report(INPUT_REPORT_ID, state)
        ));

        let mut battery = vec![0; 15];
        battery[0] = steam_controller_protocol::BATTERY_REPORT_ID;
        assert!(!is_valid_controller_state(
            &mut decoder,
            &report(steam_controller_protocol::BATTERY_REPORT_ID, battery)
        ));
        assert!(!is_valid_controller_state(
            &mut decoder,
            &report(INPUT_REPORT_ID, vec![INPUT_REPORT_ID])
        ));
        assert!(!is_latest_state_report(
            steam_controller_protocol::BATTERY_REPORT_ID
        ));
    }

    #[test]
    fn remembered_xiao_serial_survives_a_changed_port_path() {
        let valid = vec![
            (serial_info("/dev/cu.usbmodem-new", "remembered"), ()),
            (serial_info("/dev/cu.usbmodem-other", "other"), ()),
        ];
        assert!(choose_xiao_index(&valid, None).is_err());
        assert_eq!(choose_xiao_index(&valid, Some("remembered")), Ok(0));
        assert_eq!(choose_xiao_index(&valid, Some("other")), Ok(1));
    }

    #[test]
    fn charge_states_follow_the_sdl_triton_values() {
        assert_eq!(
            ControllerChargeState::from_raw(1),
            ControllerChargeState::Discharging
        );
        assert_eq!(
            ControllerChargeState::from_raw(2),
            ControllerChargeState::Charging
        );
        assert_eq!(
            ControllerChargeState::from_raw(4),
            ControllerChargeState::Charged
        );
        assert_eq!(
            ControllerChargeState::from_raw(3),
            ControllerChargeState::Unknown(3)
        );
    }

    #[test]
    fn puck_dock_shutdown_is_exact_edge_triggered_and_one_shot() {
        let config = RuntimeConfig {
            puck_dock_action: PuckDockAction::PowerOff,
            ..RuntimeConfig::default()
        };
        let puck = controller_info(
            steam_controller_device::PROTEUS_PRODUCT_ID,
            steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
            "USB",
        );
        let bluetooth = controller_info(
            steam_controller_device::STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID,
            steam_controller_device::BLUETOOTH_CONTROLLER_INTERFACE,
            "Bluetooth",
        );
        let mut automatic = AutomaticShutdownRuntime::new(&config);
        automatic.source_selected(&puck, &config);
        assert!(!automatic.observe_charge_state(
            &puck,
            ControllerChargeState::Discharging,
            PuckDockAction::PowerOff
        ));
        assert!(automatic.observe_charge_state(
            &puck,
            ControllerChargeState::Charging,
            PuckDockAction::PowerOff
        ));
        automatic.succeeded(Instant::now(), ShutdownTrigger::PuckDock);
        assert!(!automatic.observe_charge_state(
            &puck,
            ControllerChargeState::Charged,
            PuckDockAction::PowerOff
        ));

        automatic.source_selected(&puck, &config);
        assert!(!automatic.observe_charge_state(
            &puck,
            ControllerChargeState::Charging,
            PuckDockAction::PowerOff
        ));
        automatic.observe_charge_state(
            &puck,
            ControllerChargeState::Discharging,
            PuckDockAction::PowerOff,
        );
        assert!(automatic.observe_charge_state(
            &puck,
            ControllerChargeState::Charging,
            PuckDockAction::PowerOff
        ));
        assert!(!automatic.observe_charge_state(
            &bluetooth,
            ControllerChargeState::Charging,
            PuckDockAction::PowerOff
        ));
        assert!(!automatic.observe_charge_state(
            &puck,
            ControllerChargeState::Unknown(3),
            PuckDockAction::PowerOff
        ));
    }

    #[test]
    fn power_off_burst_is_scheduled_and_one_success_is_sufficient() {
        let (ack, _receiver) = mpsc::channel();
        let writer =
            FakePowerOffWriter::new([Err("first".to_owned()), Ok(()), Err("third".to_owned())]);
        let mut sequence = PowerOffSequence::new(ack, Duration::ZERO);
        assert_eq!(sequence.service(Duration::ZERO, &writer), None);
        assert_eq!(sequence.service(Duration::from_millis(9), &writer), None);
        assert_eq!(writer.writes.load(Ordering::Relaxed), 1);
        assert_eq!(sequence.service(Duration::from_millis(10), &writer), None);
        assert_eq!(
            sequence.service(Duration::from_millis(20), &writer),
            Some(Ok(()))
        );
        assert_eq!(writer.writes.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn all_failed_power_off_writes_report_the_last_error() {
        let (ack, _receiver) = mpsc::channel();
        let writer = FakePowerOffWriter::new([
            Err("one".to_owned()),
            Err("two".to_owned()),
            Err("three".to_owned()),
        ]);
        let mut sequence = PowerOffSequence::new(ack, Duration::ZERO);
        assert_eq!(sequence.service(Duration::ZERO, &writer), None);
        assert_eq!(sequence.service(Duration::from_millis(10), &writer), None);
        assert_eq!(
            sequence.service(Duration::from_millis(20), &writer),
            Some(Err("three".to_owned()))
        );
    }

    #[test]
    fn start_stop_and_shutdown_are_idempotent_while_waiting() {
        let handle = BridgeRuntime::spawn(RuntimeConfig {
            controller: ControllerSelection::Index(usize::MAX),
            output: OutputSelection::Mock,
            ..RuntimeConfig::default()
        });
        handle
            .set_idle_shutdown_timeout(Some(Duration::from_mins(5)))
            .unwrap();
        handle
            .set_idle_shutdown_timeout(Some(Duration::from_mins(5)))
            .unwrap();
        handle
            .set_puck_dock_action(PuckDockAction::PowerOff)
            .unwrap();
        let status = handle.status();
        assert_eq!(
            status.automatic_shutdown.configured_timeout,
            Some(Duration::from_mins(5))
        );
        assert_eq!(
            status.automatic_shutdown.puck_dock_action,
            PuckDockAction::PowerOff
        );
        handle.stop().unwrap();
        handle.stop().unwrap();
        assert_eq!(handle.status().state, RuntimeState::Stopped);
        handle.start().unwrap();
        handle.start().unwrap();
        handle.suspend_for_sleep().unwrap();
        let suspended = handle.status();
        assert_eq!(suspended.state, RuntimeState::Stopped);
        assert_eq!(suspended.detail, "Suspended for system sleep");
        handle.request_resume_from_wake().unwrap();
        // Stopping during the wake-settle window must cancel the pending
        // automatic restart just as an explicit user stop would.
        handle.stop().unwrap();
        handle.shutdown().unwrap();
    }
}
