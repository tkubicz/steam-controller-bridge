//! The counter cells under the header.
//!
//! Every number here says where it came from. Source drops, UI coalescing and
//! serial failures are separate columns on purpose: folding them into one
//! "errors" figure is what made the old display impossible to act on.

use bridge_output::{FirmwareVersion, GamepadOutput};
use eframe::egui;

use crate::{OutputChoice, Visualizer};

fn diagnostic_cell(ui: &mut egui::Ui, label: &str, value: impl std::fmt::Display) {
    ui.label(
        egui::RichText::new(format!("{label} {value}"))
            .small()
            .color(ui_theme::MUTED_TEXT),
    );
    ui.add_space(4.0);
}

impl Visualizer {
    pub(crate) fn diagnostics(&self, ui: &mut egui::Ui) {
        let (reports_received, coalesced, ui_dropped, overflows) =
            self.input.as_ref().map_or((0, 0, 0, 0), |input| {
                let (coalesced, dropped, overflows) = input.mailbox.counters.snapshot();
                (
                    input.mailbox.counters.published(),
                    coalesced,
                    dropped,
                    overflows,
                )
            });

        // Emit directly rather than constructing a temporary Vec<String> on
        // every live repaint. Wrapped layout still prevents clipping.
        ui.horizontal_wrapped(|ui| {
            diagnostic_cell(ui, "Reports received", reports_received);
            diagnostic_cell(ui, "Reports processed", self.report_count);
            match &self.source {
                Some(state) => diagnostic_cell(ui, "Sequence", state.sequence),
                None => diagnostic_cell(ui, "Sequence", "—"),
            }
            match self.raw_report_id {
                Some(id) => diagnostic_cell(ui, "Report ID", format_args!("0x{id:02x}")),
                None => diagnostic_cell(ui, "Report ID", "—"),
            }
            diagnostic_cell(ui, "Report bytes", self.raw.len());
            diagnostic_cell(ui, "Decode failures", self.decode_failures);
            diagnostic_cell(ui, "Source drops", self.source_report_drops);
            diagnostic_cell(ui, "UI coalesced", coalesced);
            diagnostic_cell(ui, "UI dropped", ui_dropped);
            diagnostic_cell(ui, "Mailbox overflows", overflows);
            diagnostic_cell(ui, "Packets sent", self.packets_sent);

            if self.output == OutputChoice::Serial {
                if let Some(metrics) = &self.serial_metrics {
                    diagnostic_cell(ui, "Packets received", metrics.packets_received);
                    diagnostic_cell(ui, "Framing failures", metrics.framing_failures);
                    diagnostic_cell(ui, "Checksum failures", metrics.checksum_failures);
                    diagnostic_cell(ui, "States dropped", metrics.states_dropped);
                    diagnostic_cell(ui, "Reconnects", metrics.reconnects);
                    diagnostic_cell(ui, "State refreshes", metrics.state_refreshes);
                    diagnostic_cell(ui, "Rumble received", metrics.rumble_commands_received);
                    diagnostic_cell(ui, "Rumble coalesced", metrics.rumble_commands_coalesced);
                }
                if let Some(firmware) = self
                    .serial
                    .as_ref()
                    .and_then(GamepadOutput::firmware_version)
                {
                    match firmware {
                        FirmwareVersion::Reported(revision) => {
                            diagnostic_cell(ui, "Firmware", format_args!("rev {revision}"));
                        }
                        FirmwareVersion::Pending => diagnostic_cell(ui, "Firmware", "pending"),
                        FirmwareVersion::Unreported => {
                            diagnostic_cell(ui, "Firmware", "unreported");
                        }
                        FirmwareVersion::Unrecognized => {
                            diagnostic_cell(ui, "Firmware", "unrecognized");
                        }
                    }
                }
            }
        });
    }
}
