use std::collections::BTreeSet;

use controller_art::{
    body_bounds, control_rect, normalized_point, trackpad_rect, Control, ControlState, PadSide,
};
use desktop_bindings::{
    default_store_path, load_or_create_store, save_store, BindableControl, BindingAction,
    BindingStore, KeyboardKey, Modifier, MouseButton, PadFeedbackConfig, PadFeedbackStrength,
    MAX_PROFILE_NAME_CHARS, MAX_SCROLL_SPEED_PERCENT, MIN_SCROLL_SPEED_PERCENT,
};
use eframe::egui;

use ui_theme::{
    ACCENT, BORDER, DANGER, DETAIL, MUTED_TEXT, ON_ACCENT, PANEL, SUNKEN, SURFACE, SURFACE_RAISED,
    TEXT,
};

const WINDOW_SIZE: [f32; 2] = [1260.0, 720.0];
const MIN_WINDOW_SIZE: [f32; 2] = [1080.0, 660.0];
const INSPECTOR_WIDTH: f32 = 300.0;
const COLUMN_GAP: f32 = 16.0;
const CANVAS_MIN_WIDTH: f32 = 620.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerView {
    Front,
    Rear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EditorSelection {
    Button(BindableControl),
    Pad(PadSide),
}

impl EditorSelection {
    const fn label(self) -> &'static str {
        match self {
            Self::Button(control) => control.label(),
            Self::Pad(side) => side.label(),
        }
    }
}

// The editor's vocabulary translated into the art crate's. These are plain
// functions rather than `From` impls because orphan rules forbid the impl here:
// inside this binary both types are foreign. The art crate cannot host it
// either, since it deliberately does not depend on `desktop-bindings`.

const fn art_control(control: BindableControl) -> Control {
    match control {
        BindableControl::L4 => Control::L4,
        BindableControl::L5 => Control::L5,
        BindableControl::R4 => Control::R4,
        BindableControl::R5 => Control::R5,
        BindableControl::QuickAccess => Control::QuickAccess,
    }
}

const fn art_selection(selection: EditorSelection) -> Control {
    match selection {
        EditorSelection::Button(control) => art_control(control),
        EditorSelection::Pad(PadSide::Left) => Control::LeftPad,
        EditorSelection::Pad(PadSide::Right) => Control::RightPad,
    }
}

/// What the artwork should show, given what is selected and what the pointer is
/// over. The editor only ever sets these two states; live device state is the
/// visualizer's business.
fn editor_paint(
    selected: EditorSelection,
    hovered: Option<EditorSelection>,
) -> impl Fn(Control) -> ControlState {
    let marked = art_selection(selected);
    let under_pointer = hovered.map(art_selection);
    move |control| {
        if control == marked {
            ControlState::active()
        } else if under_pointer == Some(control) {
            ControlState::hovered()
        } else {
            ControlState::IDLE
        }
    }
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
            ui_theme::configure_ui(&creation.egui_ctx);
            Ok(Box::new(BindingsEditor::new(path, store)))
        }),
    )
    .map_err(|error| error.to_string())
}

struct BindingsEditor {
    path: std::path::PathBuf,
    original_store: BindingStore,
    store: BindingStore,
    selected: usize,
    selection: EditorSelection,
    capturing: Option<BindableControl>,
    message: Option<String>,
}

impl BindingsEditor {
    fn new(path: std::path::PathBuf, store: BindingStore) -> Self {
        Self {
            path,
            original_store: store.clone(),
            store,
            selected: 0,
            selection: EditorSelection::Button(BindableControl::QuickAccess),
            capturing: None,
            message: None,
        }
    }

    fn is_dirty(&self) -> bool {
        self.store != self.original_store
    }

    fn unique_name(&self, base: &str) -> String {
        (1..=desktop_bindings::MAX_PROFILES + 1)
            .map(|number| {
                let suffix = if number == 1 {
                    String::new()
                } else {
                    format!(" {number}")
                };
                let available = MAX_PROFILE_NAME_CHARS.saturating_sub(suffix.chars().count());
                let stem = base
                    .trim()
                    .chars()
                    .take(available)
                    .collect::<String>()
                    .trim_end()
                    .to_owned();
                format!("{stem}{suffix}")
            })
            .find(|name| self.store.profile_by_name(name).is_none())
            .expect("an unused bounded profile name exists")
    }

    fn add_profile(&mut self) {
        let name = self.unique_name("New Profile");
        match self.store.create_profile(&name) {
            Ok(_) => {
                self.selected = self.store.profiles.len() - 1;
                self.capturing = None;
                self.message = None;
            }
            Err(error) => self.message = Some(error),
        }
    }

    fn duplicate_profile(&mut self) {
        let Some(source) = self.store.profiles.get(self.selected) else {
            return;
        };
        let source_id = source.id.clone();
        let name = self.unique_name(&format!("{} Copy", source.name));
        match self.store.duplicate_profile(&source_id, &name) {
            Ok(_) => {
                self.selected = self.store.profiles.len() - 1;
                self.capturing = None;
                self.message = None;
            }
            Err(error) => self.message = Some(error),
        }
    }

    fn delete_profile(&mut self) {
        let Some(id) = self
            .store
            .profiles
            .get(self.selected)
            .map(|profile| profile.id.clone())
        else {
            return;
        };
        match self.store.delete_profile(&id) {
            Ok(_) => {
                self.selected = self.selected.min(self.store.profiles.len() - 1);
                self.capturing = None;
                self.message = None;
            }
            Err(error) => self.message = Some(error),
        }
    }

    fn select_control(&mut self, control: BindableControl) {
        self.selection = EditorSelection::Button(control);
        self.capturing = None;
    }

    fn select_pad(&mut self, side: PadSide) {
        self.selection = EditorSelection::Pad(side);
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
        painter.rect_filled(canvas_rect, 14.0, SUNKEN);
        painter.rect_stroke(
            canvas_rect,
            14.0,
            egui::Stroke::new(1.0, BORDER),
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
        controller_art::draw_body(&painter, layout.front);
        controller_art::draw_body(&painter, layout.rear);
        self.draw_leaders(&painter, &layout);
        let paint = editor_paint(self.selection, hovered);
        controller_art::draw_front(&painter, layout.front, &paint);
        controller_art::draw_rear(&painter, layout.rear, &paint);
        draw_pad_labels(
            &painter,
            layout.front,
            self.store.profiles[self.selected].pads.left_scroll.enabled,
            self.store.profiles[self.selected].pads.right_mouse.enabled,
            self.selection,
        );
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
    ) -> Option<EditorSelection> {
        let mut hovered = None;
        let mut clicked = None;
        for callout in CONTROL_CALLOUTS {
            let rect =
                control_rect(layout.view(callout.view), art_control(callout.control)).expand(4.0);
            let response = ui
                .interact(
                    rect,
                    ui.id().with(("controller-hotspot", callout.control)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.hovered() {
                hovered = Some(EditorSelection::Button(callout.control));
            }
            if response.clicked() {
                clicked = Some(EditorSelection::Button(callout.control));
            }
        }
        for side in PadSide::ALL {
            let selection = EditorSelection::Pad(side);
            let response = ui
                .interact(
                    trackpad_rect(layout.front, side).expand(4.0),
                    ui.id().with(("pad-hotspot", selection)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.hovered() {
                hovered = Some(selection);
            }
            if response.clicked() {
                clicked = Some(selection);
            }
        }
        if let Some(selection) = clicked {
            match selection {
                EditorSelection::Button(control) => self.select_control(control),
                EditorSelection::Pad(side) => self.select_pad(side),
            }
        }
        hovered
    }

    fn draw_leaders(&self, painter: &egui::Painter, layout: &CanvasLayout) {
        for callout in CONTROL_CALLOUTS {
            let target =
                control_rect(layout.view(callout.view), art_control(callout.control)).center();
            let label = layout.label(callout);
            let selected = self.selection == EditorSelection::Button(callout.control);
            // The leader runs from the label's edge to the middle of the
            // control. Its tail is hidden because the control is painted over
            // it, so it lands on the control's own edge whatever shape it is.
            painter.line_segment(
                [rect_edge_towards(label, target), target],
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
            let selected = self.selection == EditorSelection::Button(callout.control);
            let summary = binding_summary(
                self.store.profiles[self.selected]
                    .bindings
                    .get(callout.control),
            );
            let button = egui::Button::new(
                egui::RichText::new(format!("{}\n{summary}", callout.control.label()))
                    .size(10.5)
                    .color(if selected { ON_ACCENT } else { TEXT }),
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
        let selection = self.selection;
        // The card matches the canvas height so the two panes read as one row.
        let card_height = ui.available_height() - 32.0;
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(egui::Stroke::new(1.0, BORDER))
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
                    ui.heading(selection.label());
                    ui.label(
                        egui::RichText::new(selection_description(selection))
                            .size(12.0)
                            .color(MUTED_TEXT),
                    );
                    ui.add_space(18.0);
                    match selection {
                        EditorSelection::Button(control) => self.binding_editor(ui, control),
                        EditorSelection::Pad(side) => self.pad_editor(ui, side),
                    }
                });
            });
    }

    fn pad_editor(&mut self, ui: &mut egui::Ui, side: PadSide) {
        ui.label("Desktop action");
        ui.label(
            egui::RichText::new(match side {
                PadSide::Left => "Accelerated smooth scroll",
                PadSide::Right => "Relative mouse",
            })
            .strong(),
        );
        ui.add_space(12.0);
        match side {
            PadSide::Left => {
                let config = &mut self.store.profiles[self.selected].pads.left_scroll;
                ui.checkbox(&mut config.enabled, "Enable this pad");
                ui.add_space(16.0);
                ui.add_enabled_ui(config.enabled, |ui| {
                    ui.label("Scroll speed");
                    ui.add(
                        egui::Slider::new(
                            &mut config.speed_percent,
                            MIN_SCROLL_SPEED_PERCENT..=MAX_SCROLL_SPEED_PERCENT,
                        )
                        .suffix("%"),
                    );
                    ui.label(
                        egui::RichText::new("Faster swipes accelerate above this base speed.")
                            .small()
                            .color(MUTED_TEXT),
                    );
                    ui.add_space(12.0);
                    ui.checkbox(&mut config.momentum, "Momentum after release");
                    ui.add_space(16.0);
                    pad_feedback_editor(ui, &mut config.feedback);
                });
            }
            PadSide::Right => {
                let config = &mut self.store.profiles[self.selected].pads.right_mouse;
                ui.checkbox(&mut config.enabled, "Enable this pad");
                ui.add_space(16.0);
                ui.add_enabled_ui(config.enabled, |ui| {
                    pad_feedback_editor(ui, &mut config.feedback);
                });
            }
        }
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new("Pad clicks, pressure actions, and gestures are not used.")
                .small()
                .color(MUTED_TEXT),
        );
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
            .fill(PANEL)
            .inner_margin(egui::Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.heading("Controller bindings");
                ui.label(
                    egui::RichText::new(
                        "Select an extra button or pad, then configure its desktop action.",
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
                        // The canvas places its labels with `Ui::put`, which
                        // leaves the cursor at the last label instead of at the
                        // canvas's own edge. Scoping it keeps the column widths
                        // honest, so the inspector ends on the frame's margin.
                        ui.scope(|ui| self.controller_canvas(ui, canvas_width));
                        ui.add_space(COLUMN_GAP);
                        self.binding_inspector(ui);
                    },
                );

                ui.add_space(12.0);
                let dirty = self.is_dirty();
                ui.allocate_ui_with_layout(
                    egui::vec2(content_width, 32.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if ui
                            .add_enabled_ui(dirty, |ui| {
                                ui.add_sized(
                                    [84.0, 32.0],
                                    egui::Button::new(
                                        egui::RichText::new("Save").strong().color(ON_ACCENT),
                                    )
                                    .fill(ACCENT),
                                )
                            })
                            .inner
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
                                ui.colored_label(DANGER, message);
                            } else if dirty {
                                ui.label(
                                    egui::RichText::new("Unsaved changes take effect after Save.")
                                        .small()
                                        .color(MUTED_TEXT),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new("No unsaved changes.")
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

fn selection_description(selection: EditorSelection) -> &'static str {
    match selection {
        EditorSelection::Button(BindableControl::L4) => "Upper left rear grip",
        EditorSelection::Button(BindableControl::L5) => "Lower left rear grip",
        EditorSelection::Button(BindableControl::R4) => "Upper right rear grip",
        EditorSelection::Button(BindableControl::R5) => "Lower right rear grip",
        EditorSelection::Button(BindableControl::QuickAccess) => "Front Quick Access button",
        EditorSelection::Pad(PadSide::Left) => "Two-axis smooth desktop scrolling",
        EditorSelection::Pad(PadSide::Right) => "Relative desktop pointer movement",
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

/// Where a ray from the middle of `rect` towards `toward` leaves the rectangle.
fn rect_edge_towards(rect: egui::Rect, toward: egui::Pos2) -> egui::Pos2 {
    let center = rect.center();
    let delta = toward - center;
    let half = rect.size() * 0.5;
    let reach = |distance: f32, half: f32| {
        if distance.abs() > f32::EPSILON {
            half / distance.abs()
        } else {
            f32::INFINITY
        }
    };
    let scale = reach(delta.x, half.x).min(reach(delta.y, half.y));
    if scale.is_finite() {
        center + delta * scale
    } else {
        center
    }
}

fn draw_pad_labels(
    painter: &egui::Painter,
    view: egui::Rect,
    left_enabled: bool,
    right_enabled: bool,
    selected: EditorSelection,
) {
    for (side, title, enabled) in [
        (PadSide::Left, "SCROLL", left_enabled),
        (PadSide::Right, "MOUSE", right_enabled),
    ] {
        let selection = EditorSelection::Pad(side);
        let rect = trackpad_rect(view, side);
        let color = if selected == selection {
            ACCENT
        } else {
            MUTED_TEXT
        };
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{title}\n{}", if enabled { "ON" } else { "OFF" }),
            egui::FontId::proportional(9.5),
            color,
        );
    }
}

fn pad_feedback_editor(ui: &mut egui::Ui, feedback: &mut PadFeedbackConfig) {
    ui.checkbox(&mut feedback.enabled, "Pad feedback");
    ui.label(
        egui::RichText::new("Emit subtle ticks that become faster as your finger moves faster.")
            .small()
            .color(MUTED_TEXT),
    );
    ui.add_space(12.0);
    ui.add_enabled_ui(feedback.enabled, |ui| {
        ui.label("Feedback strength");
        ui.horizontal(|ui| {
            for strength in PadFeedbackStrength::ALL {
                ui.selectable_value(&mut feedback.strength, strength, strength.label());
            }
        });
    });
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
        // The paddle geometry half of this now lives in `controller-art` as
        // `the_rear_view_keeps_physical_handedness`; what stays here is that
        // the callouts agree with it.
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
    fn duplicating_a_maximum_length_profile_keeps_a_valid_name() {
        let mut store = BindingStore::default();
        store.profiles[0].name = "A".repeat(MAX_PROFILE_NAME_CHARS);
        let mut editor = BindingsEditor::new(std::path::PathBuf::new(), store);

        editor.duplicate_profile();

        assert_eq!(editor.store.profiles.len(), 2);
        assert!(editor.store.profiles[1].name.chars().count() <= MAX_PROFILE_NAME_CHARS);
        editor.store.validate().unwrap();
        assert!(editor.message.is_none());
    }

    #[test]
    fn duplicated_profiles_preserve_independent_pad_settings() {
        let mut store = BindingStore::default();
        store.profiles[0].pads.right_mouse.enabled = true;
        store.profiles[0].pads.right_mouse.feedback.enabled = false;
        store.profiles[0].pads.left_scroll.feedback.strength = PadFeedbackStrength::High;
        store.profiles[0].pads.left_scroll.speed_percent = 175;
        store.profiles[0].pads.left_scroll.momentum = false;
        let mut editor = BindingsEditor::new(std::path::PathBuf::new(), store);

        editor.duplicate_profile();

        assert_eq!(editor.store.profiles[1].pads, editor.store.profiles[0].pads);
        assert_eq!(
            editor.selection,
            EditorSelection::Button(BindableControl::QuickAccess)
        );
    }

    #[test]
    fn pad_edits_participate_in_dirty_state_detection() {
        let mut editor = BindingsEditor::new(std::path::PathBuf::new(), BindingStore::default());
        assert!(!editor.is_dirty());
        editor.store.profiles[0].pads.right_mouse.enabled = true;
        assert!(editor.is_dirty());
        editor.store.profiles[0].pads.right_mouse.enabled = false;
        assert!(!editor.is_dirty());
    }

    #[test]
    fn pad_selections_have_fixed_roles_and_default_feedback() {
        assert_eq!(
            selection_description(EditorSelection::Pad(PadSide::Left)),
            "Two-axis smooth desktop scrolling"
        );
        assert_eq!(
            selection_description(EditorSelection::Pad(PadSide::Right)),
            "Relative desktop pointer movement"
        );
        let pads = desktop_bindings::PadBindings::default();
        assert!(!pads.left_scroll.enabled);
        assert!(!pads.right_mouse.enabled);
        assert!(pads.left_scroll.feedback.enabled);
        assert_eq!(pads.left_scroll.speed_percent, 100);
        assert!(pads.left_scroll.momentum);
        assert_eq!(
            pads.right_mouse.feedback.strength,
            PadFeedbackStrength::Medium
        );
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

        // Every leader has to start on its label's edge and run at the middle
        // of the control it names.
        for callout in CONTROL_CALLOUTS {
            let label = layout.label(callout);
            let target =
                control_rect(layout.view(callout.view), art_control(callout.control)).center();
            let start = rect_edge_towards(label, target);
            let on_edge = (start.x - label.left()).abs() < 0.01
                || (start.x - label.right()).abs() < 0.01
                || (start.y - label.top()).abs() < 0.01
                || (start.y - label.bottom()).abs() < 0.01;
            assert!(
                on_edge,
                "the {} leader starts at {start:?}, off its label's edge",
                callout.control.label()
            );
            // Start, label centre and target are collinear, so the leader
            // points straight at the middle of the control.
            let along = target - label.center();
            let out = start - label.center();
            let cross = along.x.mul_add(out.y, -(along.y * out.x));
            assert!(
                cross.abs() < 0.5,
                "the {} leader does not aim at the control's middle",
                callout.control.label(),
            );
        }
    }

    #[test]
    fn the_diagram_and_the_inspector_fit_inside_the_window() {
        for window in [MIN_WINDOW_SIZE[0], WINDOW_SIZE[0]] {
            let content_width = window - 40.0;
            let canvas_width = (content_width - INSPECTOR_WIDTH - COLUMN_GAP).max(CANVAS_MIN_WIDTH);
            // Exactly, not merely within: the inspector's right edge is what
            // the Save and Cancel buttons line up against.
            assert!(
                (canvas_width + COLUMN_GAP + INSPECTOR_WIDTH - content_width).abs() < f32::EPSILON,
                "at a {window}pt window the row does not fill the content width",
            );
        }
    }
}
