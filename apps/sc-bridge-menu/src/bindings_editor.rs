use std::collections::BTreeSet;

use controller_art::{
    body_bounds, control_rect, normalized_point, trackpad_rect, Control, ControlState, PadSide,
};
use desktop_bindings::{
    default_store_path, load_or_create_store, save_store, BindableControl, BindingAction,
    BindingStore, KeyboardKey, Modifier, MouseButton, PadFeedbackConfig, PadFeedbackStrength,
    MAX_PAD_SPEED_PERCENT, MAX_PROFILE_NAME_CHARS, MIN_PAD_SPEED_PERCENT,
};
use eframe::egui;

use ui_theme::{
    ACCENT, BORDER, DANGER, DETAIL, MUTED_TEXT, ON_ACCENT, PANEL, SUNKEN, SURFACE, SURFACE_RAISED,
    TEXT,
};

mod canvas;
mod inspector;

use canvas::{draw_pad_labels, rect_edge_towards, CanvasLayout};
use inspector::{
    binding_summary, keyboard_key, mouse_button_editor, pad_feedback_editor, selection_description,
};

#[cfg(test)]
mod tests;

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
        BindableControl::LeftPadClick => Control::LeftPad,
        BindableControl::RightPadClick => Control::RightPad,
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
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, true])
                        .show(ui, |ui| match selection {
                            EditorSelection::Button(control) => self.binding_editor(ui, control),
                            EditorSelection::Pad(side) => self.pad_editor(ui, side),
                        });
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
                            MIN_PAD_SPEED_PERCENT..=MAX_PAD_SPEED_PERCENT,
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
                });
            }
            PadSide::Right => {
                let config = &mut self.store.profiles[self.selected].pads.right_mouse;
                ui.checkbox(&mut config.enabled, "Enable this pad");
                ui.add_space(16.0);
                ui.add_enabled_ui(config.enabled, |ui| {
                    ui.label("Pointer speed");
                    ui.add(
                        egui::Slider::new(
                            &mut config.speed_percent,
                            MIN_PAD_SPEED_PERCENT..=MAX_PAD_SPEED_PERCENT,
                        )
                        .suffix("%"),
                    );
                    ui.label(
                        egui::RichText::new(
                            "100% matches the measured lizard-mode response; this setting scales it linearly.",
                        )
                        .small()
                        .color(MUTED_TEXT),
                    );
                });
            }
        }
        ui.add_space(16.0);
        match side {
            PadSide::Left => pad_feedback_editor(
                ui,
                &mut self.store.profiles[self.selected].pads.left_scroll.feedback,
            ),
            PadSide::Right => pad_feedback_editor(
                ui,
                &mut self.store.profiles[self.selected].pads.right_mouse.feedback,
            ),
        }
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(12.0);
        ui.label("Pad click");
        ui.label(
            egui::RichText::new("Fires even when the pad function above is disabled.")
                .small()
                .color(MUTED_TEXT),
        );
        ui.add_space(12.0);
        self.binding_editor(
            ui,
            match side {
                PadSide::Left => BindableControl::LeftPadClick,
                PadSide::Right => BindableControl::RightPadClick,
            },
        );
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new("Pressure actions and gestures are not used.")
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
