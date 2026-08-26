//! The configuration panels: mapper settings, output backend and recording.

use eframe::egui;

use bridge_output::{BridgeOutput, BridgeTransportConfig};
use controller_mapper::RightAxisSource;
use recording::{RecordingEvent, KIND_MARKER};
use serde_json::json;
use std::time::Instant;

use ui_theme::MUTED_TEXT;

use crate::{OutputChoice, Visualizer};

/// The one place each backend is named, so the closed combo and its menu items
/// cannot drift apart.
const fn output_label(choice: OutputChoice) -> &'static str {
    match choice {
        OutputChoice::Disabled => "Disabled",
        OutputChoice::Mock => "Mock (changed states)",
        OutputChoice::Serial => "Serial",
    }
}

const fn right_axis_label(source: RightAxisSource) -> &'static str {
    match source {
        RightAxisSource::RightStick => "Right stick",
        RightAxisSource::RightPad => "Right pad",
    }
}

/// A text field with a name in front of it.
fn labelled(ui: &mut egui::Ui, label: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).small().color(MUTED_TEXT));
        contents(ui);
    });
}

impl Visualizer {
    pub(crate) fn controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("Mapper");
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
        // One `label` fn feeds both the closed state and the menu, so the two
        // can never disagree the way "RightStick" and "Right stick" did.
        egui::ComboBox::from_label("Right axis source")
            .selected_text(right_axis_label(self.config.right_axis_source))
            .show_ui(ui, |ui| {
                for source in [RightAxisSource::RightStick, RightAxisSource::RightPad] {
                    changed |= ui
                        .selectable_value(
                            &mut self.config.right_axis_source,
                            source,
                            right_axis_label(source),
                        )
                        .changed();
                }
            });
        if changed {
            self.rebuild_mapper();
        }
        if ui.button("Reset to neutral").clicked() {
            self.neutralize_output();
            self.last_output = None;
            if !self.status.starts_with("Output neutralization failed") {
                "Reset to neutral".clone_into(&mut self.status);
            }
        }
        ui.checkbox(&mut self.show_raw, "Show raw report bytes");
    }

    pub(crate) fn recording_controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("Recording");
        labelled(ui, "File", |ui| {
            ui.text_edit_singleline(&mut self.recording_path);
        });
        if self.recording.is_none() {
            if ui.button("Start recording").clicked() {
                match crate::recording_sink::RecordingSession::start(&self.recording_path) {
                    Ok(recording) => {
                        self.recording_started = Instant::now();
                        self.recording_stop_error = None;
                        self.recording = Some(recording);
                        self.status = format!("Recording to {}", self.recording_path);
                    }
                    Err(error) => self.status = format!("Cannot start recording: {error}"),
                }
            }
        } else if self
            .recording
            .as_ref()
            .is_some_and(crate::recording_sink::RecordingSession::is_accepting)
        {
            if ui.button("Stop recording").clicked() {
                if let Some(recording) = &mut self.recording {
                    recording.request_finish();
                }
                "Finishing recording…".clone_into(&mut self.status);
            }
        } else {
            ui.add_enabled(false, egui::Button::new("Finishing recording…"));
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Marker").small().color(MUTED_TEXT));
            ui.text_edit_singleline(&mut self.marker);
            let can_insert = !self.marker.trim().is_empty()
                && self
                    .recording
                    .as_ref()
                    .is_some_and(crate::recording_sink::RecordingSession::is_accepting);
            if ui
                .add_enabled(can_insert, egui::Button::new("Insert marker"))
                .clicked()
            {
                let name = self.marker.trim().to_owned();
                let timestamp = self.timestamp_us();
                self.record_lazy(|| {
                    Ok(RecordingEvent::new(
                        timestamp,
                        KIND_MARKER,
                        json!({"name": name}),
                    ))
                });
                self.marker.clear();
            }
        });
        self.output_controls(ui);
    }

    fn output_controls(&mut self, ui: &mut egui::Ui) {
        let mut selected_output = self.output;
        egui::ComboBox::from_label("Output backend")
            .selected_text(output_label(self.output))
            .show_ui(ui, |ui| {
                for choice in [
                    OutputChoice::Disabled,
                    OutputChoice::Mock,
                    OutputChoice::Serial,
                ] {
                    ui.selectable_value(&mut selected_output, choice, output_label(choice));
                }
            });
        self.select_output(selected_output);
        if self.output == OutputChoice::Serial {
            labelled(ui, "Port", |ui| {
                ui.text_edit_singleline(&mut self.serial_config.path);
            });
            labelled(ui, "Baud", |ui| {
                ui.text_edit_singleline(&mut self.serial_config.baud);
            });
            // Only read by `BridgeOutput::open_serial`, so toggling it mid-session
            // would silently do nothing.
            let connected = self.serial.is_some();
            ui.add_enabled_ui(!connected, |ui| {
                ui.checkbox(
                    &mut self.serial_config.packet_logging,
                    "Log serial frame bytes",
                )
                .on_disabled_hover_text("Applies when the port is opened; disconnect to change.");
            });
            if self.serial.is_none() {
                if ui.button("Connect serial").clicked() {
                    match self
                        .serial_config
                        .baud
                        .parse()
                        .map_err(|_| "invalid baud rate".to_owned())
                        .and_then(|baud| {
                            BridgeOutput::open_serial(
                                &self.serial_config.path,
                                baud,
                                BridgeTransportConfig {
                                    packet_logging: self.serial_config.packet_logging,
                                    ..BridgeTransportConfig::default()
                                },
                            )
                            .map_err(|error| error.to_string())
                        }) {
                        Ok(serial) => {
                            self.serial = Some(serial);
                            self.publish_mapped();
                            if self.serial.is_some() {
                                "Serial connected; current state published"
                                    .clone_into(&mut self.status);
                            }
                        }
                        Err(error) => self.status = error,
                    }
                }
            } else if ui.button("Disconnect serial").clicked() {
                self.disconnect_serial();
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
