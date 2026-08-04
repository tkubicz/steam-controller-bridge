use std::collections::BTreeSet;

use desktop_bindings::{
    default_store_path, load_or_create_store, save_store, BindableControl, BindingAction,
    BindingProfile, BindingStore, ControlBindings, KeyboardKey, Modifier, MouseButton,
};
use eframe::egui;

const WINDOW_SIZE: [f32; 2] = [1180.0, 760.0];
const MIN_WINDOW_SIZE: [f32; 2] = [1040.0, 680.0];
const INSPECTOR_WIDTH: f32 = 300.0;
const ACCENT: egui::Color32 = egui::Color32::from_rgb(84, 211, 224);
const CANVAS_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(18, 21, 26);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(29, 34, 42);
const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(36, 42, 51);
const OUTLINE: egui::Color32 = egui::Color32::from_rgb(126, 136, 149);
const DETAIL: egui::Color32 = egui::Color32::from_rgb(82, 92, 105);
const MUTED_TEXT: egui::Color32 = egui::Color32::from_rgb(157, 166, 177);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerView {
    Front,
    Rear,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ControlCallout {
    control: BindableControl,
    view: ControllerView,
    anchor: [f32; 2],
    pill: [f32; 2],
}

// Coordinates are normalized within each controller view. On the rear view,
// the controller's right grips appear on the left side of the image.
const CONTROL_CALLOUTS: [ControlCallout; 5] = [
    ControlCallout {
        control: BindableControl::QuickAccess,
        view: ControllerView::Front,
        anchor: [0.50, 0.70],
        pill: [0.50, 0.92],
    },
    ControlCallout {
        control: BindableControl::R4,
        view: ControllerView::Rear,
        anchor: [0.34, 0.53],
        pill: [0.08, 0.47],
    },
    ControlCallout {
        control: BindableControl::R5,
        view: ControllerView::Rear,
        anchor: [0.31, 0.69],
        pill: [0.08, 0.72],
    },
    ControlCallout {
        control: BindableControl::L4,
        view: ControllerView::Rear,
        anchor: [0.66, 0.53],
        pill: [0.92, 0.47],
    },
    ControlCallout {
        control: BindableControl::L5,
        view: ControllerView::Rear,
        anchor: [0.69, 0.69],
        pill: [0.92, 0.72],
    },
];

pub fn run() -> Result<(), String> {
    let path = default_store_path()?;
    let store = load_or_create_store(&path)?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        "Steam Controller Bridge Bindings",
        options,
        Box::new(move |creation| {
            configure_visuals(&creation.egui_ctx);
            Ok(Box::new(BindingsEditor::new(path, store)))
        }),
    )
    .map_err(|error| error.to_string())
}

struct BindingsEditor {
    path: std::path::PathBuf,
    store: BindingStore,
    selected: usize,
    selected_control: BindableControl,
    capturing: Option<BindableControl>,
    message: Option<String>,
}

impl BindingsEditor {
    fn new(path: std::path::PathBuf, store: BindingStore) -> Self {
        Self {
            path,
            store,
            selected: 0,
            selected_control: BindableControl::QuickAccess,
            capturing: None,
            message: None,
        }
    }

    fn unique_name(&self, base: &str) -> String {
        if !self
            .store
            .profiles
            .iter()
            .any(|profile| profile.name.eq_ignore_ascii_case(base))
        {
            return base.to_owned();
        }
        (2..=desktop_bindings::MAX_PROFILES + 1)
            .map(|number| format!("{base} {number}"))
            .find(|name| {
                !self
                    .store
                    .profiles
                    .iter()
                    .any(|profile| profile.name.eq_ignore_ascii_case(name))
            })
            .expect("an unused bounded profile name exists")
    }

    fn add_profile(&mut self) {
        if self.store.profiles.len() >= desktop_bindings::MAX_PROFILES {
            self.message = Some("At most 32 profiles are supported".to_owned());
            return;
        }
        let id = self.store.next_profile_id();
        let name = self.unique_name("New Profile");
        self.store.profiles.push(BindingProfile {
            id,
            name,
            bindings: ControlBindings::default(),
        });
        self.selected = self.store.profiles.len() - 1;
        self.capturing = None;
    }

    fn duplicate_profile(&mut self) {
        if self.store.profiles.len() >= desktop_bindings::MAX_PROFILES {
            self.message = Some("At most 32 profiles are supported".to_owned());
            return;
        }
        let Some(source) = self.store.profiles.get(self.selected).cloned() else {
            return;
        };
        let mut duplicate = source;
        duplicate.id = self.store.next_profile_id();
        duplicate.name = self.unique_name(&format!("{} Copy", duplicate.name));
        self.store.profiles.push(duplicate);
        self.selected = self.store.profiles.len() - 1;
        self.capturing = None;
    }

    fn delete_profile(&mut self) {
        if self.store.profiles.len() == 1 {
            self.message = Some("The last profile cannot be deleted".to_owned());
            return;
        }
        self.store.profiles.remove(self.selected);
        self.selected = self.selected.min(self.store.profiles.len() - 1);
        self.capturing = None;
    }

    fn capture_key(&mut self, ctx: &egui::Context) {
        let Some(control) = self.capturing else {
            return;
        };
        let captured = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } => keyboard_key(*key).map(|key| (key, *modifiers)),
                _ => None,
            })
        });
        let Some((key, modifiers)) = captured else {
            return;
        };
        let mut binding_modifiers = BTreeSet::new();
        if modifiers.mac_cmd {
            binding_modifiers.insert(Modifier::Command);
        }
        if modifiers.ctrl {
            binding_modifiers.insert(Modifier::Control);
        }
        if modifiers.alt {
            binding_modifiers.insert(Modifier::Option);
        }
        if modifiers.shift {
            binding_modifiers.insert(Modifier::Shift);
        }
        *self.store.profiles[self.selected].bindings.get_mut(control) =
            Some(BindingAction::KeyChord {
                key,
                modifiers: binding_modifiers,
            });
        self.capturing = None;
    }

    fn profile_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("PROFILE").small().color(MUTED_TEXT));
            egui::ComboBox::from_id_salt("profile-picker")
                .width(180.0)
                .selected_text(&self.store.profiles[self.selected].name)
                .show_ui(ui, |ui| {
                    for (index, profile) in self.store.profiles.iter().enumerate() {
                        if ui
                            .selectable_label(index == self.selected, &profile.name)
                            .clicked()
                        {
                            self.selected = index;
                            self.capturing = None;
                        }
                    }
                });
            if ui.button("New").clicked() {
                self.add_profile();
            }
            if ui.button("Duplicate").clicked() {
                self.duplicate_profile();
            }
            if ui
                .add_enabled(self.store.profiles.len() > 1, egui::Button::new("Delete"))
                .clicked()
            {
                self.delete_profile();
            }
            ui.separator();
            ui.label("Name");
            ui.add(
                egui::TextEdit::singleline(&mut self.store.profiles[self.selected].name)
                    .desired_width(220.0),
            );
        });
    }

    fn controller_canvas(&mut self, ui: &mut egui::Ui) {
        let desired = egui::vec2(
            (ui.available_width() - INSPECTOR_WIDTH - 18.0).max(650.0),
            ui.available_height().max(510.0),
        );
        let (canvas_rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
        let painter = ui.painter_at(canvas_rect);
        painter.rect_filled(canvas_rect, 14.0, CANVAS_BACKGROUND);
        painter.rect_stroke(
            canvas_rect,
            14.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(46, 52, 62)),
            egui::StrokeKind::Inside,
        );

        let inner = canvas_rect.shrink2(egui::vec2(18.0, 14.0));
        let title_height = 34.0;
        let views = egui::Rect::from_min_max(
            egui::pos2(inner.left(), inner.top() + title_height),
            inner.max,
        );
        let gap = 14.0;
        let view_width = (views.width() - gap) * 0.5;
        let front_cell =
            egui::Rect::from_min_size(views.min, egui::vec2(view_width, views.height()));
        let rear_cell = egui::Rect::from_min_size(
            egui::pos2(front_cell.right() + gap, views.top()),
            egui::vec2(view_width, views.height()),
        );
        let front = fitted_controller_view(front_cell);
        let rear = fitted_controller_view(rear_cell);

        painter.text(
            egui::pos2(front.center().x, inner.top() + 3.0),
            egui::Align2::CENTER_TOP,
            "FRONT",
            egui::FontId::proportional(11.0),
            MUTED_TEXT,
        );
        painter.text(
            egui::pos2(rear.center().x, inner.top() + 3.0),
            egui::Align2::CENTER_TOP,
            "REAR",
            egui::FontId::proportional(11.0),
            MUTED_TEXT,
        );

        draw_front_controller(&painter, front, self.selected_control);
        draw_rear_controller(&painter, rear, self.selected_control);

        self.draw_callouts(ui, &painter, front, rear);
    }

    fn draw_callouts(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        front: egui::Rect,
        rear: egui::Rect,
    ) {
        let bindings = &self.store.profiles[self.selected].bindings;
        let mut clicked = None;
        for callout in CONTROL_CALLOUTS {
            let view_rect = match callout.view {
                ControllerView::Front => front,
                ControllerView::Rear => rear,
            };
            let anchor = normalized_point(view_rect, callout.anchor);
            let pill_center = normalized_point(view_rect, callout.pill);
            let selected = self.selected_control == callout.control;
            let summary = binding_summary(bindings.get(callout.control));
            let pill_width: f32 = if callout.control == BindableControl::QuickAccess {
                164.0
            } else {
                112.0
            };
            let pill_rect = egui::Rect::from_center_size(
                pill_center,
                egui::vec2(pill_width.min(view_rect.width() * 0.45), 42.0),
            );
            let line_end = nearest_point_on_rect(pill_rect, anchor);
            painter.line_segment(
                [anchor, line_end],
                egui::Stroke::new(
                    if selected { 2.0 } else { 1.2 },
                    if selected { ACCENT } else { DETAIL },
                ),
            );
            painter.circle_filled(
                anchor,
                if selected { 4.0 } else { 3.0 },
                if selected { ACCENT } else { OUTLINE },
            );

            let button = egui::Button::new(
                egui::RichText::new(format!("{}\n{summary}", callout.control.label()))
                    .size(11.0)
                    .color(if selected {
                        egui::Color32::BLACK
                    } else {
                        egui::Color32::WHITE
                    }),
            )
            .fill(if selected { ACCENT } else { SURFACE_RAISED })
            .stroke(egui::Stroke::new(
                1.0,
                if selected { ACCENT } else { DETAIL },
            ))
            .corner_radius(8.0);
            if ui.put(pill_rect, button).clicked() {
                clicked = Some(callout.control);
            }

            let hotspot = egui::Rect::from_center_size(anchor, egui::vec2(34.0, 48.0));
            if ui
                .interact(
                    hotspot,
                    ui.id().with(("controller-hotspot", callout.control)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                clicked = Some(callout.control);
            }
        }
        if let Some(control) = clicked {
            self.selected_control = control;
            self.capturing = None;
        }
    }

    fn binding_inspector(&mut self, ui: &mut egui::Ui) {
        let control = self.selected_control;
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(50, 58, 68)))
            .corner_radius(12.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_min_width(INSPECTOR_WIDTH - 34.0);
                ui.set_max_width(INSPECTOR_WIDTH - 34.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("SELECTED CONTROL")
                            .small()
                            .color(MUTED_TEXT),
                    );
                    ui.heading(control.label());
                    ui.label(
                        egui::RichText::new(control_description(control))
                            .size(12.0)
                            .color(MUTED_TEXT),
                    );
                    ui.add_space(18.0);
                    self.binding_editor(ui, control);
                });
            });
    }

    fn binding_editor(&mut self, ui: &mut egui::Ui, control: BindableControl) {
        let current = self.store.profiles[self.selected]
            .bindings
            .get(control)
            .cloned();
        let mut kind = match current {
            None => 0,
            Some(BindingAction::KeyChord { .. }) => 1,
            Some(BindingAction::MouseButton { .. }) => 2,
        };
        ui.label("Action");
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut kind, 0, "Unbound");
            ui.selectable_value(&mut kind, 1, "Key chord");
            ui.selectable_value(&mut kind, 2, "Mouse button");
        });
        ui.add_space(16.0);

        let mut replacement = match (kind, current) {
            (1, Some(action @ BindingAction::KeyChord { .. }))
            | (2, Some(action @ BindingAction::MouseButton { .. })) => Some(action),
            (1, _) => Some(BindingAction::KeyChord {
                key: KeyboardKey::F5,
                modifiers: BTreeSet::new(),
            }),
            (2, _) => Some(BindingAction::MouseButton {
                button: MouseButton::Middle,
            }),
            _ => None,
        };

        match replacement.as_mut() {
            Some(BindingAction::KeyChord { key, modifiers }) => {
                self.key_chord_editor(ui, control, key, modifiers);
            }
            Some(BindingAction::MouseButton { button }) => {
                mouse_button_editor(ui, control, button);
            }
            None => {
                ui.label(
                    egui::RichText::new(
                        "This control will continue to be ignored by desktop bindings.",
                    )
                    .color(MUTED_TEXT),
                );
            }
        }
        *self.store.profiles[self.selected].bindings.get_mut(control) = replacement;
    }

    fn key_chord_editor(
        &mut self,
        ui: &mut egui::Ui,
        control: BindableControl,
        key: &mut KeyboardKey,
        modifiers: &mut BTreeSet<Modifier>,
    ) {
        ui.label("Key");
        egui::ComboBox::from_id_salt(("key", control))
            .width(ui.available_width())
            .selected_text(key.label())
            .show_ui(ui, |ui| {
                for candidate in KeyboardKey::ALL {
                    ui.selectable_value(key, *candidate, candidate.label());
                }
            });
        ui.add_space(12.0);
        ui.label("Modifiers");
        egui::Grid::new(("modifiers", control))
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for (index, modifier) in Modifier::ALL.into_iter().enumerate() {
                    let mut selected = modifiers.contains(&modifier);
                    if ui.checkbox(&mut selected, modifier.label()).changed() {
                        if selected {
                            modifiers.insert(modifier);
                        } else {
                            modifiers.remove(&modifier);
                        }
                    }
                    if index % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        ui.add_space(16.0);
        let capture_label = if self.capturing == Some(control) {
            "Press any supported key…"
        } else {
            "Capture key chord"
        };
        let capture =
            egui::Button::new(capture_label).min_size(egui::vec2(ui.available_width(), 34.0));
        if ui.add(capture).clicked() {
            self.capturing = Some(control);
        }
        if self.capturing == Some(control) {
            ui.label(
                egui::RichText::new("Press a key with any modifiers you want to include.")
                    .small()
                    .color(ACCENT),
            );
        }
    }

    fn save_and_close(&mut self, ctx: &egui::Context) {
        for profile in &mut self.store.profiles {
            profile.name = profile.name.trim().to_owned();
        }
        match save_store(&self.path, &self.store) {
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(error) => self.message = Some(error),
        }
    }
}

impl eframe::App for BindingsEditor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.capture_key(ui.ctx());
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(14, 17, 21))
            .inner_margin(egui::Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.heading("Controller bindings");
                ui.label(
                    egui::RichText::new(
                        "Select an extra controller button, then choose its keyboard or mouse action.",
                    )
                    .color(MUTED_TEXT),
                );
                ui.add_space(12.0);
                self.profile_toolbar(ui);
                ui.add_space(12.0);

                let footer_height = 48.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), ui.available_height() - footer_height),
                    egui::Layout::left_to_right(egui::Align::Min),
                    |ui| {
                        self.controller_canvas(ui);
                        ui.add_space(14.0);
                        self.binding_inspector(ui);
                    },
                );

                ui.add_space(8.0);
                let footer_width = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::vec2(footer_width, 36.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui
                            .add_sized([84.0, 32.0], egui::Button::new("Save").fill(ACCENT))
                            .clicked()
                        {
                            self.save_and_close(ui.ctx());
                        }
                        if ui
                            .add_sized([84.0, 32.0], egui::Button::new("Cancel"))
                            .clicked()
                        {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            if let Some(message) = &self.message {
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 115, 115),
                                    message,
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new("Changes take effect after Save.")
                                        .small()
                                        .color(MUTED_TEXT),
                                );
                            }
                        });
                    },
                );
            });
    }
}

fn mouse_button_editor(ui: &mut egui::Ui, control: BindableControl, button: &mut MouseButton) {
    ui.label("Mouse button");
    egui::ComboBox::from_id_salt(("mouse", control))
        .width(ui.available_width())
        .selected_text(button.label())
        .show_ui(ui, |ui| {
            for candidate in MouseButton::ALL {
                ui.selectable_value(button, candidate, candidate.label());
            }
        });
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("The button stays held for as long as the controller control is held.")
            .small()
            .color(MUTED_TEXT),
    );
}

fn configure_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(14, 17, 21);
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = egui::Color32::from_rgb(20, 24, 30);
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(8, 28, 32));
    visuals.widgets.inactive.bg_fill = SURFACE_RAISED;
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(45, 54, 65);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(50, 61, 73);
    ctx.set_visuals(visuals);
}

fn control_description(control: BindableControl) -> &'static str {
    match control {
        BindableControl::L4 => "Upper left rear grip",
        BindableControl::L5 => "Lower left rear grip",
        BindableControl::R4 => "Upper right rear grip",
        BindableControl::R5 => "Lower right rear grip",
        BindableControl::QuickAccess => "Front Quick Access button",
    }
}

fn binding_summary(action: Option<&BindingAction>) -> String {
    match action {
        None => "Unbound".to_owned(),
        Some(BindingAction::MouseButton { button }) => format!("{} Mouse", button.label()),
        Some(BindingAction::KeyChord { key, modifiers }) => {
            let mut summary = String::new();
            for modifier in Modifier::ALL {
                if modifiers.contains(&modifier) {
                    summary.push_str(match modifier {
                        Modifier::Command => "⌘",
                        Modifier::Control => "⌃",
                        Modifier::Option => "⌥",
                        Modifier::Shift => "⇧",
                    });
                }
            }
            summary.push_str(&key.label());
            summary
        }
    }
}

fn normalized_point(rect: egui::Rect, point: [f32; 2]) -> egui::Pos2 {
    egui::pos2(
        egui::lerp(rect.x_range(), point[0]),
        egui::lerp(rect.y_range(), point[1]),
    )
}

fn fitted_controller_view(cell: egui::Rect) -> egui::Rect {
    let height = (cell.width() * 0.96).min(cell.height());
    egui::Rect::from_center_size(cell.center(), egui::vec2(cell.width(), height))
}

fn nearest_point_on_rect(rect: egui::Rect, point: egui::Pos2) -> egui::Pos2 {
    egui::pos2(
        point.x.clamp(rect.left(), rect.right()),
        point.y.clamp(rect.top(), rect.bottom()),
    )
}

fn controller_body(painter: &egui::Painter, rect: egui::Rect) {
    let points = [
        [0.31, 0.20],
        [0.21, 0.21],
        [0.15, 0.28],
        [0.11, 0.47],
        [0.10, 0.66],
        [0.13, 0.78],
        [0.19, 0.82],
        [0.24, 0.78],
        [0.29, 0.58],
        [0.37, 0.55],
        [0.50, 0.56],
        [0.63, 0.55],
        [0.71, 0.58],
        [0.76, 0.78],
        [0.81, 0.82],
        [0.87, 0.78],
        [0.90, 0.66],
        [0.89, 0.47],
        [0.85, 0.28],
        [0.79, 0.21],
        [0.69, 0.20],
        [0.62, 0.23],
        [0.38, 0.23],
    ]
    .map(|point| normalized_point(rect, point));
    painter.add(egui::Shape::Path(egui::epaint::PathShape {
        points: points.to_vec(),
        closed: true,
        fill: SURFACE,
        stroke: egui::Stroke::new(1.8, OUTLINE).into(),
    }));
}

fn draw_front_controller(painter: &egui::Painter, rect: egui::Rect, selected: BindableControl) {
    controller_body(painter, rect);
    let scale = rect.width().min(rect.height());
    let detail = egui::Stroke::new(1.35, DETAIL);
    let control_stroke = if selected == BindableControl::QuickAccess {
        egui::Stroke::new(2.4, ACCENT)
    } else {
        egui::Stroke::new(1.5, OUTLINE)
    };

    for (center_x, tilt) in [(0.32, -0.018), (0.68, 0.018)] {
        let points = [
            [center_x - 0.12 + tilt, 0.47],
            [center_x + 0.10 + tilt, 0.45],
            [center_x + 0.11 - tilt, 0.64],
            [center_x - 0.10 - tilt, 0.66],
        ]
        .map(|point| normalized_point(rect, point));
        painter.add(egui::Shape::Path(egui::epaint::PathShape {
            points: points.to_vec(),
            closed: true,
            fill: egui::Color32::from_rgb(24, 28, 34),
            stroke: detail.into(),
        }));
    }

    for center in [[0.39, 0.39], [0.61, 0.39]] {
        let center = normalized_point(rect, center);
        painter.circle_filled(center, scale * 0.044, egui::Color32::from_rgb(22, 26, 32));
        painter.circle_stroke(center, scale * 0.044, egui::Stroke::new(1.4, OUTLINE));
        painter.circle_stroke(center, scale * 0.030, detail);
    }

    let dpad_center = normalized_point(rect, [0.24, 0.34]);
    let d = scale * 0.029;
    let horizontal = egui::Rect::from_center_size(dpad_center, egui::vec2(d * 3.1, d));
    let vertical = egui::Rect::from_center_size(dpad_center, egui::vec2(d, d * 3.1));
    painter.rect_filled(horizontal, 3.0, egui::Color32::from_rgb(24, 28, 34));
    painter.rect_filled(vertical, 3.0, egui::Color32::from_rgb(24, 28, 34));
    painter.rect_stroke(horizontal, 3.0, detail, egui::StrokeKind::Inside);
    painter.rect_stroke(vertical, 3.0, detail, egui::StrokeKind::Inside);

    for position in [[0.76, 0.30], [0.81, 0.35], [0.71, 0.35], [0.76, 0.40]] {
        let center = normalized_point(rect, position);
        painter.circle_filled(center, scale * 0.024, egui::Color32::from_rgb(24, 28, 34));
        painter.circle_stroke(center, scale * 0.024, detail);
    }

    for x in [0.42, 0.58] {
        let center = normalized_point(rect, [x, 0.29]);
        let button = egui::Rect::from_center_size(center, egui::vec2(scale * 0.055, scale * 0.020));
        painter.rect_stroke(button, 5.0, detail, egui::StrokeKind::Inside);
    }
    painter.circle_stroke(normalized_point(rect, [0.50, 0.31]), scale * 0.025, detail);
    let quick = egui::Rect::from_center_size(
        normalized_point(rect, [0.50, 0.70]),
        egui::vec2(scale * 0.075, scale * 0.028),
    );
    painter.rect_filled(quick, 6.0, egui::Color32::from_rgb(22, 26, 32));
    painter.rect_stroke(quick, 6.0, control_stroke, egui::StrokeKind::Inside);
}

fn draw_rear_controller(painter: &egui::Painter, rect: egui::Rect, selected: BindableControl) {
    controller_body(painter, rect);
    let scale = rect.width().min(rect.height());
    let detail = egui::Stroke::new(1.35, DETAIL);

    for (x, side) in [(0.22, -1.0_f32), (0.78, 1.0_f32)] {
        let center = normalized_point(rect, [x, 0.25]);
        let shoulder =
            egui::Rect::from_center_size(center, egui::vec2(scale * 0.15, scale * 0.075));
        painter.rect_filled(shoulder, 12.0, egui::Color32::from_rgb(25, 29, 36));
        painter.rect_stroke(
            shoulder,
            12.0,
            egui::Stroke::new(1.4, OUTLINE),
            egui::StrokeKind::Inside,
        );
        let split_x = center.x + side * scale * 0.025;
        painter.line_segment(
            [
                egui::pos2(split_x, shoulder.top()),
                egui::pos2(split_x, shoulder.bottom()),
            ],
            detail,
        );
    }
    let usb = egui::Rect::from_center_size(
        normalized_point(rect, [0.50, 0.225]),
        egui::vec2(scale * 0.062, scale * 0.012),
    );
    painter.rect_stroke(usb, 3.0, detail, egui::StrokeKind::Inside);

    let dock = egui::Rect::from_center_size(
        normalized_point(rect, [0.50, 0.35]),
        egui::vec2(scale * 0.13, scale * 0.038),
    );
    painter.rect_stroke(dock, 7.0, detail, egui::StrokeKind::Inside);
    for x in [0.47, 0.50, 0.53] {
        painter.circle_filled(normalized_point(rect, [x, 0.35]), scale * 0.006, DETAIL);
    }

    for callout in CONTROL_CALLOUTS
        .into_iter()
        .filter(|callout| callout.view == ControllerView::Rear)
    {
        let center = normalized_point(rect, callout.anchor);
        let upper = matches!(callout.control, BindableControl::L4 | BindableControl::R4);
        let size = egui::vec2(scale * 0.070, scale * if upper { 0.115 } else { 0.105 });
        let paddle = egui::Rect::from_center_size(center, size);
        let is_selected = selected == callout.control;
        painter.rect_filled(
            paddle,
            size.x * 0.45,
            if is_selected {
                egui::Color32::from_rgb(34, 67, 73)
            } else {
                egui::Color32::from_rgb(23, 27, 33)
            },
        );
        painter.rect_stroke(
            paddle,
            size.x * 0.45,
            egui::Stroke::new(
                if is_selected { 2.4 } else { 1.5 },
                if is_selected { ACCENT } else { OUTLINE },
            ),
            egui::StrokeKind::Inside,
        );
    }

    for x in [0.20, 0.80] {
        painter.line_segment(
            [
                normalized_point(rect, [x, 0.44]),
                normalized_point(rect, [x, 0.72]),
            ],
            egui::Stroke::new(1.0, DETAIL),
        );
    }
    for position in [[0.18, 0.47], [0.20, 0.74], [0.82, 0.47], [0.80, 0.74]] {
        painter.circle_stroke(normalized_point(rect, position), scale * 0.009, detail);
    }
}

#[allow(clippy::too_many_lines)]
const fn keyboard_key(key: egui::Key) -> Option<KeyboardKey> {
    Some(match key {
        egui::Key::A => KeyboardKey::A,
        egui::Key::B => KeyboardKey::B,
        egui::Key::C => KeyboardKey::C,
        egui::Key::D => KeyboardKey::D,
        egui::Key::E => KeyboardKey::E,
        egui::Key::F => KeyboardKey::F,
        egui::Key::G => KeyboardKey::G,
        egui::Key::H => KeyboardKey::H,
        egui::Key::I => KeyboardKey::I,
        egui::Key::J => KeyboardKey::J,
        egui::Key::K => KeyboardKey::K,
        egui::Key::L => KeyboardKey::L,
        egui::Key::M => KeyboardKey::M,
        egui::Key::N => KeyboardKey::N,
        egui::Key::O => KeyboardKey::O,
        egui::Key::P => KeyboardKey::P,
        egui::Key::Q => KeyboardKey::Q,
        egui::Key::R => KeyboardKey::R,
        egui::Key::S => KeyboardKey::S,
        egui::Key::T => KeyboardKey::T,
        egui::Key::U => KeyboardKey::U,
        egui::Key::V => KeyboardKey::V,
        egui::Key::W => KeyboardKey::W,
        egui::Key::X => KeyboardKey::X,
        egui::Key::Y => KeyboardKey::Y,
        egui::Key::Z => KeyboardKey::Z,
        egui::Key::Num0 => KeyboardKey::Digit0,
        egui::Key::Num1 => KeyboardKey::Digit1,
        egui::Key::Num2 => KeyboardKey::Digit2,
        egui::Key::Num3 => KeyboardKey::Digit3,
        egui::Key::Num4 => KeyboardKey::Digit4,
        egui::Key::Num5 => KeyboardKey::Digit5,
        egui::Key::Num6 => KeyboardKey::Digit6,
        egui::Key::Num7 => KeyboardKey::Digit7,
        egui::Key::Num8 => KeyboardKey::Digit8,
        egui::Key::Num9 => KeyboardKey::Digit9,
        egui::Key::F1 => KeyboardKey::F1,
        egui::Key::F2 => KeyboardKey::F2,
        egui::Key::F3 => KeyboardKey::F3,
        egui::Key::F4 => KeyboardKey::F4,
        egui::Key::F5 => KeyboardKey::F5,
        egui::Key::F6 => KeyboardKey::F6,
        egui::Key::F7 => KeyboardKey::F7,
        egui::Key::F8 => KeyboardKey::F8,
        egui::Key::F9 => KeyboardKey::F9,
        egui::Key::F10 => KeyboardKey::F10,
        egui::Key::F11 => KeyboardKey::F11,
        egui::Key::F12 => KeyboardKey::F12,
        egui::Key::F13 => KeyboardKey::F13,
        egui::Key::F14 => KeyboardKey::F14,
        egui::Key::F15 => KeyboardKey::F15,
        egui::Key::F16 => KeyboardKey::F16,
        egui::Key::F17 => KeyboardKey::F17,
        egui::Key::F18 => KeyboardKey::F18,
        egui::Key::F19 => KeyboardKey::F19,
        egui::Key::F20 => KeyboardKey::F20,
        egui::Key::F21 => KeyboardKey::F21,
        egui::Key::F22 => KeyboardKey::F22,
        egui::Key::F23 => KeyboardKey::F23,
        egui::Key::F24 => KeyboardKey::F24,
        egui::Key::Escape => KeyboardKey::Escape,
        egui::Key::Tab => KeyboardKey::Tab,
        egui::Key::Enter => KeyboardKey::Return,
        egui::Key::Space => KeyboardKey::Space,
        egui::Key::Backspace => KeyboardKey::Backspace,
        egui::Key::Delete => KeyboardKey::Delete,
        egui::Key::Insert => KeyboardKey::Insert,
        egui::Key::Home => KeyboardKey::Home,
        egui::Key::End => KeyboardKey::End,
        egui::Key::PageUp => KeyboardKey::PageUp,
        egui::Key::PageDown => KeyboardKey::PageDown,
        egui::Key::ArrowLeft => KeyboardKey::ArrowLeft,
        egui::Key::ArrowRight => KeyboardKey::ArrowRight,
        egui::Key::ArrowUp => KeyboardKey::ArrowUp,
        egui::Key::ArrowDown => KeyboardKey::ArrowDown,
        egui::Key::Backtick => KeyboardKey::Grave,
        egui::Key::Minus => KeyboardKey::Minus,
        egui::Key::Equals | egui::Key::Plus => KeyboardKey::Equal,
        egui::Key::OpenBracket => KeyboardKey::LeftBracket,
        egui::Key::CloseBracket => KeyboardKey::RightBracket,
        egui::Key::Backslash | egui::Key::Pipe => KeyboardKey::Backslash,
        egui::Key::Semicolon | egui::Key::Colon => KeyboardKey::Semicolon,
        egui::Key::Quote => KeyboardKey::Quote,
        egui::Key::Comma => KeyboardKey::Comma,
        egui::Key::Period => KeyboardKey::Period,
        egui::Key::Slash | egui::Key::Questionmark => KeyboardKey::Slash,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callouts_cover_each_bindable_control_once() {
        let controls = CONTROL_CALLOUTS
            .iter()
            .map(|callout| callout.control)
            .collect::<BTreeSet<_>>();
        assert_eq!(controls.len(), BindableControl::ALL.len());
        for control in BindableControl::ALL {
            assert!(controls.contains(&control));
        }
    }

    #[test]
    fn rear_callouts_match_physical_view_orientation() {
        let r4 = CONTROL_CALLOUTS
            .iter()
            .find(|callout| callout.control == BindableControl::R4)
            .unwrap();
        let l4 = CONTROL_CALLOUTS
            .iter()
            .find(|callout| callout.control == BindableControl::L4)
            .unwrap();
        assert_eq!(r4.view, ControllerView::Rear);
        assert_eq!(l4.view, ControllerView::Rear);
        assert!(r4.anchor[0] < l4.anchor[0]);
    }

    #[test]
    fn binding_summaries_are_compact_and_mac_native() {
        let modifiers = [Modifier::Command, Modifier::Shift]
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            binding_summary(Some(&BindingAction::KeyChord {
                key: KeyboardKey::F5,
                modifiers,
            })),
            "⌘⇧F5"
        );
        assert_eq!(
            binding_summary(Some(&BindingAction::MouseButton {
                button: MouseButton::Middle,
            })),
            "Middle Mouse"
        );
        assert_eq!(binding_summary(None), "Unbound");
    }

    #[test]
    fn controller_render_keeps_its_aspect_in_a_tall_canvas() {
        let cell = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 500.0));
        let fitted = fitted_controller_view(cell);
        assert!((fitted.width() - 300.0).abs() < f32::EPSILON);
        assert!((fitted.height() - 288.0).abs() < f32::EPSILON);
        assert_eq!(fitted.center(), cell.center());
    }
}
