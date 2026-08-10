//! Lizard-mode mouse capture, analysis, comparison, and safe replay.

mod analysis;
mod capture;
mod compare;
mod metrics;
mod protocol;
mod replay;
mod results;
mod trace;
mod ui;

pub(crate) use ui::{LabAction, LabUi};

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};

#[derive(Debug, Clone, Args)]
pub(crate) struct LizardArgs {
    #[command(subcommand)]
    pub(crate) command: LizardCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum LizardCommand {
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
        #[arg(
            long,
            default_value_t = 1.0,
            allow_hyphen_values = true,
            value_parser = parse_replay_speed
        )]
        speed: f64,
    },
}

fn parse_replay_speed(value: &str) -> Result<f64, String> {
    let speed = value
        .parse::<f64>()
        .map_err(|error| format!("invalid replay speed {value:?}: {error}"))?;
    if speed.is_finite() && speed > 0.0 {
        Ok(speed)
    } else {
        Err("replay speed must be finite and positive".to_owned())
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ReplaySource {
    Reference,
    Bridge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ReplayOutput {
    Dump,
    Desktop,
}

pub(crate) fn run(command: LizardCommand) -> Result<(), String> {
    match command {
        LizardCommand::Capture {
            index,
            output,
            guided,
            duration_secs,
        } => capture::run(index, &output, guided, duration_secs),
        LizardCommand::Analyze { input, output } => {
            let trace = trace::Trace::read(&input)?;
            analysis::write_report(&trace, &output)
        }
        LizardCommand::Compare {
            input,
            output,
            profile,
            profile_name,
        } => {
            let trace = trace::Trace::read(&input)?;
            compare::write_report(&trace, &output, profile.as_deref(), profile_name.as_deref())
        }
        LizardCommand::Replay {
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

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(path, bytes)
        .map_err(|error| format!("cannot write '{}': {error}", path.display()))
}
