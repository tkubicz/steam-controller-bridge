use std::collections::BTreeSet;
use std::sync::OnceLock;

use desktop_bindings::{
    default_store_path, load_or_create_store, save_store, BindableControl, BindingAction,
    BindingProfile, BindingStore, ControlBindings, KeyboardKey, Modifier, MouseButton,
};
use eframe::egui;

const WINDOW_SIZE: [f32; 2] = [1260.0, 720.0];
const MIN_WINDOW_SIZE: [f32; 2] = [1080.0, 660.0];
const INSPECTOR_WIDTH: f32 = 300.0;
const COLUMN_GAP: f32 = 16.0;
const CANVAS_MIN_WIDTH: f32 = 620.0;
const ACCENT: egui::Color32 = egui::Color32::from_rgb(84, 211, 224);
const CANVAS_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(18, 21, 26);
const CANVAS_BORDER: egui::Color32 = egui::Color32::from_rgb(46, 52, 62);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(29, 34, 42);
const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(36, 42, 51);
const INSET: egui::Color32 = egui::Color32::from_rgb(22, 26, 32);
const OUTLINE: egui::Color32 = egui::Color32::from_rgb(126, 136, 149);
const DETAIL: egui::Color32 = egui::Color32::from_rgb(82, 92, 105);
const MUTED_TEXT: egui::Color32 = egui::Color32::from_rgb(157, 166, 177);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerView {
    Front,
    Rear,
}

/// Where a control's label sits relative to the controller drawing it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LabelSide {
    Left,
    Right,
    Below,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ControlCallout {
    control: BindableControl,
    view: ControllerView,
    side: LabelSide,
    /// Vertical position of the label inside the drawing, normalized. The
    /// labels are spread further apart than the paddles they point at so that
    /// two stacked labels never collide.
    label_y: f32,
}

// On the rear view the controller's right grip appears on the left of the
// image, so `R*` labels sit on the left and `L*` labels on the right.
const CONTROL_CALLOUTS: [ControlCallout; 5] = [
    ControlCallout {
        control: BindableControl::QuickAccess,
        view: ControllerView::Front,
        side: LabelSide::Below,
        label_y: 0.0,
    },
    ControlCallout {
        control: BindableControl::R4,
        view: ControllerView::Rear,
        side: LabelSide::Left,
        label_y: 0.49,
    },
    ControlCallout {
        control: BindableControl::R5,
        view: ControllerView::Rear,
        side: LabelSide::Left,
        label_y: 0.81,
    },
    ControlCallout {
        control: BindableControl::L4,
        view: ControllerView::Rear,
        side: LabelSide::Right,
        label_y: 0.49,
    },
    ControlCallout {
        control: BindableControl::L5,
        view: ControllerView::Rear,
        side: LabelSide::Right,
        label_y: 0.81,
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

    fn select_control(&mut self, control: BindableControl) {
        self.selected_control = control;
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

    fn controller_canvas(&mut self, ui: &mut egui::Ui, width: f32) {
        let desired = egui::vec2(width, ui.available_height());
        let (canvas_rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
        let painter = ui.painter_at(canvas_rect);
        painter.rect_filled(canvas_rect, 14.0, CANVAS_BACKGROUND);
        painter.rect_stroke(
            canvas_rect,
            14.0,
            egui::Stroke::new(1.0, CANVAS_BORDER),
            egui::StrokeKind::Inside,
        );

        let layout = CanvasLayout::new(canvas_rect);
        for (view, caption) in [
            (ControllerView::Front, "FRONT"),
            (ControllerView::Rear, "REAR"),
        ] {
            painter.text(
                egui::pos2(layout.body(view).center().x, layout.caption_top),
                egui::Align2::CENTER_TOP,
                caption,
                egui::FontId::proportional(11.0),
                MUTED_TEXT,
            );
        }

        // Hit testing happens before painting so the artwork can show hover
        // feedback, and so the leader lines end up underneath the controls
        // they point at.
        let hovered = self.controller_hotspots(ui, &layout);
        controller_body(&painter, layout.front);
        controller_body(&painter, layout.rear);
        self.draw_leaders(&painter, &layout);
        draw_front_face(&painter, layout.front, self.selected_control, hovered);
        draw_rear_face(&painter, layout.rear, self.selected_control, hovered);
        self.draw_labels(ui, &layout);

        painter.text(
            egui::pos2(canvas_rect.center().x, canvas_rect.bottom() - 16.0),
            egui::Align2::CENTER_BOTTOM,
            "Click a highlighted control, or its label, to edit that binding.",
            egui::FontId::proportional(11.0),
            DETAIL,
        );
    }

    /// Makes the controls drawn on the controller clickable, and reports which
    /// one the pointer is over.
    fn controller_hotspots(
        &mut self,
        ui: &mut egui::Ui,
        layout: &CanvasLayout,
    ) -> Option<BindableControl> {
        let mut hovered = None;
        let mut clicked = None;
        for callout in CONTROL_CALLOUTS {
            let rect = control_rect(layout.view(callout.view), callout.control).expand(4.0);
            let response = ui
                .interact(
                    rect,
                    ui.id().with(("controller-hotspot", callout.control)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.hovered() {
                hovered = Some(callout.control);
            }
            if response.clicked() {
                clicked = Some(callout.control);
            }
        }
        if let Some(control) = clicked {
            self.select_control(control);
        }
        hovered
    }

    fn draw_leaders(&self, painter: &egui::Painter, layout: &CanvasLayout) {
        for callout in CONTROL_CALLOUTS {
            let target = control_rect(layout.view(callout.view), callout.control);
            let label = layout.label(callout);
            let selected = self.selected_control == callout.control;
            painter.line_segment(
                [
                    nearest_point_on_rect(label, target.center()),
                    nearest_point_on_rect(target, label.center()),
                ],
                egui::Stroke::new(
                    if selected { 1.8 } else { 1.1 },
                    if selected { ACCENT } else { DETAIL },
                ),
            );
        }
    }

    fn draw_labels(&mut self, ui: &mut egui::Ui, layout: &CanvasLayout) {
        let mut clicked = None;
        for callout in CONTROL_CALLOUTS {
            let selected = self.selected_control == callout.control;
            let summary = binding_summary(
                self.store.profiles[self.selected]
                    .bindings
                    .get(callout.control),
            );
            let button = egui::Button::new(
                egui::RichText::new(format!("{}\n{summary}", callout.control.label()))
                    .size(10.5)
                    .color(if selected {
                        egui::Color32::from_rgb(7, 31, 35)
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
            if ui.put(layout.label(callout), button).clicked() {
                clicked = Some(callout.control);
            }
        }
        if let Some(control) = clicked {
            self.select_control(control);
        }
    }

    fn binding_inspector(&mut self, ui: &mut egui::Ui) {
        let control = self.selected_control;
        // The card matches the canvas height so the two panes read as one row.
        let card_height = ui.available_height() - 32.0;
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(50, 58, 68)))
            .corner_radius(12.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                let content_width = INSPECTOR_WIDTH - 32.0;
                ui.set_min_width(content_width);
                ui.set_max_width(content_width);
                ui.set_min_height(card_height);
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
        self.show(ui);
    }
}

impl BindingsEditor {
    fn show(&mut self, ui: &mut egui::Ui) {
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

                // Both rows are sized from the width measured here. Letting a
                // row size itself from `available_width` after another one has
                // overflowed would grow the frame past its own margin.
                let content_width = ui.available_width();
                let footer_height = 46.0;
                let canvas_width =
                    (content_width - INSPECTOR_WIDTH - COLUMN_GAP).max(CANVAS_MIN_WIDTH);
                ui.allocate_ui_with_layout(
                    egui::vec2(
                        content_width,
                        (ui.available_height() - footer_height).max(320.0),
                    ),
                    egui::Layout::left_to_right(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        self.controller_canvas(ui, canvas_width);
                        ui.add_space(COLUMN_GAP);
                        self.binding_inspector(ui);
                    },
                );

                ui.add_space(12.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(content_width, 32.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui
                            .add_sized(
                                [84.0, 32.0],
                                egui::Button::new(
                                    egui::RichText::new("Save")
                                        .strong()
                                        .color(egui::Color32::from_rgb(7, 31, 35)),
                                )
                                .fill(ACCENT),
                            )
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

/// egui's bundled fonts have ⌘ but not ⌃, ⌥ or ⇧, which would leave binding
/// summaries full of replacement boxes. Appending a macOS system font that does
/// have them keeps the rest of the interface on egui's own typeface.
fn configure_modifier_glyphs(ctx: &egui::Context) {
    const CANDIDATES: [&str; 3] = [
        "/System/Library/Fonts/Keyboard.ttf",
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/Apple Symbols.ttf",
    ];
    let Some(bytes) = CANDIDATES.iter().find_map(|path| std::fs::read(path).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    let name = "mac-modifier-symbols".to_owned();
    fonts.font_data.insert(
        name.clone(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push(name.clone());
    }
    ctx.set_fonts(fonts);
}

fn configure_visuals(ctx: &egui::Context) {
    configure_modifier_glyphs(ctx);
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

// ---------------------------------------------------------------------------
// Canvas layout
// ---------------------------------------------------------------------------

const LABEL_SIZE: [f32; 2] = [86.0, 40.0];
/// Gap between a label and the controller it points at.
const LABEL_LEAD: f32 = 12.0;
/// Gap between the front and rear groups.
const VIEW_GAP: f32 = 28.0;
/// Height of the FRONT / REAR caption row above each controller.
const CAPTION_ROW: f32 = 22.0;

/// Places both controller drawings and every callout label inside the canvas.
///
/// Each drawing is a square so the artwork below, which is expressed in a unit
/// square, keeps its proportions whatever the window size is. Everything is
/// laid out against the silhouette's bounding box rather than that square, so
/// the drawings are as large as the canvas allows and the labels sit a fixed
/// distance from the controller itself.
struct CanvasLayout {
    front: egui::Rect,
    rear: egui::Rect,
    caption_top: f32,
}

impl CanvasLayout {
    fn new(canvas: egui::Rect) -> Self {
        let inner = canvas.shrink2(egui::vec2(16.0, 14.0));
        let bounds = body_bounds();
        // Width budget: front body, gap, label column, rear body, label column.
        // The rear grips are labelled on both sides.
        let label_columns = 2.0 * (LABEL_SIZE[0] + LABEL_LEAD);
        let by_width = (inner.width() - label_columns - VIEW_GAP) * 0.5 / bounds.width();
        // Height budget: caption row, body, the Quick Access label below the
        // front body.
        let by_height =
            (inner.height() - CAPTION_ROW - LABEL_LEAD - LABEL_SIZE[1]) / bounds.height();
        let side = by_width.min(by_height).max(180.0);

        let body = egui::vec2(side * bounds.width(), side * bounds.height());
        let group_height = CAPTION_ROW + body.y + LABEL_LEAD + LABEL_SIZE[1];
        let caption_top = inner.top() + (inner.height() - group_height).max(0.0) * 0.5;
        let body_top = caption_top + CAPTION_ROW;
        let total_width = 2.0f32.mul_add(body.x, label_columns + VIEW_GAP);
        let front_left = inner.left() + (inner.width() - total_width).max(0.0) * 0.5;
        let rear_left = front_left + body.x + VIEW_GAP + LABEL_SIZE[0] + LABEL_LEAD;
        // The drawing rect is the unit square that puts the silhouette exactly
        // where the budget above says it goes.
        let view_at = |body_left: f32| {
            egui::Rect::from_min_size(
                egui::pos2(
                    bounds.left().mul_add(-side, body_left),
                    bounds.top().mul_add(-side, body_top),
                ),
                egui::vec2(side, side),
            )
        };
        Self {
            front: view_at(front_left),
            rear: view_at(rear_left),
            caption_top,
        }
    }

    fn view(&self, view: ControllerView) -> egui::Rect {
        match view {
            ControllerView::Front => self.front,
            ControllerView::Rear => self.rear,
        }
    }

    /// The silhouette's bounding box on screen, which is what labels and
    /// captions are aligned to.
    fn body(&self, view: ControllerView) -> egui::Rect {
        let view = self.view(view);
        let bounds = body_bounds();
        egui::Rect::from_min_max(
            normalized_point(view, [bounds.left(), bounds.top()]),
            normalized_point(view, [bounds.right(), bounds.bottom()]),
        )
    }

    fn label(&self, callout: ControlCallout) -> egui::Rect {
        let body = self.body(callout.view);
        let size = egui::vec2(LABEL_SIZE[0], LABEL_SIZE[1]);
        let row_top = callout
            .label_y
            .mul_add(body.height(), body.top() - size.y * 0.5);
        let min = match callout.side {
            LabelSide::Left => egui::pos2(body.left() - LABEL_LEAD - size.x, row_top),
            LabelSide::Right => egui::pos2(body.right() + LABEL_LEAD, row_top),
            LabelSide::Below => {
                egui::pos2(body.center().x - size.x * 0.5, body.bottom() + LABEL_LEAD)
            }
        };
        egui::Rect::from_min_size(min, size)
    }
}

fn normalized_point(rect: egui::Rect, point: [f32; 2]) -> egui::Pos2 {
    egui::pos2(
        egui::lerp(rect.x_range(), point[0]),
        egui::lerp(rect.y_range(), point[1]),
    )
}

fn unit_rect(view: egui::Rect, center: [f32; 2], size: [f32; 2]) -> egui::Rect {
    egui::Rect::from_center_size(
        normalized_point(view, center),
        egui::vec2(size[0] * view.width(), size[1] * view.height()),
    )
}

fn nearest_point_on_rect(rect: egui::Rect, point: egui::Pos2) -> egui::Pos2 {
    egui::pos2(
        point.x.clamp(rect.left(), rect.right()),
        point.y.clamp(rect.top(), rect.bottom()),
    )
}

// ---------------------------------------------------------------------------
// Controller artwork
//
// Every coordinate below is normalized inside a unit square that is mapped onto
// a square drawing rect, and is traced from the Steam Controller 2 hardware
// diagram: square trackpads, oval rear grip paddles, and a Quick Access button
// between the trackpads.
// ---------------------------------------------------------------------------

/// Left half of the silhouette, from the top centre down to the bottom centre.
/// The right half is mirrored from it, which keeps the body exactly symmetric.
///
/// Traced from the hardware photograph. The shell is narrowest up at the
/// shoulders; from there the outer edge rakes steadily *outwards* the whole way
/// down, so the silhouette is at its widest low in the grips before the tips
/// round off. A straight bottom edge spans the notch between the two grips.
const BODY_HALF: [[f32; 2]; 30] = [
    [0.500, 0.176],
    [0.370, 0.176],
    [0.272, 0.181],
    [0.212, 0.192],
    [0.172, 0.208],
    [0.142, 0.232],
    [0.120, 0.264],
    [0.104, 0.302],
    [0.092, 0.348],
    [0.082, 0.400],
    [0.074, 0.456],
    [0.068, 0.512],
    [0.063, 0.570],
    [0.060, 0.628],
    [0.058, 0.686],
    [0.062, 0.740],
    [0.074, 0.788],
    [0.096, 0.826],
    [0.130, 0.850],
    [0.172, 0.861],
    [0.214, 0.856],
    [0.256, 0.844],
    [0.292, 0.822],
    [0.320, 0.792],
    [0.340, 0.756],
    [0.352, 0.720],
    [0.356, 0.700],
    [0.390, 0.690],
    [0.444, 0.685],
    [0.500, 0.683],
];

const TRACKPADS: [[f32; 2]; 2] = [[0.330, 0.521], [0.670, 0.521]];
const TRACKPAD_SIZE: [f32; 2] = [0.208, 0.204];
const TRACKPAD_CORNER: f32 = 0.034;
/// The pads are canted to follow the arc a thumb sweeps through.
const TRACKPAD_TILT: f32 = 0.157;
const STICKS: [[f32; 2]; 2] = [[0.368, 0.354], [0.632, 0.354]];
const STICK_RADIUS: f32 = 0.0515;
const DPAD_CENTER: [f32; 2] = [0.203, 0.303];
const DPAD_ARM: f32 = 0.047;
const DPAD_THICKNESS: f32 = 0.016;
const FACE_BUTTONS: [f32; 2] = [0.766, 0.299];
const FACE_BUTTON_OFFSET: f32 = 0.0515;
const FACE_BUTTON_RADIUS: f32 = 0.0235;
const OPTION_BUTTONS: [[f32; 2]; 2] = [[0.335, 0.229], [0.665, 0.229]];
const OPTION_SIZE: [f32; 2] = [0.055, 0.020];
const STATUS_LED: [f32; 2] = [0.500, 0.238];
const STEAM_BUTTON: [f32; 2] = [0.500, 0.302];
const STEAM_RADIUS: f32 = 0.0265;
const QUICK_ACCESS: [f32; 2] = [0.500, 0.527];
const QUICK_ACCESS_SIZE: [f32; 2] = [0.070, 0.030];

/// Rear shoulder controls as (name, centre, size, rotation).
///
/// The two are told apart by shape and place rather than by a label: a trigger
/// is the long paddle raked back into the shell at the corner, a bumper the
/// thin strip lying along the top edge inboard of it.
const SHOULDERS: [(&str, [f32; 2], [f32; 2], f32); 4] = [
    ("R2", [0.190, 0.335], [0.082, 0.150], -0.297),
    ("R1", [0.295, 0.211], [0.140, 0.026], -0.060),
    ("L2", [0.810, 0.335], [0.082, 0.150], 0.297),
    ("L1", [0.705, 0.211], [0.140, 0.026], 0.060),
];
const SHOULDER_CORNER: f32 = 0.022;
/// The shell seam across the top, broken by the USB-C port at its centre.
const TOP_SEAM: [f32; 2] = [0.500, 0.202];
const TOP_SEAM_WIDTH: f32 = 0.240;
const USB_PORT_SIZE: [f32; 2] = [0.052, 0.014];
const PUCK_CONNECTOR: [f32; 2] = [0.500, 0.295];
const PUCK_CONNECTOR_SIZE: [f32; 2] = [0.095, 0.034];

/// Rear grip paddles as (control, centre, size). `L*` mirrors `R*`.
const GRIP_PADDLES: [(BindableControl, [f32; 2], [f32; 2]); 4] = [
    (BindableControl::R4, [0.224, 0.533], [0.058, 0.092]),
    (BindableControl::R5, [0.197, 0.658], [0.054, 0.082]),
    (BindableControl::L4, [0.776, 0.533], [0.058, 0.092]),
    (BindableControl::L5, [0.803, 0.658], [0.054, 0.082]),
];

/// Where a bindable control is drawn, so the artwork, the hit area and the
/// callout leader can never drift apart.
fn control_rect(view: egui::Rect, control: BindableControl) -> egui::Rect {
    GRIP_PADDLES
        .iter()
        .find(|(candidate, _, _)| *candidate == control)
        .map_or_else(
            || unit_rect(view, QUICK_ACCESS, QUICK_ACCESS_SIZE),
            |(_, center, size)| unit_rect(view, *center, *size),
        )
}

/// A closed polygon in unit-square coordinates, triangulated once so that
/// painting it is a plain mesh upload.
struct UnitShape {
    points: Vec<[f32; 2]>,
    triangles: Vec<[u32; 3]>,
}

impl UnitShape {
    fn new(points: Vec<[f32; 2]>) -> Self {
        let triangles = triangulate(&points);
        Self { points, triangles }
    }

    /// A rounded rectangle, optionally turned about its own centre.
    fn rounded_rect(center: [f32; 2], size: [f32; 2], corner: f32, rotation: f32) -> Self {
        const ARC_STEPS: usize = 5;
        let (half_x, half_y) = (size[0] * 0.5, size[1] * 0.5);
        let corner = corner.min(half_x).min(half_y);
        let (sin, cos) = rotation.sin_cos();
        let mut points = Vec::with_capacity(4 * (ARC_STEPS + 1));
        for (index, [sign_x, sign_y]) in [[1.0, -1.0], [1.0, 1.0], [-1.0, 1.0], [-1.0, -1.0]]
            .into_iter()
            .enumerate()
        {
            let pivot = [sign_x * (half_x - corner), sign_y * (half_y - corner)];
            #[allow(clippy::cast_precision_loss)]
            for step in 0..=ARC_STEPS {
                let angle = std::f32::consts::FRAC_PI_2
                    * (step as f32 / ARC_STEPS as f32 + index as f32 - 1.0);
                let (arc_sin, arc_cos) = angle.sin_cos();
                let local = [
                    corner.mul_add(arc_cos, pivot[0]),
                    corner.mul_add(arc_sin, pivot[1]),
                ];
                points.push([
                    local[0].mul_add(cos, center[0]) - local[1] * sin,
                    local[0].mul_add(sin, center[1]) + local[1] * cos,
                ]);
            }
        }
        Self::new(points)
    }

    fn screen_points(&self, view: egui::Rect) -> Vec<egui::Pos2> {
        self.points
            .iter()
            .map(|point| normalized_point(view, *point))
            .collect()
    }

    fn paint(
        &self,
        painter: &egui::Painter,
        view: egui::Rect,
        fill: egui::Color32,
        stroke: egui::Stroke,
    ) {
        let points = self.screen_points(view);
        // A mesh built from the triangulation fills concave silhouettes exactly.
        // egui's generic path fill leaks outside the outline around the grips.
        let mut mesh = egui::Mesh::default();
        for point in &points {
            mesh.colored_vertex(*point, fill);
        }
        for [a, b, c] in &self.triangles {
            mesh.add_triangle(*a, *b, *c);
        }
        painter.add(egui::Shape::mesh(mesh));
        painter.add(egui::Shape::closed_line(points, stroke));
    }

    fn outline(&self, painter: &egui::Painter, view: egui::Rect, stroke: egui::Stroke) {
        painter.add(egui::Shape::closed_line(self.screen_points(view), stroke));
    }
}

fn body_shape() -> &'static UnitShape {
    static BODY: OnceLock<UnitShape> = OnceLock::new();
    BODY.get_or_init(|| {
        let mut points = BODY_HALF.to_vec();
        for [x, y] in BODY_HALF
            .iter()
            .rev()
            .skip(1)
            .take(BODY_HALF.len() - 2)
            .copied()
        {
            points.push([1.0 - x, y]);
        }
        UnitShape::new(points)
    })
}

/// Bounding box of the silhouette inside the unit square.
fn body_bounds() -> egui::Rect {
    static BOUNDS: OnceLock<egui::Rect> = OnceLock::new();
    *BOUNDS.get_or_init(|| {
        let mut bounds = egui::Rect::NOTHING;
        for [x, y] in body_shape().points.iter().copied() {
            bounds.extend_with(egui::pos2(x, y));
        }
        bounds
    })
}

/// The tilted trackpads, plus the inset ring drawn inside each of them.
fn trackpad_shapes() -> &'static [(UnitShape, UnitShape); 2] {
    static PADS: OnceLock<[(UnitShape, UnitShape); 2]> = OnceLock::new();
    PADS.get_or_init(|| {
        let pad = |index: usize| {
            let tilt = if index == 0 {
                TRACKPAD_TILT
            } else {
                -TRACKPAD_TILT
            };
            let inset = [TRACKPAD_SIZE[0] - 0.028, TRACKPAD_SIZE[1] - 0.028];
            (
                UnitShape::rounded_rect(TRACKPADS[index], TRACKPAD_SIZE, TRACKPAD_CORNER, tilt),
                UnitShape::rounded_rect(TRACKPADS[index], inset, TRACKPAD_CORNER * 0.75, tilt),
            )
        };
        [pad(0), pad(1)]
    })
}

fn shoulder_shapes() -> &'static [UnitShape; 4] {
    static SHOULDER: OnceLock<[UnitShape; 4]> = OnceLock::new();
    SHOULDER.get_or_init(|| {
        SHOULDERS.map(|(_, center, size, rotation)| {
            UnitShape::rounded_rect(center, size, SHOULDER_CORNER, rotation)
        })
    })
}

fn dpad_shape() -> &'static UnitShape {
    static DPAD: OnceLock<UnitShape> = OnceLock::new();
    DPAD.get_or_init(|| {
        let (arm, thickness) = (DPAD_ARM, DPAD_THICKNESS);
        let cross = [
            [thickness, thickness],
            [arm, thickness],
            [arm, -thickness],
            [thickness, -thickness],
            [thickness, -arm],
            [-thickness, -arm],
            [-thickness, -thickness],
            [-arm, -thickness],
            [-arm, thickness],
            [-thickness, thickness],
            [-thickness, arm],
            [thickness, arm],
        ];
        UnitShape::new(
            cross
                .into_iter()
                .map(|[x, y]| [DPAD_CENTER[0] + x, DPAD_CENTER[1] + y])
                .collect(),
        )
    })
}

fn controller_body(painter: &egui::Painter, view: egui::Rect) {
    body_shape().paint(painter, view, SURFACE, egui::Stroke::new(1.8, OUTLINE));
}

/// Stroke and fill for a bindable control, given how the pointer relates to it.
fn control_style(
    control: BindableControl,
    selected: BindableControl,
    hovered: Option<BindableControl>,
) -> (egui::Color32, egui::Stroke) {
    if selected == control {
        (
            egui::Color32::from_rgb(34, 67, 73),
            egui::Stroke::new(2.4, ACCENT),
        )
    } else if hovered == Some(control) {
        (INSET, egui::Stroke::new(1.8, ACCENT.gamma_multiply(0.6)))
    } else {
        (INSET, egui::Stroke::new(1.5, OUTLINE))
    }
}

fn draw_front_face(
    painter: &egui::Painter,
    view: egui::Rect,
    selected: BindableControl,
    hovered: Option<BindableControl>,
) {
    let scale = view.width();
    let detail = egui::Stroke::new(1.3, DETAIL);
    let outline = egui::Stroke::new(1.5, OUTLINE);

    for (pad, inset) in trackpad_shapes() {
        pad.paint(painter, view, INSET, outline);
        inset.outline(painter, view, detail);
    }

    for center in STICKS {
        let center = normalized_point(view, center);
        painter.circle_filled(center, scale * STICK_RADIUS, INSET);
        painter.circle_stroke(center, scale * STICK_RADIUS, outline);
        painter.circle_stroke(center, scale * (STICK_RADIUS - 0.014), detail);
    }

    dpad_shape().paint(painter, view, INSET, detail);

    for offset in [
        [0.0, -FACE_BUTTON_OFFSET],
        [-FACE_BUTTON_OFFSET, 0.0],
        [FACE_BUTTON_OFFSET, 0.0],
        [0.0, FACE_BUTTON_OFFSET],
    ] {
        let center = normalized_point(
            view,
            [FACE_BUTTONS[0] + offset[0], FACE_BUTTONS[1] + offset[1]],
        );
        painter.circle_filled(center, scale * FACE_BUTTON_RADIUS, INSET);
        painter.circle_stroke(center, scale * FACE_BUTTON_RADIUS, detail);
    }

    for center in OPTION_BUTTONS {
        let button = unit_rect(view, center, OPTION_SIZE);
        painter.rect_filled(button, scale * 0.010, INSET);
        painter.rect_stroke(button, scale * 0.010, detail, egui::StrokeKind::Inside);
    }

    painter.circle_stroke(normalized_point(view, STATUS_LED), scale * 0.006, detail);

    let steam = normalized_point(view, STEAM_BUTTON);
    painter.circle_filled(steam, scale * STEAM_RADIUS, INSET);
    painter.circle_stroke(steam, scale * STEAM_RADIUS, detail);
    painter.circle_stroke(steam, scale * (STEAM_RADIUS - 0.009), detail);

    let (fill, stroke) = control_style(BindableControl::QuickAccess, selected, hovered);
    let quick = unit_rect(view, QUICK_ACCESS, QUICK_ACCESS_SIZE);
    let radius = quick.height() * 0.5;
    painter.rect_filled(quick, radius, fill);
    painter.rect_stroke(quick, radius, stroke, egui::StrokeKind::Inside);
    for x in [-0.014, 0.0, 0.014] {
        painter.circle_filled(
            normalized_point(view, [QUICK_ACCESS[0] + x, QUICK_ACCESS[1]]),
            scale * 0.0045,
            stroke.color,
        );
    }
}

fn draw_rear_face(
    painter: &egui::Painter,
    view: egui::Rect,
    selected: BindableControl,
    hovered: Option<BindableControl>,
) {
    let scale = view.width();
    let detail = egui::Stroke::new(1.3, DETAIL);
    let outline = egui::Stroke::new(1.4, OUTLINE);

    // The shell seam runs across the top and stops either side of the port.
    let usb = unit_rect(view, TOP_SEAM, USB_PORT_SIZE);
    let seam = unit_rect(view, TOP_SEAM, [TOP_SEAM_WIDTH, 0.0]);
    for [from, to] in [
        [seam.left(), usb.left() - scale * 0.012],
        [usb.right() + scale * 0.012, seam.right()],
    ] {
        painter.line_segment(
            [
                egui::pos2(from, seam.center().y),
                egui::pos2(to, seam.center().y),
            ],
            detail,
        );
    }
    painter.rect_filled(usb, usb.height() * 0.5, INSET);
    painter.rect_stroke(usb, usb.height() * 0.5, detail, egui::StrokeKind::Inside);

    for shape in shoulder_shapes() {
        shape.paint(painter, view, INSET, outline);
    }

    let puck = unit_rect(view, PUCK_CONNECTOR, PUCK_CONNECTOR_SIZE);
    painter.rect_filled(puck, scale * 0.012, INSET);
    painter.rect_stroke(puck, scale * 0.012, detail, egui::StrokeKind::Inside);
    for x in [-0.026, 0.0, 0.026] {
        painter.circle_filled(
            normalized_point(view, [PUCK_CONNECTOR[0] + x, PUCK_CONNECTOR[1]]),
            scale * 0.006,
            DETAIL,
        );
    }
    for (control, center, size) in GRIP_PADDLES {
        let (fill, stroke) = control_style(control, selected, hovered);
        let center = normalized_point(view, center);
        let radius = egui::vec2(size[0] * 0.5 * scale, size[1] * 0.5 * scale);
        painter.add(egui::Shape::ellipse_filled(center, radius, fill));
        painter.add(egui::Shape::ellipse_stroke(center, radius, stroke));
    }
}

// ---------------------------------------------------------------------------
// Polygon helpers
// ---------------------------------------------------------------------------

fn cross(origin: [f32; 2], first: [f32; 2], second: [f32; 2]) -> f32 {
    (first[0] - origin[0]).mul_add(
        second[1] - origin[1],
        -((first[1] - origin[1]) * (second[0] - origin[0])),
    )
}

fn signed_area(points: &[[f32; 2]]) -> f32 {
    let mut area = 0.0;
    for (index, point) in points.iter().enumerate() {
        let next = points[(index + 1) % points.len()];
        area += point[0].mul_add(next[1], -(next[0] * point[1]));
    }
    area * 0.5
}

fn point_in_triangle(point: [f32; 2], triangle: [[f32; 2]; 3]) -> bool {
    let [a, b, c] = triangle;
    cross(a, b, point) >= 0.0 && cross(b, c, point) >= 0.0 && cross(c, a, point) >= 0.0
}

/// Ear-clipping triangulation of a simple polygon, concave parts included.
fn triangulate(points: &[[f32; 2]]) -> Vec<[u32; 3]> {
    let mut remaining: Vec<u32> = (0..points.len())
        .map(|index| u32::try_from(index).expect("an outline has far fewer than u32::MAX points"))
        .collect();
    if signed_area(points) < 0.0 {
        remaining.reverse();
    }
    let corner = |index: u32| points[index as usize];
    let mut triangles = Vec::with_capacity(remaining.len().saturating_sub(2));
    while remaining.len() > 3 {
        let ear = (0..remaining.len()).find_map(|index| {
            let triangle = [
                remaining[(index + remaining.len() - 1) % remaining.len()],
                remaining[index],
                remaining[(index + 1) % remaining.len()],
            ];
            let corners = triangle.map(corner);
            if cross(corners[0], corners[1], corners[2]) <= 0.0 {
                return None;
            }
            let empty = remaining.iter().all(|candidate| {
                triangle.contains(candidate) || !point_in_triangle(corner(*candidate), corners)
            });
            empty.then_some((index, triangle))
        });
        // A non-simple polygon has no ear left; stop instead of looping forever.
        let Some((index, triangle)) = ear else {
            break;
        };
        triangles.push(triangle);
        remaining.remove(index);
    }
    if remaining.len() == 3 {
        triangles.push([remaining[0], remaining[1], remaining[2]]);
    }
    triangles
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

    /// Ray casting, used to check that no artwork escapes the silhouette.
    fn point_in_polygon(polygon: &[[f32; 2]], point: [f32; 2]) -> bool {
        let mut inside = false;
        for (index, corner) in polygon.iter().enumerate() {
            let next = polygon[(index + 1) % polygon.len()];
            let crosses = (corner[1] > point[1]) != (next[1] > point[1]);
            if crosses {
                let x = (next[0] - corner[0]) * (point[1] - corner[1]) / (next[1] - corner[1])
                    + corner[0];
                if point[0] < x {
                    inside = !inside;
                }
            }
        }
        inside
    }

    fn corners(center: [f32; 2], size: [f32; 2]) -> [[f32; 2]; 4] {
        let (half_x, half_y) = (size[0] * 0.5, size[1] * 0.5);
        [
            [center[0] - half_x, center[1] - half_y],
            [center[0] + half_x, center[1] - half_y],
            [center[0] + half_x, center[1] + half_y],
            [center[0] - half_x, center[1] + half_y],
        ]
    }

    fn assert_polygon_inside_body(what: &str, points: &[[f32; 2]]) {
        let body = &body_shape().points;
        for point in points {
            assert!(
                point_in_polygon(body, *point),
                "{what} reaches {point:?}, which is outside the controller body",
            );
        }
    }

    fn assert_inside_body(what: &str, center: [f32; 2], size: [f32; 2]) {
        assert_polygon_inside_body(what, &corners(center, size));
    }

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
        let paddle = |wanted: BindableControl| {
            GRIP_PADDLES
                .iter()
                .find(|(control, _, _)| *control == wanted)
                .expect("every grip paddle is drawn")
                .1
        };
        // The rear view is a mirror, so the right grip is drawn on the left.
        assert!(paddle(BindableControl::R4)[0] < paddle(BindableControl::L4)[0]);
        assert!(paddle(BindableControl::R5)[0] < paddle(BindableControl::L5)[0]);
        assert!(paddle(BindableControl::R4)[1] < paddle(BindableControl::R5)[1]);
        for callout in CONTROL_CALLOUTS {
            let rear = callout.view == ControllerView::Rear;
            assert_eq!(rear, callout.control != BindableControl::QuickAccess);
        }
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
    fn controller_outline_is_exactly_mirrored() {
        let body = &body_shape().points;
        assert_eq!(body.len(), 2 * BODY_HALF.len() - 2);
        for [x, y] in body.iter().copied() {
            assert!(
                body.iter().any(
                    |[other_x, other_y]| (other_x - (1.0 - x)).abs() < f32::EPSILON
                        && (other_y - y).abs() < f32::EPSILON
                ),
                "the outline point {x},{y} has no mirrored twin",
            );
        }
    }

    #[test]
    fn body_triangles_tile_the_outline_without_gaps_or_overlap() {
        let body = body_shape();
        // Ear clipping only reaches n - 2 triangles on a simple polygon, and
        // their areas only add up to the polygon's when they do not overlap.
        assert_eq!(body.triangles.len(), body.points.len() - 2);
        let covered: f32 = body
            .triangles
            .iter()
            .map(|[a, b, c]| {
                let triangle = [
                    body.points[*a as usize],
                    body.points[*b as usize],
                    body.points[*c as usize],
                ];
                cross(triangle[0], triangle[1], triangle[2]).abs() * 0.5
            })
            .sum();
        assert!(
            (covered - signed_area(&body.points).abs()).abs() < 1e-4,
            "triangles cover {covered}, the outline encloses {}",
            signed_area(&body.points).abs(),
        );
    }

    #[test]
    fn every_drawn_control_stays_inside_the_body() {
        for (control, center, size) in GRIP_PADDLES {
            assert_inside_body(control.label(), center, size);
        }
        assert_inside_body("the Quick Access button", QUICK_ACCESS, QUICK_ACCESS_SIZE);
        for (pad, _) in trackpad_shapes() {
            assert_polygon_inside_body("a trackpad", &pad.points);
        }
        for center in STICKS {
            assert_inside_body("a thumbstick", center, [STICK_RADIUS * 2.0; 2]);
        }
        assert_inside_body("the D-pad", DPAD_CENTER, [DPAD_ARM * 2.0; 2]);
        for offset in [
            [0.0, -FACE_BUTTON_OFFSET],
            [-FACE_BUTTON_OFFSET, 0.0],
            [FACE_BUTTON_OFFSET, 0.0],
            [0.0, FACE_BUTTON_OFFSET],
        ] {
            assert_inside_body(
                "an ABXY button",
                [FACE_BUTTONS[0] + offset[0], FACE_BUTTONS[1] + offset[1]],
                [FACE_BUTTON_RADIUS * 2.0; 2],
            );
        }
        for center in OPTION_BUTTONS {
            assert_inside_body("a View/Menu button", center, OPTION_SIZE);
        }
        assert_inside_body("the Steam button", STEAM_BUTTON, [STEAM_RADIUS * 2.0; 2]);
        for shoulder in shoulder_shapes() {
            assert_polygon_inside_body("a bumper or trigger", &shoulder.points);
        }
        assert_inside_body("the USB-C port", TOP_SEAM, USB_PORT_SIZE);
        assert_inside_body("the shell seam", TOP_SEAM, [TOP_SEAM_WIDTH, 0.0]);
        assert_inside_body("the puck connector", PUCK_CONNECTOR, PUCK_CONNECTOR_SIZE);
    }

    #[test]
    fn triggers_and_bumpers_are_told_apart_by_shape_and_place() {
        let names: BTreeSet<&str> = SHOULDERS.iter().map(|(name, ..)| *name).collect();
        assert_eq!(names, BTreeSet::from(["L1", "L2", "R1", "R2"]));
        let shoulder = |wanted: &str| {
            SHOULDERS
                .iter()
                .find(|(name, ..)| *name == wanted)
                .copied()
                .expect("every shoulder is described once")
        };
        for (trigger, bumper, outboard) in [("R2", "R1", -1.0_f32), ("L2", "L1", 1.0_f32)] {
            let (_, trigger_at, trigger_size, _) = shoulder(trigger);
            let (_, bumper_at, bumper_size, _) = shoulder(bumper);
            // The trigger is the deep paddle at the corner...
            assert!((trigger_at[0] - 0.5) * outboard > (bumper_at[0] - 0.5) * outboard);
            assert!(trigger_at[1] > bumper_at[1]);
            assert!(trigger_size[1] > bumper_size[1] * 3.0);
            // ...and the bumper is the thin strip lying on the top edge.
            assert!(bumper_size[0] > bumper_size[1] * 4.0);
        }
    }

    #[test]
    fn the_grips_rake_outwards_above_a_straight_bottom_edge() {
        let shoulder = BODY_HALF
            .iter()
            .copied()
            .find(|[_, y]| *y > 0.20)
            .expect("the outline reaches the shoulders");
        let widest = BODY_HALF
            .iter()
            .copied()
            .reduce(|narrowest, point| {
                if point[0] < narrowest[0] {
                    point
                } else {
                    narrowest
                }
            })
            .expect("the outline has points");
        // The outer edge leans away from the centreline the whole way down, so
        // the body is widest in the grips rather than up at the shoulders.
        assert!(
            widest[1] > shoulder[1],
            "the body is widest at {widest:?}, above the grips, so they taper inwards",
        );
        let (run, drop) = (shoulder[0] - widest[0], widest[1] - shoulder[1]);
        let rake = run.atan2(drop).to_degrees();
        assert!(
            (9.0..22.0).contains(&rake),
            "the grips rake outwards at {rake}°, which reads as straight down",
        );
        assert!(
            drop > 0.35,
            "the rake only covers {drop} of the body height",
        );
        let bottom: Vec<f32> = BODY_HALF
            .iter()
            .copied()
            .filter(|[x, y]| *x > 0.38 && *y > 0.60)
            .map(|[_, y]| y)
            .collect();
        assert!(bottom.len() >= 3, "the bottom edge needs several points");
        let (lowest, highest) = bottom.iter().fold((f32::MAX, f32::MIN), |range, y| {
            (range.0.min(*y), range.1.max(*y))
        });
        assert!(
            highest - lowest < 0.01,
            "the bottom edge is not straight: it spans {lowest}..{highest}",
        );
    }

    #[test]
    fn trackpads_are_square_and_do_not_overlap_the_quick_access_button() {
        let aspect = TRACKPAD_SIZE[0] / TRACKPAD_SIZE[1];
        assert!(
            (aspect - 1.0).abs() < 0.05,
            "the Steam Controller 2 trackpads are square, got an aspect of {aspect}",
        );
        let pad_edge = TRACKPADS[0][0] + TRACKPAD_SIZE[0] * 0.5;
        let button_edge = QUICK_ACCESS[0] - QUICK_ACCESS_SIZE[0] * 0.5;
        assert!(pad_edge < button_edge);
    }

    #[test]
    fn canvas_layout_keeps_both_views_square_and_leaves_room_for_labels() {
        let canvas = egui::Rect::from_min_size(egui::pos2(20.0, 40.0), egui::vec2(830.0, 560.0));
        let layout = CanvasLayout::new(canvas);
        for view in [layout.front, layout.rear] {
            // Square to within float noise, so the artwork keeps its aspect.
            assert!((view.width() - view.height()).abs() < 1e-3);
        }
        let (front, rear) = (
            layout.body(ControllerView::Front),
            layout.body(ControllerView::Rear),
        );
        for body in [front, rear] {
            assert!(canvas.contains_rect(body));
        }
        assert!((front.top() - rear.top()).abs() < f32::EPSILON);
        assert!(front.right() < rear.left());
        for callout in CONTROL_CALLOUTS {
            let label = layout.label(callout);
            assert!(
                canvas.contains_rect(label),
                "the {} label at {label:?} escapes the canvas",
                callout.control.label(),
            );
            assert!(
                !label.intersects(layout.body(callout.view)),
                "the {} label overlaps the controller",
                callout.control.label(),
            );
        }
        let upper = layout.label(CONTROL_CALLOUTS[1]);
        let lower = layout.label(CONTROL_CALLOUTS[2]);
        assert!(upper.bottom() < lower.top(), "stacked labels overlap");
    }

    #[test]
    fn the_diagram_and_the_inspector_fit_inside_the_window() {
        let content_width = MIN_WINDOW_SIZE[0] - 40.0;
        let canvas_width = (content_width - INSPECTOR_WIDTH - COLUMN_GAP).max(CANVAS_MIN_WIDTH);
        assert!(
            canvas_width + COLUMN_GAP + INSPECTOR_WIDTH <= content_width,
            "the row overflows the window margin at the minimum window size",
        );
    }
}
