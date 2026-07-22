use std::env;
use std::fs::File;
use std::io::{self, BufReader, Write};

use bridge_output::{DumpFormat, DumpOutput, FileOutput, GamepadOutput, MockOutput};
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
    let mut output = make_output(output_name, value_after(&args, "--output-file"))?;
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
    let stdin = io::stdin();
    let mut line = String::new();
    for event in &session.events()[session.seek_index(seek_timestamp_us)..] {
        if event.kind != KIND_MAPPED_GAMEPAD_STATE {
            continue;
        }
        eprint!(
            "{} us: press Enter to send, or q then Enter to stop > ",
            event.timestamp_us
        );
        io::stderr().flush().map_err(|error| error.to_string())?;
        line.clear();
        stdin
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
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

fn make_output(name: &str, file: Option<&str>) -> Result<Box<dyn GamepadOutput>, String> {
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
        "serial" => Err("serial output belongs to the next transport phase".to_owned()),
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
        "sc-replay RECORDING [options]\n\nOptions:\n  --speed N                Playback speed (default: 1.0)\n  --deterministic          Ignore recorded timing\n  --step                   Advance mapped states with Enter\n  --loop                   Replay repeatedly\n  --seek-us N              Start at or after timestamp N\n  --output <dump|pretty|json|raw|file|mock>\n  --output-file PATH       Required by file output\n  -h, --help"
    );
}
