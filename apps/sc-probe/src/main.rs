use std::env;
use std::fmt::Write as _;
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use recording::{RecordingEvent, RecordingWriter, KIND_DEVICE_CONNECTED, KIND_DEVICE_DISCONNECTED};
use serde_json::json;
use steam_controller_device::{
    enumerate, DeviceEvent, HidDeviceInfo, HidSession, LizardModeHeartbeat,
};
use steam_controller_protocol::{DecodedReport, SteamControllerDecoder, SteamControllerState};

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
        "suppress-lizard" => suppress_lizard(&args),
        "rumble" => rumble(&args),
        "power-off" => power_off(&args),
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
            steam_controller_device::masked_serial(device.serial_number.as_deref())
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
    let mut decoder = SteamControllerDecoder::new();
    let mut previous_state: Option<SteamControllerState> = None;
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
            Some(DeviceEvent::Disconnected) => {
                previous_state = None;
                eprintln!("disconnected; waiting for reconnect");
            }
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
                } else {
                    match decoder.decode(report.report_id, &report.data) {
                        Ok(DecodedReport::ControllerState(state)) => {
                            if previous_state.as_ref() != Some(&state) {
                                println!(
                                    "seq={} buttons={:#010x} ls=({:6},{:6}) rs=({:6},{:6}) pads=({:6},{:6})/({:6},{:6}) triggers=({:5},{:5})",
                                    state.sequence,
                                    state.buttons.0,
                                    state.left_stick_x,
                                    state.left_stick_y,
                                    state.right_stick_x,
                                    state.right_stick_y,
                                    state.left_pad_x,
                                    state.left_pad_y,
                                    state.right_pad_x,
                                    state.right_pad_y,
                                    state.left_trigger,
                                    state.right_trigger
                                );
                                previous_state = Some(state);
                            }
                        }
                        Ok(other) => println!("{other:?}"),
                        Err(error) => eprintln!("decode error: {error}"),
                    }
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
    let include_decoded_states = args.iter().any(|arg| arg == "--decoded");
    let mut session = HidSession::open_index(index).map_err(|error| error.to_string())?;
    let mut decoder = SteamControllerDecoder::new();
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
                let timestamp_us = elapsed_us(started);
                let event = RecordingEvent::raw_hid_with_metadata(
                    timestamp_us,
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
                if include_decoded_states {
                    if let Ok(DecodedReport::ControllerState(state)) =
                        decoder.decode(report.report_id, &report.data)
                    {
                        let event = RecordingEvent::decoded_steam_state(timestamp_us, &state)
                            .map_err(|error| error.to_string())?;
                        recording
                            .write_event(&event)
                            .map_err(|error| error.to_string())?;
                    }
                }
                reports += 1;
            }
            None => {}
        }
    }
    eprintln!("capture complete: {reports} reports");
    Ok(())
}

fn suppress_lizard(args: &[String]) -> Result<(), String> {
    let index = required_index(args)?;
    let duration = duration_limit(args)?;
    let (info, mut session) = open_supported_controller_input(index)?;

    let stop = Arc::new(AtomicBool::new(false));
    let signal_stop = Arc::clone(&stop);
    ctrlc::set_handler(move || signal_stop.store(true, Ordering::Release))
        .map_err(|error| format!("cannot install Ctrl-C handler: {error}"))?;

    let started = Instant::now();
    let mut heartbeat = LizardModeHeartbeat::new();
    let mut refreshes = 0_u64;
    eprintln!(
        "Testing lizard-mode suppression on Steam Controller input collection {index} \
         ({}, interface {}); press Ctrl+C to stop.",
        info.controller_transport()
            .map_or_else(|| "Unknown".to_owned(), |transport| transport.to_string()),
        info.interface_number
    );
    while !stop.load(Ordering::Acquire) && duration.is_none_or(|limit| started.elapsed() < limit) {
        if heartbeat.refresh_due(started.elapsed()) {
            send_lizard_refresh(&session, &mut heartbeat, started.elapsed(), &mut refreshes)?;
        }
        match session
            .poll(Duration::from_millis(100))
            .map_err(|error| error.to_string())?
        {
            Some(DeviceEvent::Connected(info)) => {
                heartbeat.connected();
                send_lizard_refresh(&session, &mut heartbeat, started.elapsed(), &mut refreshes)?;
                eprintln!(
                    "connected: {} ({}) lizard_suppressed=true",
                    display_optional(info.product.as_deref()),
                    info.transport
                );
            }
            Some(DeviceEvent::Disconnected) => {
                heartbeat.disconnected();
                eprintln!("disconnected; lizard heartbeat stopped, waiting for reconnect");
            }
            Some(DeviceEvent::Report(_)) | None => {}
        }
    }
    eprintln!(
        "lizard suppression stopped after {refreshes} successful writes; \
         the controller watchdog should restore desktop mode within about 10 seconds"
    );
    Ok(())
}

fn send_lizard_refresh(
    session: &HidSession,
    heartbeat: &mut LizardModeHeartbeat,
    now: Duration,
    refreshes: &mut u64,
) -> Result<(), String> {
    session.suppress_lizard_mode().map_err(|error| {
        format!(
            "lizard-mode suppression failed; stop other controller tools and \
             verify the selected Steam Controller input collection: {error}"
        )
    })?;
    heartbeat.refreshed(now);
    *refreshes += 1;
    eprintln!(
        "lizard suppression refresh={} elapsed_ms={}",
        *refreshes,
        now.as_millis()
    );
    Ok(())
}

fn rumble(args: &[String]) -> Result<(), String> {
    let index = required_index(args)?;
    let low_frequency = parse_value(args, "--low", 32_768_u16)?;
    let high_frequency = parse_value(args, "--high", 32_768_u16)?;
    let duration = Duration::from_millis(parse_value(args, "--duration-ms", 1_000_u64)?);
    let (info, mut session) = open_supported_controller_input(index)?;

    let stop = Arc::new(AtomicBool::new(false));
    let signal_stop = Arc::clone(&stop);
    ctrlc::set_handler(move || signal_stop.store(true, Ordering::Release))
        .map_err(|error| format!("cannot install Ctrl-C handler: {error}"))?;

    eprintln!(
        "Testing rumble on Steam Controller input collection {index} \
         ({}, interface {}): low={low_frequency} high={high_frequency} \
         duration_ms={}; press Ctrl+C to stop.",
        info.controller_transport()
            .map_or_else(|| "Unknown".to_owned(), |transport| transport.to_string()),
        info.interface_number,
        duration.as_millis()
    );
    let result = run_rumble_test(&mut session, &stop, low_frequency, high_frequency, duration);
    let stop_result = session
        .set_rumble(0, 0)
        .map_err(|error| format!("final rumble-zero write failed: {error}"));
    match (&result, &stop_result) {
        (_, Ok(())) => eprintln!("rumble stopped with an explicit zero write"),
        (Err(_), Err(stop_error)) => eprintln!("{stop_error}"),
        (Ok(()), Err(_)) => {}
    }
    result.and(stop_result)
}

fn power_off(args: &[String]) -> Result<(), String> {
    let index = required_index(args)?;
    let (info, mut session) = open_supported_controller_input(index)?;
    eprintln!(
        "WARNING: powering off Steam Controller 2 collection {index} ({}, interface {}).",
        info.controller_transport()
            .map_or_else(|| "Unknown".to_owned(), |transport| transport.to_string()),
        info.interface_number
    );
    let mut successes = 0_u8;
    let mut last_error = None;
    for attempt in 1..=3 {
        match session.power_off() {
            Ok(()) => {
                successes += 1;
                eprintln!("power-off write {attempt}/3 succeeded");
            }
            Err(error) => {
                last_error = Some(error.to_string());
                eprintln!("power-off write {attempt}/3 failed: {error}");
            }
        }
        if attempt < 3 {
            match session.poll(Duration::from_millis(10)) {
                Ok(_) => {}
                Err(error) if successes > 0 => {
                    eprintln!(
                        "controller became unavailable after a successful power-off write: {error}"
                    );
                    break;
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }
    }
    if successes == 0 {
        Err(last_error.unwrap_or_else(|| "all power-off writes failed".to_owned()))
    } else {
        eprintln!(
            "power-off accepted ({successes} successful write(s)); press Steam to wake the controller"
        );
        Ok(())
    }
}

fn open_supported_controller_input(index: usize) -> Result<(HidDeviceInfo, HidSession), String> {
    let devices = enumerate().map_err(|error| error.to_string())?;
    let info = devices
        .get(index)
        .cloned()
        .ok_or_else(|| format!("HID device index {index} does not exist"))?;
    if !info.is_supported_controller_source() {
        return Err(format!(
            "collection index {index} is not a supported Steam Controller 2 input; \
             select a 28de:1304 USB Puck ff00:0001 interface 2-5 or the \
             28de:1303 Bluetooth ff00:0001 interface -1 collection"
        ));
    }
    let session = HidSession::open_info(&info).map_err(|error| error.to_string())?;
    Ok((info, session))
}

fn run_rumble_test(
    session: &mut HidSession,
    stop: &AtomicBool,
    low_frequency: u16,
    high_frequency: u16,
    duration: Duration,
) -> Result<(), String> {
    const RUMBLE_REFRESH: Duration = Duration::from_millis(40);

    let started = Instant::now();
    let mut heartbeat = LizardModeHeartbeat::new();
    heartbeat.connected();
    let mut lizard_refreshes = 0_u64;
    send_lizard_refresh(
        session,
        &mut heartbeat,
        started.elapsed(),
        &mut lizard_refreshes,
    )?;
    session
        .set_rumble(low_frequency, high_frequency)
        .map_err(|error| format!("initial rumble write failed: {error}"))?;
    let mut rumble_writes = 1_u64;
    let mut next_rumble_refresh = RUMBLE_REFRESH;
    let mut connected = true;

    while !stop.load(Ordering::Acquire) && started.elapsed() < duration {
        let now = started.elapsed();
        if connected && heartbeat.refresh_due(now) {
            send_lizard_refresh(session, &mut heartbeat, now, &mut lizard_refreshes)?;
        }
        if connected && now >= next_rumble_refresh {
            session
                .set_rumble(low_frequency, high_frequency)
                .map_err(|error| format!("rumble refresh failed: {error}"))?;
            rumble_writes += 1;
            next_rumble_refresh = now.saturating_add(RUMBLE_REFRESH);
        }
        match session
            .poll(Duration::from_millis(10))
            .map_err(|error| error.to_string())?
        {
            Some(DeviceEvent::Connected(info)) => {
                connected = true;
                heartbeat.connected();
                let now = started.elapsed();
                send_lizard_refresh(session, &mut heartbeat, now, &mut lizard_refreshes)?;
                session
                    .set_rumble(low_frequency, high_frequency)
                    .map_err(|error| format!("rumble write after reconnect failed: {error}"))?;
                rumble_writes += 1;
                next_rumble_refresh = now.saturating_add(RUMBLE_REFRESH);
                eprintln!(
                    "connected: {} ({}) rumble_active=true",
                    display_optional(info.product.as_deref()),
                    info.transport
                );
            }
            Some(DeviceEvent::Disconnected) => {
                connected = false;
                heartbeat.disconnected();
                eprintln!("disconnected; rumble refresh stopped, waiting for reconnect");
            }
            Some(DeviceEvent::Report(_)) | None => {}
        }
    }
    eprintln!(
        "rumble test complete: writes={rumble_writes} \
         lizard_refreshes={lizard_refreshes}"
    );
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

fn parse_value<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> Result<T, String> {
    value_after(args, flag).map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| format!("invalid {flag} value '{value}'"))
    })
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
        "sc-probe <command> [options]\n\nCommands:\n  list                                      List every HID collection\n  inspect [--index N]                       Show complete metadata and sibling count\n  monitor --index N [--raw]                 Decode or print raw live reports\n  capture --index N --output PATH [--decoded]\n                                            Capture raw and optional decoded JSONL\n  suppress-lizard --index N                 Safely test SC2 lizard-mode suppression\n  rumble --index N [--low N] [--high N] [--duration-ms N]\n                                            Test dual SC2 rumble (defaults: 50%/50%, 1 second)\n  power-off --index N                       Power off the selected SC2\n\nOptions:\n  --duration-secs N                         Stop monitor/capture/suppression after N seconds\n  --low N --high N                          Rumble channel intensity, 0..65535\n  --duration-ms N                           Rumble duration in milliseconds\n  -h, --help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn rumble_values_accept_full_u16_range_and_reject_overflow() {
        assert_eq!(
            parse_value(&args(&["--low", "0"]), "--low", 32_768_u16).unwrap(),
            0
        );
        assert_eq!(
            parse_value(&args(&["--high", "65535"]), "--high", 32_768_u16).unwrap(),
            u16::MAX
        );
        assert!(parse_value(&args(&["--low", "65536"]), "--low", 32_768_u16).is_err());
        assert_eq!(parse_value::<u16>(&[], "--low", 32_768).unwrap(), 32_768);
    }
}
