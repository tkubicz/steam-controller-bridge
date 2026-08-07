//! The text and instrument readouts: decoded source state, mapped output,
//! the stick crosshairs and the button name tables.

use eframe::egui;
use eframe::egui::{Sense, Stroke, Vec2};

use controller_mapper::RightAxisSource;
use gamepad_state::{Button, GamepadState};
use steam_controller_protocol::{SteamButton, SteamControllerState};
use ui_theme::{
    ACCENT, BORDER, DANGER, DETAIL, INSET, MUTED_TEXT, ON_ACCENT, OUTLINE, SURFACE_RAISED, TEXT,
};

/// The radial dead zones the mapper is configured with.
///
/// Drawn on the decoded controls so resting jitter can be judged against what
/// will actually be filtered. Which control each one acts on is not a display
/// choice: `left_stick_dead_zone` always applies to the left axes, which the
/// left stick always feeds, while `right_axis_dead_zone` applies to the right
/// axes, which `right_axis_source` routes from either the right stick or the
/// right pad. Drawing the right ring on the stick regardless would show a
/// threshold that is not being applied there.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DeadZones {
    pub(crate) left_stick: f32,
    pub(crate) right_axis: f32,
    pub(crate) right_source: RightAxisSource,
}

impl DeadZones {
    /// The dead zone actually applied to one decoded control, if any.
    fn applied_to(self, control: Axis) -> Option<f32> {
        match control {
            Axis::LeftStick => Some(self.left_stick),
            Axis::RightStick => {
                (self.right_source == RightAxisSource::RightStick).then_some(self.right_axis)
            }
            Axis::RightPad => {
                (self.right_source == RightAxisSource::RightPad).then_some(self.right_axis)
            }
            // The left pad feeds no gamepad axis, so the mapper never filters
            // it. The bridge's desktop scrolling has its own threshold, which
            // this app does not run.
            Axis::LeftPad => None,
        }
    }

    /// Names the control the right dead zone is currently acting on.
    const fn right_target(self) -> &'static str {
        match self.right_source {
            RightAxisSource::RightStick => "right stick",
            RightAxisSource::RightPad => "right pad",
        }
    }
}

/// The four decoded position controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    LeftStick,
    LeftPad,
    RightPad,
    RightStick,
}

pub(crate) fn source_state_ui(
    ui: &mut egui::Ui,
    state: &SteamControllerState,
    dead_zones: DeadZones,
) {
    ui.horizontal_wrapped(|ui| {
        for (axis, label, bounds, x, y) in [
            (
                Axis::LeftStick,
                "Left stick",
                Bounds::Round,
                state.left_stick_x,
                state.left_stick_y,
            ),
            (
                Axis::LeftPad,
                "Left pad",
                Bounds::Square,
                state.left_pad_x,
                state.left_pad_y,
            ),
            (
                Axis::RightPad,
                "Right pad",
                Bounds::Square,
                state.right_pad_x,
                state.right_pad_y,
            ),
            (
                Axis::RightStick,
                "Right stick",
                Bounds::Round,
                state.right_stick_x,
                state.right_stick_y,
            ),
        ] {
            stick(ui, label, bounds, dead_zones.applied_to(axis), x, y);
        }
    });
    // Absence of a ring has to mean "no dead zone here", not "not drawn".
    ui.label(
        egui::RichText::new(format!(
            "Inner ring: radial dead zone the mapper applies — {:.3} on the left stick, \
             {:.3} on the {}. No ring means none is applied to that control.",
            dead_zones.left_stick,
            dead_zones.right_axis,
            dead_zones.right_target(),
        ))
        .small()
        .color(MUTED_TEXT),
    );
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
fn stick(ui: &mut egui::Ui, label: &str, bounds: Bounds, dead_zone: Option<f32>, x: i16, y: i16) {
    crosshair(
        ui,
        label,
        bounds,
        dead_zone,
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
        None,
        x,
        y,
        &format!("{x:>6.3} {y:>6.3}"),
    );
}

fn crosshair(
    ui: &mut egui::Ui,
    label: &str,
    bounds: Bounds,
    dead_zone: Option<f32>,
    x: f32,
    y: f32,
    value: &str,
) {
    const RING: f32 = 45.0;
    const DOT: f32 = 5.0;
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).small().color(MUTED_TEXT));
        let (response, painter) = ui.allocate_painter(Vec2::splat(100.0), Sense::hover());
        let center = response.rect.center();
        // A round control's raw magnitude can exceed 1: each axis caps at
        // 32767, so a full diagonal reaches sqrt(2). The dot has to be brought
        // back inside the well to be drawn, and that would make "past full
        // deflection" look identical to "exactly full" — so the bound itself
        // reports the clipping instead of hiding it. The mapper discards the
        // same excess (`magnitude.min(1.0)`), so this marks real lost range.
        let saturated = bounds == Bounds::Round && x.hypot(y) > 1.0;
        match bounds {
            Bounds::Round => {
                painter.circle_stroke(
                    center,
                    RING,
                    if saturated {
                        Stroke::new(2.0, DANGER)
                    } else {
                        Stroke::new(1.0, OUTLINE)
                    },
                );
            }
            Bounds::Square => {
                painter.rect_stroke(
                    egui::Rect::from_center_size(center, Vec2::splat(RING * 2.0)),
                    4.0,
                    Stroke::new(1.0, OUTLINE),
                    egui::StrokeKind::Inside,
                );
            }
        }
        painter.line_segment(
            [center - Vec2::new(RING, 0.0), center + Vec2::new(RING, 0.0)],
            Stroke::new(1.0, DETAIL),
        );
        painter.line_segment(
            [center - Vec2::new(0.0, RING), center + Vec2::new(0.0, RING)],
            Stroke::new(1.0, DETAIL),
        );
        // A round control is bounded by its magnitude, not by each axis: raw
        // stick values are not magnitude-capped the way the mapper's output is,
        // so clamping x and y separately would draw a full diagonal outside the
        // ring. The numeric readout still shows the true raw values.
        let (x, y) = match bounds {
            Bounds::Round => controller_art::clamp_to_unit_circle(x, y),
            Bounds::Square => (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0)),
        };
        // Keep the dot's own body inside the bound as well.
        painter.circle_filled(center + Vec2::new(x, -y) * (RING - DOT), DOT, ACCENT);
        // The dead zone the mapper will apply, drawn *over* the dot: a resting
        // stick's jitter is smaller than the dot itself, so a ring underneath
        // would be hidden by the very reading it is there to qualify.
        if let Some(dead_zone) = dead_zone.filter(|value| *value > 0.0) {
            painter.circle_stroke(
                center,
                RING * dead_zone,
                Stroke::new(1.0, MUTED_TEXT.gamma_multiply(0.8)),
            );
        }
        // The numeric readout these widgets never had. It always shows the
        // true reading, clipped or not.
        ui.label(
            egui::RichText::new(value)
                .monospace()
                .small()
                .color(if saturated { DANGER } else { TEXT }),
        );
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

#[cfg(test)]
mod dead_zone_tests {
    use super::{Axis, DeadZones};
    use controller_mapper::RightAxisSource;

    fn zones(source: RightAxisSource) -> DeadZones {
        DeadZones {
            left_stick: 0.08,
            right_axis: 0.15,
            right_source: source,
        }
    }

    /// `right_axis_dead_zone` filters whichever control feeds the right axes.
    /// Showing it on the stick while the pad is the source would draw a
    /// threshold that is not being applied there.
    #[test]
    fn the_right_dead_zone_follows_the_configured_axis_source() {
        let stick = zones(RightAxisSource::RightStick);
        assert_eq!(stick.applied_to(Axis::RightStick), Some(0.15));
        assert_eq!(stick.applied_to(Axis::RightPad), None);

        let pad = zones(RightAxisSource::RightPad);
        assert_eq!(pad.applied_to(Axis::RightPad), Some(0.15));
        assert_eq!(pad.applied_to(Axis::RightStick), None);
    }

    /// The left axes are always fed by the left stick, so its dead zone always
    /// applies; the left pad feeds no gamepad axis, so none ever does.
    #[test]
    fn the_left_side_does_not_depend_on_configuration() {
        for source in [RightAxisSource::RightStick, RightAxisSource::RightPad] {
            let zones = zones(source);
            assert_eq!(zones.applied_to(Axis::LeftStick), Some(0.08));
            assert_eq!(zones.applied_to(Axis::LeftPad), None);
        }
    }

    #[test]
    fn the_caption_names_the_control_the_right_zone_acts_on() {
        assert_eq!(
            zones(RightAxisSource::RightStick).right_target(),
            "right stick"
        );
        assert_eq!(zones(RightAxisSource::RightPad).right_target(), "right pad");
    }
}
