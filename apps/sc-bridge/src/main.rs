use std::env;
use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
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
use steam_controller_device::{enumerate, DeviceEvent, HidDeviceInfo, HidSession};

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
    let mut worker = spawn_hid_worker(index, Arc::clone(&stop), Arc::clone(&dropped))?;
    let duration_limit = value_after(args, "--duration-secs")
        .map(|_| parse_value(args, "--duration-secs", 0_u64).map(Duration::from_secs))
        .transpose()?;
    let mut metrics_at = Instant::now();
    eprintln!(
        "level=info event=bridge_started controller_index={index} profile=default protocol=1"
    );
    while !stop.load(Ordering::Acquire)
        && duration_limit.is_none_or(|limit| started.elapsed() < limit)
    {
        match worker.receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(DeviceEvent::Connected(info)) => {
                engine.connected();
                eprintln!(
                    "level=info event=hid_connected id={:?} transport={:?}",
                    info.id, info.transport
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
            Ok(DeviceEvent::Disconnected) => {
                engine
                    .disconnected(output)
                    .map_err(|error| error.to_string())?;
                eprintln!("level=warn event=hid_disconnected action=neutral");
                record(
                    &mut recording,
                    &RecordingEvent::new(elapsed_us(started), KIND_DEVICE_DISCONNECTED, json!({})),
                )?;
            }
            Ok(DeviceEvent::Report(report)) => {
                let timestamp = elapsed_us(started);
                record(
                    &mut recording,
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
                match engine.process_report(
                    report.report_id,
                    &report.data,
                    started.elapsed(),
                    output,
                ) {
                    Ok(ProcessOutcome::State { source, mapped, .. }) => {
                        record(
                            &mut recording,
                            &RecordingEvent::decoded_steam_state(timestamp, &source)
                                .map_err(|error| error.to_string())?,
                        )?;
                        record(
                            &mut recording,
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
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let lost = dropped.swap(0, Ordering::AcqRel);
        if lost > 0 {
            engine.note_dropped_reports(lost);
        }
        if let ProcessOutcome::Neutralized(reason) = engine
            .tick(started.elapsed(), output)
            .map_err(|error| error.to_string())?
        {
            eprintln!("level=warn event=neutralized reason={reason:?}");
        }
        if metrics_at.elapsed() >= Duration::from_secs(1) {
            print_metrics(engine.metrics(), output.diagnostics(), metrics_at.elapsed());
            metrics_at = Instant::now();
        }
    }
    worker.shutdown()?;
    engine.shutdown(output).map_err(|error| error.to_string())?;
    print_metrics(engine.metrics(), output.diagnostics(), started.elapsed());
    eprintln!("level=info event=bridge_stopped reason=shutdown action=neutral");
    Ok(())
}

fn spawn_hid_worker(
    index: usize,
    stop: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
) -> Result<HidWorker, String> {
    let mut session = HidSession::open_index(index).map_err(|error| error.to_string())?;
    let (sender, receiver) = mpsc::sync_channel(64);
    let worker_stop = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        while !worker_stop.load(Ordering::Acquire) {
            match session.poll(Duration::from_millis(10)) {
                Ok(Some(DeviceEvent::Report(report))) => send_report(&sender, report, &dropped),
                Ok(Some(event)) => {
                    if !send_lifecycle(&sender, event, &worker_stop) {
                        break;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("level=error event=hid_worker_error error={error:?}");
                    let _ = send_lifecycle(&sender, DeviceEvent::Disconnected, &worker_stop);
                    break;
                }
            }
        }
    });
    Ok(HidWorker {
        receiver,
        stop,
        handle: Some(worker),
    })
}

struct HidWorker {
    receiver: Receiver<DeviceEvent>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HidWorker {
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

fn send_lifecycle(
    sender: &SyncSender<DeviceEvent>,
    mut event: DeviceEvent,
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

fn send_report(
    sender: &SyncSender<DeviceEvent>,
    report: steam_controller_device::RawHidReport,
    dropped: &AtomicU64,
) {
    match sender.try_send(DeviceEvent::Report(report)) {
        Ok(()) | Err(TrySendError::Disconnected(_)) => {}
        Err(TrySendError::Full(_)) => {
            dropped.fetch_add(1, Ordering::Relaxed);
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
    devices.iter().position(|device| {
        device.manufacturer.as_deref().is_some_and(|name| name.eq_ignore_ascii_case("valve"))
            || device.product.as_deref().is_some_and(|name| name.to_ascii_lowercase().contains("steam"))
    }).ok_or_else(|| "--controller auto found no enumerated Valve/Steam HID collection; use sc-probe list and --index N".to_owned())
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
    elapsed: Duration,
) {
    let hz = if elapsed.is_zero() {
        0.0
    } else {
        metrics.input_reports as f64 / elapsed.as_secs_f64()
    };
    eprintln!("level=info event=metrics input_reports={} report_hz={hz:.1} dropped={} decode_failures={} state_changes={} output_packets={} skipped={} hid_reconnects={} serial_reconnects={} framing_failures={} checksum_failures={} avg_decode_us={:.2} avg_mapping_us={:.2} avg_processing_us={:.2}", metrics.input_reports, metrics.dropped_input_reports, metrics.decode_failures, metrics.state_changes, metrics.output_packets, metrics.outputs_skipped_unchanged, metrics.hid_reconnects, output.serial_reconnects, output.framing_failures, output.checksum_failures, metrics.average_decode_us(), metrics.average_mapping_us(), metrics.average_processing_us());
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
    println!("sc-bridge [options]\n\n  --input <live|replay>       Input mode (default: live)\n  --controller auto          Auto-select an enumerated Valve/Steam collection\n  --index N                  Select collection from sc-probe list\n  --file PATH                Replay recording input\n  --output <dump|pretty|json|raw|file|mock|serial>\n  --output-file PATH         Binary frame output\n  --port PATH --baud N       Serial output (default baud: 115200)\n  --serial-log               Log serial frame bytes\n  --record PATH              Record full live pipeline as JSONL\n  --input-timeout-ms N       Neutral timeout (default: 200)\n  --decode-failure-limit N   Failures before neutral (default: 3)\n  --duration-secs N          Stop live mode after N seconds\n  --deterministic --speed N --seek-us N   Replay controls\n  -h, --help");
}
