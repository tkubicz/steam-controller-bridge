use std::env;
use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bridge_core::{BridgeConfig, BridgeEngine, ProcessOutcome};
use bridge_output::{
    DumpFormat, DumpOutput, FileOutput, GamepadOutput, MockOutput, OutputDiagnostics, SerialConfig,
    SerialOutput,
};
use controller_mapper::MapperConfig;
use recording::{
    RecordingEvent, RecordingWriter, ReplayOptions, ReplaySession, ReplayTiming,
    KIND_DEVICE_CONNECTED, KIND_DEVICE_DISCONNECTED,
};
use serde_json::json;
use steam_controller_device::{
    enumerate, DeviceError, DeviceEvent, HidDeviceInfo, HidSession, LizardModeHeartbeat,
    RawHidReport,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LizardMode {
    Suppress,
    Leave,
}

#[derive(Debug, Default)]
struct SharedLizardMetrics {
    active: AtomicBool,
    refreshes: AtomicU64,
    failures: AtomicU64,
    last_refresh_millis: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct LizardDiagnostics {
    active: bool,
    refreshes: u64,
    failures: u64,
    last_refresh_age: Option<Duration>,
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

    fn snapshot(&self, now: Duration) -> LizardDiagnostics {
        let last_refresh_millis = self.last_refresh_millis.load(Ordering::Acquire);
        LizardDiagnostics {
            active: self.active.load(Ordering::Acquire),
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

    fn connected<E>(
        &mut self,
        now: Duration,
        write: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        self.heartbeat.connected();
        if self.mode == LizardMode::Suppress {
            self.refresh(now, write)?;
        }
        Ok(())
    }

    fn service<E>(
        &mut self,
        now: Duration,
        write: impl FnOnce() -> Result<(), E>,
    ) -> Result<bool, E> {
        if self.mode == LizardMode::Suppress && self.heartbeat.refresh_due(now) {
            self.refresh(now, write)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn disconnected(&mut self) {
        self.heartbeat.disconnected();
        self.metrics.record_disconnected();
    }

    fn refresh<E>(
        &mut self,
        now: Duration,
        write: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        if let Err(error) = write() {
            self.metrics.record_failure();
            return Err(error);
        }
        self.heartbeat.refreshed(now);
        self.metrics.record_success(now);
        Ok(())
    }
}

#[derive(Debug)]
enum HidWorkerEvent {
    Connected(HidDeviceInfo),
    Disconnected,
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

fn should_tick_input_timeout(replaced_reports: u64, has_pending_report: bool) -> bool {
    replaced_reports == 0 && !has_pending_report
}

fn main() {
    if let Err(error) = run() {
        eprintln!("level=error app=sc-bridge message={error:?}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }
    if value_after(&args, "--profile").is_some_and(|profile| profile != "default") {
        return Err("only --profile default is currently supported".to_owned());
    }
    if value_after(&args, "--controller").is_some_and(|controller| controller != "auto") {
        return Err(
            "--controller currently accepts only auto; use --index N for explicit selection"
                .to_owned(),
        );
    }
    let mut output = make_output(&args)?;
    if value_after(&args, "--input") == Some("replay") {
        return run_replay(&args, &mut *output);
    }
    run_live(&args, &mut *output)
}

fn run_replay(args: &[String], output: &mut dyn GamepadOutput) -> Result<(), String> {
    let path = value_after(args, "--file").ok_or("replay input requires --file PATH")?;
    let session = ReplaySession::read(io::BufReader::new(
        File::open(path).map_err(|error| format!("cannot open replay '{path}': {error}"))?,
    ))
    .map_err(|error| error.to_string())?;
    let options = ReplayOptions {
        timing: if args.iter().any(|arg| arg == "--deterministic") {
            ReplayTiming::Immediate
        } else {
            ReplayTiming::RealTime
        },
        speed: parse_value(args, "--speed", 1.0_f64)?,
        seek_timestamp_us: parse_value(args, "--seek-us", 0_u64)?,
    };
    let stats = session
        .play_once(output, options)
        .map_err(|error| error.to_string())?;
    output.send_neutral().map_err(|error| error.to_string())?;
    eprintln!(
        "level=info event=replay_complete events={} states={} ignored={}",
        stats.events_processed, stats.states_sent, stats.events_ignored
    );
    Ok(())
}

#[allow(clippy::too_many_lines)] // The linear lifecycle loop keeps shutdown ordering explicit.
fn run_live(args: &[String], output: &mut dyn GamepadOutput) -> Result<(), String> {
    let index = select_controller(args)?;
    let lizard_mode = parse_lizard_mode(args)?;
    let timeout = Duration::from_millis(parse_value(args, "--input-timeout-ms", 200_u64)?);
    let failure_limit = parse_value(args, "--decode-failure-limit", 3_u32)?;
    let mut engine = BridgeEngine::new(
        BridgeConfig {
            input_timeout: timeout,
            decode_failure_limit: failure_limit,
        },
        MapperConfig::default(),
    )
    .map_err(|error| error.to_string())?;
    let mut recording = value_after(args, "--record")
        .map(|path| {
            File::create(path)
                .map(RecordingWriter::new)
                .map_err(|error| format!("cannot create recording '{path}': {error}"))
        })
        .transpose()?;
    let started = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    let signal_stop = Arc::clone(&stop);
    ctrlc::set_handler(move || signal_stop.store(true, Ordering::Release))
        .map_err(|error| format!("cannot install Ctrl-C handler: {error}"))?;
    let dropped = Arc::new(AtomicU64::new(0));
    let mut worker = spawn_hid_worker(index, lizard_mode, Arc::clone(&dropped))?;
    let duration_limit = value_after(args, "--duration-secs")
        .map(|_| parse_value(args, "--duration-secs", 0_u64).map(Duration::from_secs))
        .transpose()?;
    let mut metrics_at = Instant::now();
    let mut previous_input_reports = 0_u64;
    eprintln!(
        "level=info event=bridge_started controller_index={index} profile=default protocol=1 lizard_mode={lizard_mode:?}"
    );
    let run_result = (|| -> Result<(), String> {
        while !stop.load(Ordering::Acquire)
            && duration_limit.is_none_or(|limit| started.elapsed() < limit)
        {
            if let Some(error) = worker.take_lizard_failure() {
                return Err(error);
            }
            match worker.receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(HidWorkerEvent::Connected(info)) => {
                    engine.connected();
                    let lizard = worker.lizard_diagnostics();
                    eprintln!(
                        "level=info event=hid_connected id={:?} transport={:?} lizard_suppressed={}",
                        info.id, info.transport, lizard.active
                    );
                    record(
                        &mut recording,
                        &RecordingEvent::new(
                            elapsed_us(started),
                            KIND_DEVICE_CONNECTED,
                            device_json(&info),
                        ),
                    )?;
                }
                Ok(HidWorkerEvent::Disconnected) => {
                    engine
                        .disconnected(output)
                        .map_err(|error| error.to_string())?;
                    eprintln!("level=warn event=hid_disconnected action=neutral");
                    record(
                        &mut recording,
                        &RecordingEvent::new(
                            elapsed_us(started),
                            KIND_DEVICE_DISCONNECTED,
                            json!({}),
                        ),
                    )?;
                }
                Ok(HidWorkerEvent::ReportReady) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if let Some(error) = worker.take_lizard_failure() {
                return Err(error);
            }
            if let Some(report) = worker.take_latest_report() {
                process_hid_report(&report, &mut engine, output, &mut recording, started)?;
            }
            let lost = dropped.swap(0, Ordering::AcqRel);
            if lost > 0 {
                engine.note_dropped_reports(lost);
            }
            // A replacement or still-pending report proves the HID source is
            // alive. Let the next loop consume the newest snapshot before
            // evaluating the processed-state timeout; otherwise a temporary
            // serial stall can create a false neutral while input is arriving.
            if should_tick_input_timeout(lost, worker.has_pending_report()) {
                if let ProcessOutcome::Neutralized(reason) = engine
                    .tick(started.elapsed(), output)
                    .map_err(|error| error.to_string())?
                {
                    eprintln!("level=warn event=neutralized reason={reason:?}");
                }
            }
            if metrics_at.elapsed() >= Duration::from_secs(1) {
                let metrics = engine.metrics();
                let interval_reports = metrics.input_reports.saturating_sub(previous_input_reports);
                print_metrics(
                    metrics,
                    output.diagnostics(),
                    worker.lizard_diagnostics(),
                    metrics_at.elapsed(),
                    interval_reports,
                );
                previous_input_reports = metrics.input_reports;
                metrics_at = Instant::now();
            }
        }
        Ok(())
    })();

    // Keep the Puck session and its lizard-off heartbeat alive until the XIAO
    // has been neutralized. The controller watchdog restores desktop mode
    // after the worker is stopped; no lizard-on command is sent.
    let cleanup_result = neutralize_then_stop_hid(
        || {
            engine
                .shutdown(output)
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
        || worker.shutdown(),
    );
    let metrics = engine.metrics();
    print_metrics(
        metrics,
        output.diagnostics(),
        worker.lizard_diagnostics(),
        started.elapsed(),
        metrics.input_reports,
    );
    eprintln!("level=info event=bridge_stopped reason=shutdown action=neutral");
    run_result.and(cleanup_result)
}

fn process_hid_report(
    report: &RawHidReport,
    engine: &mut BridgeEngine,
    output: &mut dyn GamepadOutput,
    recording: &mut Option<RecordingWriter<File>>,
    started: Instant,
) -> Result<(), String> {
    let timestamp = elapsed_us(started);
    record(
        recording,
        &RecordingEvent::raw_hid_with_metadata(
            timestamp,
            report.report_id,
            &report.data,
            Some(&report.source_device_id),
            Some(&report.transport),
            report.dropped_reports,
        )
        .map_err(|error| error.to_string())?,
    )?;
    match engine.process_report(report.report_id, &report.data, started.elapsed(), output) {
        Ok(ProcessOutcome::State { source, mapped, .. }) => {
            record(
                recording,
                &RecordingEvent::decoded_steam_state(timestamp, &source)
                    .map_err(|error| error.to_string())?,
            )?;
            record(
                recording,
                &RecordingEvent::mapped_gamepad_state(timestamp, &mapped)
                    .map_err(|error| error.to_string())?,
            )?;
        }
        Ok(ProcessOutcome::Neutralized(reason)) => {
            eprintln!("level=warn event=neutralized reason={reason:?}");
        }
        Ok(_) => {}
        Err(bridge_core::BridgeError::Decode(error)) => {
            eprintln!("level=warn event=decode_failure error={error:?}");
        }
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}

fn neutralize_then_stop_hid(
    neutralize: impl FnOnce() -> Result<(), String>,
    stop_hid: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let neutral_result = neutralize();
    let stop_result = stop_hid();
    neutral_result.and(stop_result)
}

fn spawn_hid_worker(
    index: usize,
    lizard_mode: LizardMode,
    dropped: Arc<AtomicU64>,
) -> Result<HidWorker, String> {
    let mut session = HidSession::open_index(index).map_err(|error| error.to_string())?;
    let (sender, receiver) = mpsc::sync_channel(64);
    let (lizard_failure_sender, lizard_failure_receiver) = mpsc::channel();
    let latest_report = Arc::new(LatestReportSlot::default());
    let worker_latest_report = Arc::clone(&latest_report);
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let lizard_metrics = Arc::new(SharedLizardMetrics::default());
    let worker_lizard_metrics = Arc::clone(&lizard_metrics);
    let worker_started = Instant::now();
    let worker = thread::spawn(move || {
        let mut lizard = LizardSupervisor::new(lizard_mode, worker_lizard_metrics);
        while !worker_stop.load(Ordering::Acquire) {
            if let Err(error) =
                lizard.service(worker_started.elapsed(), || session.suppress_lizard_mode())
            {
                worker_latest_report.clear(&dropped);
                report_lizard_failure(&lizard_failure_sender, &worker_stop, &error);
                break;
            }
            match session.poll(Duration::from_millis(10)) {
                Ok(Some(DeviceEvent::Connected(info))) => {
                    if let Err(error) = lizard
                        .connected(worker_started.elapsed(), || session.suppress_lizard_mode())
                    {
                        worker_latest_report.clear(&dropped);
                        report_lizard_failure(&lizard_failure_sender, &worker_stop, &error);
                        break;
                    }
                    if !send_worker_event(&sender, HidWorkerEvent::Connected(info), &worker_stop) {
                        break;
                    }
                }
                Ok(Some(DeviceEvent::Disconnected)) => {
                    lizard.disconnected();
                    worker_latest_report.clear(&dropped);
                    if !send_worker_event(&sender, HidWorkerEvent::Disconnected, &worker_stop) {
                        break;
                    }
                }
                Ok(Some(DeviceEvent::Report(report))) => {
                    if !publish_report(&sender, &worker_latest_report, report, &dropped) {
                        break;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("level=error event=hid_worker_error error={error:?}");
                    lizard.disconnected();
                    worker_latest_report.clear(&dropped);
                    let _ = send_worker_event(&sender, HidWorkerEvent::Disconnected, &worker_stop);
                    break;
                }
            }
        }
        worker_latest_report.clear(&dropped);
        lizard.disconnected();
    });
    Ok(HidWorker {
        receiver,
        lizard_failure_receiver,
        latest_report,
        stop,
        handle: Some(worker),
        started: worker_started,
        lizard_metrics,
    })
}

struct HidWorker {
    receiver: Receiver<HidWorkerEvent>,
    lizard_failure_receiver: Receiver<String>,
    latest_report: Arc<LatestReportSlot>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    started: Instant,
    lizard_metrics: Arc<SharedLizardMetrics>,
}

impl HidWorker {
    fn take_lizard_failure(&self) -> Option<String> {
        self.lizard_failure_receiver.try_recv().ok()
    }

    fn lizard_diagnostics(&self) -> LizardDiagnostics {
        self.lizard_metrics.snapshot(self.started.elapsed())
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

fn report_lizard_failure(sender: &mpsc::Sender<String>, stop: &AtomicBool, error: &DeviceError) {
    let message = format!(
        "lizard-mode suppression failed; XIAO will be neutralized and the bridge will stop: {error}"
    );
    if sender.send(message).is_ok() {
        while !stop.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn select_controller(args: &[String]) -> Result<usize, String> {
    if let Some(index) = value_after(args, "--index") {
        return index
            .parse()
            .map_err(|_| format!("invalid --index value '{index}'"));
    }
    let devices = enumerate().map_err(|error| error.to_string())?;
    devices
        .iter()
        .position(is_controller_candidate)
        .ok_or_else(|| "--controller auto found no enumerated Valve/Steam HID collection; use sc-probe list and --index N".to_owned())
}

fn is_controller_candidate(device: &HidDeviceInfo) -> bool {
    device.supports_lizard_mode_suppression()
}

fn parse_lizard_mode(args: &[String]) -> Result<LizardMode, String> {
    match value_after(args, "--lizard-mode").unwrap_or("suppress") {
        "suppress" => Ok(LizardMode::Suppress),
        "leave" => Ok(LizardMode::Leave),
        other => Err(format!(
            "invalid --lizard-mode value '{other}'; expected suppress or leave"
        )),
    }
}

fn make_output(args: &[String]) -> Result<Box<dyn GamepadOutput>, String> {
    let name = value_after(args, "--output").unwrap_or("dump");
    match name {
        "dump" | "compact" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Compact))),
        "pretty" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Pretty))),
        "json" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Json))),
        "raw" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Raw))),
        "file" => Ok(Box::new(
            FileOutput::create(
                value_after(args, "--output-file")
                    .ok_or("file output requires --output-file PATH")?,
            )
            .map_err(|error| error.to_string())?,
        )),
        "mock" => Ok(Box::new(MockOutput::default())),
        "serial" => Ok(Box::new(
            SerialOutput::open(
                value_after(args, "--port").ok_or("serial output requires --port PATH")?,
                parse_value(args, "--baud", 115_200_u32)?,
                SerialConfig {
                    packet_logging: args.iter().any(|arg| arg == "--serial-log"),
                    ..SerialConfig::default()
                },
            )
            .map_err(|error| error.to_string())?,
        )),
        other => Err(format!("unknown output '{other}'")),
    }
}

fn record(
    writer: &mut Option<RecordingWriter<File>>,
    event: &RecordingEvent,
) -> Result<(), String> {
    if let Some(writer) = writer {
        writer
            .write_event(event)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[allow(clippy::cast_precision_loss)] // Human-readable rate tolerates huge-counter precision loss.
fn print_metrics(
    metrics: bridge_core::BridgeMetrics,
    output: OutputDiagnostics,
    lizard: LizardDiagnostics,
    elapsed: Duration,
    reports_in_interval: u64,
) {
    let hz = if elapsed.is_zero() {
        0.0
    } else {
        reports_in_interval as f64 / elapsed.as_secs_f64()
    };
    let lizard_refresh_age_ms = lizard.last_refresh_age.map_or(u64::MAX, |age| {
        u64::try_from(age.as_millis()).unwrap_or(u64::MAX)
    });
    eprintln!("level=info event=metrics input_reports={} report_hz={hz:.1} dropped={} decode_failures={} state_changes={} output_packets={} skipped={} hid_reconnects={} serial_reconnects={} framing_failures={} checksum_failures={} state_refreshes={} lizard_suppressed={} lizard_refreshes={} lizard_failures={} lizard_refresh_age_ms={} avg_decode_us={:.2} avg_mapping_us={:.2} avg_processing_us={:.2}", metrics.input_reports, metrics.dropped_input_reports, metrics.decode_failures, metrics.state_changes, metrics.output_packets, metrics.outputs_skipped_unchanged, metrics.hid_reconnects, output.serial_reconnects, output.framing_failures, output.checksum_failures, output.state_refreshes, lizard.active, lizard.refreshes, lizard.failures, lizard_refresh_age_ms, metrics.average_decode_us(), metrics.average_mapping_us(), metrics.average_processing_us());
}

fn device_json(info: &HidDeviceInfo) -> serde_json::Value {
    json!({ "id": info.id, "path": info.path, "vendor_id": info.vendor_id, "product_id": info.product_id, "usage_page": info.usage_page, "usage": info.usage, "transport": info.transport, "product": info.product, "manufacturer": info.manufacturer })
}
fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}
fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}
fn parse_value<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> Result<T, String> {
    value_after(args, flag).map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| format!("invalid value for {flag}: {value}"))
    })
}

fn print_help() {
    println!("sc-bridge [options]\n\n  --input <live|replay>       Input mode (default: live)\n  --controller auto          Auto-select a supported official Puck slot\n  --index N                  Select collection from sc-probe list\n  --lizard-mode <suppress|leave>\n                             Suppress native keyboard/mouse mode (default: suppress)\n  --file PATH                Replay recording input\n  --output <dump|pretty|json|raw|file|mock|serial>\n  --output-file PATH         Binary frame output\n  --port PATH --baud N       Serial output (default baud: 115200)\n  --serial-log               Log serial frame bytes\n  --record PATH              Record full live pipeline as JSONL\n  --input-timeout-ms N       Neutral timeout (default: 200)\n  --decode-failure-limit N   Failures before neutral (default: 3)\n  --duration-secs N          Stop live mode after N seconds\n  --deterministic --speed N --seek-us N   Replay controls\n  -h, --help");
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    fn device(
        vendor_id: u16,
        product_id: u16,
        usage_page: u16,
        usage: u16,
        interface_number: i32,
    ) -> HidDeviceInfo {
        HidDeviceInfo {
            id: "test".to_owned(),
            path: "test".to_owned(),
            vendor_id,
            product_id,
            usage_page,
            usage,
            interface_number,
            serial_number: None,
            manufacturer: Some("Valve Software".to_owned()),
            product: Some("Steam Controller Puck".to_owned()),
            transport: "USB".to_owned(),
        }
    }

    #[test]
    fn auto_selection_accepts_only_supported_puck_slots() {
        assert!(is_controller_candidate(&device(
            0x28de, 0x1304, 0xff00, 0x0001, 2
        )));
        assert!(is_controller_candidate(&device(
            0x28de, 0x1304, 0xff00, 0x0001, 5
        )));
        assert!(!is_controller_candidate(&device(
            0x28de, 0x1304, 0xff00, 0x0002, 6
        )));
        assert!(!is_controller_candidate(&device(
            0x045e, 0x028e, 0x0001, 0x0005, 0
        )));
    }

    #[test]
    fn parses_lizard_mode_with_safe_default() {
        assert_eq!(parse_lizard_mode(&[]).unwrap(), LizardMode::Suppress);
        assert_eq!(
            parse_lizard_mode(&["--lizard-mode".into(), "leave".into()]).unwrap(),
            LizardMode::Leave
        );
        assert!(parse_lizard_mode(&["--lizard-mode".into(), "invalid".into()]).is_err());
    }

    #[test]
    fn lizard_supervisor_suppresses_before_becoming_active_and_refreshes() {
        let metrics = Arc::new(SharedLizardMetrics::default());
        let mut supervisor = LizardSupervisor::new(LizardMode::Suppress, Arc::clone(&metrics));
        let mut writes = 0_u64;

        supervisor
            .connected(Duration::ZERO, || {
                writes += 1;
                Ok::<(), ()>(())
            })
            .unwrap();
        assert_eq!(writes, 1);
        assert!(metrics.snapshot(Duration::ZERO).active);

        assert!(!supervisor
            .service(Duration::from_millis(2_999), || {
                writes += 1;
                Ok::<(), ()>(())
            })
            .unwrap());
        assert_eq!(writes, 1);
        assert!(supervisor
            .service(Duration::from_secs(3), || {
                writes += 1;
                Ok::<(), ()>(())
            })
            .unwrap());
        assert_eq!(writes, 2);

        supervisor.disconnected();
        assert!(!supervisor
            .service(Duration::from_secs(30), || {
                writes += 1;
                Ok::<(), ()>(())
            })
            .unwrap());
        assert_eq!(writes, 2);

        supervisor
            .connected(Duration::from_secs(30), || {
                writes += 1;
                Ok::<(), ()>(())
            })
            .unwrap();
        assert_eq!(writes, 3);
    }

    #[test]
    fn lizard_supervisor_leave_mode_never_writes() {
        let metrics = Arc::new(SharedLizardMetrics::default());
        let mut supervisor = LizardSupervisor::new(LizardMode::Leave, Arc::clone(&metrics));

        supervisor
            .connected(Duration::ZERO, || Err::<(), _>("must not write"))
            .unwrap();
        assert!(!supervisor
            .service(Duration::from_secs(30), || Err::<(), _>("must not write"))
            .unwrap());
        assert_eq!(metrics.snapshot(Duration::from_secs(30)).refreshes, 0);
    }

    #[test]
    fn lizard_supervisor_failure_is_fail_closed() {
        let metrics = Arc::new(SharedLizardMetrics::default());
        let mut supervisor = LizardSupervisor::new(LizardMode::Suppress, Arc::clone(&metrics));

        assert!(supervisor
            .connected(Duration::ZERO, || Err::<(), _>("write failed"))
            .is_err());
        let diagnostics = metrics.snapshot(Duration::ZERO);
        assert!(!diagnostics.active);
        assert_eq!(diagnostics.refreshes, 0);
        assert_eq!(diagnostics.failures, 1);
    }

    #[test]
    fn every_live_exit_neutralizes_before_stopping_hid() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let neutral_trace = Rc::clone(&trace);
        let stop_trace = Rc::clone(&trace);
        neutralize_then_stop_hid(
            move || {
                neutral_trace.borrow_mut().push("neutral");
                Err("serial neutral failed".to_owned())
            },
            move || {
                stop_trace.borrow_mut().push("stop HID");
                Ok(())
            },
        )
        .expect_err("the neutral error must be retained");
        assert_eq!(*trace.borrow(), ["neutral", "stop HID"]);
    }

    #[test]
    fn latest_report_slot_replaces_stale_input_and_renotifies_after_take() {
        fn report(marker: u8) -> RawHidReport {
            RawHidReport {
                timestamp: Duration::from_millis(u64::from(marker)),
                report_id: 0x42,
                data: vec![0x42, marker],
                source_device_id: "slot".to_owned(),
                transport: "USB".to_owned(),
                dropped_reports: 0,
            }
        }

        let slot = LatestReportSlot::default();
        let dropped = AtomicU64::new(0);
        assert!(slot.publish(report(1), &dropped));
        assert!(!slot.publish(report(2), &dropped));
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_eq!(slot.take().expect("latest report").data, [0x42, 2]);
        assert!(slot.publish(report(3), &dropped));
        assert_eq!(slot.take().expect("next report").data, [0x42, 3]);
    }

    #[test]
    fn incoming_or_pending_reports_defer_the_processed_state_timeout() {
        assert!(should_tick_input_timeout(0, false));
        assert!(!should_tick_input_timeout(1, false));
        assert!(!should_tick_input_timeout(0, true));
        assert!(!should_tick_input_timeout(1, true));
    }
}
