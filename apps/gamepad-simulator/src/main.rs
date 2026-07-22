use std::env;
use std::io::{self, BufRead};
use std::thread;
use std::time::Duration;

use bridge_output::{DumpFormat, DumpOutput, FileOutput, GamepadOutput, MockOutput};
use gamepad_simulator::{apply_keyboard_command, automated_sequence};
use gamepad_state::GamepadState;

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
    let mut output = make_output(output_name, value_after(&args, "--file"))?;
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
                        thread::sleep(Duration::from_millis(interval_ms));
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
    for line in io::stdin().lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
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

fn make_output(name: &str, file: Option<&str>) -> Result<Box<dyn GamepadOutput>, String> {
    match name {
        "dump" | "compact" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Compact))),
        "pretty" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Pretty))),
        "json" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Json))),
        "raw" => Ok(Box::new(DumpOutput::new(io::stdout(), DumpFormat::Raw))),
        "file" => Ok(Box::new(
            FileOutput::create(file.ok_or("--output file requires --file PATH")?)
                .map_err(|error| error.to_string())?,
        )),
        "mock" => Ok(Box::new(MockOutput::default())),
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
    println!("gamepad-simulator <keyboard|automated> [options]\n\nOptions:\n  --output <dump|pretty|json|raw|file|mock>\n  --file PATH              Required by file output\n  --cycles N               Automated cycles (default: 1)\n  --interval-ms N          Delay between states (default: 50)\n  -h, --help");
}
