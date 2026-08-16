use std::collections::BTreeSet;

use controller_art::{
    body_bounds, control_rect, draw_trackpad_surface, normalized_point, trackpad_rect,
    trackpad_surface_point, Control, ControlState, PadSide,
};
use desktop_bindings::{
    default_store_path, load_or_create_store, save_store, BindableControl, BindingAction,
    BindingStore, KeyboardKey, Modifier, MouseButton, PadBindings, PadConfig, PadFeedbackConfig,
    PadFeedbackStrength, PadMotionMode, PadRegion, PadRegionShape, PadTrigger, MAX_PAD_REGIONS,
    MAX_PAD_SPEED_PERCENT, MAX_PROFILE_NAME_CHARS, MAX_REGION_NAME_CHARS, MIN_PAD_SPEED_PERCENT,
};
use eframe::egui;

use ui_theme::{
    ACCENT, BORDER, DANGER, DETAIL, MUTED_TEXT, ON_ACCENT, PANEL, SUNKEN, SURFACE, SURFACE_RAISED,
    TEXT,
};

mod canvas;
mod inspector;
mod regions;

use canvas::{draw_pad_labels, rect_edge_towards, CanvasLayout};
use inspector::{
    binding_summary, keyboard_key, mouse_button_editor, pad_feedback_editor, selection_description,
};
use regions::draw_region_map;

#[cfg(test)]
mod tests;

const WINDOW_SIZE: [f32; 2] = [1260.0, 720.0];
const MIN_WINDOW_SIZE: [f32; 2] = [1080.0, 660.0];
const INSPECTOR_WIDTH: f32 = 300.0;
const COLUMN_GAP: f32 = 16.0;
const CANVAS_MIN_WIDTH: f32 = 620.0;
type RegionPreset = (&'static str, fn() -> Vec<PadRegion>);
const REGION_PRESETS: [RegionPreset; 5] = [
    ("Whole pad", PadRegion::whole),
    ("Four way", PadRegion::four_way),
    ("Four way + center", PadRegion::four_way_with_center),
    ("Eight way", PadRegion::eight_way),
    ("Eight way + center", PadRegion::eight_way_with_center),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerView {
    Front,
    Rear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EditorSelection {
    Button(BindableControl),
    Pad(PadSide),
    /// A region of a pad, by its position in that pad's ordered list.
    PadRegion(PadSide, usize),
}

impl EditorSelection {
    const fn label(self) -> &'static str {
        match self {
            Self::Button(control) => control.label(),
            Self::Pad(side) | Self::PadRegion(side, _) => side.label(),
        }
    }
}

/// Which action slot the inspector is editing. Buttons and both region triggers
/// share one action picker, so they share one way of naming a slot; it doubles
/// as the egui ID salt that keeps their widgets distinct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ActionTarget {
    Button(BindableControl),
    Region(PadSide, usize, PadTrigger),
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

const fn binding_side(side: PadSide) -> desktop_bindings::PadSide {
    match side {
        PadSide::Left => desktop_bindings::PadSide::Left,
        PadSide::Right => desktop_bindings::PadSide::Right,
    }
}

const fn art_selection(selection: EditorSelection) -> Control {
    match selection {
        EditorSelection::Button(control) => art_control(control),
        EditorSelection::Pad(PadSide::Left) | EditorSelection::PadRegion(PadSide::Left, _) => {
            Control::LeftPad
        }
        EditorSelection::Pad(PadSide::Right) | EditorSelection::PadRegion(PadSide::Right, _) => {
            Control::RightPad
        }
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
    capturing: Option<ActionTarget>,
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
        let Some(target) = self.capturing else {
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
        if let Some(slot) = self.action_slot(target) {
            *slot = Some(BindingAction::KeyChord {
                key,
                modifiers: binding_modifiers,
            });
        }
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
            &self.store.profiles[self.selected].pads,
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
                EditorSelection::Pad(side) | EditorSelection::PadRegion(side, _) => {
                    self.select_pad(side);
                }
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
                            EditorSelection::Button(control) => {
                                self.binding_editor(ui, ActionTarget::Button(control));
                            }
                            EditorSelection::Pad(side) | EditorSelection::PadRegion(side, _) => {
                                self.pad_editor(ui, side);
                            }
                        });
                });
            });
    }

    fn pad(&mut self, side: PadSide) -> &mut PadConfig {
        self.store.profiles[self.selected]
            .pads
            .get_mut(binding_side(side))
    }

    fn pad_editor(&mut self, ui: &mut egui::Ui, side: PadSide) {
        ui.label("Motion");
        let config = self.pad(side);
        ui.horizontal_wrapped(|ui| {
            for mode in PadMotionMode::ALL {
                ui.selectable_value(&mut config.motion, mode, mode.label());
            }
        });
        ui.add_space(16.0);
        ui.add_enabled_ui(config.motion != PadMotionMode::None, |ui| {
            ui.label(match config.motion {
                PadMotionMode::Scroll => "Scroll speed",
                _ => "Pointer speed",
            });
            ui.add(
                egui::Slider::new(
                    &mut config.speed_percent,
                    MIN_PAD_SPEED_PERCENT..=MAX_PAD_SPEED_PERCENT,
                )
                .suffix("%"),
            );
            ui.label(
                egui::RichText::new(match config.motion {
                    PadMotionMode::Scroll => "Faster swipes accelerate above this base speed.",
                    _ => "100% matches the measured lizard-mode response; this setting scales it linearly.",
                })
                .small()
                .color(MUTED_TEXT),
            );
            if config.motion == PadMotionMode::Scroll {
                ui.add_space(12.0);
                ui.checkbox(&mut config.momentum, "Momentum after release");
            }
        });
        ui.add_space(16.0);
        pad_feedback_editor(ui, &mut self.pad(side).feedback);
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(12.0);
        self.region_editor(ui, side);
    }

    /// The region list, its map, and the selected region's two action slots.
    fn region_editor(&mut self, ui: &mut egui::Ui, side: PadSide) {
        ui.label("Regions");
        ui.label(
            egui::RichText::new(
                "Areas of the pad with their own actions. Regions may overlap; the first one in \
                 this list that contains the finger wins. Clicks and touches fire whether or not \
                 the pad drives motion.",
            )
            .small()
            .color(MUTED_TEXT),
        );
        ui.add_space(12.0);

        let mut preset = None;
        egui::ComboBox::from_id_salt(("region-preset", side))
            .width(ui.available_width())
            .selected_text("Apply a layout…")
            .show_ui(ui, |ui| {
                for (label, build) in REGION_PRESETS {
                    if ui.selectable_label(false, label).clicked() {
                        preset = Some(build);
                    }
                }
            });
        if let Some(build) = preset {
            self.pad(side).regions = build();
            self.selection = EditorSelection::Pad(side);
            self.capturing = None;
        }
        ui.add_space(12.0);
        self.region_list(ui, side);

        let Some(index) = self.selected_region(side) else {
            return;
        };
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(12.0);
        self.region_shape_editor(ui, side, index);
        for trigger in PadTrigger::ALL {
            ui.add_space(16.0);
            ui.label(format!("{} action", trigger.label()));
            ui.label(
                egui::RichText::new(match trigger {
                    PadTrigger::Click => {
                        "Held from the press until the pad is released, even if the finger slides \
                         into another region."
                    }
                    PadTrigger::Touch => {
                        "Held while the finger is inside this region, and handed over when it \
                         crosses into another."
                    }
                })
                .small()
                .color(MUTED_TEXT),
            );
            ui.add_space(8.0);
            self.binding_editor(ui, ActionTarget::Region(side, index, trigger));
        }
    }

    /// The map of this pad's regions and the ordered list beneath it.
    fn region_list(&mut self, ui: &mut egui::Ui, side: PadSide) {
        let selected = self.selected_region(side);
        let (map_rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ui.available_width().min(190.0)),
            egui::Sense::hover(),
        );
        draw_region_map(
            &ui.painter_at(map_rect),
            map_rect,
            side,
            &self.pad(side).regions,
            selected,
        );
        ui.add_space(12.0);

        let mut clicked = None;
        let mut remove = None;
        let mut raise = None;
        for (index, region) in self.pad(side).regions.iter().enumerate() {
            ui.horizontal(|ui| {
                let chosen = selected == Some(index);
                let label = format!(
                    "{}  ·  {} / {}",
                    region.name,
                    binding_summary(region.click.as_ref()),
                    binding_summary(region.touch.as_ref())
                );
                if ui.selectable_label(chosen, label).clicked() {
                    clicked = Some(index);
                }
                if ui
                    .add_enabled(index > 0, egui::Button::new("↑"))
                    .on_hover_text("Resolve this region before the one above it")
                    .clicked()
                {
                    raise = Some(index);
                }
                if ui.button("✕").on_hover_text("Delete this region").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = clicked {
            self.selection = EditorSelection::PadRegion(side, index);
            self.capturing = None;
        }
        if let Some(index) = raise {
            self.pad(side).regions.swap(index - 1, index);
            self.selection = EditorSelection::PadRegion(side, index - 1);
            self.capturing = None;
        }
        if let Some(index) = remove {
            self.pad(side).regions.remove(index);
            self.selection = EditorSelection::Pad(side);
            self.capturing = None;
        }

        ui.add_space(8.0);
        let room = self.pad(side).regions.len() < MAX_PAD_REGIONS;
        if ui
            .add_enabled(room, egui::Button::new("Add region"))
            .clicked()
        {
            self.add_region(side);
        }
        if !room {
            ui.label(
                egui::RichText::new(format!("At most {MAX_PAD_REGIONS} regions per pad."))
                    .small()
                    .color(MUTED_TEXT),
            );
        }
    }

    fn region_shape_editor(&mut self, ui: &mut egui::Ui, side: PadSide, index: usize) {
        let Some(region) = self.pad(side).regions.get_mut(index) else {
            return;
        };
        ui.label("Region name");
        ui.add(
            egui::TextEdit::singleline(&mut region.name)
                .char_limit(MAX_REGION_NAME_CHARS)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(12.0);
        ui.label("Shape");
        ui.label(
            egui::RichText::new("Zero degrees points up; angles increase clockwise.")
                .small()
                .color(MUTED_TEXT),
        );
        let shape = &mut region.shape;
        ui.add(egui::Slider::new(&mut shape.start_degrees, 0..=359).text("Start"));
        ui.add(egui::Slider::new(&mut shape.sweep_degrees, 1..=360).text("Sweep"));
        ui.add(egui::Slider::new(&mut shape.inner_percent, 0..=99).text("Inner %"));
        ui.add(egui::Slider::new(&mut shape.outer_percent, 1..=100).text("Outer %"));
        // The store rejects an inverted band, so keep the two ends from crossing
        // instead of letting the user build something Save would refuse.
        if shape.inner_percent >= shape.outer_percent {
            shape.inner_percent = shape.outer_percent - 1;
        }
    }

    /// Adds a region with an unused name and selects it.
    fn add_region(&mut self, side: PadSide) {
        let pad = self.pad(side);
        let name = (1..=MAX_PAD_REGIONS + 1)
            .map(|number| format!("Region {number}"))
            .find(|candidate| {
                !pad.regions
                    .iter()
                    .any(|region| region.name.eq_ignore_ascii_case(candidate))
            })
            .expect("a bounded region list always leaves a name free");
        pad.regions
            .push(PadRegion::new(name, PadRegionShape::WHOLE));
        self.selection = EditorSelection::PadRegion(side, self.pad(side).regions.len() - 1);
        self.capturing = None;
    }

    /// The selected region index, if the selection names one on this pad and it
    /// still exists.
    fn selected_region(&self, side: PadSide) -> Option<usize> {
        match self.selection {
            EditorSelection::PadRegion(selected, index)
                if selected == side
                    && index
                        < self.store.profiles[self.selected]
                            .pads
                            .get(binding_side(side))
                            .regions
                            .len() =>
            {
                Some(index)
            }
            _ => None,
        }
    }

    /// The action slot a target names, or `None` if it names a region that has
    /// since been deleted.
    fn action_slot(&mut self, target: ActionTarget) -> Option<&mut Option<BindingAction>> {
        match target {
            ActionTarget::Button(control) => {
                Some(self.store.profiles[self.selected].bindings.get_mut(control))
            }
            ActionTarget::Region(side, index, trigger) => {
                Some(self.pad(side).regions.get_mut(index)?.action_mut(trigger))
            }
        }
    }

    /// The one action picker, shared by the buttons and by both region triggers.
    fn binding_editor(&mut self, ui: &mut egui::Ui, target: ActionTarget) {
        let Some(current) = self.action_slot(target).map(|slot| slot.clone()) else {
            return;
        };
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
                self.key_chord_editor(ui, target, key, modifiers);
            }
            Some(BindingAction::MouseButton { button }) => {
                mouse_button_editor(ui, target, button);
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
        if let Some(slot) = self.action_slot(target) {
            *slot = replacement;
        }
    }

    fn key_chord_editor(
        &mut self,
        ui: &mut egui::Ui,
        target: ActionTarget,
        key: &mut KeyboardKey,
        modifiers: &mut BTreeSet<Modifier>,
    ) {
        ui.label("Key");
        egui::ComboBox::from_id_salt(("key", target))
            .width(ui.available_width())
            .selected_text(key.label())
            .show_ui(ui, |ui| {
                for candidate in KeyboardKey::ALL {
                    ui.selectable_value(key, *candidate, candidate.label());
                }
            });
        ui.add_space(12.0);
        ui.label("Modifiers");
        egui::Grid::new(("modifiers", target))
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
        let capture_label = if self.capturing == Some(target) {
            "Press any supported key…"
        } else {
            "Capture key chord"
        };
        let capture =
            egui::Button::new(capture_label).min_size(egui::vec2(ui.available_width(), 34.0));
        if ui.add(capture).clicked() {
            self.capturing = Some(target);
        }
        if self.capturing == Some(target) {
            ui.label(
                egui::RichText::new("Press a key with any modifiers you want to include.")
                    .small()
                    .color(ACCENT),
            );
        }
    }

    fn normalize_names(&mut self) {
        for profile in &mut self.store.profiles {
            profile.name = profile.name.trim().to_owned();
            for pad in [&mut profile.pads.left, &mut profile.pads.right] {
                for region in &mut pad.regions {
                    region.name = region.name.trim().to_owned();
                }
            }
        }
    }

    fn save_and_close(&mut self, ctx: &egui::Context) {
        self.normalize_names();
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
