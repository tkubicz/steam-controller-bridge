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
    BridgeRuntime, BridgeStatus, ControllerSelection, LizardMode, OutputSelection, PuckDockAction,
    RuntimeConfig, RuntimeState, SerialSelection, MAX_IDLE_SHUTDOWN_TIMEOUT,
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
        let summary = status_summary(&status);
        if last_summary.as_ref() != Some(&summary) {
            log_status(&status);
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

fn status_summary(status: &BridgeStatus) -> String {
    format!(
        "{:?}|{}|{:?}|{}|{:?}|{:?}|{}|{}|{}|{:?}|{}|{}|{:?}|{:?}|{:?}|{}|{}|{}|{}",
        status.state,
        status.detail,
        status.source,
        status.controller.connected,
        status.xiao.path,
        status.battery_percent,
        status.lizard.suppressed,
        status.lizard.refreshes,
        status.lizard.failures,
        status.haptics.state,
        status.haptics.refreshes / 25,
        status.haptics.failures,
        status.last_error,
        status.automatic_shutdown.phase,
        status.automatic_shutdown.trigger,
        status.automatic_shutdown.puck_dock_episode_handled,
        status.automatic_shutdown.successful_shutdowns,
        status.automatic_shutdown.failures,
        status
            .automatic_shutdown
            .neutral_idle_age
            .map_or(0, |age| age.as_secs() / 60)
    )
}

fn log_status(status: &BridgeStatus) {
    eprintln!(
        "level=info event=status state={:?} detail={:?} input_connected={} \
         input_active={} input_transport={:?} input_product={:?} input_serial={} \
         controller_connected={} xiao_path={:?} battery={:?} charge_state={:?} \
         lizard_suppressed={} lizard_refreshes={} lizard_failures={} lizard_refresh_age_ms={:?}",
        status.state,
        status.detail,
        status.source.connected,
        status.source.active,
        status.source.transport,
        status
            .source
            .identity
            .as_ref()
            .and_then(|info| info.product.as_deref()),
        bridge_runtime::mask_serial_for_display(
            status
                .source
                .identity
                .as_ref()
                .and_then(|info| info.serial_number.as_deref()),
        ),
        status.controller.connected,
        status.xiao.path,
        status.battery_percent,
        status.battery_charge_state,
        status.lizard.suppressed,
        status.lizard.refreshes,
        status.lizard.failures,
        status.lizard.last_refresh_age.map(|age| age.as_millis())
    );
    eprintln!(
        "level=info event=automatic_shutdown phase={:?} trigger={:?} \
         idle_timeout_secs={:?} idle_age_ms={:?} puck_dock_action={:?} \
         puck_dock_handled={} successes={} failures={} retry_after_ms={:?}",
        status.automatic_shutdown.phase,
        status.automatic_shutdown.trigger,
        status
            .automatic_shutdown
            .configured_timeout
            .map(|timeout| timeout.as_secs()),
        status
            .automatic_shutdown
            .neutral_idle_age
            .map(|age| age.as_millis()),
        status.automatic_shutdown.puck_dock_action,
        status.automatic_shutdown.puck_dock_episode_handled,
        status.automatic_shutdown.successful_shutdowns,
        status.automatic_shutdown.failures,
        status
            .automatic_shutdown
            .retry_after
            .map(|delay| delay.as_millis())
    );
    eprintln!(
        "level=info event=haptics state={:?} commands={} writes={} refreshes={} \
         coalesced={} failures={} last_command_age_ms={:?}",
        status.haptics.state,
        status.haptics.commands_received,
        status.haptics.writes,
        status.haptics.refreshes,
        status.haptics.coalesced_commands,
        status.haptics.failures,
        status.haptics.last_command_age.map(|age| age.as_millis())
    );
    if let Some(error) = &status.last_error {
        eprintln!("level=warn event=status_error message={error:?}");
    }
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
    let mut idle_shutdown_timeout = parse_idle_shutdown(args)?;
    let mut puck_dock_action = match value_after(args, "--puck-dock-action").unwrap_or("leave") {
        "leave" => PuckDockAction::LeaveOn,
        "power-off" => PuckDockAction::PowerOff,
        other => {
            return Err(format!(
                "invalid --puck-dock-action value '{other}'; expected leave or power-off"
            ));
        }
    };
    if output != OutputSelection::Serial {
        if value_after(args, "--idle-shutdown").is_some()
            || value_after(args, "--puck-dock-action").is_some()
        {
            return Err(
                "automatic controller shutdown requires live serial output to a ready XIAO"
                    .to_owned(),
            );
        }
        idle_shutdown_timeout = None;
        puck_dock_action = PuckDockAction::LeaveOn;
    }
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
        idle_shutdown_timeout,
        puck_dock_action,
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
    if value_after(args, "--idle-shutdown").is_some()
        || value_after(args, "--puck-dock-action").is_some()
    {
        return Err(
            "automatic controller shutdown options require live input and are unavailable in replay mode"
                .to_owned(),
        );
    }
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

fn parse_idle_shutdown(args: &[String]) -> Result<Option<Duration>, String> {
    let Some(value) = value_after(args, "--idle-shutdown") else {
        return Ok(RuntimeConfig::default().idle_shutdown_timeout);
    };
    if value == "never" {
        return Ok(None);
    }
    let minutes = value.parse::<u64>().map_err(|_| {
        format!("invalid --idle-shutdown value '{value}'; expected never or MINUTES")
    })?;
    let timeout = Duration::from_secs(minutes.saturating_mul(60));
    if minutes == 0 || timeout > MAX_IDLE_SHUTDOWN_TIMEOUT {
        return Err("--idle-shutdown must be never or a whole number from 1 to 1440".to_owned());
    }
    Ok(Some(timeout))
}

fn print_help() {
    println!(
        "sc-bridge [options]\n\n\
         With no arguments, waits for one active Steam Controller 2 input source\n\
         (Puck or Bluetooth) and the XIAO, then starts the serial bridge and recovers\n\
         after reconnects.\n\n\
         --input <live|replay>       Input mode (default: live)\n\
         --controller auto          Explicit automatic active-source discovery\n\
         --index N                  Select collection from sc-probe list\n\
         --port <auto|PATH>         Automatic XIAO discovery or fixed CDC port\n\
         --lizard-mode <suppress|leave>\n\
                                    Suppress native keyboard/mouse mode (default: suppress)\n\
         --idle-shutdown <never|MINUTES>\n\
                                    Neutral idle timeout (default: 15; maximum: 1440)\n\
         --puck-dock-action <leave|power-off>\n\
                                    Optional immediate shutdown when placed on Puck\n\
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
        assert_eq!(config.idle_shutdown_timeout, Some(Duration::from_mins(15)));
        assert_eq!(config.puck_dock_action, PuckDockAction::LeaveOn);
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
    fn automatic_shutdown_options_parse_independently() {
        let config = live_config(&args(&[
            "--idle-shutdown",
            "5",
            "--puck-dock-action",
            "power-off",
        ]))
        .unwrap();
        assert_eq!(config.idle_shutdown_timeout, Some(Duration::from_mins(5)));
        assert_eq!(config.puck_dock_action, PuckDockAction::PowerOff);

        let never = live_config(&args(&["--idle-shutdown", "never"])).unwrap();
        assert_eq!(never.idle_shutdown_timeout, None);
        assert_eq!(never.puck_dock_action, PuckDockAction::LeaveOn);
    }

    #[test]
    fn automatic_shutdown_rejects_invalid_values() {
        for value in ["0", "1441", "soon"] {
            assert!(live_config(&args(&["--idle-shutdown", value])).is_err());
        }
        assert!(live_config(&args(&["--puck-dock-action", "maybe"])).is_err());
    }

    #[test]
    fn replay_serial_requires_an_explicit_port() {
        assert!(make_replay_output(&args(&["--input", "replay", "--output", "serial"])).is_err());
    }
}
