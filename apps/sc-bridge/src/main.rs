use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bridge_core::BridgeConfig;
use bridge_output::{
    BridgeOutput, BridgeTransportConfig, DumpFormat, DumpOutput, FeedbackObserverOutput,
    FileOutput, GamepadOutput, MockOutput,
};
use bridge_runtime::{
    BridgeEndpointSelection, BridgeRuntime, ControllerSelection, LizardMode, OutputSelection,
    PuckDockAction, RuntimeConfig, RuntimeState, StatusLogTracker,
};
use clap::Parser;
use desktop_bindings::load_store;
use recording::{ReplayOptions, ReplaySession, ReplayTiming};

mod cli;

use cli::{Cli, ControllerMode, InputMode, LizardArg, OutputArg, PuckDockArg};

fn main() {
    if let Err(error) = run() {
        eprintln!("level=error app=sc-bridge message={error:?}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    #[cfg(target_os = "macos")]
    let virtual_hid_enabled = virtual_gamepad::virtual_hid_enabled(cli.enable_virtual_hid)?;
    #[cfg(not(target_os = "macos"))]
    let virtual_hid_enabled = true;
    // Every cross-field rule is checked before a backend is built, so a bad
    // combination cannot leave a truncated output file behind.
    cli.validate(virtual_hid_enabled)?;
    match cli.input {
        InputMode::Live => run_live(&cli),
        InputMode::Replay => run_replay(&cli),
    }
}

fn run_live(cli: &Cli) -> Result<(), String> {
    let config = live_config(cli)?;
    let duration = cli.duration_secs.map(Duration::from_secs);
    let handle = BridgeRuntime::spawn(config);
    let stop = Arc::new(AtomicBool::new(false));
    let signal_stop = Arc::clone(&stop);
    ctrlc::set_handler(move || signal_stop.store(true, Ordering::Release))
        .map_err(|error| format!("cannot install Ctrl-C handler: {error}"))?;
    let started = Instant::now();
    let mut status_log = StatusLogTracker::default();
    let result = loop {
        let status = handle.status();
        for record in status_log.observe(started.elapsed(), &status) {
            eprintln!("{record}");
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

fn live_config(cli: &Cli) -> Result<RuntimeConfig, String> {
    let binding_profile = parse_binding_profile(cli)?;
    // `--controller auto` is the default; it exists only to say so out loud.
    let _ = cli.controller.unwrap_or(ControllerMode::Auto);
    let controller = cli
        .index
        .map_or(ControllerSelection::AutoActive, ControllerSelection::Index);
    let bridge_endpoint = match cli.port.as_deref() {
        None | Some("auto") => BridgeEndpointSelection::AutoBridgeDevice,
        Some(path) => BridgeEndpointSelection::SerialPort(path.to_owned()),
    };
    let output = live_output(cli)?;
    let lizard_mode = match cli.lizard_mode {
        LizardArg::Suppress => LizardMode::Suppress,
        LizardArg::Leave => LizardMode::Leave,
    };
    // `validate` has already rejected these alongside a non-serial output, so
    // reaching here with either set means the output is serial.
    let idle_shutdown_timeout = cli.idle_shutdown.map_or_else(
        || RuntimeConfig::default().idle_shutdown_timeout,
        cli::IdleShutdown::timeout,
    );
    let puck_dock_action = match cli.puck_dock_action {
        Some(PuckDockArg::PowerOff) => PuckDockAction::PowerOff,
        Some(PuckDockArg::Leave) | None => PuckDockAction::LeaveOn,
    };
    Ok(RuntimeConfig {
        controller,
        bridge_endpoint,
        output,
        lizard_mode,
        bridge: BridgeConfig {
            input_timeout: Duration::from_millis(cli.input_timeout_ms),
            decode_failure_limit: cli.decode_failure_limit,
        },
        bridge_transport_config: BridgeTransportConfig {
            packet_logging: cli.serial_log,
            ..BridgeTransportConfig::default()
        },
        serial_baud_rate: cli.baud,
        recording_path: cli.record.clone(),
        idle_shutdown_timeout,
        puck_dock_action,
        binding_profile,
        ..RuntimeConfig::default()
    })
}

fn live_output(cli: &Cli) -> Result<OutputSelection, String> {
    Ok(match cli.output() {
        OutputArg::Serial => OutputSelection::BridgeDevice,
        OutputArg::VirtualGamepad => {
            OutputSelection::VirtualGamepad(cli.virtual_hid().virtual_gamepad_config()?)
        }
        OutputArg::Dump => OutputSelection::Dump(DumpFormat::Compact),
        OutputArg::Pretty => OutputSelection::Dump(DumpFormat::Pretty),
        OutputArg::Json => OutputSelection::Dump(DumpFormat::Json),
        OutputArg::Raw => OutputSelection::Dump(DumpFormat::Raw),
        OutputArg::Mock => OutputSelection::Mock,
        OutputArg::File => OutputSelection::File(
            cli.output_file
                .clone()
                .ok_or("file output requires --output-file PATH")?,
        ),
    })
}

fn run_replay(cli: &Cli) -> Result<(), String> {
    // `validate` guarantees `--file`, so this cannot be reached without one.
    let path = cli
        .file
        .as_deref()
        .ok_or("replay input requires --file PATH")?;
    // Read the recording before opening the backend, so a bad path cannot
    // truncate an output file on its way to failing.
    let session =
        ReplaySession::read(io::BufReader::new(File::open(path).map_err(|error| {
            format!("cannot open replay '{}': {error}", path.display())
        })?))
        .map_err(|error| error.to_string())?;
    let mut output = make_replay_output(cli)?;
    let options = ReplayOptions {
        timing: if cli.deterministic {
            ReplayTiming::Immediate
        } else {
            ReplayTiming::RealTime
        },
        speed: cli.speed,
        seek_timestamp_us: cli.seek_us,
    };
    let stats = session
        .play_once(&mut *output, options)
        .map_err(|error| error.to_string())?;
    output.send_neutral().map_err(|error| error.to_string())?;
    output.service().map_err(|error| error.to_string())?;
    eprintln!(
        "level=info event=replay_complete events={} states={} ignored={}",
        stats.events_processed, stats.states_sent, stats.events_ignored
    );
    Ok(())
}

fn parse_binding_profile(cli: &Cli) -> Result<Option<desktop_bindings::BindingProfile>, String> {
    // clap's `requires` already guarantees the two arrive together.
    let (Some(path), Some(name)) = (cli.bindings.as_deref(), cli.profile.as_deref()) else {
        return Ok(None);
    };
    let store = load_store(path)?;
    store
        .profile_by_name(name)
        .cloned()
        .map(Some)
        .ok_or_else(|| {
            format!(
                "binding profile '{name}' does not exist in '{}'",
                path.display()
            )
        })
}

fn make_replay_output(cli: &Cli) -> Result<Box<dyn GamepadOutput>, String> {
    Ok(match cli.output() {
        OutputArg::Dump => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Compact)),
        OutputArg::Pretty => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Pretty)),
        OutputArg::Json => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Json)),
        OutputArg::Raw => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Raw)),
        OutputArg::Mock => Box::new(MockOutput::default()),
        OutputArg::File => Box::new(
            FileOutput::create(
                cli.output_file
                    .as_deref()
                    .ok_or("file output requires --output-file PATH")?,
            )
            .map_err(|error| error.to_string())?,
        ),
        OutputArg::Serial => {
            let port = cli
                .replay_port()
                .ok_or("serial replay requires an explicit --port PATH")?;
            Box::new(
                BridgeOutput::open_serial(
                    port,
                    cli.baud,
                    BridgeTransportConfig {
                        packet_logging: cli.serial_log,
                        ..BridgeTransportConfig::default()
                    },
                )
                .map_err(|error| error.to_string())?,
            )
        }
        OutputArg::VirtualGamepad => Box::new(FeedbackObserverOutput::new(
            cli.virtual_hid().open_virtual_gamepad()?,
            |feedback| eprintln!("level=info event=output_{feedback}"),
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses, validates, then maps to a runtime config - the whole path a
    /// real invocation takes, so these stay end-to-end rather than testing the
    /// mapping against a hand-built `Cli`.
    fn live(values: &[&str]) -> Result<RuntimeConfig, String> {
        let cli = Cli::try_parse_from(std::iter::once("sc-bridge").chain(values.iter().copied()))
            .map_err(|error| error.to_string())?;
        cli.validate(cli.enable_virtual_hid)?;
        live_config(&cli)
    }

    fn replay(values: &[&str]) -> Result<(), String> {
        let cli = Cli::try_parse_from(std::iter::once("sc-bridge").chain(values.iter().copied()))
            .map_err(|error| error.to_string())?;
        cli.validate(cli.enable_virtual_hid)?;
        run_replay(&cli)
    }

    #[test]
    fn zero_arguments_select_zero_configuration_live_bridge() {
        let config = live(&[]).unwrap();
        assert_eq!(config.controller, ControllerSelection::AutoActive);
        assert_eq!(
            config.bridge_endpoint,
            BridgeEndpointSelection::AutoBridgeDevice
        );
        assert_eq!(config.output, OutputSelection::BridgeDevice);
        assert_eq!(config.idle_shutdown_timeout, Some(Duration::from_mins(15)));
        assert_eq!(config.puck_dock_action, PuckDockAction::LeaveOn);
    }

    #[test]
    fn live_virtual_gamepad_uses_the_platform_selection() {
        let arguments: &[&str] = if cfg!(target_os = "macos") {
            &[
                "--output",
                "virtual-gamepad",
                "--enable-virtual-hid",
                "--virtual-hid-helper",
                "/tmp/helper",
            ]
        } else {
            &["--output", "virtual-gamepad"]
        };

        let config = live(arguments).unwrap();
        let OutputSelection::VirtualGamepad(gamepad_config) = config.output else {
            panic!("virtual-gamepad did not map to the canonical runtime selection");
        };
        assert_eq!(
            gamepad_config.backend,
            virtual_gamepad::VirtualGamepadBackendKind::Automatic
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            gamepad_config.macos_helper_path.as_deref(),
            Some(std::path::Path::new("/tmp/helper"))
        );
        #[cfg(not(target_os = "macos"))]
        assert_eq!(
            gamepad_config,
            virtual_gamepad::VirtualGamepadConfig::default()
        );
    }

    #[test]
    fn explicit_controller_and_port_override_discovery() {
        let config = live(&["--index", "43", "--port", "/dev/cu.test"]).unwrap();
        assert_eq!(config.controller, ControllerSelection::Index(43));
        assert_eq!(
            config.bridge_endpoint,
            BridgeEndpointSelection::SerialPort("/dev/cu.test".to_owned())
        );
    }

    #[test]
    fn explicit_auto_forms_match_defaults() {
        let config = live(&["--controller", "auto", "--port", "auto"]).unwrap();
        assert_eq!(config.controller, ControllerSelection::AutoActive);
        assert_eq!(
            config.bridge_endpoint,
            BridgeEndpointSelection::AutoBridgeDevice
        );
    }

    #[test]
    fn automatic_shutdown_options_parse_independently() {
        let config = live(&["--idle-shutdown", "5", "--puck-dock-action", "power-off"]).unwrap();
        assert_eq!(config.idle_shutdown_timeout, Some(Duration::from_mins(5)));
        assert_eq!(config.puck_dock_action, PuckDockAction::PowerOff);

        let never = live(&["--idle-shutdown", "never"]).unwrap();
        assert_eq!(never.idle_shutdown_timeout, None);
        assert_eq!(never.puck_dock_action, PuckDockAction::LeaveOn);
    }

    #[test]
    fn automatic_shutdown_rejects_invalid_values() {
        for value in ["0", "1441", "soon"] {
            assert!(live(&["--idle-shutdown", value]).is_err());
        }
        assert!(live(&["--puck-dock-action", "maybe"]).is_err());
    }

    #[test]
    fn replay_serial_requires_an_explicit_port() {
        assert!(replay(&["--input", "replay", "--file", "x", "--output", "serial"]).is_err());
    }

    #[test]
    fn desktop_binding_options_are_paired_and_live_only() {
        assert!(live(&["--bindings", "/tmp/missing"]).is_err());
        assert!(live(&["--profile", "Default"]).is_err());
        assert!(replay(&[
            "--input",
            "replay",
            "--file",
            "/tmp/ignored.jsonl",
            "--bindings",
            "/tmp/ignored",
            "--profile",
            "Default"
        ])
        .unwrap_err()
        .contains("never inject desktop input"));
    }

    #[test]
    fn live_binding_profile_loads_by_display_name() {
        let path =
            std::env::temp_dir().join(format!("sc-bridge-{}-bindings.json", std::process::id()));
        let mut store = desktop_bindings::BindingStore::default();
        store.profiles[0].bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::default(),
        });
        desktop_bindings::save_store(&path, &store).unwrap();
        let config = live(&["--bindings", path.to_str().unwrap(), "--profile", "Default"]).unwrap();
        assert_eq!(config.binding_profile.unwrap().configured_output_count(), 1);
        let _ = std::fs::remove_file(path);
    }
}
