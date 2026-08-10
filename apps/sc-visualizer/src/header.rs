//! The top bar: what is connected, how fast it is reporting, whether we are
//! recording, and the current status line.

use eframe::egui;
use steam_controller_discovery::{ControllerSearch, CONTROLLER_OWNERSHIP_GUIDANCE};
use ui_theme::{ACCENT, DANGER, MUTED_TEXT, SUCCESS, TEXT};

use crate::Visualizer;

#[derive(Debug, PartialEq, Eq)]
struct ConnectionProblem {
    title: &'static str,
    explanation: &'static str,
    details: Vec<String>,
    guidance: Option<&'static str>,
}

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
        if let Some(problem) = self.connection_search.as_ref().and_then(connection_problem) {
            show_connection_problem(ui, &problem);
        } else {
            ui.add(egui::Label::new(egui::RichText::new(&self.status).color(MUTED_TEXT)).wrap());
        }
        self.diagnostics(ui);
    }
}

fn connection_problem(search: &ControllerSearch) -> Option<ConnectionProblem> {
    let ControllerSearch::CannotOpen {
        detail: raw_details,
        ownership_conflict,
    } = search
    else {
        return None;
    };
    let cleaned = raw_details
        .replace(&format!(". {CONTROLLER_OWNERSHIP_GUIDANCE}"), "")
        .replace(CONTROLLER_OWNERSHIP_GUIDANCE, "");
    let mut details = Vec::new();
    for detail in cleaned
        .split("; ")
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
    {
        let detail = detail.trim_end_matches('.').to_owned();
        if !details.contains(&detail) {
            details.push(detail);
        }
    }
    Some(ConnectionProblem {
        title: if *ownership_conflict {
            "Controller is already in use"
        } else {
            "Controller input could not be opened"
        },
        explanation: if *ownership_conflict {
            "Another Steam or controller process currently owns the compatible HID interfaces."
        } else {
            "The controller was found, but none of its compatible input interfaces could be opened."
        },
        details,
        guidance: ownership_conflict.then_some(CONTROLLER_OWNERSHIP_GUIDANCE),
    })
}

fn show_connection_problem(ui: &mut egui::Ui, problem: &ConnectionProblem) {
    ui.group(|ui| {
        ui.set_width(ui.available_width());
        ui.label(
            egui::RichText::new(format!("● {}", problem.title))
                .strong()
                .color(DANGER),
        );
        ui.add(egui::Label::new(egui::RichText::new(problem.explanation).color(TEXT)).wrap());
        if let Some(guidance) = problem.guidance {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("What to do: {guidance}.")).color(MUTED_TEXT),
                )
                .wrap(),
            );
        }
        if !problem.details.is_empty() {
            egui::CollapsingHeader::new(format!(
                "Technical details ({} interface{})",
                problem.details.len(),
                if problem.details.len() == 1 { "" } else { "s" }
            ))
            .default_open(false)
            .show(ui, |ui| {
                for detail in &problem.details {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("• {detail}"))
                                .monospace()
                                .color(MUTED_TEXT),
                        )
                        .wrap(),
                    );
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_failures_become_one_summary_and_collapsed_interface_details() {
        let search = ControllerSearch::CannotOpen {
            detail: format!(
                "Puck interface 3: HID interface 3 is already owned. {CONTROLLER_OWNERSHIP_GUIDANCE}; Puck interface 4: HID interface 4 is already owned. {CONTROLLER_OWNERSHIP_GUIDANCE}"
            ),
            ownership_conflict: true,
        };
        let problem = connection_problem(&search).expect("cannot-open errors are structured");
        assert_eq!(problem.title, "Controller is already in use");
        assert_eq!(problem.guidance, Some(CONTROLLER_OWNERSHIP_GUIDANCE));
        assert_eq!(problem.details.len(), 2);
        assert!(problem.details[0].contains("interface 3"));
        assert!(problem.details[1].contains("interface 4"));
        assert!(problem
            .details
            .iter()
            .all(|detail| !detail.contains("Fully quit Steam")));
    }

    #[test]
    fn ordinary_status_messages_keep_the_simple_header_path() {
        assert!(connection_problem(&ControllerSearch::NoController).is_none());
        assert!(connection_problem(&ControllerSearch::NoInputYet).is_none());
    }
}
