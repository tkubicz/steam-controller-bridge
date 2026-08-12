use std::f32::consts::{PI, TAU};

use eframe::egui::{self, Atom};

use crate::{ACCENT, ACCENT_SUBTLE, BORDER, MUTED_TEXT, SURFACE_RAISED, TEXT};

/// A primary application action with shared hover, pressed, focus, and disabled visuals.
///
/// Keep layout decisions such as centering and maximum width at the call site;
/// this widget owns only the interaction behavior and visual language.
#[must_use = "Call `show` to add the button to a UI"]
pub struct ActionButton<'a> {
    label: &'a str,
    enabled: bool,
    min_size: egui::Vec2,
    leading_icon: Option<LeadingIcon>,
}

#[derive(Clone, Copy)]
enum LeadingIcon {
    Refresh { spinning: bool },
}

impl<'a> ActionButton<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            enabled: true,
            min_size: egui::Vec2::ZERO,
            leading_icon: None,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn min_size(mut self, min_size: egui::Vec2) -> Self {
        self.min_size = min_size;
        self
    }

    /// Add a refresh arrow. While `spinning` is true, the arrow rotates and
    /// requests repaints until the operation finishes.
    pub fn refresh_icon(mut self, spinning: bool) -> Self {
        self.leading_icon = Some(LeadingIcon::Refresh { spinning });
        self
    }

    pub fn show(self, ui: &mut egui::Ui) -> egui::Response {
        ui.scope(|ui| {
            configure_action_visuals(ui);
            ui.add_enabled_ui(self.enabled, |ui| self.show_inner(ui))
                .inner
        })
        .inner
    }

    fn show_inner(self, ui: &mut egui::Ui) -> egui::Response {
        let label = egui::RichText::new(self.label).strong();
        let Some(icon) = self.leading_icon else {
            return ui.add(
                egui::Button::new(label)
                    .corner_radius(8)
                    .min_size(self.min_size),
            );
        };

        let icon_id = ui.next_auto_id().with("action-button-leading-icon");
        let response = egui::Button::new((Atom::custom(icon_id, egui::vec2(16.0, 16.0)), label))
            .gap(8.0)
            .corner_radius(8)
            .min_size(self.min_size)
            .atom_ui(ui);
        if let Some(rect) = response.rect(icon_id) {
            let state = response.response.widget_state();
            let color = ui.style().visuals.widgets.state(state).text_color();
            match icon {
                LeadingIcon::Refresh { spinning } => {
                    paint_refresh_icon(ui, rect, color, spinning);
                }
            }
        }
        response.response
    }
}

fn configure_action_visuals(ui: &mut egui::Ui) {
    let visuals = &mut ui.style_mut().visuals.widgets;

    visuals.inactive.weak_bg_fill = ACCENT_SUBTLE;
    visuals.inactive.bg_stroke = egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.7));
    visuals.inactive.fg_stroke = egui::Stroke::new(1.0, ACCENT);
    visuals.inactive.corner_radius = 8.into();

    visuals.hovered.weak_bg_fill = ACCENT_SUBTLE.gamma_multiply(1.35);
    visuals.hovered.bg_stroke = egui::Stroke::new(1.5, ACCENT);
    visuals.hovered.fg_stroke = egui::Stroke::new(1.5, ACCENT);
    visuals.hovered.corner_radius = 8.into();
    visuals.hovered.expansion = 1.0;

    visuals.active.weak_bg_fill = ACCENT;
    visuals.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    visuals.active.fg_stroke = egui::Stroke::new(1.5, TEXT);
    visuals.active.corner_radius = 8.into();
    visuals.active.expansion = 0.0;

    visuals.noninteractive.weak_bg_fill = SURFACE_RAISED;
    visuals.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
    visuals.noninteractive.fg_stroke = egui::Stroke::new(1.0, MUTED_TEXT);
    visuals.noninteractive.corner_radius = 8.into();
}

fn paint_refresh_icon(ui: &egui::Ui, rect: egui::Rect, color: egui::Color32, spinning: bool) {
    let rotation = if spinning {
        ui.ctx().request_repaint();
        let bounded = ui.input(|input| input.time.rem_euclid(1.0) * f64::from(TAU));
        // The value is reduced to one turn before conversion, so the loss is
        // far below a visible fraction of a pixel at this icon size.
        #[allow(clippy::cast_possible_truncation)]
        let bounded = bounded as f32;
        bounded
    } else {
        0.0
    };
    let center = rect.center();
    let radius = rect.width().min(rect.height()) * 0.34;
    let start = rotation - PI * 0.7;
    let end = start + PI * 1.55;
    let points = (0_i16..=18)
        .map(|index| {
            let angle = egui::lerp(start..=end, f32::from(index) / 18.0);
            center + radius * egui::vec2(angle.cos(), angle.sin())
        })
        .collect();
    let stroke = egui::Stroke::new(1.8, color);
    ui.painter().add(egui::Shape::line(points, stroke));

    let tip = center + radius * egui::vec2(end.cos(), end.sin());
    let tangent = egui::vec2(-end.sin(), end.cos());
    let normal = egui::vec2(-tangent.y, tangent.x);
    let back = tip - tangent * 4.5;
    ui.painter().add(egui::Shape::line(
        vec![back + normal * 2.3, tip, back - normal * 2.3],
        stroke,
    ));
}

#[cfg(test)]
mod tests {
    use super::{configure_action_visuals, ACCENT, ACCENT_SUBTLE, TEXT};
    use eframe::egui;

    #[test]
    fn action_states_have_distinct_hover_and_pressed_feedback() {
        egui::__run_test_ui(|ui| {
            configure_action_visuals(ui);
            let widgets = &ui.visuals().widgets;
            assert_eq!(widgets.inactive.weak_bg_fill, ACCENT_SUBTLE);
            assert_ne!(widgets.hovered.weak_bg_fill, widgets.inactive.weak_bg_fill);
            assert_eq!(widgets.active.weak_bg_fill, ACCENT);
            assert_eq!(widgets.active.text_color(), TEXT);
        });
    }
}
