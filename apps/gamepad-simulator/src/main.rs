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
use gamepad_state::GamepadState;
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

    /// Automated cycles.
    #[arg(long, value_name = "N", default_value_t = 1)]
    cycles: u32,

    /// Delay between states; 0 sends as fast as the backend allows.
    #[arg(long, value_name = "N", default_value_t = 50)]
    interval_ms: u64,
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
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gamepad-simulator: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    // The mode is validated by clap before this point, so an unknown mode can
    // no longer create or truncate an output file on its way to failing.
    let mut output = make_output(&cli)?;
    match cli.mode {
        Mode::Automated => {
            let interval_ms = cli.interval_ms;
            for _ in 0..cli.cycles {
                for state in automated_sequence(32) {
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
    })
}
