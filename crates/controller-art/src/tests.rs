use std::collections::BTreeSet;

use eframe::egui;

use crate::geometry::{
    body_bounds, body_shape, control_rect, dpad_arm, dpad_shape, locus_point, shoulder_shapes,
    trackpad_shapes, view_for_available, BODY_HALF, DPAD_ARM, DPAD_CENTER, FACE_BUTTONS,
    FACE_BUTTON_OFFSET, FACE_BUTTON_RADIUS, GRIP_PADDLES, OPTION_BUTTONS, OPTION_SIZE,
    PUCK_CONNECTOR, PUCK_CONNECTOR_SIZE, QUICK_ACCESS, QUICK_ACCESS_SIZE, SHOULDERS, STEAM_BUTTON,
    STEAM_RADIUS, STICKS, STICK_RADIUS, TOP_SEAM, TOP_SEAM_WIDTH, TRACKPADS, TRACKPAD_SIZE,
    USB_PORT_SIZE,
};
use crate::shape::{cross, signed_area};
use crate::{Analog, Control, ControlState, Face, Highlight};

/// A view large enough that rounding never decides an assertion.
fn test_view() -> egui::Rect {
    view_for_available(egui::Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(1200.0, 900.0),
    ))
}

/// Ray casting, used to check that no artwork escapes the silhouette.
fn point_in_polygon(polygon: &[[f32; 2]], point: [f32; 2]) -> bool {
    let mut inside = false;
    for (index, corner) in polygon.iter().enumerate() {
        let next = polygon[(index + 1) % polygon.len()];
        let crosses = (corner[1] > point[1]) != (next[1] > point[1]);
        if crosses {
            let x =
                (next[0] - corner[0]) * (point[1] - corner[1]) / (next[1] - corner[1]) + corner[0];
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

// ---------------------------------------------------------------------------
// Geometry, moved from the bindings editor. These now run on every platform
// rather than only on macOS, because the editor is macOS-gated and this is not.
// ---------------------------------------------------------------------------

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
    // Ear clipping only reaches n - 2 triangles on a simple polygon, and their
    // areas only add up to the polygon's when they do not overlap.
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
    for control in [
        Control::DpadUp,
        Control::DpadDown,
        Control::DpadLeft,
        Control::DpadRight,
    ] {
        let (center, size) = dpad_arm(control);
        assert_inside_body(control.label(), center, size);
    }
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
    // The shoulders are deliberately not checked: in both references the
    // bumpers and trigger wings wrap over the shell's top edge rather than
    // sitting inside it. Their placement is covered by the test below.
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
    for (trigger, bumper) in [("R2", "R1"), ("L2", "L1")] {
        let (_, trigger_at, trigger_size, _) = shoulder(trigger);
        let (_, bumper_at, bumper_size, _) = shoulder(bumper);
        // In the reference the trigger is the deep wing wrapping the top
        // corner, with the bumper as a thin strip riding above it.
        assert!(
            trigger_at[1] > bumper_at[1],
            "the bumper should sit above the trigger"
        );
        assert!(
            trigger_size[1] > bumper_size[1] * 2.5,
            "the trigger should be far deeper than the bumper",
        );
        assert!(
            bumper_size[0] > bumper_size[1] * 3.0,
            "the bumper should read as a strip, not a block",
        );
    }
}

#[test]
fn the_silhouette_keeps_the_proportions_of_the_reference_drawing() {
    // Every figure here was measured off reference-front.jpg by flood filling
    // its background, so this fails if the outline drifts away from the
    // hardware rather than merely away from a previous drawing.
    let bounds = body_bounds();
    let aspect = bounds.width() / bounds.height();
    assert!(
        (aspect - 1.463).abs() < 0.03,
        "the shell is 1.463 times as wide as it is tall in the reference, got {aspect}",
    );

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
    let widest_at = (widest[1] - bounds.top()) / bounds.height();
    assert!(
        (widest_at - 0.733).abs() < 0.04,
        "the reference is widest 73% of the way down, this is widest at {widest_at}",
    );

    // The notch between the grips: the bottom edge is the run of points that
    // sits above the grip tips.
    let tip = BODY_HALF
        .iter()
        .copied()
        .reduce(|lowest, point| if point[1] > lowest[1] { point } else { lowest })
        .expect("the outline has points");
    let bottom: Vec<[f32; 2]> = BODY_HALF
        .iter()
        .copied()
        .filter(|[x, y]| *x > 0.28 && *y > 0.60 && *y < tip[1] - 0.05)
        .collect();
    assert!(bottom.len() >= 4, "the bottom edge needs several points");
    let notch_at = (bottom[0][1] - bounds.top()) / bounds.height();
    assert!(
        (notch_at - 0.819).abs() < 0.04,
        "the notch opens 82% down in the reference, here it opens at {notch_at}",
    );
    let notch_width = 2.0 * (0.5 - bottom[0][0]) / bounds.width();
    assert!(
        (notch_width - 0.447).abs() < 0.06,
        "the notch spans 45% of the width in the reference, here {notch_width}",
    );
    let (lowest, highest) = bottom.iter().fold((f32::MAX, f32::MIN), |range, [_, y]| {
        (range.0.min(*y), range.1.max(*y))
    });
    assert!(
        highest - lowest < 0.01,
        "the bottom edge is not straight: it spans {lowest}..{highest}",
    );

    // The grips hang below that bottom edge and taper to narrow tips.
    let hang = (tip[1] - bottom[0][1]) / bounds.height();
    assert!(
        (hang - 0.181).abs() < 0.04,
        "the grips hang 18% of the body below the bottom edge, here {hang}",
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

// ---------------------------------------------------------------------------
// New invariants for the shared API.
// ---------------------------------------------------------------------------

#[test]
fn every_control_is_listed_once_and_has_drawable_geometry() {
    let unique: BTreeSet<Control> = Control::ALL.into_iter().collect();
    assert_eq!(unique.len(), Control::ALL.len(), "ALL repeats a control");

    let view = test_view();
    for control in Control::ALL {
        let rect = control_rect(view, control);
        assert!(
            rect.is_positive() && rect.width() > 0.5 && rect.height() > 0.5,
            "{} has no drawable rect: {rect:?}",
            control.label(),
        );
        assert!(!control.label().is_empty());
    }
}

#[test]
fn the_dpad_arms_are_disjoint_and_sit_inside_the_cross() {
    let arms = [
        Control::DpadUp,
        Control::DpadDown,
        Control::DpadLeft,
        Control::DpadRight,
    ];
    let cross_points = &dpad_shape().points;
    for control in arms {
        let (center, size) = dpad_arm(control);
        // The arm's own corners lie within the cross it is drawn over.
        for corner in corners(center, [size[0] * 0.98, size[1] * 0.98]) {
            assert!(
                point_in_polygon(cross_points, corner),
                "{} reaches {corner:?}, outside the D-pad cross",
                control.label(),
            );
        }
    }

    // Adjacent arms meet at a corner point, which is the cross's own geometry;
    // what must not happen is a shared *area*.
    let view = test_view();
    for (index, first) in arms.iter().enumerate() {
        for second in &arms[index + 1..] {
            let overlap = control_rect(view, *first).intersect(control_rect(view, *second));
            let area = overlap.width().max(0.0) * overlap.height().max(0.0);
            assert!(
                area < 0.01,
                "{} and {} share {area} of area; the arms exclude the centre by design",
                first.label(),
                second.label(),
            );
        }
    }
}

#[test]
fn the_face_buttons_follow_the_xbox_arrangement() {
    let view = test_view();
    let at = |control| control_rect(view, control).center();
    // Screen y grows downward.
    assert!(at(Control::Y).y < at(Control::A).y, "Y sits above A");
    assert!(at(Control::X).x < at(Control::B).x, "X sits left of B");
    assert!(
        (at(Control::Y).x - at(Control::A).x).abs() < 1.0,
        "Y and A share a column"
    );
    assert!(
        (at(Control::X).y - at(Control::B).y).abs() < 1.0,
        "X and B share a row"
    );
}

/// The source-bit names are reversed against the physical buttons, so the one
/// thing that must never drift is which side each physical button is drawn on.
/// See `docs/MAPPING.md` and the `Control` docs.
#[test]
fn the_view_button_is_on_the_left_and_menu_on_the_right() {
    let view = test_view();
    let steam = control_rect(view, Control::Steam).center().x;
    assert!(
        control_rect(view, Control::View).center().x < steam,
        "the physical View/Back button is left of the Steam button"
    );
    assert!(
        control_rect(view, Control::Menu).center().x > steam,
        "the physical Menu/Start button is right of the Steam button"
    );
}

#[test]
fn the_rear_view_keeps_physical_handedness() {
    let view = test_view();
    let at = |control| control_rect(view, control).center().x;
    // Seen from behind, the physical right side appears on the image's left.
    assert!(at(Control::RightTrigger) < at(Control::LeftTrigger));
    assert!(at(Control::R4) < at(Control::L4));
    assert!(at(Control::R5) < at(Control::L5));
}

#[test]
fn each_bumper_rides_over_its_own_trigger_and_the_sides_stay_apart() {
    let view = test_view();
    let rect = |control| control_rect(view, control);
    for (trigger, bumper) in [
        (Control::RightTrigger, Control::RightBumper),
        (Control::LeftTrigger, Control::LeftBumper),
    ] {
        // Intersecting is the point: the bumper is a strip riding on the
        // trigger wing's upper edge, and it is painted afterwards.
        assert!(
            rect(trigger).intersect(rect(bumper)).is_positive(),
            "{} should overlap {}",
            bumper.label(),
            trigger.label(),
        );
    }
    let right = rect(Control::RightTrigger).union(rect(Control::RightBumper));
    let left = rect(Control::LeftTrigger).union(rect(Control::LeftBumper));
    assert!(
        !right.intersect(left).is_positive(),
        "the two shoulder groups must not touch each other"
    );
}

#[test]
fn faces_are_assigned_and_only_the_bumpers_are_drawn_on_both() {
    for control in Control::ALL {
        let expected = match control {
            Control::LeftTrigger
            | Control::RightTrigger
            | Control::LeftBumper
            | Control::RightBumper
            | Control::L4
            | Control::L5
            | Control::R4
            | Control::R5 => Face::Rear,
            _ => Face::Front,
        };
        assert_eq!(control.face(), expected, "{}", control.label());
    }
}

#[test]
fn a_neutral_locus_lands_on_the_control_centre() {
    let view = test_view();
    for control in [
        Control::LeftStick,
        Control::RightStick,
        Control::LeftPad,
        Control::RightPad,
    ] {
        let center = control_rect(view, control).center();
        let neutral = locus_point(view, control, [0.0, 0.0]);
        assert!(
            (neutral - center).length() < 0.5,
            "{} neutral locus is {neutral:?}, centre is {center:?}",
            control.label(),
        );
    }
}

#[test]
fn a_full_deflection_stays_inside_its_control() {
    let view = test_view();
    for control in [
        Control::LeftStick,
        Control::RightStick,
        Control::LeftPad,
        Control::RightPad,
    ] {
        let rect = control_rect(view, control);
        for offset in [[1.0, 0.0], [-1.0, 0.0], [0.0, 1.0], [0.0, -1.0]] {
            let point = locus_point(view, control, offset);
            assert!(
                rect.contains(point),
                "{} at {offset:?} lands {point:?}, outside {rect:?}",
                control.label(),
            );
        }
    }
}

#[test]
fn an_upward_deflection_draws_above_the_centre() {
    let view = test_view();
    for control in [Control::LeftStick, Control::LeftPad] {
        let center = locus_point(view, control, [0.0, 0.0]);
        let up = locus_point(view, control, [0.0, 1.0]);
        assert!(
            up.y < center.y,
            "{} should draw a positive Y above centre",
            control.label(),
        );
    }
}

#[test]
fn the_two_pads_cant_in_opposite_directions() {
    let view = test_view();
    // Pushing straight right on each pad tilts the dot the opposite way.
    let left = locus_point(view, Control::LeftPad, [1.0, 0.0])
        - locus_point(view, Control::LeftPad, [0.0, 0.0]);
    let right = locus_point(view, Control::RightPad, [1.0, 0.0])
        - locus_point(view, Control::RightPad, [0.0, 0.0]);
    assert!(
        left.y.signum() != right.y.signum(),
        "pad cants should mirror: left {left:?}, right {right:?}"
    );
}

#[test]
fn non_finite_analog_input_falls_back_to_neutral() {
    let view = test_view();
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let point = locus_point(view, Control::LeftStick, [bad, bad]);
        let center = locus_point(view, Control::LeftStick, [0.0, 0.0]);
        assert!(
            (point - center).length() < 0.5,
            "a {bad} locus should neutralize, got {point:?}"
        );
        assert!(point.x.is_finite() && point.y.is_finite());
    }
}

#[test]
fn out_of_range_analog_input_clamps_instead_of_escaping() {
    let view = test_view();
    let rect = control_rect(view, Control::RightStick);
    let point = locus_point(view, Control::RightStick, [12.0, -12.0]);
    assert!(rect.contains(point), "{point:?} escaped {rect:?}");
}

#[test]
fn analog_kinds_match_the_controls_that_have_sensors() {
    use crate::AnalogKind;
    for control in Control::ALL {
        let expected = match control {
            Control::LeftStick | Control::RightStick | Control::LeftPad | Control::RightPad => {
                Some(AnalogKind::Position)
            }
            Control::LeftTrigger | Control::RightTrigger => Some(AnalogKind::Trigger),
            _ => None,
        };
        assert_eq!(control.analog_kind(), expected, "{}", control.label());
    }
}

/// The whole point of the shared state mechanism: an active control must be
/// visibly different from an idle one, for every control, in both fill and
/// stroke. This is what would silently break "pressed".
#[test]
fn active_differs_from_idle_for_every_control() {
    let ctx = egui::Context::default();
    let render = |state: ControlState| {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let view = test_view();
            let painter = ui.painter_at(egui::Rect::EVERYTHING);
            let lookup = |control: Control| {
                if control == Control::A {
                    state
                } else {
                    ControlState::IDLE
                }
            };
            crate::draw_front(&painter, view, &lookup);
        })
        .shapes
        .iter()
        .fold(String::new(), |mut acc, clipped| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{:?}", clipped.shape);
            acc
        })
    };

    let idle = render(ControlState::IDLE);
    let active = render(ControlState::active());
    let hovered = render(ControlState::hovered());
    assert_ne!(idle, active, "an active control must not paint like idle");
    assert_ne!(idle, hovered, "a hovered control must not paint like idle");
    assert_ne!(active, hovered, "active and hover must be distinguishable");
}

#[test]
fn a_touched_pad_paints_differently_from_an_untouched_one() {
    let ctx = egui::Context::default();
    let render = |analog: Option<Analog>| {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let view = test_view();
            let painter = ui.painter_at(egui::Rect::EVERYTHING);
            let lookup = |control: Control| {
                if control == Control::LeftPad {
                    ControlState {
                        highlight: Highlight::Idle,
                        analog,
                    }
                } else {
                    ControlState::IDLE
                }
            };
            crate::draw_front(&painter, view, &lookup);
        })
        .shapes
        .len()
    };

    let untouched = render(None);
    let touched = render(Some(Analog::Position {
        offset: Some([0.4, -0.2]),
        touched: true,
    }));
    assert!(
        touched > untouched,
        "a touched pad should add its position dot: {touched} vs {untouched}"
    );
}

#[test]
fn a_pulled_trigger_paints_more_than_a_resting_one() {
    let ctx = egui::Context::default();
    let render = |travel: f32| {
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let view = test_view();
            let painter = ui.painter_at(egui::Rect::EVERYTHING);
            let lookup = |control: Control| {
                if control == Control::LeftTrigger {
                    ControlState {
                        highlight: Highlight::Idle,
                        analog: Some(Analog::Trigger { travel }),
                    }
                } else {
                    ControlState::IDLE
                }
            };
            crate::draw_rear(&painter, view, &lookup);
        })
        .shapes
        .len()
    };

    assert!(render(1.0) > render(0.0), "a full pull should add fill");
    assert!(render(0.5) > render(0.0), "a half pull should add fill");
}

#[test]
fn view_for_available_keeps_the_silhouette_aspect_and_fits() {
    let bounds = body_bounds();
    for size in [
        egui::vec2(1200.0, 900.0),
        egui::vec2(400.0, 900.0),
        egui::vec2(1200.0, 200.0),
        egui::vec2(281.0, 281.0),
    ] {
        let available = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), size);
        let view = view_for_available(available);
        let body = crate::body_rect(view);
        let aspect = body.width() / body.height();
        assert!(
            (aspect - bounds.width() / bounds.height()).abs() < 0.01,
            "aspect drifted to {aspect} at {size:?}",
        );
        assert!(
            body.width() <= size.x + 0.5 && body.height() <= size.y + 0.5,
            "{body:?} does not fit inside {available:?}",
        );
    }
}

/// `control_rect` writes its own coordinates rather than searching the paint
/// tables, so that it can stay total. This is what keeps the two in step.
#[test]
fn the_paint_tables_agree_with_control_rect() {
    use crate::geometry::{FACE_BUTTON_LAYOUT, OPTION_LAYOUT};
    let view = test_view();
    for (control, offset) in FACE_BUTTON_LAYOUT {
        let from_table = crate::normalized_point(
            view,
            [FACE_BUTTONS[0] + offset[0], FACE_BUTTONS[1] + offset[1]],
        );
        assert!(
            (from_table - control_rect(view, control).center()).length() < 0.5,
            "{} disagrees between FACE_BUTTON_LAYOUT and control_rect",
            control.label(),
        );
    }
    for (control, center) in OPTION_LAYOUT {
        let from_table = crate::normalized_point(view, center);
        assert!(
            (from_table - control_rect(view, control).center()).length() < 0.5,
            "{} disagrees between OPTION_LAYOUT and control_rect",
            control.label(),
        );
    }
}

#[test]
fn shoulder_shapes_stay_in_their_documented_index_order() {
    // Index order is load-bearing for `shoulder_index`.
    assert_eq!(SHOULDERS[0].0, "R2");
    assert_eq!(SHOULDERS[1].0, "R1");
    assert_eq!(SHOULDERS[2].0, "L2");
    assert_eq!(SHOULDERS[3].0, "L1");
    assert_eq!(shoulder_shapes().len(), 4);
}
