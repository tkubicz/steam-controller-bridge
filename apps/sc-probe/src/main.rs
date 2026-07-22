use std::env;
use std::fmt::Write as _;
use std::fs::File;
use std::time::{Duration, Instant};

use recording::{RecordingEvent, RecordingWriter, KIND_DEVICE_CONNECTED, KIND_DEVICE_DISCONNECTED};
use serde_json::json;
use steam_controller_device::{enumerate, DeviceEvent, HidDeviceInfo, HidSession};

fn main() {
    if let Err(error) = run() {
        eprintln!("sc-probe: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }
    match args[0].as_str() {
        "list" => list_devices(),
        "inspect" => inspect_devices(optional_index(&args)?),
        "monitor" => monitor(&args),
        "capture" => capture(&args),
        command => Err(format!("unknown command '{command}'")),
    }
}

fn list_devices() -> Result<(), String> {
    let devices = enumerate().map_err(|error| error.to_string())?;
    if devices.is_empty() {
        println!("No HID collections found.");
        return Ok(());
    }
    println!("index  VID:PID    usage       transport  product");
    for (index, device) in devices.iter().enumerate() {
        println!(
            "{index:<6} {:04x}:{:04x}  {:04x}:{:04x}  {:<9}  {}",
            device.vendor_id,
            device.product_id,
            device.usage_page,
            device.usage,
            device.transport,
            display_optional(device.product.as_deref())
        );
    }
    println!("\nSelect a collection explicitly with --index N; no VID/PID is assumed.");
    Ok(())
}

fn inspect_devices(index: Option<usize>) -> Result<(), String> {
    let devices = enumerate().map_err(|error| error.to_string())?;
    let selected: Vec<(usize, &HidDeviceInfo)> = if let Some(index) = index {
        vec![(
            index,
            devices
                .get(index)
                .ok_or_else(|| format!("HID device index {index} does not exist"))?,
        )]
    } else {
        devices.iter().enumerate().collect()
    };
    if selected.is_empty() {
        println!("No HID collections found.");
    }
    for (index, device) in selected {
        let sibling_count = devices
            .iter()
            .filter(|candidate| device.same_physical_device(candidate))
            .count();
        println!("[{index}]");
        println!("  id:                 {}", device.id);
        println!("  path:               {}", device.path);
        println!("  vendor ID:          0x{:04x}", device.vendor_id);
        println!("  product ID:         0x{:04x}", device.product_id);
        println!("  usage page:         0x{:04x}", device.usage_page);
        println!("  usage:              0x{:04x}", device.usage);
        println!("  interface:          {}", device.interface_number);
        println!("  transport:          {}", device.transport);
        println!(
            "  manufacturer:       {}",
            display_optional(device.manufacturer.as_deref())
        );
        println!(
            "  product:            {}",
            display_optional(device.product.as_deref())
        );
        println!(
            "  serial:             {}",
            display_optional(device.serial_number.as_deref())
        );
        println!("  candidate sibling collections: {sibling_count}");
    }
    Ok(())
}

fn monitor(args: &[String]) -> Result<(), String> {
    let index = required_index(args)?;
    let raw = args.iter().any(|arg| arg == "--raw");
    let duration = duration_limit(args)?;
    let mut session = HidSession::open_index(index).map_err(|error| error.to_string())?;
    eprintln!("Monitoring collection {index}; press Ctrl+C to stop.");
    let started = Instant::now();
    let mut window_started = Instant::now();
    let mut window_reports = 0_u64;
    loop {
        if duration.is_some_and(|limit| started.elapsed() >= limit) {
            break;
        }
        match session
            .poll(Duration::from_millis(100))
            .map_err(|error| error.to_string())?
        {
            Some(DeviceEvent::Connected(info)) => {
                eprintln!(
                    "connected: {} ({})",
                    display_optional(info.product.as_deref()),
                    info.transport
                );
            }
            Some(DeviceEvent::Disconnected) => eprintln!("disconnected; waiting for reconnect"),
            Some(DeviceEvent::Report(report)) => {
                window_reports += 1;
                if raw {
                    println!(
                        "{:>12} us id=0x{:02x} len={} {}",
                        report.timestamp.as_micros(),
                        report.report_id,
                        report.data.len(),
                        hex_bytes(&report.data)
                    );
                }
            }
            None => {}
        }
        if window_started.elapsed() >= Duration::from_secs(1) {
            eprintln!("report rate: {window_reports} Hz");
            window_reports = 0;
            window_started = Instant::now();
        }
    }
    Ok(())
}

fn capture(args: &[String]) -> Result<(), String> {
    let index = required_index(args)?;
    let output_path = value_after(args, "--output").ok_or("capture requires --output PATH")?;
    let duration = duration_limit(args)?;
    let mut session = HidSession::open_index(index).map_err(|error| error.to_string())?;
    let mut recording = RecordingWriter::new(
        File::create(output_path)
            .map_err(|error| format!("cannot create capture '{output_path}': {error}"))?,
    );
    eprintln!("Capturing collection {index} to {output_path}; press Ctrl+C to stop.");
    let started = Instant::now();
    let mut reports = 0_u64;
    loop {
        if duration.is_some_and(|limit| started.elapsed() >= limit) {
            break;
        }
        let event = session
            .poll(Duration::from_millis(100))
            .map_err(|error| error.to_string())?;
        match event {
            Some(DeviceEvent::Connected(info)) => {
                let event = RecordingEvent::new(
                    elapsed_us(started),
                    KIND_DEVICE_CONNECTED,
                    device_json(&info),
                );
                recording
                    .write_event(&event)
                    .map_err(|error| error.to_string())?;
                eprintln!(
                    "connected: {} ({})",
                    display_optional(info.product.as_deref()),
                    info.transport
                );
            }
            Some(DeviceEvent::Disconnected) => {
                let event =
                    RecordingEvent::new(elapsed_us(started), KIND_DEVICE_DISCONNECTED, json!({}));
                recording
                    .write_event(&event)
                    .map_err(|error| error.to_string())?;
                eprintln!("disconnected; waiting for reconnect");
            }
            Some(DeviceEvent::Report(report)) => {
                let event = RecordingEvent::raw_hid_with_metadata(
                    elapsed_us(started),
                    report.report_id,
                    &report.data,
                    Some(&report.source_device_id),
                    Some(&report.transport),
                    report.dropped_reports,
                )
                .map_err(|error| error.to_string())?;
                recording
                    .write_event(&event)
                    .map_err(|error| error.to_string())?;
                reports += 1;
            }
            None => {}
        }
    }
    eprintln!("capture complete: {reports} reports");
    Ok(())
}

fn device_json(info: &HidDeviceInfo) -> serde_json::Value {
    json!({
        "id": info.id,
        "path": info.path,
        "vendor_id": info.vendor_id,
        "product_id": info.product_id,
        "usage_page": info.usage_page,
        "usage": info.usage,
        "interface_number": info.interface_number,
        "serial_number": info.serial_number,
        "manufacturer": info.manufacturer,
        "product": info.product,
        "transport": info.transport,
    })
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn duration_limit(args: &[String]) -> Result<Option<Duration>, String> {
    value_after(args, "--duration-secs")
        .map(|value| {
            value
                .parse::<u64>()
                .map(Duration::from_secs)
                .map_err(|_| format!("invalid --duration-secs value '{value}'"))
        })
        .transpose()
}

fn optional_index(args: &[String]) -> Result<Option<usize>, String> {
    value_after(args, "--index")
        .map(|value| {
            value
                .parse()
                .map_err(|_| format!("invalid --index value '{value}'"))
        })
        .transpose()
}

fn required_index(args: &[String]) -> Result<usize, String> {
    optional_index(args)?
        .ok_or_else(|| "this command requires --index N from `sc-probe list`".to_owned())
}

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn display_optional(value: Option<&str>) -> &str {
    value.filter(|text| !text.is_empty()).unwrap_or("<unknown>")
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(3));
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn print_help() {
    println!(
        "sc-probe <command> [options]\n\nCommands:\n  list                              List every HID collection\n  inspect [--index N]               Show complete metadata and sibling count\n  monitor --index N [--raw]         Monitor reports and reconnects\n  capture --index N --output PATH   Capture raw reports as JSONL\n\nOptions:\n  --duration-secs N                 Stop monitor/capture after N seconds\n  -h, --help"
    );
}
