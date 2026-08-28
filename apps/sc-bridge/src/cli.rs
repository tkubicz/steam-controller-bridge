//! Command-line surface.
//!
//! The flag names, accepted values and defaults are exactly those the
//! hand-rolled parser accepted, including the `compact` alias for `dump` which
//! has always worked but appears in no help text. What clap changes is the
//! handling of mistakes: unknown flags and unparsable numbers are now reported
//! instead of being silently ignored or silently defaulted.

use std::path::PathBuf;
use std::time::Duration;

use bridge_runtime::MAX_IDLE_SHUTDOWN_TIMEOUT;
use clap::{Parser, ValueEnum};
use virtual_gamepad::{parse_usb_id, VirtualHidOptions};

/// Bridges a Steam Controller 2 to a protocol-compatible output device, or replays a recording.
///
/// With no arguments, waits for one active Steam Controller 2 input source
/// (Puck or Bluetooth) and the output device, then starts the serial bridge and recovers
/// after reconnects.
#[derive(Debug, Clone, Parser)]
#[command(name = "sc-bridge", version, about, long_about = None)]
pub(crate) struct Cli {
    /// Expose the entitlement-gated experimental virtual-HID backend on macOS.
    #[arg(long)]
    pub(crate) enable_virtual_hid: bool,

    /// Input mode.
    #[arg(long, value_enum, default_value_t = InputMode::Live)]
    pub(crate) input: InputMode,

    /// Explicit automatic active-source discovery. This is already the default;
    /// use --index for an explicit collection.
    #[arg(long, value_enum)]
    pub(crate) controller: Option<ControllerMode>,

    /// Select a collection from `sc-probe list` instead of discovering one.
    #[arg(long, value_name = "N")]
    pub(crate) index: Option<usize>,

    /// Automatic bridge-device discovery, or a fixed CDC port path.
    #[arg(long, value_name = "auto|PATH")]
    pub(crate) port: Option<String>,

    /// Suppress the controller's native keyboard/mouse mode.
    #[arg(long, value_enum, default_value_t = LizardArg::Suppress)]
    pub(crate) lizard_mode: LizardArg,

    /// Neutral idle timeout in minutes, or `never`. Maximum 1440.
    #[arg(long, value_name = "never|MINUTES", value_parser = parse_idle_shutdown)]
    pub(crate) idle_shutdown: Option<IdleShutdown>,

    /// Optional immediate shutdown when the controller is placed on the Puck.
    #[arg(long, value_enum)]
    pub(crate) puck_dock_action: Option<PuckDockArg>,

    /// Opt-in desktop binding profile store (live only).
    #[arg(long, value_name = "PATH", requires = "profile")]
    pub(crate) bindings: Option<PathBuf>,

    /// Binding profile name; requires --bindings (live only).
    #[arg(long, value_name = "NAME", requires = "bindings")]
    pub(crate) profile: Option<String>,

    /// Replay recording input.
    #[arg(long, value_name = "PATH")]
    pub(crate) file: Option<PathBuf>,

    /// Output backend. Live default: serial; replay default: dump.
    #[arg(long, value_enum)]
    pub(crate) output: Option<OutputArg>,

    /// Binary frame output path, required by `--output file`.
    #[arg(long, value_name = "PATH")]
    pub(crate) output_file: Option<PathBuf>,

    /// Rust `IOHIDUserDevice` helper executable; required by virtual-gamepad output on macOS.
    #[arg(long, value_name = "PATH")]
    pub(crate) virtual_hid_helper: Option<PathBuf>,

    /// Override the macOS virtual controller vendor ID (decimal or 0x-prefixed hex).
    #[arg(
        long,
        value_name = "VID",
        value_parser = parse_usb_id,
        requires = "virtual_hid_product_id"
    )]
    pub(crate) virtual_hid_vendor_id: Option<u16>,

    /// Override the macOS virtual controller product ID (decimal or 0x-prefixed hex).
    #[arg(
        long,
        value_name = "PID",
        value_parser = parse_usb_id,
        requires = "virtual_hid_vendor_id"
    )]
    pub(crate) virtual_hid_product_id: Option<u16>,

    /// Serial baud rate.
    #[arg(long, value_name = "N", default_value_t = 115_200)]
    pub(crate) baud: u32,

    /// Log serial frame bytes.
    #[arg(long)]
    pub(crate) serial_log: bool,

    /// Record the full live pipeline as JSONL.
    #[arg(long, value_name = "PATH")]
    pub(crate) record: Option<PathBuf>,

    /// Neutral timeout in milliseconds.
    #[arg(long, value_name = "N", default_value_t = 200)]
    pub(crate) input_timeout_ms: u64,

    /// Decode failures before the output goes neutral.
    #[arg(long, value_name = "N", default_value_t = 3)]
    pub(crate) decode_failure_limit: u32,

    /// Stop live mode after N seconds.
    #[arg(long, value_name = "N")]
    pub(crate) duration_secs: Option<u64>,

    /// Ignore recorded timing and replay as fast as possible.
    #[arg(long)]
    pub(crate) deterministic: bool,

    /// Replay playback speed.
    #[arg(long, value_name = "N", default_value_t = 1.0)]
    pub(crate) speed: f64,

    /// Start replay at or after this timestamp.
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub(crate) seek_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum InputMode {
    Live,
    Replay,
}

/// `auto` is the only accepted value; it exists so the intent can be written
/// down explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ControllerMode {
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum LizardArg {
    Suppress,
    Leave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum PuckDockArg {
    Leave,
    PowerOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputArg {
    Serial,
    #[value(name = "virtual-gamepad", alias = "virtual-hid")]
    VirtualGamepad,
    /// `compact` has always been accepted as a synonym and stays accepted, but
    /// it is hidden so the help text keeps offering one name per behaviour.
    #[value(alias = "compact")]
    Dump,
    Pretty,
    Json,
    Raw,
    File,
    Mock,
}

/// `--idle-shutdown never` or a whole number of minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleShutdown {
    Never,
    After(Duration),
}

impl IdleShutdown {
    pub(crate) const fn timeout(self) -> Option<Duration> {
        match self {
            Self::Never => None,
            Self::After(timeout) => Some(timeout),
        }
    }
}

fn parse_idle_shutdown(value: &str) -> Result<IdleShutdown, String> {
    if value == "never" {
        return Ok(IdleShutdown::Never);
    }
    let minutes = value.parse::<u64>().map_err(|_| {
        format!("invalid --idle-shutdown value '{value}'; expected never or MINUTES")
    })?;
    let timeout = Duration::from_secs(minutes.saturating_mul(60));
    if minutes == 0 || timeout > MAX_IDLE_SHUTDOWN_TIMEOUT {
        return Err("--idle-shutdown must be never or a whole number from 1 to 1440".to_owned());
    }
    Ok(IdleShutdown::After(timeout))
}

impl Cli {
    /// Rejects combinations clap's own rules cannot express: the ones that
    /// depend on the selected mode or on the chosen output backend.
    ///
    /// This runs before any backend is constructed, which also fixes a
    /// long-standing wart: `--input replay --output file` used to create and
    /// truncate the output file before noticing that `--file` was missing.
    pub(crate) fn validate(&self, virtual_hid_enabled: bool) -> Result<(), String> {
        if cfg!(target_os = "macos")
            && self.output() == OutputArg::VirtualGamepad
            && !virtual_hid_enabled
        {
            return Err(format!(
                "virtual HID output is experimental; pass --enable-virtual-hid or set {}=1",
                virtual_gamepad::ENABLE_VIRTUAL_HID_ENV
            ));
        }
        match self.input {
            InputMode::Live => self.validate_live(),
            InputMode::Replay => self.validate_replay(),
        }
    }

    fn validate_live(&self) -> Result<(), String> {
        if !matches!(self.output(), OutputArg::Serial | OutputArg::VirtualGamepad)
            && (self.idle_shutdown.is_some() || self.puck_dock_action.is_some())
        {
            return Err(
                "automatic controller shutdown requires live serial or virtual gamepad output"
                    .to_owned(),
            );
        }
        self.require_output_file()
    }

    fn validate_replay(&self) -> Result<(), String> {
        if self.bindings.is_some() || self.profile.is_some() {
            return Err(
                "--bindings and --profile are live-only; replay recordings never inject desktop input"
                    .to_owned(),
            );
        }
        if self.idle_shutdown.is_some() || self.puck_dock_action.is_some() {
            return Err(
                "automatic controller shutdown options require live input and are unavailable in replay mode"
                    .to_owned(),
            );
        }
        if self.file.is_none() {
            return Err("replay input requires --file PATH".to_owned());
        }
        if self.output() == OutputArg::Serial && self.replay_port().is_none() {
            return Err("serial replay requires an explicit --port PATH".to_owned());
        }
        self.require_output_file()
    }

    fn require_output_file(&self) -> Result<(), String> {
        if self.output() == OutputArg::File && self.output_file.is_none() {
            return Err("file output requires --output-file PATH".to_owned());
        }
        self.virtual_hid()
            .validate_platform(self.output() == OutputArg::VirtualGamepad)
    }

    pub(crate) fn virtual_hid(&self) -> VirtualHidOptions {
        VirtualHidOptions {
            helper_path: self.virtual_hid_helper.clone(),
            vendor_id: self.virtual_hid_vendor_id,
            product_id: self.virtual_hid_product_id,
        }
    }

    /// The backend, defaulting by mode: serial for live, dump for replay.
    pub(crate) fn output(&self) -> OutputArg {
        self.output.unwrap_or(match self.input {
            InputMode::Live => OutputArg::Serial,
            InputMode::Replay => OutputArg::Dump,
        })
    }

    /// Replay has no bridge-device discovery, so `auto` is not a port here.
    pub(crate) fn replay_port(&self) -> Option<&str> {
        self.port.as_deref().filter(|value| *value != "auto")
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, IdleShutdown, InputMode, LizardArg, OutputArg, PuckDockArg};
    use clap::Parser;
    use std::time::Duration;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("sc-bridge").chain(args.iter().copied()))
            .expect("these arguments should parse")
    }

    fn reject(args: &[&str]) -> String {
        let cli = Cli::try_parse_from(std::iter::once("sc-bridge").chain(args.iter().copied()));
        match cli {
            Ok(cli) => cli
                .validate(cli.enable_virtual_hid)
                .expect_err("should have been rejected"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn bare_invocation_keeps_every_documented_default() {
        let cli = parse(&[]);
        assert_eq!(cli.input, InputMode::Live);
        assert_eq!(cli.output(), OutputArg::Serial);
        assert_eq!(cli.lizard_mode, LizardArg::Suppress);
        assert_eq!(cli.baud, 115_200);
        assert_eq!(cli.input_timeout_ms, 200);
        assert_eq!(cli.decode_failure_limit, 3);
        assert!((cli.speed - 1.0).abs() < f64::EPSILON);
        assert_eq!(cli.seek_us, 0);
        assert!(cli.index.is_none());
        assert!(cli.port.is_none());
        assert!(cli.duration_secs.is_none());
        assert!(!cli.serial_log);
        assert!(!cli.deterministic);
        cli.validate(cli.enable_virtual_hid)
            .expect("a bare live run is valid");
    }

    #[test]
    fn replay_defaults_to_dump_and_live_defaults_to_serial() {
        assert_eq!(parse(&["--input", "replay"]).output(), OutputArg::Dump);
        assert_eq!(parse(&["--input", "live"]).output(), OutputArg::Serial);
    }

    /// Undocumented but long-accepted; dropping it would be a silent removal.
    #[test]
    fn compact_is_still_accepted_as_a_synonym_for_dump() {
        assert_eq!(parse(&["--output", "compact"]).output(), OutputArg::Dump);
    }

    #[test]
    fn every_output_backend_still_parses() {
        for (value, expected) in [
            ("serial", OutputArg::Serial),
            ("virtual-gamepad", OutputArg::VirtualGamepad),
            ("virtual-hid", OutputArg::VirtualGamepad),
            ("dump", OutputArg::Dump),
            ("pretty", OutputArg::Pretty),
            ("json", OutputArg::Json),
            ("raw", OutputArg::Raw),
            ("file", OutputArg::File),
            ("mock", OutputArg::Mock),
        ] {
            assert_eq!(parse(&["--output", value]).output(), expected, "{value}");
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn identity_override_is_paired_and_requires_virtual_gamepad_output() {
        let message = reject(&[
            "--virtual-hid-vendor-id",
            "0xcafe",
            "--virtual-hid-product-id",
            "0x4001",
        ]);
        assert!(message.contains("only valid with --output virtual-hid"));

        let message = reject(&["--virtual-hid-vendor-id", "0xcafe"]);
        assert!(message.contains("virtual-hid-product-id"));

        let cli = parse(&[
            "--enable-virtual-hid",
            "--output",
            "virtual-hid",
            "--virtual-hid-helper",
            "/tmp/helper",
            "--virtual-hid-vendor-id",
            "0xcafe",
            "--virtual-hid-product-id",
            "16385",
        ]);
        assert_eq!(cli.virtual_hid_vendor_id, Some(0xcafe));
        assert_eq!(cli.virtual_hid_product_id, Some(0x4001));
        cli.validate(cli.enable_virtual_hid).unwrap();
    }

    #[test]
    fn idle_shutdown_accepts_never_and_the_documented_range() {
        assert_eq!(
            parse(&["--idle-shutdown", "never"]).idle_shutdown,
            Some(IdleShutdown::Never)
        );
        assert_eq!(
            parse(&["--idle-shutdown", "1440"]).idle_shutdown,
            Some(IdleShutdown::After(Duration::from_hours(24)))
        );
        assert!(reject(&["--idle-shutdown", "0"]).contains("from 1 to 1440"));
        assert!(reject(&["--idle-shutdown", "1441"]).contains("from 1 to 1440"));
        assert!(reject(&["--idle-shutdown", "soon"]).contains("expected never or MINUTES"));
    }

    #[test]
    fn shutdown_options_require_a_live_output() {
        let message = reject(&["--output", "dump", "--idle-shutdown", "5"]);
        assert!(
            message.contains("requires live serial or virtual gamepad output"),
            "{message}"
        );
        let message = reject(&["--output", "mock", "--puck-dock-action", "power-off"]);
        assert!(
            message.contains("requires live serial or virtual gamepad output"),
            "{message}"
        );
        // Both live backends support controller shutdown.
        parse(&["--idle-shutdown", "5"])
            .validate(false)
            .expect("serial is the live default");
        let mut arguments = vec!["--output", "virtual-hid", "--idle-shutdown", "5"];
        if cfg!(target_os = "macos") {
            arguments.extend([
                "--enable-virtual-hid",
                "--virtual-hid-helper",
                "/tmp/helper",
            ]);
        }
        parse(&arguments)
            .validate(true)
            .expect("virtual gamepad is also a live output");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn virtual_hid_output_requires_the_explicit_runtime_opt_in() {
        let arguments = [
            "--output",
            "virtual-hid",
            "--virtual-hid-helper",
            "/tmp/sc-virtual-hid-helper",
        ];
        let message = reject(&arguments);
        assert!(message.contains("--enable-virtual-hid"), "{message}");
        assert!(
            message.contains("SC_BRIDGE_ENABLE_VIRTUAL_HID=1"),
            "{message}"
        );

        let mut enabled = vec!["--enable-virtual-hid"];
        enabled.extend(arguments);
        let cli = parse(&enabled);
        cli.validate(cli.enable_virtual_hid).unwrap();
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn virtual_gamepad_requires_no_macos_opt_in_or_helper() {
        let cli = parse(&["--output", "virtual-gamepad"]);
        cli.validate(false).unwrap();

        let message = reject(&[
            "--output",
            "virtual-gamepad",
            "--virtual-hid-helper",
            "/tmp/helper",
        ]);
        assert!(message.contains("only valid on macOS"), "{message}");
    }

    #[test]
    fn shutdown_options_are_rejected_in_replay() {
        let message = reject(&["--input", "replay", "--file", "x", "--idle-shutdown", "5"]);
        assert!(message.contains("unavailable in replay mode"), "{message}");
    }

    #[test]
    fn bindings_and_profile_must_be_supplied_together() {
        let message = reject(&["--bindings", "store.json"]);
        assert!(message.contains("--profile"), "{message}");
        let message = reject(&["--profile", "Default"]);
        assert!(message.contains("--bindings"), "{message}");
        parse(&["--bindings", "store.json", "--profile", "Default"]);
    }

    #[test]
    fn bindings_are_rejected_in_replay() {
        let message = reject(&[
            "--input",
            "replay",
            "--file",
            "x",
            "--bindings",
            "s.json",
            "--profile",
            "Default",
        ]);
        assert!(message.contains("live-only"), "{message}");
    }

    #[test]
    fn replay_requires_a_recording_and_an_explicit_serial_port() {
        assert!(reject(&["--input", "replay"]).contains("requires --file PATH"));
        let message = reject(&["--input", "replay", "--file", "x", "--output", "serial"]);
        assert!(message.contains("explicit --port PATH"), "{message}");
        // `auto` is bridge-device discovery, which replay does not do.
        let message = reject(&[
            "--input", "replay", "--file", "x", "--output", "serial", "--port", "auto",
        ]);
        assert!(message.contains("explicit --port PATH"), "{message}");
        parse(&[
            "--input",
            "replay",
            "--file",
            "x",
            "--output",
            "serial",
            "--port",
            "/dev/cu.x",
        ])
        .validate(false)
        .expect("an explicit port is enough");
    }

    #[test]
    fn file_output_requires_a_path_in_both_modes() {
        assert!(reject(&["--output", "file"]).contains("--output-file PATH"));
        let message = reject(&["--input", "replay", "--file", "x", "--output", "file"]);
        assert!(message.contains("--output-file PATH"), "{message}");
    }

    #[test]
    fn controller_accepts_only_auto() {
        assert!(parse(&["--controller", "auto"]).controller.is_some());
        let message = reject(&["--controller", "first"]);
        assert!(message.contains("auto"), "{message}");
    }

    #[test]
    fn puck_dock_action_keeps_its_two_spellings() {
        assert_eq!(
            parse(&["--puck-dock-action", "leave"]).puck_dock_action,
            Some(PuckDockArg::Leave)
        );
        assert_eq!(
            parse(&["--puck-dock-action", "power-off"]).puck_dock_action,
            Some(PuckDockArg::PowerOff)
        );
    }

    #[test]
    fn lizard_mode_keeps_its_two_spellings() {
        assert_eq!(
            parse(&["--lizard-mode", "leave"]).lizard_mode,
            LizardArg::Leave
        );
        assert_eq!(
            parse(&["--lizard-mode", "suppress"]).lizard_mode,
            LizardArg::Suppress
        );
    }

    /// The hand-rolled parser ignored anything it did not recognize, so a typo
    /// silently ran a differently-configured bridge.
    #[test]
    fn unknown_flags_are_now_reported() {
        assert!(reject(&["--typo", "value"]).contains("unexpected argument"));
    }

    /// It also silently used the default when a numeric value was unparsable.
    #[test]
    fn unparsable_numbers_are_now_reported() {
        assert!(reject(&["--baud", "fast"]).contains("invalid value"));
        assert!(reject(&["--duration-secs", "later"]).contains("invalid value"));
    }
}
