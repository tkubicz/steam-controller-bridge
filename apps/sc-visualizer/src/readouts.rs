//! The text and instrument readouts: decoded source state, mapped output,
//! the stick crosshairs and the button name tables.

use eframe::egui;
use eframe::egui::{Sense, Stroke, Vec2};

use gamepad_state::{Button, GamepadState};
use steam_controller_protocol::{SteamButton, SteamControllerState};
use ui_theme::{
    ACCENT, BORDER, DETAIL, INSET, MUTED_TEXT, ON_ACCENT, OUTLINE, SURFACE_RAISED, TEXT,
};

pub(crate) fn source_state_ui(ui: &mut egui::Ui, state: &SteamControllerState) {
    ui.horizontal_wrapped(|ui| {
        stick(
            ui,
            "Left stick",
            Bounds::Round,
            state.left_stick_x,
            state.left_stick_y,
        );
        stick(
            ui,
            "Left pad",
            Bounds::Square,
            state.left_pad_x,
            state.left_pad_y,
        );
        stick(
            ui,
            "Right pad",
            Bounds::Square,
            state.right_pad_x,
            state.right_pad_y,
        );
        stick(
            ui,
            "Right stick",
            Bounds::Round,
            state.right_stick_x,
            state.right_stick_y,
        );
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        cell(ui, "Trigger L", &state.left_trigger.to_string());
        cell(ui, "Trigger R", &state.right_trigger.to_string());
        cell(ui, "L pad touch", yes_no(state.left_pad_touched));
        cell(ui, "L pad click", yes_no(state.left_pad_pressed));
        cell(ui, "R pad touch", yes_no(state.right_pad_touched));
        cell(ui, "R pad click", yes_no(state.right_pad_pressed));
        cell(ui, "L pad pressure", &state.left_pad_pressure.to_string());
        cell(ui, "R pad pressure", &state.right_pad_pressure.to_string());
        cell(ui, "IMU timestamp", &state.imu_timestamp.to_string());
    });
    ui.add_space(6.0);
    imu(ui, state);
    ui.add_space(6.0);
    button_sections(ui, &steam_sections(), |button| {
        state.buttons.contains(button)
    });
}

/// The Steam buttons, grouped as the controller is laid out.
///
/// Report-bit order interleaves the hands — it puts `R-grip Touch` next to
/// `L-grip Touch` — so the two hands are their own sections and get drawn as
/// two columns, left on the left. `sections_cover_every_steam_button` proves
/// nothing is lost in the regrouping.
fn steam_sections() -> Sections<SteamButton> {
    Sections {
        shared: vec![
            (
                "Face",
                vec![
                    (SteamButton::A, "A"),
                    (SteamButton::B, "B"),
                    (SteamButton::X, "X"),
                    (SteamButton::Y, "Y"),
                ],
            ),
            (
                "D-pad",
                vec![
                    (SteamButton::DpadUp, "Up"),
                    (SteamButton::DpadDown, "Down"),
                    (SteamButton::DpadLeft, "Left"),
                    (SteamButton::DpadRight, "Right"),
                ],
            ),
            (
                "System",
                vec![
                    // Physical names. The source bits are crossed here exactly
                    // as `hero::control_for_button` documents.
                    (SteamButton::Menu, "View"),
                    (SteamButton::View, "Menu"),
                    (SteamButton::Steam, "Steam"),
                    (SteamButton::QuickAccess, "Quick Access"),
                ],
            ),
        ],
        left: vec![
            (SteamButton::LeftShoulder, "LB"),
            (SteamButton::LeftTriggerClick, "LT Click"),
            (SteamButton::LeftStickPress, "L3"),
            (SteamButton::LeftStickTouch, "L-stick Touch"),
            (SteamButton::LeftPadClick, "L-pad Click"),
            (SteamButton::LeftPadTouch, "L-pad Touch"),
            (SteamButton::LeftGrip4, "L4"),
            (SteamButton::LeftGrip5, "L5"),
            (SteamButton::LeftGripTouch, "L-grip Touch"),
        ],
        right: vec![
            (SteamButton::RightShoulder, "RB"),
            (SteamButton::RightTriggerClick, "RT Click"),
            (SteamButton::RightStickPress, "R3"),
            (SteamButton::RightStickTouch, "R-stick Touch"),
            (SteamButton::RightPadClick, "R-pad Click"),
            (SteamButton::RightPadTouch, "R-pad Touch"),
            (SteamButton::RightGrip4, "R4"),
            (SteamButton::RightGrip5, "R5"),
            (SteamButton::RightGripTouch, "R-grip Touch"),
        ],
    }
}

/// The mapped gamepad buttons, grouped the same way.
fn gamepad_sections() -> Sections<Button> {
    Sections {
        shared: vec![
            (
                "Face",
                vec![
                    (Button::South, "South"),
                    (Button::East, "East"),
                    (Button::West, "West"),
                    (Button::North, "North"),
                ],
            ),
            (
                "System",
                vec![
                    (Button::Back, "Back"),
                    (Button::Start, "Start"),
                    (Button::Guide, "Guide"),
                    (Button::Extra1, "Extra 1"),
                    (Button::Extra2, "Extra 2"),
                    (Button::Extra3, "Extra 3"),
                ],
            ),
        ],
        left: vec![
            (Button::LeftShoulder, "LB"),
            (Button::LeftStick, "L3"),
            (Button::LeftGrip, "Left Grip"),
        ],
        right: vec![
            (Button::RightShoulder, "RB"),
            (Button::RightStick, "R3"),
            (Button::RightGrip, "Right Grip"),
        ],
    }
}

/// A labelled value. Keeps its place whatever the value does.
fn cell(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).small().color(MUTED_TEXT));
    ui.label(egui::RichText::new(value).monospace().color(TEXT));
    ui.add_space(8.0);
}

const fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

/// Gyro and acceleration as labelled bar meters.
///
/// Rendered as raw counts on purpose. `docs/STEAM_CONTROLLER_PROTOCOL.md` lists
/// the IMU scaling as unverified and there is no quaternion anywhere in the
/// repo, so anything that looked like a physical orientation would be a guess.
fn imu(ui: &mut egui::Ui, state: &SteamControllerState) {
    let Some(gyro) = state.gyro else {
        ui.label(egui::RichText::new("No IMU data in this report.").color(MUTED_TEXT));
        return;
    };
    let accel = state.acceleration;
    ui.label(
        egui::RichText::new("IMU — raw counts, scaling unverified")
            .small()
            .color(MUTED_TEXT),
    );
    ui.horizontal_wrapped(|ui| {
        meter(ui, "Gyro X", gyro.x);
        meter(ui, "Gyro Y", gyro.y);
        meter(ui, "Gyro Z", gyro.z);
    });
    if let Some(accel) = accel {
        ui.horizontal_wrapped(|ui| {
            meter(ui, "Accel X", accel.x);
            meter(ui, "Accel Y", accel.y);
            meter(ui, "Accel Z", accel.z);
        });
    }
}

/// One signed bar, filling from the centre, plus the raw number.
fn meter(ui: &mut egui::Ui, label: &str, value: i16) {
    ui.label(egui::RichText::new(label).small().color(MUTED_TEXT));
    let (response, painter) = ui.allocate_painter(Vec2::new(90.0, 12.0), Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 2.0, INSET);
    let middle = rect.center().x;
    painter.line_segment(
        [
            egui::pos2(middle, rect.top()),
            egui::pos2(middle, rect.bottom()),
        ],
        Stroke::new(1.0, DETAIL),
    );
    let fraction = (f32::from(value) / 32768.0).clamp(-1.0, 1.0);
    let reach = rect.width() * 0.5 * fraction;
    let bar = egui::Rect::from_min_max(
        egui::pos2(middle.min(middle + reach), rect.top() + 2.0),
        egui::pos2(middle.max(middle + reach), rect.bottom() - 2.0),
    );
    painter.rect_filled(bar, 1.0, ACCENT);
    ui.label(
        egui::RichText::new(format!("{value:>6}"))
            .monospace()
            .color(TEXT),
    );
    ui.add_space(8.0);
}

/// Sections of the button list, so the grid reads as the controller rather
/// than as report-bit order.
///
/// Bit order interleaves the hands — it puts `R-grip Touch` next to
/// `L-grip Touch` — which is why the left-hand and right-hand groups are drawn
/// as two columns with the left one on the left.
struct Sections<T> {
    /// Drawn full width, above the two hands.
    shared: Vec<(&'static str, Vec<(T, &'static str)>)>,
    left: Vec<(T, &'static str)>,
    right: Vec<(T, &'static str)>,
}

fn button_sections<T: Copy>(ui: &mut egui::Ui, sections: &Sections<T>, held: impl Fn(T) -> bool) {
    for (heading, entries) in &sections.shared {
        section_label(ui, heading);
        chip_row(ui, entries, &held);
        ui.add_space(4.0);
    }
    // The two hands mirror each other, so pair them row by row: every `L`
    // control sits on the left, directly beside its `R` twin, which makes them
    // comparable at a glance. A fixed two-column grid also wraps
    // deterministically, where a wrapped row of chips depends on whatever width
    // it happens to be handed.
    section_label(ui, "Left / Right");
    egui::Grid::new("hands")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for (left, right) in sections.left.iter().zip(&sections.right) {
                chip(ui, left.1, held(left.0));
                chip(ui, right.1, held(right.0));
                ui.end_row();
            }
        });
}

fn section_label(ui: &mut egui::Ui, heading: &str) {
    ui.label(
        egui::RichText::new(heading.to_uppercase())
            .small()
            .color(DETAIL),
    );
}

/// Every name keeps its place; a press changes a chip's style, never the
/// layout, so nothing shifts under the eye.
fn chip_row<T: Copy>(ui: &mut egui::Ui, entries: &[(T, &'static str)], held: &impl Fn(T) -> bool) {
    ui.horizontal_wrapped(|ui| {
        for (button, name) in entries {
            chip(ui, name, held(*button));
        }
    });
}

/// Active chips differ in fill, outline and text weight, not colour alone.
fn chip(ui: &mut egui::Ui, name: &str, held: bool) {
    let text = if held {
        egui::RichText::new(name).strong().color(ON_ACCENT)
    } else {
        egui::RichText::new(name).small().color(MUTED_TEXT)
    };
    egui::Frame::new()
        .fill(if held { ACCENT } else { SURFACE_RAISED })
        .stroke(egui::Stroke::new(
            if held { 1.6 } else { 1.0 },
            if held { ACCENT } else { BORDER },
        ))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| ui.label(text));
}

pub(crate) fn mapped_state_ui(ui: &mut egui::Ui, state: &GamepadState) {
    ui.horizontal_wrapped(|ui| {
        normalized_stick(ui, "Left", state.left_x, state.left_y);
        normalized_stick(ui, "Right", state.right_x, state.right_y);
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        cell(ui, "Hat", &format!("{:?}", state.hat));
        cell(ui, "Trigger L", &format!("{:.3}", state.left_trigger));
        cell(ui, "Trigger R", &format!("{:.3}", state.right_trigger));
    });
    ui.add_space(6.0);
    button_sections(ui, &gamepad_sections(), |button| {
        state.buttons.contains(button)
    });
}

/// The travel a control's own bounds have: a stick sweeps a circle, a trackpad
/// is a square. Drawing both as circles implies the pads clip at their
/// diagonals, which they do not.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bounds {
    Round,
    Square,
}

/// A source axis pair: the crosshair plus the raw `i16` values it came from.
///
/// The illustration shows where a control physically is; this shows what the
/// report actually said, which is what you need when the two disagree.
fn stick(ui: &mut egui::Ui, label: &str, bounds: Bounds, x: i16, y: i16) {
    crosshair(
        ui,
        label,
        bounds,
        f32::from(x) / 32767.0,
        f32::from(y) / 32767.0,
        &format!("{x:>6} {y:>6}"),
    );
}

/// A mapped axis pair, already normalized to `-1..=1`.
fn normalized_stick(ui: &mut egui::Ui, label: &str, x: f32, y: f32) {
    crosshair(
        ui,
        label,
        Bounds::Round,
        x,
        y,
        &format!("{x:>6.3} {y:>6.3}"),
    );
}

fn crosshair(ui: &mut egui::Ui, label: &str, bounds: Bounds, x: f32, y: f32, value: &str) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).small().color(MUTED_TEXT));
        let (response, painter) = ui.allocate_painter(Vec2::splat(100.0), Sense::hover());
        let center = response.rect.center();
        match bounds {
            Bounds::Round => {
                painter.circle_stroke(center, 45.0, Stroke::new(1.0, OUTLINE));
            }
            Bounds::Square => {
                painter.rect_stroke(
                    egui::Rect::from_center_size(center, Vec2::splat(90.0)),
                    4.0,
                    Stroke::new(1.0, OUTLINE),
                    egui::StrokeKind::Inside,
                );
            }
        }
        painter.line_segment(
            [center - Vec2::new(45.0, 0.0), center + Vec2::new(45.0, 0.0)],
            Stroke::new(1.0, DETAIL),
        );
        painter.line_segment(
            [center - Vec2::new(0.0, 45.0), center + Vec2::new(0.0, 45.0)],
            Stroke::new(1.0, DETAIL),
        );
        painter.circle_filled(
            center + Vec2::new(x.clamp(-1.0, 1.0), -y.clamp(-1.0, 1.0)) * 45.0,
            5.0,
            ACCENT,
        );
        // The numeric readout these widgets never had.
        ui.label(egui::RichText::new(value).monospace().small().color(TEXT));
        ui.add_space(6.0);
    });
}

#[cfg(test)]
mod tests {
    use super::{gamepad_sections, steam_sections, Sections};
    use crate::hero::ALL_STEAM_BUTTONS;
    use gamepad_state::Button;
    use std::collections::BTreeSet;
    use steam_controller_protocol::SteamButton;

    fn flatten<T: Copy>(sections: &Sections<T>) -> Vec<(T, &'static str)> {
        sections
            .shared
            .iter()
            .flat_map(|(_, entries)| entries.iter())
            .chain(sections.left.iter())
            .chain(sections.right.iter())
            .copied()
            .collect()
    }

    /// Regrouping must not quietly drop a button. The flat 30-entry table used
    /// to be the guarantee; this is.
    #[test]
    fn sections_cover_every_steam_button_exactly_once() {
        let listed = flatten(&steam_sections());
        let buttons: BTreeSet<u8> = listed.iter().map(|(b, _)| *b as u8).collect();
        assert_eq!(
            buttons.len(),
            listed.len(),
            "a Steam button is listed twice"
        );
        let expected: BTreeSet<u8> = ALL_STEAM_BUTTONS.iter().map(|b| *b as u8).collect();
        assert_eq!(buttons, expected, "the sections and the bit list disagree");
    }

    #[test]
    fn sections_cover_every_gamepad_button_exactly_once() {
        let listed = flatten(&gamepad_sections());
        let buttons: BTreeSet<u8> = listed.iter().map(|(b, _)| *b as u8).collect();
        assert_eq!(
            buttons.len(),
            listed.len(),
            "a gamepad button is listed twice"
        );
        let expected: BTreeSet<u8> = [
            Button::South,
            Button::East,
            Button::West,
            Button::North,
            Button::LeftShoulder,
            Button::RightShoulder,
            Button::LeftStick,
            Button::RightStick,
            Button::Back,
            Button::Start,
            Button::Guide,
            Button::LeftGrip,
            Button::RightGrip,
            Button::Extra1,
            Button::Extra2,
            Button::Extra3,
        ]
        .iter()
        .map(|b| *b as u8)
        .collect();
        assert_eq!(buttons, expected);
    }

    /// The complaint that started this: bit order put `R-grip Touch` beside
    /// `L-grip Touch`. Every left-hand control must now be in the left column.
    #[test]
    fn the_hands_are_separated_and_left_comes_first() {
        let sections = steam_sections();
        for (button, name) in &sections.left {
            assert!(
                matches!(
                    button,
                    SteamButton::LeftShoulder
                        | SteamButton::LeftTriggerClick
                        | SteamButton::LeftStickPress
                        | SteamButton::LeftStickTouch
                        | SteamButton::LeftPadClick
                        | SteamButton::LeftPadTouch
                        | SteamButton::LeftGrip4
                        | SteamButton::LeftGrip5
                        | SteamButton::LeftGripTouch
                ),
                "{name} is not a left-hand control"
            );
        }
        assert_eq!(
            sections.left.len(),
            sections.right.len(),
            "the two hands should mirror each other"
        );
        // And no left-hand control leaked into the shared sections.
        for (_, entries) in &sections.shared {
            for (_, name) in entries {
                assert!(
                    !name.starts_with("L-") && !name.starts_with("R-"),
                    "{name} belongs to a hand, not to a shared section"
                );
            }
        }
    }

    /// The physical View/Menu buttons are drawn from the crossed source bits;
    /// see `hero::control_for_button`.
    #[test]
    fn the_system_section_uses_physical_button_names() {
        let sections = steam_sections();
        let system = sections
            .shared
            .iter()
            .find(|(heading, _)| *heading == "System")
            .expect("a System section exists");
        assert!(system.1.contains(&(SteamButton::Menu, "View")));
        assert!(system.1.contains(&(SteamButton::View, "Menu")));
    }
}
