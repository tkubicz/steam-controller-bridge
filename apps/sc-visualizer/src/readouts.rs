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
        stick(ui, "Left stick", state.left_stick_x, state.left_stick_y);
        stick(ui, "Left pad", state.left_pad_x, state.left_pad_y);
        stick(ui, "Right pad", state.right_pad_x, state.right_pad_y);
        stick(ui, "Right stick", state.right_stick_x, state.right_stick_y);
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
    button_grid(
        ui,
        "steam-buttons",
        STEAM_BUTTONS
            .iter()
            .map(|(button, name)| (*name, state.buttons.contains(*button))),
    );
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

/// Every name, always in the same place. A press changes a chip's style, never
/// the layout, so nothing shifts under the eye.
fn button_grid<'a>(ui: &mut egui::Ui, id: &str, entries: impl Iterator<Item = (&'a str, bool)>) {
    // A width ladder rather than a division, so there is no float-to-integer
    // cast to reason about. Bounded at both ends so the grid keeps a stable
    // shape as the window moves.
    const LADDER: [(f32, usize); 7] = [
        (944.0, 8),
        (826.0, 7),
        (708.0, 6),
        (590.0, 5),
        (472.0, 4),
        (354.0, 3),
        (0.0, 2),
    ];
    let available = ui.available_width();
    let columns = LADDER
        .iter()
        .find(|(width, _)| available >= *width)
        .map_or(2, |(_, columns)| *columns);
    egui::Grid::new(id)
        .num_columns(columns)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            for (index, (name, held)) in entries.enumerate() {
                chip(ui, name, held);
                if (index + 1) % columns == 0 {
                    ui.end_row();
                }
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
    button_grid(
        ui,
        "gamepad-buttons",
        GAMEPAD_BUTTONS
            .iter()
            .map(|(button, name)| (*name, state.buttons.contains(*button))),
    );
}

/// A source axis pair: the crosshair plus the raw `i16` values it came from.
///
/// The illustration shows where a control physically is; this shows what the
/// report actually said, which is what you need when the two disagree.
fn stick(ui: &mut egui::Ui, label: &str, x: i16, y: i16) {
    crosshair(
        ui,
        label,
        f32::from(x) / 32767.0,
        f32::from(y) / 32767.0,
        &format!("{x:>6} {y:>6}"),
    );
}

/// A mapped axis pair, already normalized to `-1..=1`.
fn normalized_stick(ui: &mut egui::Ui, label: &str, x: f32, y: f32) {
    crosshair(ui, label, x, y, &format!("{x:>6.3} {y:>6.3}"));
}

pub(crate) fn crosshair(ui: &mut egui::Ui, label: &str, x: f32, y: f32, value: &str) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).small().color(MUTED_TEXT));
        let (response, painter) = ui.allocate_painter(Vec2::splat(100.0), Sense::hover());
        let center = response.rect.center();
        painter.circle_stroke(center, 45.0, Stroke::new(1.0, OUTLINE));
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

const STEAM_BUTTONS: [(SteamButton, &str); 30] = [
    (SteamButton::A, "A"),
    (SteamButton::B, "B"),
    (SteamButton::X, "X"),
    (SteamButton::Y, "Y"),
    (SteamButton::QuickAccess, "Quick Access"),
    (SteamButton::RightStickPress, "R3"),
    (SteamButton::View, "View"),
    (SteamButton::RightGrip4, "R4"),
    (SteamButton::RightGrip5, "R5"),
    (SteamButton::RightShoulder, "RB"),
    (SteamButton::DpadDown, "D-pad Down"),
    (SteamButton::DpadRight, "D-pad Right"),
    (SteamButton::DpadLeft, "D-pad Left"),
    (SteamButton::DpadUp, "D-pad Up"),
    (SteamButton::Menu, "Menu"),
    (SteamButton::LeftStickPress, "L3"),
    (SteamButton::Steam, "Steam"),
    (SteamButton::LeftGrip4, "L4"),
    (SteamButton::LeftGrip5, "L5"),
    (SteamButton::LeftShoulder, "LB"),
    (SteamButton::RightStickTouch, "R-stick Touch"),
    (SteamButton::RightPadTouch, "R-pad Touch"),
    (SteamButton::RightPadClick, "R-pad Click"),
    (SteamButton::RightTriggerClick, "RT Click"),
    (SteamButton::LeftStickTouch, "L-stick Touch"),
    (SteamButton::LeftPadTouch, "L-pad Touch"),
    (SteamButton::LeftPadClick, "L-pad Click"),
    (SteamButton::LeftTriggerClick, "LT Click"),
    (SteamButton::RightGripTouch, "R-grip Touch"),
    (SteamButton::LeftGripTouch, "L-grip Touch"),
];

const GAMEPAD_BUTTONS: [(Button, &str); 16] = [
    (Button::South, "South"),
    (Button::East, "East"),
    (Button::West, "West"),
    (Button::North, "North"),
    (Button::LeftShoulder, "LB"),
    (Button::RightShoulder, "RB"),
    (Button::LeftStick, "L3"),
    (Button::RightStick, "R3"),
    (Button::Back, "Back"),
    (Button::Start, "Start"),
    (Button::Guide, "Guide"),
    (Button::LeftGrip, "Left Grip"),
    (Button::RightGrip, "Right Grip"),
    (Button::Extra1, "Extra 1"),
    (Button::Extra2, "Extra 2"),
    (Button::Extra3, "Extra 3"),
];
