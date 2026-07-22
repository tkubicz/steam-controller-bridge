use std::env;
use std::fs::File;
use std::io::{self, BufReader, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use bridge_output::{
    DumpFormat, DumpOutput, FileOutput, GamepadOutput, MockOutput, SerialConfig, SerialOutput,
};
use recording::{ReplayOptions, ReplaySession, ReplayTiming, KIND_MAPPED_GAMEPAD_STATE};

fn main() {
    if let Err(error) = run() {
        eprintln!("sc-replay: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    let input_path = &args[0];
    let session = ReplaySession::read(BufReader::new(
        File::open(input_path).map_err(|error| format!("cannot open '{input_path}': {error}"))?,
    ))
    .map_err(|error| error.to_string())?;
    let output_name = value_after(&args, "--output").unwrap_or("dump");
    let mut output = make_output(output_name, &args)?;
    let speed = parse_value(&args, "--speed", 1.0_f64)?;
    let seek_timestamp_us = parse_value(&args, "--seek-us", 0_u64)?;
    let deterministic = args.iter().any(|arg| arg == "--deterministic");
    let step = args.iter().any(|arg| arg == "--step");
    let repeat = args.iter().any(|arg| arg == "--loop");

    if step {
        play_step(&session, &mut *output, seek_timestamp_us)?;
        return Ok(());
    }

    let options = ReplayOptions {
        timing: if deterministic {
            ReplayTiming::Immediate
        } else {
            ReplayTiming::RealTime
        },
        speed,
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

fn make_output(name: &str, args: &[String]) -> Result<Box<dyn GamepadOutput>, String> {
    let file = value_after(args, "--output-file");
    match name {
        "dump" | "compact" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Compact))),
        "pretty" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Pretty))),
        "json" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Json))),
        "raw" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Raw))),
        "file" => Ok(Box::new(
            FileOutput::create(file.ok_or("--output file requires --output-file PATH")?)
                .map_err(|error| error.to_string())?,
        )),
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
    println!(
        "sc-replay RECORDING [options]\n\nOptions:\n  --speed N                Playback speed (default: 1.0)\n  --deterministic          Ignore recorded timing\n  --step                   Advance mapped states with Enter\n  --loop                   Replay repeatedly\n  --seek-us N              Start at or after timestamp N\n  --output <dump|pretty|json|raw|file|mock|serial>\n  --output-file PATH       Required by file output\n  --port PATH              Required by serial output\n  --baud N                 Serial baud rate (default: 115200)\n  --serial-log             Log serial frame bytes\n  -h, --help"
    );
}
