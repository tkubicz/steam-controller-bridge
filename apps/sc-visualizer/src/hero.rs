//! The live controller illustration.
//!
//! Turns a decoded `SteamControllerState` into the per-control states the art
//! crate paints, and lays the two faces out side by side or stacked.

use controller_art::{Analog, Control, ControlState, Face, Highlight};
use eframe::egui;
use steam_controller_protocol::{SteamButton, SteamButtons, SteamControllerState};
use ui_theme::{BORDER, MUTED_TEXT, SUNKEN};

use crate::Visualizer;

/// Below this width per face the two stack instead of sitting side by side.
const MIN_FACE_WIDTH: f32 = 280.0;
const FACE_GAP: f32 = 16.0;
const CAPTION_ROW: f32 = 18.0;

/// Which control a report bit lights, and through which sensor.
///
/// **The two option buttons are crossed on purpose.** `docs/MAPPING.md` records
/// that the source-bit names are reversed against the physical buttons:
/// `SteamButton::View` is the physical Menu/Start button on the right, and
/// `SteamButton::Menu` is the physical View/Back button on the left.
/// `crates/controller-mapper` encodes the same crossover (`View => Start`,
/// `Menu => Back`) and tests it in
/// `view_and_menu_follow_sdl_xbox_button_semantics`.
///
/// `None` means the bit has no drawn geometry. That is only ever the two
/// grip-shell capacitive sensors, which are the shell itself rather than the
/// L4/L5 paddles; they stay visible in the button grid.
pub(crate) const fn control_for_button(button: SteamButton) -> Option<(Control, Sensor)> {
    Some(match button {
        SteamButton::A => (Control::A, Sensor::Press),
        SteamButton::B => (Control::B, Sensor::Press),
        SteamButton::X => (Control::X, Sensor::Press),
        SteamButton::Y => (Control::Y, Sensor::Press),
        SteamButton::DpadUp => (Control::DpadUp, Sensor::Press),
        SteamButton::DpadDown => (Control::DpadDown, Sensor::Press),
        SteamButton::DpadLeft => (Control::DpadLeft, Sensor::Press),
        SteamButton::DpadRight => (Control::DpadRight, Sensor::Press),
        // Crossed. See the doc comment above.
        SteamButton::View => (Control::Menu, Sensor::Press),
        SteamButton::Menu => (Control::View, Sensor::Press),
        SteamButton::Steam => (Control::Steam, Sensor::Press),
        SteamButton::QuickAccess => (Control::QuickAccess, Sensor::Press),
        SteamButton::LeftShoulder => (Control::LeftBumper, Sensor::Press),
        SteamButton::RightShoulder => (Control::RightBumper, Sensor::Press),
        SteamButton::LeftTriggerClick => (Control::LeftTrigger, Sensor::Press),
        SteamButton::RightTriggerClick => (Control::RightTrigger, Sensor::Press),
        // Touch and click are separate sensors on the same control. Keeping
        // them apart is what makes a resting thumb look different from a
        // click, which it cannot if both simply light the control.
        SteamButton::LeftStickPress => (Control::LeftStick, Sensor::Press),
        SteamButton::LeftStickTouch => (Control::LeftStick, Sensor::Touch),
        SteamButton::RightStickPress => (Control::RightStick, Sensor::Press),
        SteamButton::RightStickTouch => (Control::RightStick, Sensor::Touch),
        SteamButton::LeftPadClick => (Control::LeftPad, Sensor::Press),
        SteamButton::LeftPadTouch => (Control::LeftPad, Sensor::Touch),
        SteamButton::RightPadClick => (Control::RightPad, Sensor::Press),
        SteamButton::RightPadTouch => (Control::RightPad, Sensor::Touch),
        SteamButton::LeftGrip4 => (Control::L4, Sensor::Press),
        SteamButton::LeftGrip5 => (Control::L5, Sensor::Press),
        SteamButton::RightGrip4 => (Control::R4, Sensor::Press),
        SteamButton::RightGrip5 => (Control::R5, Sensor::Press),
        // The grip shell, not a paddle: no geometry to light.
        SteamButton::LeftGripTouch | SteamButton::RightGripTouch => return None,
    })
}

/// Which sensor on a control a report bit represents.
///
/// A click always implies contact, so a control that only knew "active" would
/// render a rested thumb and a pressed stick identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sensor {
    /// A switch closed: the control is being pressed or clicked.
    Press,
    /// Capacitive contact only.
    Touch,
}

/// Normalizes a symmetric `i16` axis to `-1.0..=1.0`.
fn axis(value: i16) -> f32 {
    (f32::from(value) / 32767.0).clamp(-1.0, 1.0)
}

/// Trigger travel against the mapper's own calibration. Using `u16::MAX` here
/// would show a full physical pull as half travel, because a full pull tops out
/// near `trigger_full_scale` (0x8000 by default).
fn travel(raw: u16, full_scale: u16) -> f32 {
    if full_scale == 0 {
        return 0.0;
    }
    (f32::from(raw) / f32::from(full_scale)).clamp(0.0, 1.0)
}

/// What every control should look like, given one decoded report.
pub(crate) fn control_states(
    state: &SteamControllerState,
    held: SteamButtons,
    trigger_full_scale: u16,
) -> impl Fn(Control) -> ControlState {
    let mut pressed = [false; 24];
    for button in ALL_STEAM_BUTTONS {
        if !held.contains(button) {
            continue;
        }
        // Only a real press lights the control. Touch is reported separately so
        // that resting a thumb and clicking read differently.
        if let Some((control, Sensor::Press)) = control_for_button(button) {
            pressed[control as usize] = true;
        }
    }

    let left_stick = [axis(state.left_stick_x), axis(state.left_stick_y)];
    let right_stick = [axis(state.right_stick_x), axis(state.right_stick_y)];
    let left_pad = [axis(state.left_pad_x), axis(state.left_pad_y)];
    let right_pad = [axis(state.right_pad_x), axis(state.right_pad_y)];
    // Every touch comes from `held`, not from the mirrored struct fields. The
    // decoder sets `left_pad_touched` from the same bit, so the two agree — but
    // taking one source keeps sticks and pads consistent, and keeps working if
    // `held` is ever latched across reports to catch a sub-frame tap.
    let left_pad_touched = held.contains(SteamButton::LeftPadTouch);
    let right_pad_touched = held.contains(SteamButton::RightPadTouch);
    let left_stick_touched = held.contains(SteamButton::LeftStickTouch);
    let right_stick_touched = held.contains(SteamButton::RightStickTouch);
    let left_travel = travel(state.left_trigger, trigger_full_scale);
    let right_travel = travel(state.right_trigger, trigger_full_scale);

    move |control| {
        let highlight = if pressed[control as usize] {
            Highlight::Active
        } else {
            Highlight::Idle
        };
        let analog = match control {
            // A stick's deflection is always meaningful; its capacitive ring
            // is a separate sensor and only adds the halo.
            Control::LeftStick => Some(Analog::Position {
                offset: Some(left_stick),
                touched: left_stick_touched,
            }),
            Control::RightStick => Some(Analog::Position {
                offset: Some(right_stick),
                touched: right_stick_touched,
            }),
            // A pad's coordinates are stale while untouched, so its dot
            // disappears with the finger.
            Control::LeftPad => Some(Analog::Position {
                offset: left_pad_touched.then_some(left_pad),
                touched: left_pad_touched,
            }),
            Control::RightPad => Some(Analog::Position {
                offset: right_pad_touched.then_some(right_pad),
                touched: right_pad_touched,
            }),
            Control::LeftTrigger => Some(Analog::Trigger {
                travel: left_travel,
            }),
            Control::RightTrigger => Some(Analog::Trigger {
                travel: right_travel,
            }),
            _ => None,
        };
        ControlState { highlight, analog }
    }
}

/// Every `SteamButton`, in bit order. The protocol crate has no `ALL`.
pub(crate) const ALL_STEAM_BUTTONS: [SteamButton; 30] = [
    SteamButton::A,
    SteamButton::B,
    SteamButton::X,
    SteamButton::Y,
    SteamButton::QuickAccess,
    SteamButton::RightStickPress,
    SteamButton::View,
    SteamButton::RightGrip4,
    SteamButton::RightGrip5,
    SteamButton::RightShoulder,
    SteamButton::DpadDown,
    SteamButton::DpadRight,
    SteamButton::DpadLeft,
    SteamButton::DpadUp,
    SteamButton::Menu,
    SteamButton::LeftStickPress,
    SteamButton::Steam,
    SteamButton::LeftGrip4,
    SteamButton::LeftGrip5,
    SteamButton::LeftShoulder,
    SteamButton::RightStickTouch,
    SteamButton::RightPadTouch,
    SteamButton::RightPadClick,
    SteamButton::RightTriggerClick,
    SteamButton::LeftStickTouch,
    SteamButton::LeftPadTouch,
    SteamButton::LeftPadClick,
    SteamButton::LeftTriggerClick,
    SteamButton::RightGripTouch,
    SteamButton::LeftGripTouch,
];

impl Visualizer {
    pub(crate) fn hero(&self, ui: &mut egui::Ui) {
        let available = ui.available_width();
        let stacked = (available - FACE_GAP) * 0.5 < MIN_FACE_WIDTH;
        let face_width = if stacked {
            available
        } else {
            (available - FACE_GAP) * 0.5
        };
        // Bounded, so the readouts below are never pushed off the screen.
        let face_height = (face_width * 0.62).min(300.0);
        let total_height = if stacked {
            (face_height + CAPTION_ROW) * 2.0 + FACE_GAP
        } else {
            face_height + CAPTION_ROW
        };

        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(available, total_height), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 12.0, SUNKEN);
        painter.rect_stroke(
            rect,
            12.0,
            egui::Stroke::new(1.0, BORDER),
            egui::StrokeKind::Inside,
        );

        let paint = self.live_states();
        for (index, (face, caption)) in [(Face::Front, "FRONT"), (Face::Rear, "REAR")]
            .into_iter()
            .enumerate()
        {
            #[allow(clippy::cast_precision_loss)]
            let slot = if stacked {
                egui::Rect::from_min_size(
                    egui::pos2(
                        rect.left(),
                        rect.top() + index as f32 * (face_height + CAPTION_ROW + FACE_GAP),
                    ),
                    egui::vec2(available, face_height + CAPTION_ROW),
                )
            } else {
                egui::Rect::from_min_size(
                    egui::pos2(
                        rect.left() + index as f32 * (face_width + FACE_GAP),
                        rect.top(),
                    ),
                    egui::vec2(face_width, face_height + CAPTION_ROW),
                )
            };

            painter.text(
                egui::pos2(slot.center().x, slot.top() + 3.0),
                egui::Align2::CENTER_TOP,
                caption,
                egui::FontId::proportional(11.0),
                MUTED_TEXT,
            );

            let drawing = egui::Rect::from_min_max(
                egui::pos2(slot.left() + 8.0, slot.top() + CAPTION_ROW),
                egui::pos2(slot.right() - 8.0, slot.bottom() - 4.0),
            );
            if drawing.width() < 8.0 || drawing.height() < 8.0 {
                continue;
            }
            let view = controller_art::view_for_available(drawing);
            controller_art::draw_body(&painter, view);
            match face {
                Face::Front => controller_art::draw_front(&painter, view, &paint),
                Face::Rear => controller_art::draw_rear(&painter, view, &paint),
            }
        }
    }

    /// The per-control states for the current frame. Neutral when nothing has
    /// been decoded yet, so the artwork always draws.
    fn live_states(&self) -> Box<dyn Fn(Control) -> ControlState + '_> {
        match &self.source {
            Some(state) => Box::new(control_states(
                state,
                state.buttons,
                self.config.trigger_full_scale,
            )),
            None => Box::new(|_| ControlState::IDLE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{control_for_button, control_states, travel, Sensor, ALL_STEAM_BUTTONS};
    use controller_art::{Analog, Control, ControlState, Highlight};
    use std::collections::BTreeSet;
    use steam_controller_protocol::{SteamButton, SteamButtons, SteamControllerState};

    fn neutral() -> SteamControllerState {
        SteamControllerState {
            report_id: 0x45,
            sequence: 0,
            buttons: SteamButtons(0),
            left_trigger: 0,
            right_trigger: 0,
            left_stick_x: 0,
            left_stick_y: 0,
            right_stick_x: 0,
            right_stick_y: 0,
            left_pad_x: 0,
            left_pad_y: 0,
            left_pad_pressure: 0,
            left_pad_touched: false,
            left_pad_pressed: false,
            right_pad_x: 0,
            right_pad_y: 0,
            right_pad_pressure: 0,
            right_pad_touched: false,
            right_pad_pressed: false,
            left_grip_touched: false,
            right_grip_touched: false,
            imu_timestamp: 0,
            gyro: None,
            acceleration: None,
            raw_report: Vec::new(),
        }
    }

    #[test]
    fn the_button_table_lists_all_thirty_bits_once() {
        let unique: BTreeSet<u8> = ALL_STEAM_BUTTONS.iter().map(|b| *b as u8).collect();
        assert_eq!(unique.len(), 30, "every bit appears exactly once");
        assert_eq!(*unique.iter().next().unwrap(), 0);
        assert_eq!(*unique.iter().next_back().unwrap(), 29);
    }

    /// Only the two grip-shell sensors may be undrawable.
    #[test]
    fn every_button_maps_to_a_control_except_the_grip_shell() {
        let unmapped: Vec<SteamButton> = ALL_STEAM_BUTTONS
            .into_iter()
            .filter(|button| control_for_button(*button).is_none())
            .collect();
        assert_eq!(
            unmapped,
            vec![SteamButton::RightGripTouch, SteamButton::LeftGripTouch]
        );
    }

    /// The trap: the source-bit names are reversed against the physical
    /// buttons. See `docs/MAPPING.md`.
    #[test]
    fn the_option_buttons_are_crossed_over() {
        assert_eq!(
            control_for_button(SteamButton::Menu),
            Some((Control::View, Sensor::Press))
        );
        assert_eq!(
            control_for_button(SteamButton::View),
            Some((Control::Menu, Sensor::Press))
        );
    }

    #[test]
    fn a_neutral_state_lights_nothing() {
        let state = neutral();
        let paint = control_states(&state, state.buttons, 0x8000);
        for control in Control::ALL {
            assert_eq!(
                paint(control).highlight,
                Highlight::Idle,
                "{} should be idle",
                control.label()
            );
        }
    }

    #[test]
    fn each_press_lights_exactly_the_control_it_maps_to() {
        for button in ALL_STEAM_BUTTONS {
            let Some((expected, Sensor::Press)) = control_for_button(button) else {
                continue;
            };
            let state = neutral();
            let held = SteamButtons(1 << (button as u8));
            let paint = control_states(&state, held, 0x8000);
            for control in Control::ALL {
                let lit = paint(control).highlight == Highlight::Active;
                assert_eq!(
                    lit,
                    control == expected,
                    "{:?} lit {} unexpectedly",
                    button,
                    control.label()
                );
            }
        }
    }

    /// Clicking a stick necessarily touches it, so if touch also lit the
    /// control the two would be indistinguishable on the illustration.
    #[test]
    fn a_touch_does_not_light_the_control_the_way_a_press_does() {
        for (touch, press, control) in [
            (
                SteamButton::LeftStickTouch,
                SteamButton::LeftStickPress,
                Control::LeftStick,
            ),
            (
                SteamButton::RightStickTouch,
                SteamButton::RightStickPress,
                Control::RightStick,
            ),
            (
                SteamButton::LeftPadTouch,
                SteamButton::LeftPadClick,
                Control::LeftPad,
            ),
            (
                SteamButton::RightPadTouch,
                SteamButton::RightPadClick,
                Control::RightPad,
            ),
        ] {
            let state = neutral();

            let touched = control_states(&state, SteamButtons(1 << (touch as u8)), 0x8000);
            assert_eq!(
                touched(control).highlight,
                Highlight::Idle,
                "{} should not read as pressed when only touched",
                control.label()
            );
            assert!(
                matches!(
                    touched(control).analog,
                    Some(Analog::Position { touched: true, .. })
                ),
                "{} should report contact",
                control.label()
            );

            let clicked = control_states(&state, SteamButtons(1 << (press as u8)), 0x8000);
            assert_eq!(
                clicked(control).highlight,
                Highlight::Active,
                "{} should read as pressed when clicked",
                control.label()
            );

            // And the real hardware case: a click arrives with its touch bit.
            let both = control_states(
                &state,
                SteamButtons((1 << (press as u8)) | (1 << (touch as u8))),
                0x8000,
            );
            assert_eq!(both(control).highlight, Highlight::Active);
            assert!(matches!(
                both(control).analog,
                Some(Analog::Position { touched: true, .. })
            ));
        }
    }

    #[test]
    fn a_full_pull_reaches_exactly_full_travel() {
        assert!((travel(0x8000, 0x8000) - 1.0).abs() < f32::EPSILON);
        assert!((travel(0x4000, 0x8000) - 0.5).abs() < f32::EPSILON);
        assert!(travel(0, 0x8000).abs() < f32::EPSILON);
        // Beyond calibration clamps rather than overflowing the fill.
        assert!((travel(u16::MAX, 0x8000) - 1.0).abs() < f32::EPSILON);
        // A zero calibration must not divide by zero.
        assert!(travel(1234, 0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_pad_dot_only_appears_under_a_finger() {
        let mut state = neutral();
        state.left_pad_x = 16_384;
        state.left_pad_y = -16_384;

        let untouched = control_states(&state, state.buttons, 0x8000);
        assert!(
            matches!(
                untouched(Control::LeftPad).analog,
                Some(Analog::Position {
                    offset: None,
                    touched: false
                })
            ),
            "a stale pad coordinate must not draw a dot"
        );

        // The decoder sets both the bit and the mirrored field; do the same.
        state.left_pad_touched = true;
        state.buttons = SteamButtons(1 << (SteamButton::LeftPadTouch as u8));
        let touched = control_states(&state, state.buttons, 0x8000);
        match touched(Control::LeftPad).analog {
            Some(Analog::Position {
                offset: Some(offset),
                touched,
            }) => {
                assert!(touched);
                assert!(offset[0] > 0.0, "positive x reads right");
                assert!(offset[1] < 0.0, "negative y reads down");
            }
            _ => panic!("a touched pad reports a position"),
        }
    }

    #[test]
    fn stick_deflection_is_reported_whether_or_not_it_is_touched() {
        let mut state = neutral();
        state.left_stick_x = -32_767;
        let paint = control_states(&state, state.buttons, 0x8000);
        match paint(Control::LeftStick).analog {
            Some(Analog::Position {
                offset: Some(offset),
                touched,
            }) => {
                assert!(!touched, "the capacitive ring is a separate sensor");
                assert!((offset[0] + 1.0).abs() < 0.01, "full left deflection");
            }
            _ => panic!("a stick always reports its position"),
        }
    }

    #[test]
    fn triggers_carry_travel_and_nothing_else_does() {
        let mut state = neutral();
        state.right_trigger = 0x8000;
        let paint = control_states(&state, state.buttons, 0x8000);
        assert!(matches!(
            paint(Control::RightTrigger).analog,
            Some(Analog::Trigger { travel }) if (travel - 1.0).abs() < f32::EPSILON
        ));
        assert_eq!(paint(Control::A), ControlState::IDLE);
    }
}
