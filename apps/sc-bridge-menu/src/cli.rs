use std::ffi::OsString;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::app_center_protocol::AppCenterPage;

pub const APP_CENTER_COMMAND: &str = "app-center";
pub const BINDINGS_EDITOR_COMMAND: &str = "bindings-editor";
pub const PROFILE_OVERLAY_COMMAND: &str = "profile-overlay";

/// Parses this process's arguments.
pub fn parse() -> Cli {
    Cli::parse_from(launch_arguments(std::env::args_os()))
}

/// Drops the process-serial-number argument `LaunchServices` can still inject
/// into a bundled launch. Rejecting it would exit the menu with no status item
/// and no window, leaving the user nothing to act on.
fn launch_arguments(arguments: impl Iterator<Item = OsString>) -> impl Iterator<Item = OsString> {
    arguments.filter(|argument| !argument.to_string_lossy().starts_with("-psn_"))
}

#[derive(Debug, Parser)]
#[command(
    name = "sc-bridge-menu",
    version,
    about = "Steam Controller Bridge menu app"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open application information, release notes, or updates.
    #[command(name = "app-center")]
    AppCenter(AppCenterArgs),

    /// Internal child process for editing profiles.
    #[command(name = "bindings-editor", hide = true)]
    BindingsEditor,

    /// Internal child process for the controller-driven profile wheel.
    #[command(name = "profile-overlay", hide = true)]
    ProfileOverlay,
}

#[derive(Debug, Args)]
pub struct AppCenterArgs {
    /// Page to open. Defaults to About, or Updates in demo mode.
    #[arg(long, value_enum)]
    pub tab: Option<AppCenterPage>,

    /// Render a safe local fixture instead of contacting update services or hardware.
    #[arg(long, value_enum, num_args = 0..=1, default_missing_value = "available")]
    pub demo: Option<DemoMode>,

    /// Firmware revision reported by the parent menu process.
    #[arg(long, default_value = "unknown", hide = true)]
    pub firmware_version: String,
}

impl AppCenterArgs {
    #[cfg(any(feature = "updater", test))]
    #[must_use]
    pub fn page(&self) -> AppCenterPage {
        self.tab.unwrap_or(if self.demo.is_some() {
            AppCenterPage::Updates
        } else {
            AppCenterPage::About
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DemoMode {
    Available,
    Current,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subcommand_launches_the_menu() {
        let cli = Cli::try_parse_from(["sc-bridge-menu"]).expect("menu arguments");
        assert!(cli.command.is_none());
    }

    #[test]
    fn app_center_arguments_are_typed() {
        let cli = Cli::try_parse_from([
            "sc-bridge-menu",
            "app-center",
            "--tab",
            "changelog",
            "--firmware-version",
            "7",
        ])
        .expect("app center arguments");
        let Some(Command::AppCenter(arguments)) = cli.command else {
            panic!("expected app center command");
        };
        assert_eq!(arguments.page(), AppCenterPage::Changelog);
        assert_eq!(arguments.firmware_version, "7");
        assert_eq!(arguments.demo, None);
    }

    #[test]
    fn demo_defaults_to_available_updates() {
        let cli = Cli::try_parse_from(["sc-bridge-menu", "app-center", "--demo"])
            .expect("demo arguments");
        let Some(Command::AppCenter(arguments)) = cli.command else {
            panic!("expected app center command");
        };
        assert_eq!(arguments.page(), AppCenterPage::Updates);
        assert_eq!(arguments.demo, Some(DemoMode::Available));
    }

    /// The invocation `docs/UPDATES.md` tells reviewers to run.
    #[test]
    fn a_demo_mode_can_be_named_alongside_a_tab() {
        let cli = Cli::try_parse_from([
            "sc-bridge-menu",
            "app-center",
            "--demo",
            "current",
            "--tab",
            "changelog",
        ])
        .expect("demo arguments");
        let Some(Command::AppCenter(arguments)) = cli.command else {
            panic!("expected app center command");
        };
        assert_eq!(arguments.demo, Some(DemoMode::Current));
        assert_eq!(arguments.page(), AppCenterPage::Changelog);
    }

    #[test]
    fn an_injected_process_serial_number_still_launches_the_menu() {
        let arguments = ["sc-bridge-menu", "-psn_0_774931"]
            .into_iter()
            .map(OsString::from);
        let cli = Cli::try_parse_from(launch_arguments(arguments)).expect("menu arguments");
        assert!(cli.command.is_none());
    }

    #[test]
    fn obsolete_updater_aliases_are_rejected() {
        assert!(Cli::try_parse_from(["sc-bridge-menu", "--update-center-demo"]).is_err());
        assert!(Cli::try_parse_from(["sc-bridge-menu", "--app-center-demo"]).is_err());
    }
}
