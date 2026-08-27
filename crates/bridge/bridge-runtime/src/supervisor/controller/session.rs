#[allow(
    clippy::wildcard_imports,
    reason = "session helpers share the controller supervisor's private dependencies"
)]
use super::*;

pub(crate) struct IndexedControllerDiscoveryState {
    pub(crate) info: Option<HidDeviceInfo>,
    pub(crate) next_scan: Instant,
    pub(crate) stable_scan_interval: Duration,
    pub(crate) scan_error: Option<String>,
}

impl IndexedControllerDiscoveryState {
    pub(crate) fn new() -> Self {
        Self {
            info: None,
            next_scan: Instant::now(),
            stable_scan_interval: MIN_STABLE_CONTROLLER_SCAN_INTERVAL,
            scan_error: None,
        }
    }

    pub(crate) fn scan_due(&self) -> bool {
        Instant::now() >= self.next_scan
    }

    pub(crate) fn refresh(&mut self, index: usize, discovered: Result<Vec<HidDeviceInfo>, String>) {
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

    pub(crate) fn clear(&mut self) {
        self.info = None;
        self.next_scan = Instant::now();
        self.stable_scan_interval = MIN_STABLE_CONTROLLER_SCAN_INTERVAL;
        self.scan_error = None;
    }

    pub(crate) fn info(&self) -> Option<&HidDeviceInfo> {
        self.info.as_ref()
    }

    pub(crate) fn scan_error(&self) -> Option<&str> {
        self.scan_error.as_deref()
    }
}

pub(crate) fn controller_discovery_loop_delay(elapsed: Duration) -> Duration {
    DISCOVERY_INTERVAL.saturating_sub(elapsed)
}

pub(crate) enum Discovery<T> {
    Ready(T),
    Wait {
        detail: String,
        error: Option<String>,
    },
    Error(String),
}

/// Output discovery can also ask to be retried with backoff, or report that
/// this backend will never open. Controller discovery has no equivalent, so
/// the two no longer share an outcome type and neither carries arms its
/// producer cannot emit.
pub(crate) enum OutputDiscovery {
    Ready(OutputSession),
    Wait {
        detail: String,
        error: Option<String>,
    },
    Retry {
        detail: String,
        error: String,
    },
    Error(String),
    Blocked(String),
}

pub(crate) struct ActiveControllerSource {
    pub(crate) info: HidDeviceInfo,
    pub(crate) session: HidSession,
    pub(crate) controller_seen: bool,
}

pub(crate) struct OutputSession {
    pub(crate) output: Box<dyn GamepadOutput>,
    pub(crate) bridge_endpoint: Option<BridgeEndpoint>,
    pub(crate) capabilities: OutputCapabilities,
    pub(crate) first_observed_receipt: FirstObservedReceiptState,
    pub(crate) feedback: OutputFeedbackRelay,
}

const OUTPUT_FEEDBACK_RENEW_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Default)]
pub(crate) struct OutputFeedbackRelay {
    semantics: OutputFeedbackSemantics,
    rumble: Option<RumbleCommand>,
    changed: bool,
    next_renewal: Duration,
}

impl OutputFeedbackRelay {
    pub(crate) fn new(semantics: OutputFeedbackSemantics) -> Self {
        Self {
            semantics,
            ..Self::default()
        }
    }

    fn observe(&mut self, feedback: OutputFeedback) {
        match feedback {
            OutputFeedback::Rumble {
                low_frequency,
                high_frequency,
            } => {
                self.rumble = Some(RumbleCommand {
                    low_frequency,
                    high_frequency,
                });
                self.changed = true;
            }
        }
    }

    pub(crate) fn drain(&mut self, output: &mut dyn GamepadOutput) {
        while let Some(feedback) = output.take_feedback() {
            self.observe(feedback);
        }
    }

    pub(crate) fn wait_without_consumer(&mut self) {
        if self.semantics == OutputFeedbackSemantics::Leased {
            self.rumble = None;
            self.changed = false;
        }
    }

    pub(crate) fn activate(&mut self) {
        self.next_renewal = Duration::ZERO;
        if self.semantics == OutputFeedbackSemantics::Stateful {
            self.changed = self.rumble.is_some();
        } else {
            self.rumble = None;
            self.changed = false;
        }
    }

    pub(crate) fn command_due(&mut self, now: Duration) -> Option<RumbleCommand> {
        let rumble = self.rumble?;
        if !self.changed && (!rumble.is_active() || now < self.next_renewal) {
            return None;
        }
        self.changed = false;
        self.next_renewal = now + OUTPUT_FEEDBACK_RENEW_INTERVAL;
        if self.semantics == OutputFeedbackSemantics::Leased {
            self.rumble = None;
        }
        Some(rumble)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FirstObservedReceiptRequest {
    pub(crate) request_id: u32,
    pub(crate) receipt: FirmwareInstallReceipt,
}

#[derive(Clone, Copy)]
pub(crate) enum FirstObservedReceiptState {
    Idle,
    Waiting {
        request: FirstObservedReceiptRequest,
        deadline: Instant,
    },
    Backoff {
        request: Option<FirstObservedReceiptRequest>,
        retry_at: Instant,
    },
}

/// Parks the gamepad output at neutral before desktop-input reconfiguration.
///
/// Constructing the desktop-input sink is synchronous, and Enigo's macOS
/// destructor can deliberately sleep after sustained event traffic. Those
/// operations now run on the dedicated desktop-input worker, so they cannot
/// block the supervisor's output-device deadline. Retaining this neutral transition also
/// makes reconfiguration safe if worker ownership changes again later.
///
/// The next controller report restores the real state, and `reset` keeps the
/// unchanged-output dedupe consistent so that restore is not skipped.
pub(crate) fn neutralize_before_desktop_work(
    engine: &mut BridgeEngine,
    output: &mut OutputSession,
) {
    if let Err(error) = engine.reset(&mut *output.output) {
        // Worth knowing about, but not worth failing the command over: the
        // caller's own error handling covers a genuinely dead link.
        eprintln!("level=warn event=neutral_before_desktop_work_failed error={error:?}");
    }
}

pub(crate) fn service_waiting_output(
    output: Option<&mut OutputSession>,
) -> Result<(), OutputError> {
    let Some(output) = output else {
        return Ok(());
    };
    output.output.service()?;
    output.feedback.drain(&mut *output.output);
    output.feedback.wait_without_consumer();
    Ok(())
}

pub(crate) enum ActiveExit {
    SourceLost,
    OutputLost(String),
    OutputBlocked(String),
    OutputChange(OutputSelection, CommandAck),
    AutomaticShutdown {
        info: HidDeviceInfo,
        trigger: ShutdownTrigger,
    },
    StoppedWithAck(CommandAck),
    ShutdownWithAck(CommandAck),
    SuspendedWithAck(CommandAck),
}

impl ActiveExit {
    pub(crate) const fn has_acknowledgement(&self) -> bool {
        matches!(
            self,
            Self::StoppedWithAck(_)
                | Self::ShutdownWithAck(_)
                | Self::SuspendedWithAck(_)
                | Self::OutputChange(_, _)
        )
    }

    /// Whether the output must accept a neutral state before the session is
    /// released. False only where the exit itself means the output is already
    /// unusable: demanding neutral there would replace the real diagnosis with
    /// a neutralization failure that says nothing new.
    pub(crate) const fn requires_neutral_before_release(&self) -> bool {
        !matches!(self, Self::OutputLost(_) | Self::OutputBlocked(_))
    }
}

/// Enforces the public command contract: no acknowledgement can become
/// observable until both the bridge output and every
/// controller-discovery session have been dropped.
pub(crate) fn acknowledge_after_hardware_release(
    output: OutputSession,
    release_controller_sessions: impl FnOnce(),
    acknowledgement: &CommandAck,
    result: Result<(), String>,
) {
    drop(output);
    release_controller_sessions();
    let _ = acknowledgement.send(result);
}

pub(crate) struct OpenedNonBridgeOutput {
    pub(crate) output: Box<dyn GamepadOutput>,
    pub(crate) virtual_hid: Option<crate::VirtualHidStatus>,
}

pub(crate) enum OutputOpenError {
    /// Worth reopening later; carries the status detail to show while waiting,
    /// which only the backend that failed can word correctly.
    Transient {
        detail: String,
        error: String,
    },
    Permanent(String),
}

pub(crate) fn make_non_bridge_output(
    selection: &OutputSelection,
) -> Result<OpenedNonBridgeOutput, OutputOpenError> {
    match selection {
        OutputSelection::BridgeDevice => Err(OutputOpenError::Permanent(
            "bridge-device output requires endpoint discovery".to_owned(),
        )),
        OutputSelection::VirtualGamepad(config) => VirtualGamepad::open(config.clone())
            .map(opened_virtual_gamepad)
            .map_err(|error| classify_virtual_gamepad_open_error(&error)),
        OutputSelection::VirtualHid(config) => VirtualGamepad::open_macos_helper(config)
            .map(opened_virtual_gamepad)
            .map_err(|error| classify_virtual_gamepad_open_error(&error)),
        OutputSelection::Dump(format) => Ok(OpenedNonBridgeOutput {
            output: Box::new(DumpOutput::new(io::stdout(), *format)),
            virtual_hid: None,
        }),
        OutputSelection::File(path) => FileOutput::create(path)
            .map(|output| OpenedNonBridgeOutput {
                output: Box::new(output),
                virtual_hid: None,
            })
            .map_err(|error| OutputOpenError::Permanent(error.to_string())),
        OutputSelection::Mock => Ok(OpenedNonBridgeOutput {
            output: Box::new(MockOutput::default()),
            virtual_hid: None,
        }),
    }
}

fn opened_virtual_gamepad(output: VirtualGamepad) -> OpenedNonBridgeOutput {
    let virtual_hid = output.macos_helper_metadata();
    OpenedNonBridgeOutput {
        output: Box::new(output),
        virtual_hid,
    }
}

fn classify_virtual_gamepad_open_error(
    error: &virtual_gamepad::VirtualGamepadError,
) -> OutputOpenError {
    if error.is_permanent_configuration_failure() {
        OutputOpenError::Permanent(error.to_string())
    } else {
        OutputOpenError::Transient {
            detail: "Waiting to restart virtual gamepad output".to_owned(),
            error: error.to_string(),
        }
    }
}

pub(crate) fn output_candidates(
    devices: Vec<BridgeEndpoint>,
    selection: &BridgeEndpointSelection,
) -> Vec<BridgeEndpoint> {
    match selection {
        BridgeEndpointSelection::AutoBridgeDevice => devices
            .into_iter()
            .filter(BridgeEndpoint::is_bridge_device)
            .collect(),
        BridgeEndpointSelection::SerialPort(path) => devices
            .into_iter()
            .filter(|endpoint| endpoint.serial_path() == Some(path))
            .collect(),
    }
}

pub(crate) fn discover_output_endpoints_with(
    selection: &BridgeEndpointSelection,
    discover_all: impl FnOnce() -> Result<BridgeEndpointDiscovery, BridgeTransportError>,
    discover_serial: impl FnOnce() -> Result<Vec<BridgeEndpoint>, BridgeTransportError>,
) -> Result<BridgeEndpointDiscovery, BridgeTransportError> {
    match selection {
        BridgeEndpointSelection::AutoBridgeDevice => discover_all(),
        BridgeEndpointSelection::SerialPort(path) => {
            let fallback =
                BridgeEndpoint::serial_port(path, bridge_output::DEFAULT_BRIDGE_BAUD_RATE);
            let (endpoint, warnings) = match discover_serial() {
                Ok(endpoints) => (
                    endpoints
                        .into_iter()
                        .find(|endpoint| endpoint.serial_path() == Some(path))
                        .unwrap_or(fallback),
                    Vec::new(),
                ),
                Err(error) => (fallback, vec![error]),
            };
            Ok(BridgeEndpointDiscovery {
                endpoints: vec![endpoint],
                warnings,
            })
        }
    }
}

pub(crate) fn open_output_candidates_with<T, E>(
    candidates: Vec<BridgeEndpoint>,
    preferred_stable_id: Option<&str>,
    mut open: impl FnMut(&BridgeEndpoint) -> Result<T, E>,
) -> (Vec<(BridgeEndpoint, T)>, Vec<String>)
where
    E: std::fmt::Display,
{
    let mut groups: Vec<Vec<BridgeEndpoint>> = Vec::new();
    for candidate in candidates {
        let matching_group = candidate.stable_id().and_then(|stable_id| {
            groups.iter().position(|group| {
                group
                    .first()
                    .and_then(BridgeEndpoint::stable_id)
                    .is_some_and(|existing| existing == stable_id)
            })
        });
        if let Some(index) = matching_group {
            groups[index].push(candidate);
        } else {
            groups.push(vec![candidate]);
        }
    }

    let mut failures = Vec::new();
    if let Some(index) = preferred_stable_id.and_then(|preferred| {
        groups.iter().position(|group| {
            group
                .first()
                .and_then(BridgeEndpoint::stable_id)
                .is_some_and(|stable_id| stable_id == preferred)
        })
    }) {
        let preferred = groups.remove(index);
        let (valid, preferred_failures) = open_candidate_group(preferred, &mut open);
        failures.extend(preferred_failures);
        if !valid.is_empty() {
            return (valid, failures);
        }
    }

    let mut valid = Vec::new();
    for group in groups {
        let (opened, group_failures) = open_candidate_group(group, &mut open);
        valid.extend(opened);
        failures.extend(group_failures);
    }
    (valid, failures)
}

fn open_candidate_group<T, E>(
    group: Vec<BridgeEndpoint>,
    open: &mut impl FnMut(&BridgeEndpoint) -> Result<T, E>,
) -> (Vec<(BridgeEndpoint, T)>, Vec<String>)
where
    E: std::fmt::Display,
{
    let (serial, raw_usb): (Vec<_>, Vec<_>) = group
        .into_iter()
        .partition(|candidate| candidate.kind() == bridge_output::BridgeTransportKind::SerialPort);
    let mut valid = Vec::new();
    let mut failures = Vec::new();
    for candidates in [serial, raw_usb] {
        for candidate in candidates {
            match open(&candidate) {
                Ok(output) => valid.push((candidate, output)),
                Err(error) => {
                    failures.push(format!("{}: {error}", candidate.display_label()));
                }
            }
        }
        if !valid.is_empty() {
            break;
        }
    }
    (valid, failures)
}

pub(crate) fn choose_output_index<T>(
    valid: &[(BridgeEndpoint, T)],
    preferred_stable_id: Option<&str>,
) -> Result<usize, String> {
    if valid.len() == 1 {
        return Ok(0);
    }
    if let Some(preferred) = preferred_stable_id {
        let preferred_matches: Vec<_> = valid
            .iter()
            .enumerate()
            .filter(|(_, (endpoint, _))| endpoint.stable_id() == Some(preferred))
            .map(|(index, _)| index)
            .collect();
        if preferred_matches.len() == 1 {
            return Ok(preferred_matches[0]);
        }
    }
    Err(output_ambiguity_message(valid))
}

pub(crate) fn output_ambiguity_message<T>(valid: &[(BridgeEndpoint, T)]) -> String {
    let devices = valid
        .iter()
        .map(|(endpoint, _)| {
            format!(
                "{} (device {})",
                endpoint.display_label(),
                masked_serial(endpoint.stable_id())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let remedy = if valid
        .iter()
        .all(|(endpoint, _)| endpoint.serial_path().is_some())
    {
        "restart with --port PATH"
    } else {
        "disconnect all but one bridge device and retry"
    };
    format!("multiple valid bridge devices found: {devices}; {remedy}")
}

pub(crate) fn controller_source_description(
    enumeration_index: usize,
    info: &HidDeviceInfo,
) -> String {
    format!(
        "index {enumeration_index} {}",
        controller_source_identity(info)
    )
}

pub(crate) fn controller_source_identity(info: &HidDeviceInfo) -> String {
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

pub(crate) fn acknowledge_all(acks: &mut Vec<CommandAck>) {
    for ack in acks.drain(..) {
        let _ = ack.send(Ok(()));
    }
}

pub(crate) fn acknowledge_all_with_result(acks: &mut Vec<CommandAck>, result: &Result<(), String>) {
    for ack in acks.drain(..) {
        let _ = ack.send(result.clone());
    }
}

pub(crate) fn is_latest_state_report(report_id: u8) -> bool {
    matches!(report_id, INPUT_REPORT_ID | EXTENDED_INPUT_REPORT_ID)
}

#[derive(Debug)]
pub(crate) enum ReportEffect {
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

pub(crate) fn process_report(
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
                desktop_input: DesktopInputSnapshot::from(&source),
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

pub(crate) fn is_output_error(message: &str) -> bool {
    message.contains("output failed") || message.contains("serial") || message.contains("transport")
}

/// Recovers the permanent/transient split from an error that
/// [`process_report`] has already flattened to a string. Anchored to
/// [`bridge_output::CONFIGURATION_FAILURE_PREFIX`] so it tracks the rendering
/// of [`OutputError::Configuration`].
pub(crate) fn is_permanent_output_error(message: &str) -> bool {
    message.contains(bridge_output::CONFIGURATION_FAILURE_PREFIX)
}

pub(crate) fn valid_battery_percent(percent: u8) -> Option<u8> {
    (percent <= 100).then_some(percent)
}

pub(crate) fn record_lazy(
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

pub(crate) fn record_device_event(
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

pub(crate) fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

pub(crate) fn should_tick_input_timeout(replaced_reports: u64, has_pending_report: bool) -> bool {
    replaced_reports == 0 && !has_pending_report
}
