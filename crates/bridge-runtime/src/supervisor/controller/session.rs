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

pub(crate) struct ActiveControllerSource {
    pub(crate) info: HidDeviceInfo,
    pub(crate) session: HidSession,
    pub(crate) controller_seen: bool,
}

pub(crate) struct OutputSession {
    pub(crate) output: Box<dyn GamepadOutput>,
    pub(crate) xiao: Option<SerialDeviceInfo>,
    pub(crate) first_observed_receipt: FirstObservedReceiptState,
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
/// block the supervisor's XIAO deadline. Retaining this neutral transition also
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

pub(crate) fn service_waiting_output(output: Option<&mut OutputSession>) -> bool {
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

pub(crate) enum ActiveExit {
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
    pub(crate) const fn has_acknowledgement(&self) -> bool {
        matches!(
            self,
            Self::StoppedWithAck(_) | Self::ShutdownWithAck(_) | Self::SuspendedWithAck(_)
        )
    }
}

/// Enforces the public command contract: no acknowledgement can become
/// observable until both the output (and therefore the serial port) and every
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

pub(crate) fn make_nonserial_output(
    selection: &OutputSelection,
) -> Result<Box<dyn GamepadOutput>, String> {
    match selection {
        OutputSelection::Serial => Err("serial output requires XIAO discovery".to_owned()),
        OutputSelection::Dump(format) => Ok(Box::new(DumpOutput::new(io::stdout(), *format))),
        OutputSelection::File(path) => FileOutput::create(path)
            .map(|output| Box::new(output) as Box<dyn GamepadOutput>)
            .map_err(|error| error.to_string()),
        OutputSelection::Mock => Ok(Box::new(MockOutput::default())),
    }
}

pub(crate) fn choose_xiao_index<T>(
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

pub(crate) fn xiao_ambiguity_message<T>(valid: &[(SerialDeviceInfo, T)]) -> String {
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

pub(crate) fn ownership_guidance(error: &DeviceError) -> String {
    format!(
        "{error}. Fully quit Steam and other controller tools; if Steam's ipcserver remains, \
         stop its LaunchAgent manually"
    )
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
