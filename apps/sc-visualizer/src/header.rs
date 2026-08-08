//! The top bar: what is connected, how fast it is reporting, whether we are
//! recording, and the current status line.

use eframe::egui;
use ui_theme::{ACCENT, DANGER, MUTED_TEXT, SUCCESS, TEXT};

use crate::Visualizer;

impl Visualizer {
    pub(crate) fn header(&self, ui: &mut egui::Ui) {
        // Wrapped rather than one unbroken row: narrowing the window or
        // widening the sidebar must never clip the connection state.
        ui.horizontal_wrapped(|ui| {
            let (dot, colour, text) = if self.connected {
                ("●", SUCCESS, "Connected")
            } else {
                ("●", DANGER, "Disconnected")
            };
            ui.label(egui::RichText::new(format!("{dot} {text}")).color(colour));
            ui.add_space(8.0);
            ui.label(egui::RichText::new(&self.device).color(TEXT));
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!("{:.1} input Hz", self.report_hz))
                    .monospace()
                    .color(if self.connected { ACCENT } else { MUTED_TEXT }),
            );
            if let Some(recording) = &self.recording {
                ui.add_space(8.0);
                let elapsed = self.recording_started.elapsed().as_secs();
                let state = if recording.is_accepting() {
                    "REC"
                } else {
                    "FLUSH"
                };
                ui.label(
                    egui::RichText::new(format!("● {state} {}:{:02}", elapsed / 60, elapsed % 60))
                        .color(DANGER),
                );
            }
        });
        ui.label(egui::RichText::new(&self.status).color(MUTED_TEXT));
        self.diagnostics(ui);
    }
}
