use std::fmt::Write as _;
use std::time::{Duration, Instant};

use bridge_output::{GamepadOutput, SerialOutput};
use controller_mapper::{ControllerMapper, MapperConfig};
use eframe::egui::{self, RichText};
use gamepad_state::GamepadState;
use steam_controller_protocol::{SteamControllerDecoder, SteamControllerState};
use ui_theme::{DETAIL, MUTED_TEXT, PANEL, SURFACE};

/// Initial/fallback mapper timestep. Live reports use their device timestamps,
/// so pressure coalescing still advances smoothing by the elapsed interval.
const FRAME_TIME: f32 = 1.0 / 250.0;

/// Window sizing. The minimum is small enough to sit beside a game window and
/// still reach every control, because both columns scroll.
const DEFAULT_WINDOW: [f32; 2] = [1100.0, 760.0];
const MIN_WINDOW: [f32; 2] = [820.0, 600.0];
const SIDEBAR_DEFAULT_WIDTH: f32 = 300.0;
const SIDEBAR_MIN_WIDTH: f32 = 260.0;
const SIDEBAR_MAX_WIDTH: f32 = 360.0;

mod cli;
mod demo;
mod device;
mod diagnostics;
mod header;
mod hero;
mod mailbox;
mod readouts;
mod recording_sink;
mod sidebar;

use clap::Parser;

use cli::{Cli, Source};
use demo::DemoState;
use device::{input_worker, InputChannel};
use mailbox::InputEvent;
use readouts::{mapped_state_ui, source_state_ui, DeadZones};
use recording_sink::RecordingSession;

fn main() -> eframe::Result {
    let source = Cli::parse().source();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_WINDOW)
            .with_min_inner_size(MIN_WINDOW),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "Steam Controller Visualizer",
        options,
        Box::new(move |creation| {
            ui_theme::configure_ui(&creation.egui_ctx);
            Ok(Box::new(Visualizer::new(source)))
        }),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputChoice {
    Disabled,
    Mock,
    Serial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputState {
    Neutralized,
    Active,
}

struct SerialUiConfig {
    path: String,
    baud: String,
    packet_logging: bool,
}

struct Visualizer {
    /// `None` for a demo run: no worker, no device, no ownership lock.
    input: Option<InputChannel>,
    /// Reused drain storage; neither side of the mailbox reallocates every
    /// repaint under ordinary 250 Hz input.
    input_events: Vec<InputEvent>,
    /// Reports the device layer says it lost before we saw them. Wired for
    /// when the platform backend starts reporting it; currently always zero.
    source_report_drops: u64,
    decoder: SteamControllerDecoder,
    mapper: ControllerMapper,
    config: MapperConfig,
    source: Option<SteamControllerState>,
    mapped: GamepadState,
    connected: bool,
    device: String,
    status: String,
    raw: Vec<u8>,
    /// The report's own id, not `raw[0]`. `None` until a report arrives.
    raw_report_id: Option<u8>,
    show_raw: bool,
    report_count: u64,
    rate_report_start: u64,
    rate_started: Instant,
    report_hz: f32,
    decode_failures: u64,
    consecutive_decode_failures: u32,
    last_report_timestamp: Option<Duration>,
    last_controller_input: Option<Instant>,
    input_state: InputState,
    packets_sent: u64,
    /// The full serial metric set, kept whole so nothing is silently dropped
    /// and so it can be cleared when the backend is no longer Serial.
    serial_metrics: Option<bridge_output::SerialMetrics>,
    last_output: Option<GamepadState>,
    output: OutputChoice,
    serial: Option<SerialOutput>,
    serial_config: SerialUiConfig,
    recording: Option<RecordingSession>,
    /// Preserved while an overloaded or failed recording drains accepted
    /// events asynchronously.
    recording_stop_error: Option<String>,
    recording_started: Instant,
    recording_path: String,
    marker: String,
    smoothing_enabled: bool,
    smoothing_time_constant: f32,
}

impl Visualizer {
    fn new(source: Source) -> Self {
        let config = MapperConfig::default();
        let demo = match source {
            Source::Demo(mode) => Some(mode),
            Source::Discover | Source::Collection(_) => None,
        };
        let mut visualizer = Self {
            // A demo run starts no worker at all, so it takes no HID ownership
            // lock. The old code opened a collection and only skipped draining
            // it, which is not the same thing.
            input: (demo.is_none()).then(|| input_worker(source)),
            input_events: Vec::with_capacity(mailbox::CAPACITY),
            source_report_drops: 0,
            decoder: SteamControllerDecoder::new(),
            mapper: ControllerMapper::default(),
            config,
            source: None,
            mapped: GamepadState::neutral(),
            connected: false,
            device: match source {
                Source::Discover => "no controller yet".to_owned(),
                Source::Collection(index) => format!("HID collection {index}"),
                Source::Demo(mode) => format!("demo ({})", mode.label()),
            },
            status: match source {
                Source::Discover => "Searching for a controller…".to_owned(),
                Source::Collection(index) => format!("Opening HID collection {index}…"),
                Source::Demo(mode) => format!("Demo state: {}", mode.label()),
            },
            raw: Vec::new(),
            raw_report_id: None,
            show_raw: false,
            report_count: 0,
            rate_report_start: 0,
            rate_started: Instant::now(),
            report_hz: 0.0,
            decode_failures: 0,
            consecutive_decode_failures: 0,
            last_report_timestamp: None,
            last_controller_input: None,
            input_state: InputState::Neutralized,
            packets_sent: 0,
            serial_metrics: None,
            last_output: None,
            output: OutputChoice::Disabled,
            serial: None,
            serial_config: SerialUiConfig {
                path: "/dev/cu.usbmodem".to_owned(),
                baud: "115200".to_owned(),
                packet_logging: false,
            },
            recording: None,
            recording_stop_error: None,
            recording_started: Instant::now(),
            recording_path: "sc-visualizer.jsonl".to_owned(),
            marker: String::new(),
            smoothing_enabled: false,
            smoothing_time_constant: 0.02,
        };
        if let Some(mode) = demo {
            visualizer.apply_demo(mode);
        }
        visualizer
    }

    /// Feeds a decoded fixture through the ordinary mapper and renderer. Demo
    /// states intentionally carry no fabricated raw wire bytes.
    fn apply_demo(&mut self, mode: DemoState) {
        self.status = format!("Demo state: {}", mode.label());
        if let Some(state) = mode.state() {
            self.connected = true;
            self.device = format!("demo ({})", mode.label());
            self.mapped = self.mapper.map(&state, FRAME_TIME);
            self.raw_report_id = None;
            self.raw.clear();
            self.source = Some(state);
            self.input_state = if self.mapped == GamepadState::neutral() {
                InputState::Neutralized
            } else {
                InputState::Active
            };
        } else {
            self.connected = false;
            "demo (disconnected)".clone_into(&mut self.device);
            self.source = None;
            self.mapped = GamepadState::neutral();
            self.input_state = InputState::Neutralized;
        }
    }
}

impl eframe::App for Visualizer {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.input.is_some() {
            self.process_events();
        }
        self.check_input_timeout();
        self.service_recording();
        if self.output == OutputChoice::Serial {
            if let Some(serial) = &mut self.serial {
                if let Err(error) = serial.service() {
                    self.status = format!("Serial service failed: {error}");
                    self.serial = None;
                    self.serial_metrics = None;
                } else {
                    let metrics = serial.metrics();
                    self.packets_sent = metrics.packets_sent;
                    self.serial_metrics = Some(metrics);
                }
            }
        }
        if self.output != OutputChoice::Serial {
            // Otherwise the last session's counters linger and read as live.
            self.serial_metrics = None;
        }
        self.schedule_repaint(ui);
        self.content(ui);
    }
}

impl Visualizer {
    fn schedule_repaint(&self, ui: &egui::Ui) {
        if (self.input.is_some() && self.connected) || self.serial.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        } else if self.input.is_some() || self.recording.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        // `eframe::App` hands us the root `Ui`, not a `Context`, so the panels
        // take `show(ui, ..)`. `show_inside` is the deprecated spelling of the
        // same thing and would fail the `-D warnings` gate. egui 0.35 also
        // folded `SidePanel`/`TopBottomPanel` into one `Panel` type.
        egui::Panel::top("header")
            .frame(egui::Frame::new().fill(SURFACE).inner_margin(egui::Margin {
                left: 14,
                right: 14,
                top: 10,
                bottom: 8,
            }))
            .show(ui, |ui| self.header(ui));

        egui::Panel::left("config")
            .resizable(true)
            .default_size(SIDEBAR_DEFAULT_WIDTH)
            .size_range(SIDEBAR_MIN_WIDTH..=SIDEBAR_MAX_WIDTH)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(12, 10)),
            )
            .show(ui, |ui| {
                // Its own scroll area, so every control stays reachable at the
                // minimum window height.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.controls(ui);
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(10.0);
                        self.recording_controls(ui);
                    });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 12)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.central(ui));
            });
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        self.hero(ui);
        ui.add_space(12.0);

        egui::CollapsingHeader::new("Decoded state")
            .default_open(true)
            .show(ui, |ui| {
                if let Some(source) = &self.source {
                    source_state_ui(
                        ui,
                        source,
                        DeadZones {
                            left_stick: self.config.left_stick_dead_zone,
                            right_axis: self.config.right_axis_dead_zone,
                            right_source: self.config.right_axis_source,
                        },
                    );
                } else {
                    ui.label(
                        egui::RichText::new("No decoded controller state yet.").color(MUTED_TEXT),
                    );
                }
            });

        egui::CollapsingHeader::new("Outgoing gamepad")
            .default_open(true)
            .show(ui, |ui| mapped_state_ui(ui, &self.mapped));

        // Carries the old `show_raw` capability; closed by default, exactly as
        // that checkbox defaulted to false.
        egui::CollapsingHeader::new("Raw report")
            .default_open(self.show_raw)
            .show(ui, |ui| self.raw_report(ui));
    }

    fn raw_report(&self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(format!(
                "ID {}, {} bytes",
                self.raw_report_id
                    .map_or_else(|| "—".to_owned(), |id| format!("0x{id:02x}")),
                self.raw.len()
            ))
            .color(MUTED_TEXT),
        );
        if self.raw.is_empty() {
            return;
        }
        // Rows scroll sideways rather than wrapping, so a byte never changes
        // column between frames.
        egui::ScrollArea::horizontal()
            .id_salt("raw-hex")
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    for (offset, row) in self.raw.chunks(16).enumerate() {
                        let mut bytes = String::with_capacity(row.len() * 3);
                        for (index, byte) in row.iter().enumerate() {
                            if index > 0 {
                                bytes.push(' ');
                            }
                            let _ = write!(bytes, "{byte:02x}");
                        }
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{:04x}", offset * 16))
                                    .monospace()
                                    .color(DETAIL),
                            );
                            ui.label(RichText::new(bytes).monospace());
                        });
                    }
                });
            });
    }
}
