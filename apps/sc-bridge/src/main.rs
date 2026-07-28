use std::env;
use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bridge_core::BridgeConfig;
use bridge_output::{
    DumpFormat, DumpOutput, FileOutput, GamepadOutput, MockOutput, SerialConfig, SerialOutput,
};
use bridge_runtime::{
    BridgeRuntime, ControllerSelection, LizardMode, OutputSelection, RuntimeConfig, RuntimeState,
    SerialSelection,
};
use recording::{ReplayOptions, ReplaySession, ReplayTiming};

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
    match value_after(&args, "--input").unwrap_or("live") {
        "live" => run_live(&args),
        "replay" => run_replay(&args),
        other => Err(format!(
            "invalid --input value '{other}'; expected live or replay"
        )),
    }
}

fn run_live(args: &[String]) -> Result<(), String> {
    let config = live_config(args)?;
    let duration = value_after(args, "--duration-secs")
        .map(|_| parse_value(args, "--duration-secs", 0_u64).map(Duration::from_secs))
        .transpose()?;
    let handle = BridgeRuntime::spawn(config);
    let stop = Arc::new(AtomicBool::new(false));
    let signal_stop = Arc::clone(&stop);
    ctrlc::set_handler(move || signal_stop.store(true, Ordering::Release))
        .map_err(|error| format!("cannot install Ctrl-C handler: {error}"))?;
    let started = Instant::now();
    let mut last_summary = None;
    let result = loop {
        let status = handle.status();
        let summary = format!(
            "{:?}|{}|{}|{}|{:?}|{:?}|{}|{:?}",
            status.state,
            status.detail,
            status.puck.connected,
            status.controller.connected,
            status.xiao.path,
            status.battery_percent,
            status.lizard.suppressed,
            status.last_error
        );
        if last_summary.as_ref() != Some(&summary) {
            eprintln!(
                "level=info event=status state={:?} detail={:?} puck_connected={} \
                 controller_connected={} xiao_path={:?} battery={:?} lizard_suppressed={}",
                status.state,
                status.detail,
                status.puck.connected,
                status.controller.connected,
                status.xiao.path,
                status.battery_percent,
                status.lizard.suppressed
            );
            if let Some(error) = &status.last_error {
                eprintln!("level=warn event=status_error message={error:?}");
            }
            last_summary = Some(summary);
        }
        if status.state == RuntimeState::Error {
            break Err(status.last_error.unwrap_or_else(|| status.detail.clone()));
        }
        if stop.load(Ordering::Acquire) || duration.is_some_and(|limit| started.elapsed() >= limit)
        {
            break Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    };
    let shutdown = handle.shutdown().map_err(|error| error.to_string());
    result.and(shutdown)
}

fn live_config(args: &[String]) -> Result<RuntimeConfig, String> {
    if value_after(args, "--controller").is_some_and(|value| value != "auto") {
        return Err(
            "--controller accepts only auto; use --index N for an explicit collection".to_owned(),
        );
    }
    let controller =
        value_after(args, "--index").map_or(Ok(ControllerSelection::AutoActive), |value| {
            value
                .parse()
                .map(ControllerSelection::Index)
                .map_err(|_| format!("invalid --index value '{value}'"))
        })?;
    let serial = match value_after(args, "--port") {
        None | Some("auto") => SerialSelection::AutoXiao,
        Some(path) => SerialSelection::Port(path.to_owned()),
    };
    let output = parse_live_output(args)?;
    let lizard_mode = match value_after(args, "--lizard-mode").unwrap_or("suppress") {
        "suppress" => LizardMode::Suppress,
        "leave" => LizardMode::Leave,
        other => {
            return Err(format!(
                "invalid --lizard-mode value '{other}'; expected suppress or leave"
            ));
        }
    };
    Ok(RuntimeConfig {
        controller,
        serial,
        output,
        lizard_mode,
        bridge: BridgeConfig {
            input_timeout: Duration::from_millis(parse_value(args, "--input-timeout-ms", 200_u64)?),
            decode_failure_limit: parse_value(args, "--decode-failure-limit", 3_u32)?,
        },
        serial_config: SerialConfig {
            packet_logging: args.iter().any(|arg| arg == "--serial-log"),
            ..SerialConfig::default()
        },
        baud_rate: parse_value(args, "--baud", 115_200_u32)?,
        recording_path: value_after(args, "--record").map(PathBuf::from),
        ..RuntimeConfig::default()
    })
}

fn parse_live_output(args: &[String]) -> Result<OutputSelection, String> {
    match value_after(args, "--output").unwrap_or("serial") {
        "serial" => Ok(OutputSelection::Serial),
        "dump" | "compact" => Ok(OutputSelection::Dump(DumpFormat::Compact)),
        "pretty" => Ok(OutputSelection::Dump(DumpFormat::Pretty)),
        "json" => Ok(OutputSelection::Dump(DumpFormat::Json)),
        "raw" => Ok(OutputSelection::Dump(DumpFormat::Raw)),
        "file" => Ok(OutputSelection::File(PathBuf::from(
            value_after(args, "--output-file").ok_or("file output requires --output-file PATH")?,
        ))),
        "mock" => Ok(OutputSelection::Mock),
        other => Err(format!("unknown output '{other}'")),
    }
}

fn run_replay(args: &[String]) -> Result<(), String> {
    let mut output = make_replay_output(args)?;
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
        .play_once(&mut *output, options)
        .map_err(|error| error.to_string())?;
    output.send_neutral().map_err(|error| error.to_string())?;
    eprintln!(
        "level=info event=replay_complete events={} states={} ignored={}",
        stats.events_processed, stats.states_sent, stats.events_ignored
    );
    Ok(())
}

fn make_replay_output(args: &[String]) -> Result<Box<dyn GamepadOutput>, String> {
    match value_after(args, "--output").unwrap_or("dump") {
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
        "serial" => {
            let port = value_after(args, "--port")
                .filter(|value| *value != "auto")
                .ok_or("serial replay requires an explicit --port PATH")?;
            Ok(Box::new(
                SerialOutput::open(
                    port,
                    parse_value(args, "--baud", 115_200_u32)?,
                    SerialConfig {
                        packet_logging: args.iter().any(|arg| arg == "--serial-log"),
                        ..SerialConfig::default()
                    },
                )
                .map_err(|error| error.to_string())?,
            ))
        }
        other => Err(format!("unknown output '{other}'")),
    }
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
    println!(
        "sc-bridge [options]\n\n\
         With no arguments, waits for the active official Puck slot and the XIAO,\n\
         then starts the serial bridge and recovers after reconnects.\n\n\
         --input <live|replay>       Input mode (default: live)\n\
         --controller auto          Explicit automatic active-slot discovery\n\
         --index N                  Select collection from sc-probe list\n\
         --port <auto|PATH>         Automatic XIAO discovery or fixed CDC port\n\
         --lizard-mode <suppress|leave>\n\
                                    Suppress native keyboard/mouse mode (default: suppress)\n\
         --file PATH                Replay recording input\n\
         --output <dump|pretty|json|raw|file|mock|serial>\n\
                                    Live default: serial; replay default: dump\n\
         --output-file PATH         Binary frame output\n\
         --baud N                   Serial baud rate (default: 115200)\n\
         --serial-log               Log serial frame bytes\n\
         --record PATH              Record full live pipeline as JSONL\n\
         --input-timeout-ms N       Neutral timeout (default: 200)\n\
         --decode-failure-limit N   Failures before neutral (default: 3)\n\
         --duration-secs N          Stop live mode after N seconds\n\
         --deterministic --speed N --seek-us N   Replay controls\n\
         -h, --help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn zero_arguments_select_zero_configuration_live_bridge() {
        let config = live_config(&[]).unwrap();
        assert_eq!(config.controller, ControllerSelection::AutoActive);
        assert_eq!(config.serial, SerialSelection::AutoXiao);
        assert_eq!(config.output, OutputSelection::Serial);
    }

    #[test]
    fn explicit_controller_and_port_override_discovery() {
        let config = live_config(&args(&["--index", "43", "--port", "/dev/cu.test"])).unwrap();
        assert_eq!(config.controller, ControllerSelection::Index(43));
        assert_eq!(
            config.serial,
            SerialSelection::Port("/dev/cu.test".to_owned())
        );
    }

    #[test]
    fn explicit_auto_forms_match_defaults() {
        let config = live_config(&args(&["--controller", "auto", "--port", "auto"])).unwrap();
        assert_eq!(config.controller, ControllerSelection::AutoActive);
        assert_eq!(config.serial, SerialSelection::AutoXiao);
    }

    #[test]
    fn replay_serial_requires_an_explicit_port() {
        assert!(make_replay_output(&args(&["--input", "replay", "--output", "serial"])).is_err());
    }
}
