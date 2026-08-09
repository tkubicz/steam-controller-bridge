mod analysis;
mod capture;
mod compare;
mod metrics;
mod replay;
mod trace;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "sc-lizard-lab",
    version,
    about = "Capture and compare Steam Controller lizard-mode mouse behavior"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Capture raw controller and passive macOS pointer events.
    Capture {
        /// Override automatic active-controller detection with a global HID index.
        #[arg(long, value_name = "N")]
        index: Option<usize>,
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        #[arg(long, conflicts_with = "duration_secs")]
        guided: bool,
        #[arg(long, value_name = "N", conflicts_with = "guided")]
        duration_secs: Option<u64>,
    },
    /// Measure a lizard-mode capture without running the bridge algorithm.
    Analyze {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(long, value_name = "REPORT.json")]
        output: PathBuf,
    },
    /// Replay controller states through the current mouse-only `BindingEngine`.
    Compare {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(long, value_name = "REPORT.json")]
        output: PathBuf,
        #[arg(long, value_name = "PATH", requires = "profile_name")]
        profile: Option<PathBuf>,
        #[arg(long, value_name = "NAME", requires = "profile")]
        profile_name: Option<String>,
    },
    /// Replay reference or bridge motion; textual dump is the safe default.
    Replay {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(long, value_enum, default_value_t = ReplaySource::Reference)]
        source: ReplaySource,
        #[arg(long, value_enum, default_value_t = ReplayOutput::Dump)]
        output: ReplayOutput,
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ReplaySource {
    Reference,
    Bridge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ReplayOutput {
    Dump,
    Desktop,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("sc-lizard-lab: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    match Cli::parse().command {
        Command::Capture {
            index,
            output,
            guided,
            duration_secs,
        } => capture::run(index, &output, guided, duration_secs),
        Command::Analyze { input, output } => {
            let trace = trace::Trace::read(&input)?;
            analysis::write_report(&trace, &output)
        }
        Command::Compare {
            input,
            output,
            profile,
            profile_name,
        } => {
            let trace = trace::Trace::read(&input)?;
            compare::write_report(&trace, &output, profile.as_deref(), profile_name.as_deref())
        }
        Command::Replay {
            input,
            source,
            output,
            speed,
        } => {
            let trace = trace::Trace::read(&input)?;
            replay::run(&trace, source, output, speed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_auto_detects_when_index_is_omitted() {
        let cli = Cli::try_parse_from([
            "sc-lizard-lab",
            "capture",
            "--output",
            "capture.jsonl",
            "--guided",
        ])
        .unwrap();
        let Command::Capture { index, .. } = cli.command else {
            panic!("expected capture command");
        };
        assert_eq!(index, None);
    }

    #[test]
    fn capture_accepts_an_explicit_index_override() {
        let cli = Cli::try_parse_from([
            "sc-lizard-lab",
            "capture",
            "--index",
            "43",
            "--output",
            "capture.jsonl",
        ])
        .unwrap();
        let Command::Capture { index, .. } = cli.command else {
            panic!("expected capture command");
        };
        assert_eq!(index, Some(43));
    }
}
