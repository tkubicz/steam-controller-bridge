//! Reusable live Steam Controller 2 bridge orchestration.
//!
//! The runtime deliberately keeps discovery separate from ownership: candidate
//! Puck and Bluetooth collections are only read during discovery, and the
//! lizard-mode feature command is sent only after exactly one active source has
//! been identified.

use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bridge_core::{BridgeConfig, BridgeEngine, BridgeMetrics, ProcessOutcome};
use bridge_output::{
    available_serial_devices, DumpFormat, DumpOutput, FileOutput, GamepadOutput, MockOutput,
    OutputDiagnostics, OutputFeedback, SerialConfig, SerialDeviceInfo, SerialOutput,
};
use controller_mapper::MapperConfig;
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
    ConnectionState, DecodedReport, SteamControllerDecoder, EXTENDED_INPUT_REPORT_ID,
    INPUT_REPORT_ID,
};

const DISCOVERY_INTERVAL: Duration = Duration::from_millis(500);
const MIN_STABLE_CONTROLLER_SCAN_INTERVAL: Duration = Duration::from_secs(2);
const MAX_STABLE_CONTROLLER_SCAN_INTERVAL: Duration = Duration::from_secs(10);
const MAX_DISCOVERY_REPORTS_PER_CANDIDATE: usize = 4;
const ACTIVE_SLOT_TIMEOUT: Duration = Duration::from_secs(1);
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
const RUNTIME_POLL_INTERVAL: Duration = Duration::from_millis(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const RUMBLE_REFRESH_INTERVAL: Duration = Duration::from_millis(40);
const RUMBLE_LEASE_TIMEOUT: Duration = Duration::from_millis(100);
const RUMBLE_RETRY_INTERVAL: Duration = Duration::from_millis(500);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HapticsStatus {
    pub state: HapticsState,
    pub commands_received: u64,
    pub writes: u64,
    pub refreshes: u64,
    pub coalesced_commands: u64,
    pub failures: u64,
    pub last_command_age: Option<Duration>,
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
    pub lizard: LizardStatus,
    pub haptics: HapticsStatus,
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
            lizard: LizardStatus::default(),
            haptics: HapticsStatus::default(),
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
}

pub struct BridgeRuntime;

impl BridgeRuntime {
    #[must_use]
    pub fn spawn(config: RuntimeConfig) -> BridgeHandle {
        let status = Arc::new(Mutex::new(BridgeStatus {
            state: RuntimeState::Discovering,
            detail: "Starting bridge runtime".to_owned(),
            ..BridgeStatus::default()
        }));
        let worker_status = Arc::clone(&status);
        let (command_sender, command_receiver) = mpsc::channel();
        let join = thread::spawn(move || {
            let mut supervisor = Supervisor::new(config, worker_status, command_receiver);
            supervisor.run();
        });
        BridgeHandle {
            command_sender,
            status,
            join: Mutex::new(Some(join)),
        }
    }
}

pub struct BridgeHandle {
    command_sender: mpsc::Sender<RuntimeCommand>,
    status: Arc<Mutex<BridgeStatus>>,
    join: Mutex<Option<JoinHandle<()>>>,
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

    /// Stops safely and terminates the runtime thread.
    ///
    /// # Errors
    /// Returns an error if cleanup or joining fails.
    pub fn shutdown(&self) -> Result<(), RuntimeError> {
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
}

impl Drop for BridgeHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct Supervisor {
    config: RuntimeConfig,
    status: Arc<Mutex<BridgeStatus>>,
    commands: Receiver<RuntimeCommand>,
    desired_running: bool,
    shutdown_requested: bool,
    pending_stop_acks: Vec<CommandAck>,
    pending_shutdown_acks: Vec<CommandAck>,
    preferred_xiao_serial: Option<String>,
    controller_enumerator: Option<ControllerEnumerator>,
    controller_discovery: ControllerDiscoveryState<HidSession>,
    indexed_controller_discovery: IndexedControllerDiscoveryState,
}

impl Supervisor {
    fn new(
        config: RuntimeConfig,
        status: Arc<Mutex<BridgeStatus>>,
        commands: Receiver<RuntimeCommand>,
    ) -> Self {
        Self {
            config,
            status,
            commands,
            desired_running: true,
            shutdown_requested: false,
            pending_stop_acks: Vec::new(),
            pending_shutdown_acks: Vec::new(),
            preferred_xiao_serial: None,
            controller_enumerator: None,
            controller_discovery: ControllerDiscoveryState::new(),
            indexed_controller_discovery: IndexedControllerDiscoveryState::new(),
        }
    }

    #[allow(clippy::too_many_lines)] // The supervisor keeps endpoint ownership transitions linear.
    fn run(&mut self) {
        let mut retained_output = None;
        loop {
            self.service_idle_commands();
            if self.shutdown_requested {
                drop(retained_output.take());
                self.clear_controller_discovery();
                self.clear_hardware_status();
                self.transition(RuntimeState::Stopped, "Bridge stopped", None);
                acknowledge_all(&mut self.pending_shutdown_acks);
                acknowledge_all(&mut self.pending_stop_acks);
                break;
            }
            if !self.desired_running {
                drop(retained_output.take());
                self.clear_controller_discovery();
                if self.current_state() != RuntimeState::Error {
                    self.transition(RuntimeState::Stopped, "Bridge stopped", None);
                }
                acknowledge_all(&mut self.pending_stop_acks);
                self.wait_for_command();
                continue;
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
                Ok((ActiveExit::SourceLost, output)) => {
                    retained_output = Some(output);
                    self.clear_controller_discovery();
                    self.transition(
                        RuntimeState::Discovering,
                        "Controller stopped reporting; rediscovering active input source",
                        None,
                    );
                }
                Ok((ActiveExit::OutputLost(message), _)) => {
                    retained_output = None;
                    self.update_status(|status| {
                        status.xiao = XiaoStatus::default();
                    });
                    self.transition(RuntimeState::Waiting, &message, Some(&message));
                }
                Ok((ActiveExit::Stopped, _)) => {
                    retained_output = None;
                    self.clear_hardware_status();
                    self.transition(RuntimeState::Stopped, "Bridge stopped", None);
                }
                Ok((ActiveExit::Shutdown, _)) => {
                    retained_output = None;
                    self.clear_hardware_status();
                    self.shutdown_requested = true;
                }
                Ok((ActiveExit::StoppedWithAck(ack), _)) => {
                    retained_output = None;
                    self.clear_hardware_status();
                    let _ = ack.send(Ok(()));
                    self.desired_running = false;
                    self.transition(RuntimeState::Stopped, "Bridge stopped", None);
                }
                Ok((ActiveExit::ShutdownWithAck(ack), _)) => {
                    retained_output = None;
                    self.clear_hardware_status();
                    let _ = ack.send(Ok(()));
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
        self.transition(RuntimeState::Stopped, "Bridge stopped", None);
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

    #[allow(clippy::too_many_lines)] // Safety ordering is clearest in one linear ownership loop.
    fn run_active(
        &mut self,
        active: ActiveControllerSource,
        mut output: OutputSession,
    ) -> Result<(ActiveExit, OutputSession), String> {
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
        engine.connected();
        self.transition(RuntimeState::Running, "Bridge running", None);
        self.update_status(|status| {
            status.source.connected = true;
            status.source.active = initial_controller_seen;
            status.controller.connected = initial_controller_seen;
            status.controller.last_state_age = initial_controller_seen.then_some(Duration::ZERO);
            status.lizard = worker.lizard_diagnostics();
            status.haptics = worker.haptics_diagnostics();
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

        let exit = loop {
            if let Some(command_exit) = self.service_active_commands() {
                break command_exit;
            }
            if let Some(error) = worker.take_failure() {
                let _ = engine.shutdown(&mut *output.output);
                worker.shutdown()?;
                self.clear_controller_status();
                return Err(error);
            }
            let mut direct_report = None;
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
            if let Some(report) = direct_report.or_else(|| worker.take_latest_report()) {
                match process_report(
                    &report,
                    &mut engine,
                    &mut *output.output,
                    &mut recording,
                    started,
                ) {
                    Ok(ReportEffect::ControllerState) => {
                        last_controller_state = Instant::now();
                        controller_connected = true;
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
                    Ok(ReportEffect::Battery(percent)) => {
                        self.update_status(|status| {
                            status.battery_percent = percent;
                        });
                    }
                    Ok(ReportEffect::Disconnected) => {
                        let _ = engine.disconnected(&mut *output.output);
                        break ActiveExit::SourceLost;
                    }
                    Ok(ReportEffect::None) => {}
                    Err(error) if is_output_error(&error) => {
                        break ActiveExit::OutputLost(format!(
                            "XIAO output failed; waiting for reconnect: {error}"
                        ));
                    }
                    Err(error) => {
                        eprintln!("level=warn event=report_processing_failed error={error:?}");
                    }
                }
            }
            let lost = dropped.swap(0, Ordering::AcqRel);
            if lost > 0 {
                engine.note_dropped_reports(lost);
            }
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
            while let Some(feedback) = output.output.take_feedback() {
                match feedback {
                    OutputFeedback::Rumble {
                        low_frequency,
                        high_frequency,
                    } => worker.set_rumble(low_frequency, high_frequency),
                }
            }
            if last_controller_state.elapsed() >= ACTIVE_SLOT_TIMEOUT {
                let _ = engine.disconnected(&mut *output.output);
                break ActiveExit::SourceLost;
            }
            if last_status.elapsed() >= STATUS_INTERVAL {
                let controller_age = last_controller_state.elapsed();
                self.update_status(|status| {
                    status.bridge_metrics = engine.metrics();
                    status.output_diagnostics = output.output.diagnostics();
                    status.lizard = worker.lizard_diagnostics();
                    status.haptics = worker.haptics_diagnostics();
                    status.source.active =
                        controller_state_seen && controller_age < ACTIVE_SLOT_TIMEOUT;
                    status.controller.connected =
                        controller_connected && controller_age < ACTIVE_SLOT_TIMEOUT;
                    status.controller.last_state_age =
                        controller_connected.then_some(controller_age);
                });
                last_status = Instant::now();
            }
        };

        self.transition(RuntimeState::Stopping, "Neutralizing output", None);
        let neutral_result = engine.shutdown(&mut *output.output);
        let worker_result = worker.shutdown();
        self.update_status(|status| {
            status.bridge_metrics = engine.metrics();
            status.output_diagnostics = output.output.diagnostics();
            status.lizard = worker.lizard_diagnostics();
            status.haptics = worker.haptics_diagnostics();
        });
        self.clear_controller_status();
        record_device_event(&mut recording, started, KIND_DEVICE_DISCONNECTED, None)?;
        worker_result?;
        if let Err(error) = neutral_result {
            if !matches!(exit, ActiveExit::OutputLost(_)) {
                return Err(format!(
                    "cannot neutralize XIAO before HID release: {error}"
                ));
            }
        }
        Ok((exit.acknowledge(), output))
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
                self.desired_running = true;
                let _ = ack.send(Ok(()));
            }
            RuntimeCommand::Stop(ack) => {
                self.desired_running = false;
                self.transition(RuntimeState::Stopping, "Stopping bridge", None);
                self.clear_hardware_status();
                self.pending_stop_acks.push(ack);
            }
            RuntimeCommand::Shutdown(ack) => {
                self.desired_running = false;
                self.shutdown_requested = true;
                self.transition(RuntimeState::Stopping, "Stopping bridge", None);
                self.clear_hardware_status();
                self.pending_shutdown_acks.push(ack);
            }
        }
    }

    fn service_active_commands(&mut self) -> Option<ActiveExit> {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                RuntimeCommand::Start(ack) => {
                    let _ = ack.send(Ok(()));
                }
                RuntimeCommand::Stop(ack) => {
                    self.desired_running = false;
                    // The active loop acknowledges after its neutral-before-release cleanup.
                    return Some(ActiveExit::StoppedWithAck(ack));
                }
                RuntimeCommand::Shutdown(ack) => {
                    self.desired_running = false;
                    self.shutdown_requested = true;
                    return Some(ActiveExit::ShutdownWithAck(ack));
                }
            }
        }
        None
    }

    fn transition(&self, state: RuntimeState, detail: &str, error: Option<&str>) {
        let changed = self.update_status(|status| {
            status.state = state;
            detail.clone_into(&mut status.detail);
            if let Some(error) = error {
                status.last_error = Some(error.to_owned());
            } else if matches!(state, RuntimeState::Running | RuntimeState::Stopped) {
                status.last_error = None;
            }
        });
        if changed {
            let level = if state == RuntimeState::Error {
                "error"
            } else {
                "info"
            };
            eprintln!(
                "level={level} event=runtime_state state={state:?} detail={detail:?} error={error:?}"
            );
        }
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
            status.lizard = LizardStatus::default();
        });
    }

    fn clear_hardware_status(&self) {
        self.update_status(|status| {
            status.source = ControllerSourceStatus::default();
            status.controller = ControllerStatus::default();
            status.xiao = XiaoStatus::default();
            status.battery_percent = None;
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
            || status.lizard != previous.lizard
            || status.haptics != previous.haptics
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
    Stopped,
    Shutdown,
    StoppedWithAck(CommandAck),
    ShutdownWithAck(CommandAck),
}

impl ActiveExit {
    fn acknowledge(self) -> Self {
        match self {
            Self::StoppedWithAck(ack) => {
                let _ = ack.send(Ok(()));
                Self::Stopped
            }
            Self::ShutdownWithAck(ack) => {
                let _ = ack.send(Ok(()));
                Self::Shutdown
            }
            other => other,
        }
    }
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
    ControllerState,
    Connected,
    Battery(Option<u8>),
    Disconnected,
    None,
}

fn process_report(
    report: &RawHidReport,
    engine: &mut BridgeEngine,
    output: &mut dyn GamepadOutput,
    recording: &mut Option<RecordingWriter<File>>,
    started: Instant,
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
        Ok(ProcessOutcome::State { source, mapped, .. }) => {
            record_lazy(recording, || {
                RecordingEvent::decoded_steam_state(timestamp, &source)
            })?;
            record_lazy(recording, || {
                RecordingEvent::mapped_gamepad_state(timestamp, &mapped)
            })?;
            Ok(ReportEffect::ControllerState)
        }
        Ok(ProcessOutcome::Status(DecodedReport::Battery { status, .. })) => {
            Ok(ReportEffect::Battery(valid_battery_percent(status.percent)))
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
    commands_received: AtomicU64,
    writes: AtomicU64,
    refreshes: AtomicU64,
    coalesced_commands: AtomicU64,
    failures: AtomicU64,
    last_command_millis: AtomicU64,
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
        self.last_command_millis.store(0, Ordering::Release);
    }

    fn snapshot(&self, now: Duration) -> HapticsStatus {
        let last_command_millis = self.last_command_millis.load(Ordering::Acquire);
        let state = if self.degraded.load(Ordering::Acquire) {
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

trait RumbleWriter {
    fn write_rumble(&self, low_frequency: u16, high_frequency: u16) -> Result<(), String>;
}

impl RumbleWriter for HidSession {
    fn write_rumble(&self, low_frequency: u16, high_frequency: u16) -> Result<(), String> {
        self.set_rumble(low_frequency, high_frequency)
            .map_err(|error| error.to_string())
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

#[derive(Debug, Default)]
struct LatestReportState {
    report: Option<RawHidReport>,
    notification_pending: bool,
}

#[derive(Debug, Default)]
struct LatestReportSlot {
    state: Mutex<LatestReportState>,
}

impl LatestReportSlot {
    fn publish(&self, report: RawHidReport, dropped: &AtomicU64) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.report.replace(report).is_some() {
            dropped.fetch_add(1, Ordering::Relaxed);
        }
        let needs_notification = !state.notification_pending;
        state.notification_pending = true;
        needs_notification
    }

    fn take(&self) -> Option<RawHidReport> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.notification_pending = false;
        state.report.take()
    }

    fn has_pending(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .report
            .is_some()
    }

    fn clear(&self, dropped: &AtomicU64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.report.take().is_some() {
            dropped.fetch_add(1, Ordering::Relaxed);
        }
        state.notification_pending = false;
    }
}

struct HidWorker {
    receiver: Receiver<HidWorkerEvent>,
    failure_receiver: Receiver<String>,
    latest_report: Arc<LatestReportSlot>,
    latest_rumble: Arc<LatestRumbleSlot>,
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
        let latest_report = Arc::new(LatestReportSlot::default());
        let worker_latest_report = Arc::clone(&latest_report);
        let latest_rumble = Arc::new(LatestRumbleSlot::default());
        let worker_latest_rumble = Arc::clone(&latest_rumble);
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
            while !worker_stop.load(Ordering::Acquire) {
                if let Err(error) = lizard.service(worker_started.elapsed(), &session) {
                    worker_latest_report.clear(&dropped);
                    let _ = failure_sender.send(format!(
                        "lizard-mode refresh failed; XIAO was neutralized and input stopped: {error}"
                    ));
                    break;
                }
                if let Some(command) = worker_latest_rumble.take() {
                    haptics.command(worker_started.elapsed(), &session, command);
                }
                haptics.service(worker_started.elapsed(), &session);
                match session.poll(RUNTIME_POLL_INTERVAL) {
                    Ok(Some(DeviceEvent::Connected(info))) => {
                        if let Err(error) = lizard.connected(worker_started.elapsed(), &session) {
                            worker_latest_report.clear(&dropped);
                            let _ = failure_sender.send(format!(
                                "lizard-mode suppression failed after reconnect: {error}"
                            ));
                            break;
                        }
                        haptics.connected(worker_started.elapsed(), &session);
                        if !send_worker_event(
                            &sender,
                            HidWorkerEvent::Connected(info),
                            &worker_stop,
                        ) {
                            break;
                        }
                    }
                    Ok(Some(DeviceEvent::Disconnected)) => {
                        haptics.disconnected();
                        worker_latest_rumble.clear();
                        lizard.disconnected();
                        worker_latest_report.clear(&dropped);
                        if !send_worker_event(&sender, HidWorkerEvent::Disconnected, &worker_stop) {
                            break;
                        }
                    }
                    Ok(Some(DeviceEvent::Report(report))) => {
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
                        worker_latest_rumble.clear();
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
            haptics.shutdown(worker_started.elapsed(), &session);
            lizard.disconnected();
        });
        Ok(Self {
            receiver,
            failure_receiver,
            latest_report,
            latest_rumble,
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

    fn take_latest_report(&self) -> Option<RawHidReport> {
        self.latest_report.take()
    }

    fn has_pending_report(&self) -> bool {
        self.latest_report.has_pending()
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
    latest_report: &LatestReportSlot,
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
    fn latest_report_slot_replaces_stale_input() {
        let slot = LatestReportSlot::default();
        let dropped = AtomicU64::new(0);
        let report = |id| RawHidReport {
            timestamp: Duration::ZERO,
            report_id: id,
            data: vec![id],
            source_device_id: "slot".to_owned(),
            transport: "USB".to_owned(),
            dropped_reports: 0,
        };
        assert!(slot.publish(report(1), &dropped));
        assert!(!slot.publish(report(2), &dropped));
        assert_eq!(slot.take().map(|value| value.report_id), Some(2));
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
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
    fn start_stop_and_shutdown_are_idempotent_while_waiting() {
        let handle = BridgeRuntime::spawn(RuntimeConfig {
            controller: ControllerSelection::Index(usize::MAX),
            output: OutputSelection::Mock,
            ..RuntimeConfig::default()
        });
        handle.stop().unwrap();
        handle.stop().unwrap();
        assert_eq!(handle.status().state, RuntimeState::Stopped);
        handle.start().unwrap();
        handle.start().unwrap();
        handle.shutdown().unwrap();
    }
}
