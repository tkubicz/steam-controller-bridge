use super::{
    automatic_shutdown_phase, available_serial_devices, bounded_error, choose_unique_active,
    controller_inventory_scan_interval, controller_open_error, controller_open_error_message,
    desktop_transition_mask, io, json, masked_serial, mpsc, next_stable_controller_scan_interval,
    output_reopens_with_backoff, picker_status, same_controller_collection, thread,
    validate_idle_shutdown_timeout, Arc, AtomicBool, AtomicU64, AutomaticShutdownPhase,
    AutomaticShutdownRuntime, BridgeEngine, BridgeStatus, CommandAck, ConnectionState,
    ControllerChargeState, ControllerCooldown, ControllerDiscoveryState, ControllerEnumerator,
    ControllerOpenFailure, ControllerSelection, ControllerSourceStatus, ControllerStatus,
    ControllerTransport, DecodedReport, DesktopBindingsWorker, DesktopInputSnapshot, DeviceError,
    DeviceEvent, DumpOutput, Duration, File, FileOutput, FirmwareInstallReceipt, GamepadOutput,
    HapticsState, HapticsStatus, HidDeviceInfo, HidSession, IdleActivityTracker, Instant,
    JoinHandle, LizardMode, LizardModeHeartbeat, LizardStatus, MockOutput, Mutex, Ordering,
    OutputCapabilities, OutputError, OutputFeedback, OutputRetryState, OutputSelection,
    OutputStatus, PadFeedbackRequest, PadHapticGain, PadHapticSide, PickerConfig, PickerEvent,
    PickerEventSink, PickerInput, PickerRuntime, ProcessOutcome, RawHidReport, Receiver,
    RecordingError, RecordingEvent, RecordingWriter, RuntimeCommand, RuntimeConfig, RuntimeState,
    SerialDeviceInfo, SerialOutput, SerialSelection, ShutdownTrigger, StableTransitionRun,
    SteamButton, SupervisorIterationTimer, SyncSender, TrySendError, VecDeque, ACTIVE_SLOT_TIMEOUT,
    BATTERY_STATUS_FRESHNESS, COMMAND_TIMEOUT, DISCOVERY_INTERVAL, EXTENDED_INPUT_REPORT_ID,
    EXTENDED_INPUT_REPORT_SIZE, INPUT_MAILBOX_CAPACITY, INPUT_REPORT_ID, INPUT_REPORT_SIZE,
    KIND_DEVICE_CONNECTED, KIND_DEVICE_DISCONNECTED, MIN_STABLE_CONTROLLER_SCAN_INTERVAL,
    PAD_FEEDBACK_RETRY_INTERVAL, POWER_OFF_BURST_INTERVAL, POWER_OFF_BURST_WRITES,
    POWER_OFF_COOLDOWN, RUMBLE_LEASE_TIMEOUT, RUMBLE_REFRESH_INTERVAL, RUMBLE_RETRY_INTERVAL,
    RUNTIME_POLL_INTERVAL, STATUS_INTERVAL, WAKE_SETTLE_DELAY,
};
use virtual_gamepad::VirtualGamepad;

mod active_session;
mod commands;
mod controller;
mod discovery;

#[allow(
    clippy::wildcard_imports,
    reason = "the supervisor owns the controller lifecycle as one safety boundary"
)]
#[cfg(not(test))]
use controller::*;
#[cfg(test)]
pub(crate) use controller::*;

#[derive(Debug, Default)]
struct SuspensionState {
    system_sleep: bool,
    update: bool,
}

impl SuspensionState {
    fn active(&self) -> bool {
        self.system_sleep || self.update
    }

    fn detail(&self) -> Option<&'static str> {
        if self.system_sleep {
            Some("Suspended for system sleep")
        } else if self.update {
            Some("Suspended for application update")
        } else {
            None
        }
    }
}

pub(crate) struct Supervisor {
    config: RuntimeConfig,
    status: Arc<Mutex<BridgeStatus>>,
    desktop_bindings: DesktopBindingsWorker,
    commands: Receiver<RuntimeCommand>,
    /// A safety prerequisite that failed before the supervisor started. When
    /// present, hardware stays closed and Start requests are rejected.
    startup_blocker: Option<String>,
    desired_running: bool,
    /// Independent owners that require every hardware handle to stay closed.
    /// User start/stop intent remains in `desired_running` while either owner
    /// is active.
    suspension: SuspensionState,
    /// Hardware discovery holds off until this instant after a system wake.
    wake_settle: Option<Instant>,
    shutdown_requested: bool,
    pending_stop_acks: Vec<CommandAck>,
    pending_shutdown_acks: Vec<CommandAck>,
    pending_output_change: Option<(OutputSelection, CommandAck)>,
    output_retry: OutputRetryState,
    virtual_helper_restarts: u64,
    preferred_output_serial: Option<String>,
    controller_enumerator: Option<ControllerEnumerator>,
    controller_discovery: ControllerDiscoveryState<HidSession>,
    indexed_controller_discovery: IndexedControllerDiscoveryState,
    automatic_shutdown: AutomaticShutdownRuntime,
    controller_cooldown: Option<ControllerCooldown>,
    picker_events: PickerEventSink,
}

impl Supervisor {
    pub(crate) fn new(
        config: RuntimeConfig,
        status: Arc<Mutex<BridgeStatus>>,
        commands: Receiver<RuntimeCommand>,
        picker_events: PickerEventSink,
        startup_blocker: Option<String>,
    ) -> Self {
        let automatic_shutdown = AutomaticShutdownRuntime::new(&config);
        let desired_running =
            startup_blocker.is_none() || !OutputCapabilities::for_selection(&config.output).live;
        let desktop_bindings =
            DesktopBindingsWorker::spawn(config.binding_profile.clone(), Arc::clone(&status));
        Self {
            picker_events,
            config,
            status,
            desktop_bindings,
            commands,
            desired_running,
            startup_blocker,
            suspension: SuspensionState::default(),
            wake_settle: None,
            shutdown_requested: false,
            pending_stop_acks: Vec::new(),
            pending_shutdown_acks: Vec::new(),
            pending_output_change: None,
            output_retry: OutputRetryState::new(Instant::now()),
            virtual_helper_restarts: 0,
            preferred_output_serial: None,
            controller_enumerator: None,
            controller_discovery: ControllerDiscoveryState::new(),
            indexed_controller_discovery: IndexedControllerDiscoveryState::new(),
            automatic_shutdown,
            controller_cooldown: None,
        }
    }

    #[allow(clippy::too_many_lines)] // The supervisor keeps endpoint ownership transitions linear.
    pub(crate) fn run(&mut self) {
        let mut retained_output: Option<OutputSession> = None;
        if OutputCapabilities::for_selection(&self.config.output).live {
            if let Some(error) = self.startup_blocker.clone() {
                self.transition(
                    RuntimeState::Error,
                    "Hardware safety monitor unavailable",
                    Some(&error),
                );
            }
        }
        loop {
            self.service_idle_commands();
            if let Some((selection, acknowledgement)) = self.pending_output_change.take() {
                let cleanup = retained_output.as_mut().map_or(Ok(()), |output| {
                    output.output.send_neutral().map_err(|error| {
                        format!("cannot neutralize old output before backend switch: {error}")
                    })
                });
                drop(retained_output.take());
                self.clear_controller_discovery();
                if cleanup.is_ok() {
                    self.apply_output_selection(selection);
                }
                let _ = acknowledgement.send(cleanup);
                continue;
            }
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
            if !self.desired_running || self.suspension.active() {
                drop(retained_output.take());
                self.clear_controller_discovery();
                if self.current_state() != RuntimeState::Error {
                    let detail = self.suspension.detail().unwrap_or("Bridge stopped");
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
                    "Looking for Steam Controller 2 and a bridge device",
                    None,
                );
            }
            if retained_output.is_none() {
                let now = Instant::now();
                if now < self.output_retry.next_attempt {
                    self.wait_or_command(
                        self.output_retry
                            .next_attempt
                            .saturating_duration_since(now),
                    );
                    continue;
                }
                match self.discover_output() {
                    OutputDiscovery::Ready(output) => {
                        self.output_retry.mark_ready(Instant::now());
                        retained_output = Some(output);
                    }
                    OutputDiscovery::Wait { detail, error } => {
                        self.clear_hardware_status();
                        self.transition(RuntimeState::Waiting, &detail, error.as_deref());
                        self.wait_or_command(DISCOVERY_INTERVAL);
                        continue;
                    }
                    OutputDiscovery::Retry { detail, error } => {
                        self.schedule_output_retry();
                        self.clear_hardware_status();
                        self.transition(RuntimeState::Waiting, &detail, Some(&error));
                        continue;
                    }
                    OutputDiscovery::Error(message) => {
                        self.clear_hardware_status();
                        self.transition(RuntimeState::Error, &message, Some(&message));
                        self.wait_or_command(DISCOVERY_INTERVAL);
                        continue;
                    }
                    OutputDiscovery::Blocked(message) => {
                        self.clear_hardware_status();
                        self.desired_running = false;
                        self.transition(RuntimeState::Error, &message, Some(&message));
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
                    match service_waiting_output(retained_output.as_mut()) {
                        Ok(()) => {
                            if let Some(session) = retained_output
                                .as_mut()
                                .filter(|session| session.capabilities.firmware)
                            {
                                // The firmware's DeviceInfo usually lands after
                                // discovery published OutputStatus; keep it current
                                // while no controller is present.
                                self.refresh_output_firmware(session);
                            }
                        }
                        Err(error) => {
                            let permanent = matches!(&error, OutputError::Configuration(_));
                            let message = format!("gamepad output failed while waiting: {error}");
                            retained_output = None;
                            self.update_status(|status| {
                                status.output = OutputStatus::configured(&self.config.output);
                            });
                            if permanent {
                                self.desired_running = false;
                                self.transition(RuntimeState::Error, &message, Some(&message));
                            } else {
                                if output_reopens_with_backoff(&self.config.output) {
                                    self.schedule_output_retry();
                                }
                                self.transition(RuntimeState::Waiting, &message, Some(&message));
                            }
                        }
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
                    if output_reopens_with_backoff(&self.config.output) {
                        self.schedule_output_retry();
                    }
                    self.update_status(|status| {
                        status.output = OutputStatus::configured(&self.config.output);
                    });
                    self.transition(RuntimeState::Waiting, &message, Some(&message));
                }
                Ok((ActiveExit::OutputBlocked(message), _, _)) => {
                    retained_output = None;
                    self.desired_running = false;
                    self.update_status(|status| {
                        status.output = OutputStatus::configured(&self.config.output);
                    });
                    self.transition(RuntimeState::Error, &message, Some(&message));
                }
                Ok((ActiveExit::OutputChange(selection, ack), output, cleanup)) => {
                    drop(output);
                    self.clear_controller_discovery();
                    if cleanup.is_ok() {
                        self.apply_output_selection(selection);
                    }
                    let _ = ack.send(cleanup);
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
                    self.transition(
                        RuntimeState::Stopped,
                        self.suspension
                            .detail()
                            .expect("a suspended exit has an owner"),
                        None,
                    );
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
}
