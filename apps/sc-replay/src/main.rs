use std::fs::File;
use std::io::{self, BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use bridge_output::{
    BridgeOutput, BridgeTransportConfig, DumpFormat, DumpOutput, FileOutput, GamepadOutput,
    MockOutput,
};
use clap::{Parser, ValueEnum};
use recording::{ReplayOptions, ReplaySession, ReplayTiming, KIND_MAPPED_GAMEPAD_STATE};
use virtual_gamepad::{parse_usb_id, VirtualHidOptions};

/// Replays a recorded session through any output backend.
#[derive(Debug, Clone, Parser)]
#[command(name = "sc-replay", version, about, long_about = None)]
// Four independent switches is what this CLI has; grouping them into a
// sub-struct to satisfy the lint would only make the flags harder to find.
#[allow(clippy::struct_excessive_bools)]
struct Cli {
    /// Recording to replay.
    #[arg(value_name = "RECORDING")]
    recording: PathBuf,

    /// Playback speed.
    #[arg(long, value_name = "N", default_value_t = 1.0)]
    speed: f64,

    /// Ignore recorded timing.
    #[arg(long)]
    deterministic: bool,

    /// Advance mapped states with Enter. Takes precedence over --loop.
    #[arg(long)]
    step: bool,

    /// Replay repeatedly.
    #[arg(long = "loop")]
    repeat: bool,

    /// Start at or after this timestamp.
    #[arg(long, value_name = "N", default_value_t = 0)]
    seek_us: u64,

    /// Output backend.
    #[arg(long, value_enum, default_value_t = OutputArg::Dump)]
    output: OutputArg,

    /// Required by `--output file`.
    #[arg(long, value_name = "PATH")]
    output_file: Option<PathBuf>,

    /// Required by `--output serial`.
    #[arg(long, value_name = "PATH")]
    port: Option<String>,

    /// Serial baud rate.
    #[arg(long, value_name = "N", default_value_t = 115_200)]
    baud: u32,

    /// Log serial frame bytes.
    #[arg(long)]
    serial_log: bool,

    /// Rust `IOHIDUserDevice` helper executable; required by virtual-gamepad output.
    #[arg(long, value_name = "PATH")]
    virtual_hid_helper: Option<PathBuf>,

    /// Override the virtual controller vendor ID (decimal or 0x-prefixed hex).
    #[arg(long, value_name = "VID", value_parser = parse_usb_id, requires = "virtual_hid_product_id")]
    virtual_hid_vendor_id: Option<u16>,

    /// Override the virtual controller product ID (decimal or 0x-prefixed hex).
    #[arg(long, value_name = "PID", value_parser = parse_usb_id, requires = "virtual_hid_vendor_id")]
    virtual_hid_product_id: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputArg {
    /// `compact` remains accepted as a long-standing synonym.
    #[value(alias = "compact")]
    Dump,
    Pretty,
    Json,
    Raw,
    File,
    Mock,
    Serial,
    #[value(name = "virtual-gamepad", alias = "virtual-hid")]
    VirtualHid,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("sc-replay: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    validate_cli(&cli)?;
    let path = &cli.recording;
    let session = ReplaySession::read(BufReader::new(
        File::open(path).map_err(|error| format!("cannot open '{}': {error}", path.display()))?,
    ))
    .map_err(|error| error.to_string())?;
    let mut output = make_output(&cli)?;
    let seek_timestamp_us = cli.seek_us;

    if cli.step {
        play_step(&session, &mut *output, seek_timestamp_us)?;
        return Ok(());
    }

    let repeat = cli.repeat;
    let options = ReplayOptions {
        timing: if cli.deterministic {
            ReplayTiming::Immediate
        } else {
            ReplayTiming::RealTime
        },
        speed: cli.speed,
        seek_timestamp_us,
    };
    loop {
        let stats = session
            .play_once(&mut *output, options)
            .map_err(|error| error.to_string())?;
        eprintln!(
            "processed {} events, sent {} states, ignored {} events",
            stats.events_processed, stats.states_sent, stats.events_ignored
        );
        if !repeat {
            break;
        }
    }
    output.send_neutral().map_err(|error| error.to_string())
}

fn validate_cli(cli: &Cli) -> Result<(), String> {
    if !cli.speed.is_finite() || cli.speed <= 0.0 {
        return Err("--speed must be a finite number greater than zero".to_owned());
    }
    match cli.output {
        OutputArg::File if cli.output_file.is_none() => {
            Err("--output file requires --output-file PATH".to_owned())
        }
        OutputArg::Serial if cli.port.is_none() => {
            Err("--output serial requires --port PATH".to_owned())
        }
        OutputArg::Serial if cli.baud == 0 => Err("--baud must be greater than zero".to_owned()),
        _ => virtual_hid_options(cli).validate(cli.output == OutputArg::VirtualHid),
    }
}

fn virtual_hid_options(cli: &Cli) -> VirtualHidOptions {
    VirtualHidOptions {
        helper_path: cli.virtual_hid_helper.clone(),
        vendor_id: cli.virtual_hid_vendor_id,
        product_id: cli.virtual_hid_product_id,
    }
}

fn play_step(
    session: &ReplaySession,
    output: &mut dyn GamepadOutput,
    seek_timestamp_us: u64,
) -> Result<(), String> {
    for event in &session.events()[session.seek_index(seek_timestamp_us)..] {
        if event.kind != KIND_MAPPED_GAMEPAD_STATE {
            continue;
        }
        eprint!(
            "{} us: press Enter to send, or q then Enter to stop > ",
            event.timestamp_us
        );
        io::stderr().flush().map_err(|error| error.to_string())?;
        let line = read_line_with_service(output)?;
        if line.trim().eq_ignore_ascii_case("q") {
            break;
        }
        output
            .send_state(
                &event
                    .decode_gamepad_state()
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
    }
    output.send_neutral().map_err(|error| error.to_string())
}

fn read_line_with_service(output: &mut dyn GamepadOutput) -> Result<String, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut line = String::new();
        let result = io::stdin().read_line(&mut line).map(|_| line);
        let _ = sender.send(result);
    });
    loop {
        match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(result) => return result.map_err(|error| error.to_string()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                output.service().map_err(|error| error.to_string())?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("stdin reader stopped".to_owned());
            }
        }
    }
}

fn make_output(cli: &Cli) -> Result<Box<dyn GamepadOutput>, String> {
    Ok(match cli.output {
        OutputArg::Dump => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Compact)),
        OutputArg::Pretty => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Pretty)),
        OutputArg::Json => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Json)),
        OutputArg::Raw => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Raw)),
        OutputArg::Mock => Box::new(MockOutput::default()),
        OutputArg::File => Box::new(
            FileOutput::create(
                cli.output_file
                    .as_deref()
                    .ok_or("--output file requires --output-file PATH")?,
            )
            .map_err(|error| error.to_string())?,
        ),
        OutputArg::Serial => Box::new(
            BridgeOutput::open_serial(
                cli.port
                    .as_deref()
                    .ok_or("--output serial requires --port PATH")?,
                cli.baud,
                BridgeTransportConfig {
                    packet_logging: cli.serial_log,
                    ..BridgeTransportConfig::default()
                },
            )
            .map_err(|error| error.to_string())?,
        ),
        OutputArg::VirtualHid => Box::new(virtual_hid_options(cli).open()?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Cli {
        Cli::try_parse_from(arguments).expect("valid CLI syntax")
    }

    #[test]
    fn output_specific_arguments_are_validated_before_playback() {
        assert!(validate_cli(&parse(&["sc-replay", "recording.jsonl"])).is_ok());
        assert!(validate_cli(&parse(&[
            "sc-replay",
            "recording.jsonl",
            "--output",
            "file"
        ]))
        .is_err());
        assert!(validate_cli(&parse(&[
            "sc-replay",
            "recording.jsonl",
            "--output",
            "serial"
        ]))
        .is_err());
        assert!(validate_cli(&parse(&[
            "sc-replay",
            "recording.jsonl",
            "--output",
            "serial",
            "--port",
            "/dev/null",
            "--baud",
            "0"
        ]))
        .is_err());
    }

    #[test]
    fn speed_must_be_finite_and_positive() {
        for value in ["0", "NaN", "inf"] {
            let cli = parse(&["sc-replay", "recording.jsonl", "--speed", value]);
            assert!(validate_cli(&cli).is_err(), "accepted speed {value}");
        }
        let negative = parse(&["sc-replay", "recording.jsonl", "--speed=-1"]);
        assert!(validate_cli(&negative).is_err());
        assert!(validate_cli(&parse(&["sc-replay", "recording.jsonl", "--speed", "2.5"])).is_ok());
    }

    #[test]
    fn compact_remains_an_alias_for_dump() {
        let cli = parse(&["sc-replay", "recording.jsonl", "--output", "compact"]);
        assert_eq!(cli.output, OutputArg::Dump);
    }

    #[test]
    fn identity_override_is_paired_and_requires_virtual_hid_output() {
        assert_eq!(
            OutputArg::from_str("virtual-hid", false).unwrap(),
            OutputArg::VirtualHid
        );
        assert!(Cli::try_parse_from([
            "sc-replay",
            "recording.jsonl",
            "--virtual-hid-vendor-id",
            "0xcafe",
        ])
        .is_err());
        let cli = parse(&[
            "sc-replay",
            "recording.jsonl",
            "--virtual-hid-vendor-id",
            "0xcafe",
            "--virtual-hid-product-id",
            "0x4001",
        ]);
        assert!(validate_cli(&cli).is_err());
        let cli = parse(&[
            "sc-replay",
            "recording.jsonl",
            "--output",
            "virtual-gamepad",
            "--virtual-hid-helper",
            "/tmp/helper",
            "--virtual-hid-vendor-id",
            "0xcafe",
            "--virtual-hid-product-id",
            "16385",
        ]);
        assert_eq!(cli.virtual_hid_vendor_id, Some(0xcafe));
        assert_eq!(cli.virtual_hid_product_id, Some(0x4001));
        assert!(validate_cli(&cli).is_ok());
    }
}
