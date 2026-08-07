//! Command-line surface.

use clap::Parser;

use crate::demo::DemoState;

/// Live view of a Steam Controller 2: decoded input, mapped output and
/// diagnostics.
#[derive(Debug, Clone, Parser)]
#[command(name = "sc-visualizer", version, about, long_about = None)]
pub(crate) struct Cli {
    /// Open a specific HID collection from `sc-probe list` instead of
    /// discovering one automatically.
    #[arg(long, value_name = "N", conflicts_with = "demo_state")]
    pub(crate) index: Option<usize>,

    /// Show a fixed state instead of opening a device, for visual checks.
    #[arg(long, value_name = "STATE", value_enum)]
    pub(crate) demo_state: Option<DemoState>,
}

impl Cli {
    /// How the app should get its input.
    pub(crate) fn source(&self) -> Source {
        match (self.demo_state, self.index) {
            (Some(mode), _) => Source::Demo(mode),
            (None, Some(index)) => Source::Collection(index),
            (None, None) => Source::Discover,
        }
    }
}

/// Where a run gets its controller state from. Exactly one applies, which is
/// what stops a demo run from opening hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// Find a supported controller, and keep looking until one appears.
    Discover,
    /// Open exactly this collection.
    Collection(usize),
    /// Open nothing; render a fixed state.
    Demo(DemoState),
}

#[cfg(test)]
mod tests {
    use super::{Cli, Source};
    use crate::demo::DemoState;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("sc-visualizer").chain(args.iter().copied()))
            .expect("these arguments should parse")
    }

    #[test]
    fn no_arguments_discovers_a_controller() {
        assert_eq!(parse(&[]).source(), Source::Discover);
    }

    #[test]
    fn an_explicit_index_wins_over_discovery() {
        assert_eq!(parse(&["--index", "43"]).source(), Source::Collection(43));
        // clap accepts the `=` spelling the hand-rolled parser never did.
        assert_eq!(parse(&["--index=43"]).source(), Source::Collection(43));
    }

    #[test]
    fn a_demo_state_opens_no_device() {
        assert_eq!(
            parse(&["--demo-state", "analog"]).source(),
            Source::Demo(DemoState::Analog)
        );
    }

    /// The old parser printed "ignoring --index" and then opened the collection
    /// anyway, taking the HID ownership lock during what was meant to be an
    /// offline run. Now the combination cannot be expressed at all.
    #[test]
    fn a_demo_state_and_an_index_together_are_rejected() {
        let error =
            Cli::try_parse_from(["sc-visualizer", "--demo-state", "analog", "--index", "1"])
                .expect_err("the two are mutually exclusive");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    /// Previously `--index abc` silently opened collection 0.
    #[test]
    fn a_non_numeric_index_is_an_error_rather_than_a_silent_zero() {
        let error = Cli::try_parse_from(["sc-visualizer", "--index", "abc"])
            .expect_err("an index must be a number");
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn an_unknown_demo_state_is_rejected_with_the_choices() {
        let error = Cli::try_parse_from(["sc-visualizer", "--demo-state", "nonsense"])
            .expect_err("only the four states are valid");
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidValue);
        let rendered = error.to_string();
        for state in ["neutral", "digital", "analog", "disconnected"] {
            assert!(rendered.contains(state), "{state} should be offered");
        }
    }

    /// The old parser silently ignored anything it did not recognize.
    #[test]
    fn an_unknown_flag_is_reported_rather_than_ignored() {
        let error = Cli::try_parse_from(["sc-visualizer", "--typo"])
            .expect_err("unknown flags should not be silently dropped");
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    /// The app had no `--help` at all; asking for it launched the GUI.
    #[test]
    fn help_is_available() {
        let error =
            Cli::try_parse_from(["sc-visualizer", "--help"]).expect_err("help short-circuits");
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
    }
}
