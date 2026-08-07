use std::fs::File;
use std::time::{Duration, Instant};

use bridge_output::{GamepadOutput, SerialOutput};
use controller_mapper::{ControllerMapper, MapperConfig};
use eframe::egui::{self, RichText};
use gamepad_state::GamepadState;
use recording::RecordingWriter;
use steam_controller_protocol::{SteamControllerDecoder, SteamControllerState};
use ui_theme::{DETAIL, MUTED_TEXT, PANEL, SURFACE};

/// The fixed timestep handed to the mapper. Not measured elapsed time: the
/// smoothing filter always assumes the device's 250 Hz report rate.
const FRAME_TIME: f32 = 1.0 / 250.0;

/// Window sizing. The minimum is small enough to sit beside a game window and
/// still reach every control, because both columns scroll.
const DEFAULT_WINDOW: [f32; 2] = [1100.0, 760.0];
const MIN_WINDOW: [f32; 2] = [820.0, 600.0];
const SIDEBAR_DEFAULT_WIDTH: f32 = 300.0;
const SIDEBAR_MIN_WIDTH: f32 = 260.0;
const SIDEBAR_MAX_WIDTH: f32 = 360.0;

mod demo;
mod device;
mod diagnostics;
mod header;
mod hero;
mod mailbox;
mod readouts;
mod sidebar;

use demo::DemoState;
use device::{input_worker, InputChannel};
use readouts::{mapped_state_ui, source_state_ui};

fn main() -> eframe::Result {
    let demo = parse_demo_state();
    let index = parse_index();
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
            ui_theme::configure_visuals(&creation.egui_ctx);
            Ok(Box::new(Visualizer::new(index, demo)))
        }),
    )
}

fn parse_index() -> usize {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == "--index")
        .and_then(|pair| pair[1].parse().ok())
        .unwrap_or(0)
}

/// `--demo-state neutral|digital|analog|disconnected`, mutually exclusive with
/// `--index`: a demo run opens no device.
fn parse_demo_state() -> Option<DemoState> {
    let args: Vec<String> = std::env::args().collect();
    let requested = args
        .windows(2)
        .find(|pair| pair[0] == "--demo-state")
        .map(|pair| pair[1].clone())?;
    let Some(mode) = DemoState::parse(&requested) else {
        eprintln!(
            "unknown --demo-state {requested:?}; expected neutral, digital, analog or disconnected"
        );
        std::process::exit(2);
    };
    if args.iter().any(|arg| arg == "--index") {
        eprintln!("--demo-state and --index are mutually exclusive; ignoring --index");
    }
    Some(mode)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputChoice {
    Disabled,
    Mock,
    Serial,
}

struct SerialUiConfig {
    path: String,
    baud: String,
    packet_logging: bool,
}

struct Visualizer {
    input: InputChannel,
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
    rate_count: u16,
    rate_started: Instant,
    report_hz: f32,
    decode_failures: u64,
    packets_sent: u64,
    /// The full serial metric set, kept whole so nothing is silently dropped
    /// and so it can be cleared when the backend is no longer Serial.
    serial_metrics: Option<bridge_output::SerialMetrics>,
    last_output: Option<GamepadState>,
    output: OutputChoice,
    serial: Option<SerialOutput>,
    serial_config: SerialUiConfig,
    recording: Option<RecordingWriter<File>>,
    recording_started: Instant,
    recording_path: String,
    marker: String,
    smoothing_enabled: bool,
    smoothing_time_constant: f32,
    /// Set when the app was launched with `--demo-state`; live input is then
    /// ignored so the picture stays deterministic for a capture.
    demo: Option<DemoState>,
}

impl Visualizer {
    fn new(index: usize, demo: Option<DemoState>) -> Self {
        let config = MapperConfig::default();
        let mut visualizer = Self {
            // A demo run never opens a device, so the worker is pointed at a
            // collection index that will simply fail to open.
            input: input_worker(index),
            source_report_drops: 0,
            decoder: SteamControllerDecoder::new(),
            mapper: ControllerMapper::default(),
            config,
            source: None,
            mapped: GamepadState::neutral(),
            connected: false,
            device: format!("HID collection {index}"),
            status: "Opening device…".to_owned(),
            raw: Vec::new(),
            raw_report_id: None,
            show_raw: false,
            report_count: 0,
            rate_count: 0,
            rate_started: Instant::now(),
            report_hz: 0.0,
            decode_failures: 0,
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
            recording_started: Instant::now(),
            recording_path: "sc-visualizer.jsonl".to_owned(),
            marker: String::new(),
            smoothing_enabled: false,
            smoothing_time_constant: 0.02,
            demo,
        };
        if let Some(mode) = demo {
            visualizer.apply_demo(mode);
        }
        visualizer
    }

    /// Feeds a demo state through the ordinary decode/map path, so what is
    /// drawn is produced exactly the way hardware input would produce it.
    fn apply_demo(&mut self, mode: DemoState) {
        self.status = format!("Demo state: {}", mode.label());
        if let Some(state) = mode.state() {
            self.connected = true;
            self.device = format!("demo ({})", mode.label());
            self.mapped = self.mapper.map(&state, FRAME_TIME);
            self.raw_report_id = Some(state.report_id);
            self.raw.clone_from(&state.raw_report);
            self.source = Some(state);
        } else {
            self.connected = false;
            "demo (disconnected)".clone_into(&mut self.device);
            self.source = None;
            self.mapped = GamepadState::neutral();
        }
    }
}

impl eframe::App for Visualizer {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.demo.is_none() {
            self.process_events();
        }
        if let Some(serial) = &mut self.serial {
            if let Err(error) = serial.service() {
                self.status = format!("Serial service failed: {error}");
            } else {
                let metrics = serial.metrics();
                self.packets_sent = metrics.packets_sent;
                self.serial_metrics = Some(metrics);
            }
        }
        if self.output != OutputChoice::Serial {
            // Otherwise the last session's counters linger and read as live.
            self.serial_metrics = None;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(16));

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
}

impl Visualizer {
    fn central(&mut self, ui: &mut egui::Ui) {
        self.hero(ui);
        ui.add_space(12.0);

        egui::CollapsingHeader::new("Decoded state")
            .default_open(true)
            .show(ui, |ui| {
                if let Some(source) = &self.source {
                    source_state_ui(ui, source);
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
                        let bytes: Vec<String> =
                            row.iter().map(|byte| format!("{byte:02x}")).collect();
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{:04x}", offset * 16))
                                    .monospace()
                                    .color(DETAIL),
                            );
                            ui.label(RichText::new(bytes.join(" ")).monospace());
                        });
                    }
                });
            });
    }
}
