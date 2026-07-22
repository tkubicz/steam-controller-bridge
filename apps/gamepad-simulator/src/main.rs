use std::env;
use std::fs::File;
use std::io::{self, BufRead};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use bridge_output::{
    DumpFormat, DumpOutput, FileOutput, GamepadOutput, MockOutput, SerialConfig, SerialOutput,
};
use gamepad_simulator::{apply_keyboard_command, automated_sequence};
use gamepad_state::GamepadState;
use recording::RecordingOutput;

fn main() {
    if let Err(error) = run() {
        eprintln!("gamepad-simulator: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }
    let mode = args[0].as_str();
    let output_name = value_after(&args, "--output").unwrap_or("dump");
    let mut output = make_output(output_name, &args)?;
    match mode {
        "automated" => {
            let interval_ms = parse_value(&args, "--interval-ms", 50_u64)?;
            let cycles = parse_value(&args, "--cycles", 1_u32)?;
            for _ in 0..cycles {
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
        "keyboard" => keyboard_mode(&mut *output)?,
        other => {
            return Err(format!(
                "unknown mode '{other}' (expected keyboard or automated)"
            ))
        }
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
    loop {
        output.service().map_err(|error| error.to_string())?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        thread::sleep(remaining.min(Duration::from_millis(25)));
    }
}

fn make_output(name: &str, args: &[String]) -> Result<Box<dyn GamepadOutput>, String> {
    let file = value_after(args, "--file");
    match name {
        "dump" | "compact" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Compact))),
        "pretty" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Pretty))),
        "json" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Json))),
        "raw" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Raw))),
        "file" => Ok(Box::new(
            FileOutput::create(file.ok_or("--output file requires --file PATH")?)
                .map_err(|error| error.to_string())?,
        )),
        "recording" => Ok(Box::new(RecordingOutput::new(
            File::create(file.ok_or("--output recording requires --file PATH")?)
                .map_err(|error| error.to_string())?,
        ))),
        "mock" => Ok(Box::new(MockOutput::default())),
        "serial" => Ok(Box::new(
            SerialOutput::open(
                value_after(args, "--port").ok_or("--output serial requires --port PATH")?,
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
    println!("gamepad-simulator <keyboard|automated> [options]\n\nOptions:\n  --output <dump|pretty|json|raw|file|recording|mock|serial>\n  --file PATH              Required by file/recording output\n  --port PATH              Required by serial output\n  --baud N                 Serial baud rate (default: 115200)\n  --serial-log             Log serial frame bytes\n  --cycles N               Automated cycles (default: 1)\n  --interval-ms N          Delay between states (default: 50)\n  -h, --help");
}
