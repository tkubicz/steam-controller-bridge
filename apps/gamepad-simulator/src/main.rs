use std::fs::File;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use bridge_output::{
    DumpFormat, DumpOutput, FileOutput, GamepadOutput, MockOutput, SerialConfig, SerialOutput,
};
use clap::{Parser, ValueEnum};
use gamepad_simulator::{apply_keyboard_command, automated_sequence};
use gamepad_state::{Button, GamepadState};
use macos_virtual_hid::{parse_usb_id, VirtualHidConfig, VirtualHidOutput};
use recording::RecordingOutput;

/// Drives any output backend with synthetic gamepad states.
#[derive(Debug, Clone, Parser)]
#[command(name = "gamepad-simulator", version, about, long_about = None)]
struct Cli {
    /// Type control names on stdin, or run a fixed sequence.
    #[arg(value_enum)]
    mode: Mode,

    /// Output backend.
    #[arg(long, value_enum, default_value_t = OutputArg::Dump)]
    output: OutputArg,

    /// Required by `--output file` and `--output recording`. Note this app
    /// spells it `--file`, where sc-bridge and sc-replay use `--output-file`.
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,

    /// Required by `--output serial`.
    #[arg(long, value_name = "PATH")]
    port: Option<String>,

    /// Serial baud rate.
    #[arg(long, value_name = "N", default_value_t = 115_200)]
    baud: u32,

    /// Log serial frame bytes.
    #[arg(long)]
    serial_log: bool,

    /// Rust `IOHIDUserDevice` helper executable; required by `--output virtual-hid`.
    /// Example: gamepad-simulator automated --output virtual-hid --virtual-hid-helper ./sc-virtual-hid-helper
    #[arg(long, value_name = "PATH")]
    virtual_hid_helper: Option<PathBuf>,

    /// Override the virtual controller vendor ID (decimal or 0x-prefixed hex).
    #[arg(
        long,
        value_name = "VID",
        value_parser = parse_usb_id,
        requires = "virtual_hid_product_id"
    )]
    virtual_hid_vendor_id: Option<u16>,

    /// Override the virtual controller product ID (decimal or 0x-prefixed hex).
    #[arg(
        long,
        value_name = "PID",
        value_parser = parse_usb_id,
        requires = "virtual_hid_vendor_id"
    )]
    virtual_hid_product_id: Option<u16>,

    /// Automated cycles.
    #[arg(long, value_name = "N", default_value_t = 1)]
    cycles: u32,

    /// Delay between states; 0 sends as fast as the backend allows.
    #[arg(long, value_name = "N", default_value_t = 50)]
    interval_ms: u64,

    /// Include the Guide/Steam system button in automated mode. On macOS this
    /// can invoke the Games app, so automated runs omit it by default.
    #[arg(long)]
    include_guide_button: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    Keyboard,
    Automated,
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
    /// Writes a replayable JSONL recording.
    Recording,
    Mock,
    Serial,
    VirtualHid,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gamepad-simulator: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    validate_cli(&cli)?;
    // The mode is validated by clap before this point, so an unknown mode can
    // no longer create or truncate an output file on its way to failing.
    let mut output = make_output(&cli)?;
    match cli.mode {
        Mode::Automated => {
            let interval_ms = cli.interval_ms;
            for _ in 0..cli.cycles {
                for state in automated_states(cli.include_guide_button) {
                    output
                        .send_state(&state)
                        .map_err(|error| error.to_string())?;
                    if interval_ms > 0 {
                        service_delay(&mut *output, Duration::from_millis(interval_ms))?;
                    }
                }
            }
            output.send_neutral().map_err(|error| error.to_string())?;
        }
        Mode::Keyboard => keyboard_mode(&mut *output)?,
    }
    Ok(())
}

fn automated_states(include_guide_button: bool) -> Vec<GamepadState> {
    let mut states = automated_sequence(32);
    if !include_guide_button {
        for state in &mut states {
            state.buttons.set(Button::Guide, false);
        }
    }
    states
}

fn validate_cli(cli: &Cli) -> Result<(), String> {
    match cli.output {
        OutputArg::VirtualHid if cli.virtual_hid_helper.is_none() => {
            Err("--output virtual-hid requires --virtual-hid-helper PATH".to_owned())
        }
        OutputArg::VirtualHid => Ok(()),
        _ if cli.virtual_hid_helper.is_some() => {
            Err("--virtual-hid-helper is only valid with --output virtual-hid".to_owned())
        }
        _ if cli.virtual_hid_vendor_id.is_some() || cli.virtual_hid_product_id.is_some() => Err(
            "virtual HID identity overrides are only valid with --output virtual-hid".to_owned(),
        ),
        _ => Ok(()),
    }
}

fn keyboard_mode(output: &mut dyn GamepadOutput) -> Result<(), String> {
    eprintln!("Enter a control name (w/a/s/d, up/left/down/right, q/e, i/j/k/l, space, 1-9, r, exit). Each line is one state.");
    let mut state = GamepadState::neutral();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        let line = match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok(line) => line.map_err(|error| error.to_string())?,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                output.service().map_err(|error| error.to_string())?;
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match apply_keyboard_command(&mut state, &line) {
            Ok(true) => output
                .send_state(&state)
                .map_err(|error| error.to_string())?,
            Ok(false) => break,
            Err(error) => eprintln!("{error}"),
        }
    }
    output.send_neutral().map_err(|error| error.to_string())
}

fn service_delay(output: &mut dyn GamepadOutput, duration: Duration) -> Result<(), String> {
    let deadline = Instant::now() + duration;
    let mut next_service = Instant::now();
    loop {
        output.service().map_err(|error| error.to_string())?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        next_service += Duration::from_millis(25);
        let until_service = next_service.saturating_duration_since(Instant::now());
        thread::sleep(remaining.min(until_service));
    }
}

fn make_output(cli: &Cli) -> Result<Box<dyn GamepadOutput>, String> {
    let file = cli.file.as_deref();
    Ok(match cli.output {
        OutputArg::Dump => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Compact)),
        OutputArg::Pretty => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Pretty)),
        OutputArg::Json => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Json)),
        OutputArg::Raw => Box::new(DumpOutput::new(io::stdout(), DumpFormat::Raw)),
        OutputArg::Mock => Box::new(MockOutput::default()),
        OutputArg::File => Box::new(
            FileOutput::create(file.ok_or("--output file requires --file PATH")?)
                .map_err(|error| error.to_string())?,
        ),
        OutputArg::Recording => Box::new(RecordingOutput::new(
            File::create(file.ok_or("--output recording requires --file PATH")?)
                .map_err(|error| error.to_string())?,
        )),
        OutputArg::Serial => Box::new(
            SerialOutput::open(
                cli.port
                    .as_deref()
                    .ok_or("--output serial requires --port PATH")?,
                cli.baud,
                SerialConfig {
                    packet_logging: cli.serial_log,
                    ..SerialConfig::default()
                },
            )
            .map_err(|error| error.to_string())?,
        ),
        OutputArg::VirtualHid => {
            let mut config = VirtualHidConfig::new(
                cli.virtual_hid_helper
                    .clone()
                    .ok_or("--output virtual-hid requires --virtual-hid-helper PATH")?,
            );
            if let (Some(vendor_id), Some(product_id)) =
                (cli.virtual_hid_vendor_id, cli.virtual_hid_product_id)
            {
                config = config.with_identity(vendor_id, product_id);
            }
            let output = VirtualHidOutput::open(config).map_err(|error| error.to_string())?;
            let metadata = output.helper_metadata();
            eprintln!(
                "level=info event=virtual_hid_ready vendor_id={:04x} product_id={:04x} protocol={} dry_run={}",
                metadata.vendor_id,
                metadata.product_id,
                metadata.protocol_version,
                metadata.dry_run
            );
            Box::new(output)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_override_is_paired_and_requires_virtual_hid_output() {
        assert!(Cli::try_parse_from([
            "gamepad-simulator",
            "automated",
            "--virtual-hid-vendor-id",
            "0xcafe",
        ])
        .is_err());

        let cli = Cli::try_parse_from([
            "gamepad-simulator",
            "automated",
            "--virtual-hid-vendor-id",
            "0xcafe",
            "--virtual-hid-product-id",
            "0x4001",
        ])
        .unwrap();
        assert!(validate_cli(&cli).is_err());

        let cli = Cli::try_parse_from([
            "gamepad-simulator",
            "automated",
            "--output",
            "virtual-hid",
            "--virtual-hid-helper",
            "/tmp/helper",
            "--virtual-hid-vendor-id",
            "0xcafe",
            "--virtual-hid-product-id",
            "16385",
        ])
        .unwrap();
        assert_eq!(cli.virtual_hid_vendor_id, Some(0xcafe));
        assert_eq!(cli.virtual_hid_product_id, Some(0x4001));
        assert!(validate_cli(&cli).is_ok());
    }

    #[test]
    fn automated_runs_omit_the_system_guide_button_by_default() {
        let default = Cli::try_parse_from(["gamepad-simulator", "automated"]).unwrap();
        assert!(!default.include_guide_button);
        let explicit =
            Cli::try_parse_from(["gamepad-simulator", "automated", "--include-guide-button"])
                .unwrap();
        assert!(explicit.include_guide_button);
        assert!(automated_states(false)
            .iter()
            .all(|state| !state.buttons.contains(Button::Guide)));
        assert!(automated_states(true)
            .iter()
            .any(|state| state.buttons.contains(Button::Guide)));
    }
}
