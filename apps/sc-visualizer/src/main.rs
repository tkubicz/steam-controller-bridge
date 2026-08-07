use std::fs::File;
use std::sync::mpsc::{self, Receiver, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use bridge_output::{GamepadOutput, SerialConfig, SerialOutput};
use controller_mapper::{ControllerMapper, MapperConfig, RightAxisSource};
use eframe::egui::{self, RichText, Sense, Stroke, Vec2};
use gamepad_state::{Button, GamepadState};
use recording::{
    RecordingEvent, RecordingWriter, KIND_DEVICE_CONNECTED, KIND_DEVICE_DISCONNECTED, KIND_MARKER,
};
use serde_json::json;
use steam_controller_device::{DeviceEvent, HidSession};
use steam_controller_protocol::{
    DecodedReport, SteamButton, SteamControllerDecoder, SteamControllerState,
};
use ui_theme::{ACCENT, DANGER, DETAIL, OUTLINE, SUCCESS};

const FRAME_TIME: f32 = 1.0 / 250.0;

fn main() -> eframe::Result {
    let index = parse_index();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 760.0]),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "Steam Controller Visualizer",
        options,
        Box::new(move |creation| {
            ui_theme::configure_visuals(&creation.egui_ctx);
            Ok(Box::new(Visualizer::new(index)))
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

fn input_worker(index: usize) -> Receiver<Result<DeviceEvent, String>> {
    let (sender, receiver) = mpsc::sync_channel(64);
    thread::spawn(move || {
        let mut session = match HidSession::open_index(index) {
            Ok(session) => session,
            Err(error) => {
                let _ = sender.send(Err(error.to_string()));
                return;
            }
        };
        loop {
            let event = session
                .poll(Duration::from_millis(10))
                .map_err(|error| error.to_string());
            match event {
                Ok(Some(event)) => match sender.try_send(Ok(event)) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => return,
                },
                Ok(None) => {}
                Err(error) => {
                    if sender.send(Err(error)).is_err() {
                        return;
                    }
                }
            }
        }
    });
    receiver
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
    receiver: Receiver<Result<DeviceEvent, String>>,
    decoder: SteamControllerDecoder,
    mapper: ControllerMapper,
    config: MapperConfig,
    source: Option<SteamControllerState>,
    mapped: GamepadState,
    connected: bool,
    device: String,
    status: String,
    raw: Vec<u8>,
    show_raw: bool,
    report_count: u64,
    rate_count: u16,
    rate_started: Instant,
    report_hz: f32,
    decode_failures: u64,
    framing_failures: u64,
    packets_sent: u64,
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
}

impl Visualizer {
    fn new(index: usize) -> Self {
        let config = MapperConfig::default();
        Self {
            receiver: input_worker(index),
            decoder: SteamControllerDecoder::new(),
            mapper: ControllerMapper::default(),
            config,
            source: None,
            mapped: GamepadState::neutral(),
            connected: false,
            device: format!("HID collection {index}"),
            status: "Opening device…".to_owned(),
            raw: Vec::new(),
            show_raw: false,
            report_count: 0,
            rate_count: 0,
            rate_started: Instant::now(),
            report_hz: 0.0,
            decode_failures: 0,
            framing_failures: 0,
            packets_sent: 0,
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
        }
    }

    fn timestamp_us(&self) -> u64 {
        self.recording_started
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn record(&mut self, event: &RecordingEvent) {
        if let Some(writer) = &mut self.recording {
            if let Err(error) = writer.write_event(event) {
                self.status = format!("Recording stopped: {error}");
                self.recording = None;
            }
        }
    }

    fn process_events(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                Err(error) => self.status = error,
                Ok(DeviceEvent::Connected(info)) => {
                    self.connected = true;
                    self.device = info.product.clone().unwrap_or(info.id.clone());
                    self.status = format!("Connected via {}", info.transport);
                    let event = RecordingEvent::new(
                        self.timestamp_us(),
                        KIND_DEVICE_CONNECTED,
                        json!({"id": info.id, "transport": info.transport}),
                    );
                    self.record(&event);
                }
                Ok(DeviceEvent::Disconnected) => {
                    self.connected = false;
                    self.source = None;
                    self.mapped = GamepadState::neutral();
                    self.mapper.reset();
                    "Disconnected; waiting for reconnect".clone_into(&mut self.status);
                    self.record(&RecordingEvent::new(
                        self.timestamp_us(),
                        KIND_DEVICE_DISCONNECTED,
                        json!({}),
                    ));
                }
                Ok(DeviceEvent::Report(report)) => {
                    self.report_count += 1;
                    self.rate_count = self.rate_count.saturating_add(1);
                    self.raw.clone_from(&report.data);
                    let timestamp = self.timestamp_us();
                    if let Ok(event) = RecordingEvent::raw_hid_with_metadata(
                        timestamp,
                        report.report_id,
                        &report.data,
                        Some(&report.source_device_id),
                        Some(&report.transport),
                        report.dropped_reports,
                    ) {
                        self.record(&event);
                    }
                    match self.decoder.decode(report.report_id, &report.data) {
                        Ok(DecodedReport::ControllerState(state)) => {
                            self.mapped = self.mapper.map(&state, FRAME_TIME);
                            if let Ok(event) =
                                RecordingEvent::decoded_steam_state(timestamp, &state)
                            {
                                self.record(&event);
                            }
                            if let Ok(event) =
                                RecordingEvent::mapped_gamepad_state(timestamp, &self.mapped)
                            {
                                self.record(&event);
                            }
                            self.source = Some(state);
                            if self.output == OutputChoice::Mock
                                && self.last_output != Some(self.mapped)
                            {
                                self.packets_sent += 1;
                                self.last_output = Some(self.mapped);
                            }
                            if self.output == OutputChoice::Serial {
                                if let Some(serial) = &mut self.serial {
                                    match serial.send_state(&self.mapped) {
                                        Ok(()) => {
                                            let metrics = serial.metrics();
                                            self.packets_sent = metrics.packets_sent;
                                            self.framing_failures = metrics.framing_failures;
                                        }
                                        Err(error) => self.status = error.to_string(),
                                    }
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            self.decode_failures += 1;
                            self.status = format!("Decode error: {error}");
                        }
                    }
                }
            }
        }
        let elapsed = self.rate_started.elapsed();
        if elapsed >= Duration::from_secs(1) {
            self.report_hz = f32::from(self.rate_count) / elapsed.as_secs_f32();
            self.rate_count = 0;
            self.rate_started = Instant::now();
        }
    }

    fn rebuild_mapper(&mut self) {
        self.config.smoothing_time_constant = self
            .smoothing_enabled
            .then_some(self.smoothing_time_constant);
        match ControllerMapper::new(self.config) {
            Ok(mapper) => self.mapper = mapper,
            Err(error) => self.status = error.to_string(),
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("Controls");
        let mut changed = false;
        changed |= ui
            .add(
                egui::Slider::new(&mut self.config.left_stick_dead_zone, 0.0..=0.5)
                    .text("Left dead zone"),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut self.config.right_axis_dead_zone, 0.0..=0.5)
                    .text("Right dead zone"),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut self.config.trigger_dead_zone, 0.0..=0.5)
                    .text("Trigger dead zone"),
            )
            .changed();
        changed |= ui
            .checkbox(&mut self.config.axis_inversion.left_x, "Invert left X")
            .changed();
        changed |= ui
            .checkbox(&mut self.config.axis_inversion.left_y, "Invert left Y")
            .changed();
        changed |= ui
            .checkbox(&mut self.config.axis_inversion.right_x, "Invert right X")
            .changed();
        changed |= ui
            .checkbox(&mut self.config.axis_inversion.right_y, "Invert right Y")
            .changed();
        changed |= ui
            .checkbox(&mut self.smoothing_enabled, "Low-pass smoothing")
            .changed();
        if self.smoothing_enabled {
            changed |= ui
                .add(
                    egui::Slider::new(&mut self.smoothing_time_constant, 0.001..=0.2)
                        .logarithmic(true)
                        .text("Time constant"),
                )
                .changed();
        }
        egui::ComboBox::from_label("Mapping profile")
            .selected_text(format!("{:?}", self.config.right_axis_source))
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(
                        &mut self.config.right_axis_source,
                        RightAxisSource::RightPad,
                        "Right pad",
                    )
                    .changed();
                changed |= ui
                    .selectable_value(
                        &mut self.config.right_axis_source,
                        RightAxisSource::RightStick,
                        "Right stick",
                    )
                    .changed();
            });
        if changed {
            self.rebuild_mapper();
        }
        if ui.button("Reset to neutral").clicked() {
            self.mapper.reset();
            self.mapped = GamepadState::neutral();
            self.last_output = None;
        }
        ui.checkbox(&mut self.show_raw, "Show raw report bytes");
    }

    fn recording_controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("Recording");
        ui.text_edit_singleline(&mut self.recording_path);
        if self.recording.is_none() {
            if ui.button("Start recording").clicked() {
                match File::create(&self.recording_path) {
                    Ok(file) => {
                        self.recording_started = Instant::now();
                        self.recording = Some(RecordingWriter::new(file));
                        self.status = format!("Recording to {}", self.recording_path);
                    }
                    Err(error) => self.status = format!("Cannot start recording: {error}"),
                }
            }
        } else if ui.button("Stop recording").clicked() {
            self.recording = None;
            "Recording stopped".clone_into(&mut self.status);
        }
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.marker);
            if ui.button("Insert marker").clicked() && !self.marker.trim().is_empty() {
                let name = self.marker.trim().to_owned();
                self.record(&RecordingEvent::new(
                    self.timestamp_us(),
                    KIND_MARKER,
                    json!({"name": name}),
                ));
                self.marker.clear();
            }
        });
        egui::ComboBox::from_label("Output backend")
            .selected_text(format!("{:?}", self.output))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.output, OutputChoice::Disabled, "Disabled");
                ui.selectable_value(
                    &mut self.output,
                    OutputChoice::Mock,
                    "Mock (changed states)",
                );
                ui.selectable_value(&mut self.output, OutputChoice::Serial, "Serial");
            });
        if self.output == OutputChoice::Serial {
            ui.text_edit_singleline(&mut self.serial_config.path);
            ui.text_edit_singleline(&mut self.serial_config.baud);
            ui.checkbox(
                &mut self.serial_config.packet_logging,
                "Log serial frame bytes",
            );
            if self.serial.is_none() {
                if ui.button("Connect serial").clicked() {
                    match self
                        .serial_config
                        .baud
                        .parse()
                        .map_err(|_| "invalid baud rate".to_owned())
                        .and_then(|baud| {
                            SerialOutput::open(
                                &self.serial_config.path,
                                baud,
                                SerialConfig {
                                    packet_logging: self.serial_config.packet_logging,
                                    ..SerialConfig::default()
                                },
                            )
                            .map_err(|error| error.to_string())
                        }) {
                        Ok(serial) => {
                            self.serial = Some(serial);
                            "Serial connected".clone_into(&mut self.status);
                        }
                        Err(error) => self.status = error,
                    }
                }
            } else if ui.button("Disconnect serial").clicked() {
                self.serial = None;
                "Serial disconnected".clone_into(&mut self.status);
            }
            ui.label(format!(
                "Serial: {}",
                self.serial
                    .as_ref()
                    .map_or("disconnected".to_owned(), |serial| format!(
                        "{:?}",
                        serial.status()
                    ))
            ));
        }
    }
}

impl eframe::App for Visualizer {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.process_events();
        if let Some(serial) = &mut self.serial {
            if let Err(error) = serial.service() {
                self.status = format!("Serial service failed: {error}");
            } else {
                let metrics = serial.metrics();
                self.packets_sent = metrics.packets_sent;
                self.framing_failures = metrics.framing_failures;
            }
        }
        ui.ctx().request_repaint_after(Duration::from_millis(16));
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(240.0);
                self.controls(ui);
                ui.separator();
                self.recording_controls(ui);
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal(|ui| {
                    let color = if self.connected { SUCCESS } else { DANGER };
                    ui.label(
                        RichText::new(if self.connected {
                            "● Connected"
                        } else {
                            "● Disconnected"
                        })
                        .color(color),
                    );
                    ui.label(&self.device);
                    ui.label(format!("{:.1} Hz", self.report_hz));
                });
                ui.label(&self.status);
                ui.horizontal(|ui| {
                    ui.label(format!("Reports: {}", self.report_count));
                    ui.label(format!(
                        "Sequence: {}",
                        self.source.as_ref().map_or(0, |state| state.sequence)
                    ));
                    ui.label(format!("Packets sent: {}", self.packets_sent));
                    ui.label(format!("Decode failures: {}", self.decode_failures));
                    ui.label(format!("Framing failures: {}", self.framing_failures));
                    ui.label(format!(
                        "Raw: ID 0x{:02x} / {} bytes",
                        self.raw.first().copied().unwrap_or(0),
                        self.raw.len()
                    ));
                });
                ui.separator();
                if let Some(source) = &self.source {
                    source_state_ui(ui, source);
                } else {
                    ui.label("No decoded controller state yet.");
                }
                ui.separator();
                mapped_state_ui(ui, &self.mapped);
                if self.show_raw {
                    ui.separator();
                    ui.label(format!(
                        "Raw report: ID 0x{:02x}, {} bytes",
                        self.raw.first().copied().unwrap_or(0),
                        self.raw.len()
                    ));
                    let hex = self
                        .raw
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    ui.monospace(hex);
                }
            });
        });
    }
}

fn source_state_ui(ui: &mut egui::Ui, state: &SteamControllerState) {
    ui.heading("Decoded Steam Controller");
    ui.horizontal(|ui| {
        stick(
            ui,
            "Left stick",
            f32::from(state.left_stick_x) / 32767.0,
            f32::from(state.left_stick_y) / 32767.0,
        );
        stick(
            ui,
            "Left pad",
            f32::from(state.left_pad_x) / 32767.0,
            f32::from(state.left_pad_y) / 32767.0,
        );
        stick(
            ui,
            "Right pad",
            f32::from(state.right_pad_x) / 32767.0,
            f32::from(state.right_pad_y) / 32767.0,
        );
        stick(
            ui,
            "Right stick",
            f32::from(state.right_stick_x) / 32767.0,
            f32::from(state.right_stick_y) / 32767.0,
        );
    });
    ui.label(format!(
        "Triggers: L {}  R {}",
        state.left_trigger, state.right_trigger
    ));
    ui.label(format!(
        "Pad touch/click: L {}/{}  R {}/{}",
        state.left_pad_touched,
        state.left_pad_pressed,
        state.right_pad_touched,
        state.right_pad_pressed
    ));
    ui.label(format!(
        "Gyro: {:?}   Acceleration: {:?}",
        state.gyro, state.acceleration
    ));
    let pressed = STEAM_BUTTONS
        .iter()
        .filter(|(button, _)| state.buttons.contains(*button))
        .map(|(_, name)| *name)
        .collect::<Vec<_>>();
    ui.label(format!(
        "Buttons: {}",
        if pressed.is_empty() {
            "none".to_owned()
        } else {
            pressed.join(", ")
        }
    ));
}

fn mapped_state_ui(ui: &mut egui::Ui, state: &GamepadState) {
    ui.heading("Outgoing generic gamepad");
    ui.horizontal(|ui| {
        stick(ui, "Left", state.left_x, state.left_y);
        stick(ui, "Right", state.right_x, state.right_y);
        ui.vertical(|ui| {
            ui.label(format!("Hat: {:?}", state.hat));
            ui.label(format!(
                "Triggers: {:.3} / {:.3}",
                state.left_trigger, state.right_trigger
            ));
            let pressed = GAMEPAD_BUTTONS
                .iter()
                .filter(|(button, _)| state.buttons.contains(*button))
                .map(|(_, name)| *name)
                .collect::<Vec<_>>();
            ui.label(format!(
                "Buttons: {}",
                if pressed.is_empty() {
                    "none".to_owned()
                } else {
                    pressed.join(", ")
                }
            ));
        });
    });
}

fn stick(ui: &mut egui::Ui, label: &str, x: f32, y: f32) {
    ui.vertical(|ui| {
        ui.label(label);
        let (response, painter) = ui.allocate_painter(Vec2::splat(100.0), Sense::hover());
        let center = response.rect.center();
        painter.circle_stroke(center, 45.0, Stroke::new(1.0, OUTLINE));
        painter.line_segment(
            [center - Vec2::new(45.0, 0.0), center + Vec2::new(45.0, 0.0)],
            Stroke::new(1.0, DETAIL),
        );
        painter.line_segment(
            [center - Vec2::new(0.0, 45.0), center + Vec2::new(0.0, 45.0)],
            Stroke::new(1.0, DETAIL),
        );
        painter.circle_filled(
            center + Vec2::new(x.clamp(-1.0, 1.0), -y.clamp(-1.0, 1.0)) * 45.0,
            5.0,
            ACCENT,
        );
    });
}

const STEAM_BUTTONS: [(SteamButton, &str); 30] = [
    (SteamButton::A, "A"),
    (SteamButton::B, "B"),
    (SteamButton::X, "X"),
    (SteamButton::Y, "Y"),
    (SteamButton::QuickAccess, "Quick Access"),
    (SteamButton::RightStickPress, "R3"),
    (SteamButton::View, "View"),
    (SteamButton::RightGrip4, "R4"),
    (SteamButton::RightGrip5, "R5"),
    (SteamButton::RightShoulder, "RB"),
    (SteamButton::DpadDown, "D-pad Down"),
    (SteamButton::DpadRight, "D-pad Right"),
    (SteamButton::DpadLeft, "D-pad Left"),
    (SteamButton::DpadUp, "D-pad Up"),
    (SteamButton::Menu, "Menu"),
    (SteamButton::LeftStickPress, "L3"),
    (SteamButton::Steam, "Steam"),
    (SteamButton::LeftGrip4, "L4"),
    (SteamButton::LeftGrip5, "L5"),
    (SteamButton::LeftShoulder, "LB"),
    (SteamButton::RightStickTouch, "R-stick Touch"),
    (SteamButton::RightPadTouch, "R-pad Touch"),
    (SteamButton::RightPadClick, "R-pad Click"),
    (SteamButton::RightTriggerClick, "RT Click"),
    (SteamButton::LeftStickTouch, "L-stick Touch"),
    (SteamButton::LeftPadTouch, "L-pad Touch"),
    (SteamButton::LeftPadClick, "L-pad Click"),
    (SteamButton::LeftTriggerClick, "LT Click"),
    (SteamButton::RightGripTouch, "R-grip Touch"),
    (SteamButton::LeftGripTouch, "L-grip Touch"),
];

const GAMEPAD_BUTTONS: [(Button, &str); 16] = [
    (Button::South, "South"),
    (Button::East, "East"),
    (Button::West, "West"),
    (Button::North, "North"),
    (Button::LeftShoulder, "LB"),
    (Button::RightShoulder, "RB"),
    (Button::LeftStick, "L3"),
    (Button::RightStick, "R3"),
    (Button::Back, "Back"),
    (Button::Start, "Start"),
    (Button::Guide, "Guide"),
    (Button::LeftGrip, "Left Grip"),
    (Button::RightGrip, "Right Grip"),
    (Button::Extra1, "Extra 1"),
    (Button::Extra2, "Extra 2"),
    (Button::Extra3, "Extra 3"),
];
