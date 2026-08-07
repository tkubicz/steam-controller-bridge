//! The counter cells under the header.
//!
//! Every number here says where it came from. Source drops, UI coalescing and
//! serial failures are separate columns on purpose: folding them into one
//! "errors" figure is what made the old display impossible to act on.

use eframe::egui;

use crate::{OutputChoice, Visualizer};

/// A value that has no meaning yet shows an em dash rather than a zero, so a
/// real zero and "nothing has arrived" cannot be confused.
fn unknown() -> String {
    "—".to_owned()
}

impl Visualizer {
    pub(crate) fn diagnostics(&self, ui: &mut egui::Ui) {
        let (coalesced, ui_dropped, overflows) = self.input.mailbox.counters.snapshot();

        let mut cells: Vec<(&str, String)> = vec![
            ("Reports", self.report_count.to_string()),
            (
                "Sequence",
                self.source
                    .as_ref()
                    .map_or_else(unknown, |state| state.sequence.to_string()),
            ),
            (
                "Report ID",
                self.raw_report_id
                    .map_or_else(unknown, |id| format!("0x{id:02x}")),
            ),
            ("Report bytes", self.raw.len().to_string()),
            ("Decode failures", self.decode_failures.to_string()),
            ("Source drops", self.source_report_drops.to_string()),
            ("UI coalesced", coalesced.to_string()),
            ("UI dropped", ui_dropped.to_string()),
            ("Mailbox overflows", overflows.to_string()),
            ("Packets sent", self.packets_sent.to_string()),
        ];

        if self.output == OutputChoice::Serial {
            if let Some(metrics) = &self.serial_metrics {
                cells.extend([
                    ("Packets received", metrics.packets_received.to_string()),
                    ("Framing failures", metrics.framing_failures.to_string()),
                    ("Checksum failures", metrics.checksum_failures.to_string()),
                    ("States dropped", metrics.states_dropped.to_string()),
                    ("Reconnects", metrics.reconnects.to_string()),
                    ("State refreshes", metrics.state_refreshes.to_string()),
                    (
                        "Rumble received",
                        metrics.rumble_commands_received.to_string(),
                    ),
                    (
                        "Rumble coalesced",
                        metrics.rumble_commands_coalesced.to_string(),
                    ),
                ]);
            }
        }

        // Wrapped, not one unbroken row: resizing must never clip a counter.
        ui.horizontal_wrapped(|ui| {
            for (label, value) in cells {
                ui.label(
                    egui::RichText::new(format!("{label} {value}"))
                        .small()
                        .color(ui_theme::MUTED_TEXT),
                );
                ui.add_space(4.0);
            }
        });
    }
}
