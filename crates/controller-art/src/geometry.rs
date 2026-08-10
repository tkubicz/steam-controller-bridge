//! The traced artwork, in unit-square coordinates, and the transforms onto it.
//!
//! Every coordinate below lives in the unit square. A `view: egui::Rect` is a
//! square screen rect standing for that unit square, so it is the only
//! transform: there is no matrix and no scale field.

use std::sync::OnceLock;

use eframe::egui;

use crate::shape::UnitShape;
use crate::{Control, PadSide};

/// Left half of the silhouette, from the top centre down to the bottom centre.
/// The right half is mirrored from it, which keeps the body exactly symmetric.
///
/// Traced pixel by pixel from `reference-front.jpg`: the drawing's background
/// was flood filled from the border, and this is the boundary of what the fill
/// could not reach, resampled along the path. The gap in the top edge is where
/// the bumpers cover the shell.
///
/// Key proportions, for anyone checking against the reference: the shell is
/// 1.46 times as wide as it is tall, at its widest 73% of the way down, and the
/// notch between the grips opens 82% down and spans 45% of the width.
pub(crate) const BODY_HALF: [[f32; 2]; 43] = [
    [0.500, 0.200],
    [0.472, 0.200],
    [0.443, 0.200],
    [0.415, 0.201],
    [0.386, 0.202],
    [0.358, 0.202],
    [0.330, 0.204],
    [0.159, 0.237],
    [0.131, 0.283],
    [0.121, 0.309],
    [0.114, 0.336],
    [0.106, 0.362],
    [0.099, 0.389],
    [0.091, 0.414],
    [0.084, 0.441],
    [0.077, 0.467],
    [0.071, 0.493],
    [0.066, 0.520],
    [0.061, 0.546],
    [0.057, 0.573],
    [0.053, 0.598],
    [0.051, 0.625],
    [0.050, 0.651],
    [0.051, 0.678],
    [0.053, 0.704],
    [0.058, 0.726],
    [0.067, 0.749],
    [0.080, 0.771],
    [0.101, 0.793],
    [0.158, 0.815],
    [0.166, 0.815],
    [0.175, 0.815],
    [0.183, 0.815],
    [0.218, 0.793],
    [0.232, 0.771],
    [0.243, 0.749],
    [0.259, 0.726],
    [0.299, 0.706],
    [0.339, 0.705],
    [0.379, 0.704],
    [0.420, 0.704],
    [0.460, 0.703],
    [0.500, 0.703],
];

// Front-face controls, all measured from `reference-front.jpg` by finding the
// regions its outlines enclose.
pub(crate) const TRACKPADS: [[f32; 2]; 2] = [[0.329, 0.567], [0.671, 0.567]];
pub(crate) const TRACKPAD_SIZE: [f32; 2] = [0.180, 0.180];
pub(crate) const TRACKPAD_CORNER: f32 = 0.031;
/// The pads are canted to follow the arc a thumb sweeps through.
pub(crate) const TRACKPAD_TILT: f32 = 0.171;
pub(crate) const STICKS: [[f32; 2]; 2] = [[0.370, 0.399], [0.630, 0.399]];
pub(crate) const STICK_RADIUS: f32 = 0.066;
pub(crate) const DPAD_CENTER: [f32; 2] = [0.225, 0.334];
pub(crate) const DPAD_ARM: f32 = 0.064;
pub(crate) const DPAD_THICKNESS: f32 = 0.021;
pub(crate) const FACE_BUTTONS: [f32; 2] = [0.776, 0.334];
pub(crate) const FACE_BUTTON_OFFSET: f32 = 0.052;
pub(crate) const FACE_BUTTON_RADIUS: f32 = 0.024;
pub(crate) const OPTION_BUTTONS: [[f32; 2]; 2] = [[0.341, 0.271], [0.659, 0.271]];
pub(crate) const OPTION_SIZE: [f32; 2] = [0.052, 0.020];
pub(crate) const STEAM_BUTTON: [f32; 2] = [0.500, 0.334];
pub(crate) const STEAM_RADIUS: f32 = 0.025;
pub(crate) const QUICK_ACCESS: [f32; 2] = [0.500, 0.569];
pub(crate) const QUICK_ACCESS_SIZE: [f32; 2] = [0.066, 0.022];
/// The bumpers ride on top of the shell at the corners, so they deliberately
/// break the outline just as they do in the reference.
pub(crate) const BUMPERS: [[f32; 2]; 2] = [[0.234, 0.213], [0.766, 0.213]];
pub(crate) const BUMPER_SIZE: [f32; 2] = [0.132, 0.043];

/// Rear shoulder controls as (name, centre, size, rotation), measured from
/// `reference-back.jpeg`. Seen from behind each side is one big trigger wing
/// wrapping the top corner, with the bumper riding along its upper edge.
///
/// Index order is load-bearing: 0 = R2, 1 = R1, 2 = L2, 3 = L1. Seen from
/// behind, the physical right side appears on the image's left, which is why
/// R2/R1 sit at x ≈ 0.23.
pub(crate) const SHOULDERS: [(&str, [f32; 2], [f32; 2], f32); 4] = [
    ("R2", [0.232, 0.262], [0.147, 0.116], -0.240),
    ("R1", [0.234, 0.207], [0.132, 0.040], -0.140),
    ("L2", [0.768, 0.262], [0.147, 0.116], 0.240),
    ("L1", [0.766, 0.207], [0.132, 0.040], 0.140),
];
pub(crate) const SHOULDER_CORNER: f32 = 0.030;
/// The shell seam across the top, broken by the USB-C port at its centre.
pub(crate) const TOP_SEAM: [f32; 2] = [0.500, 0.222];
pub(crate) const TOP_SEAM_WIDTH: f32 = 0.300;
pub(crate) const USB_PORT_SIZE: [f32; 2] = [0.045, 0.012];
pub(crate) const PUCK_CONNECTOR: [f32; 2] = [0.500, 0.291];
pub(crate) const PUCK_CONNECTOR_SIZE: [f32; 2] = [0.104, 0.027];

/// Rear grip paddles as (control, centre, size), measured from
/// `reference-back.jpeg`. The lower pair sits further out than the upper pair,
/// following the grips as they splay.
pub(crate) const GRIP_PADDLES: [(Control, [f32; 2], [f32; 2]); 4] = [
    (Control::R4, [0.266, 0.533], [0.065, 0.093]),
    (Control::R5, [0.226, 0.655], [0.052, 0.090]),
    (Control::L4, [0.734, 0.533], [0.065, 0.093]),
    (Control::L5, [0.774, 0.655], [0.052, 0.090]),
];

/// The four face buttons, in the order the original artwork emitted them:
/// up, left, right, down. The Xbox convention puts Y on top and A at the
/// bottom, with X on the left and B on the right.
///
/// The reference images this was traced from are not in the repository, so
/// this assignment cannot be confirmed from source. The letters are drawn on
/// the buttons so a single press falsifies it.
pub(crate) const FACE_BUTTON_LAYOUT: [(Control, [f32; 2]); 4] = [
    (Control::Y, [0.0, -FACE_BUTTON_OFFSET]),
    (Control::X, [-FACE_BUTTON_OFFSET, 0.0]),
    (Control::B, [FACE_BUTTON_OFFSET, 0.0]),
    (Control::A, [0.0, FACE_BUTTON_OFFSET]),
];

/// The two small buttons either side of the Steam button.
///
/// `OPTION_BUTTONS[0]` is on the image's left and `[1]` on its right. The
/// source-bit names are reversed against the physical buttons — see the
/// `Control` docs and `docs/MAPPING.md` — so this table is keyed by the
/// physical button, not by the report bit.
pub(crate) const OPTION_LAYOUT: [(Control, [f32; 2]); 2] = [
    (Control::View, OPTION_BUTTONS[0]),
    (Control::Menu, OPTION_BUTTONS[1]),
];

pub(crate) fn body_shape() -> &'static UnitShape {
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
#[must_use]
pub fn body_bounds() -> egui::Rect {
    static BOUNDS: OnceLock<egui::Rect> = OnceLock::new();
    *BOUNDS.get_or_init(|| body_shape().bounds())
}

/// The tilted trackpads, plus the inset ring drawn inside each of them.
pub(crate) fn trackpad_shapes() -> &'static [(UnitShape, UnitShape); 2] {
    static PADS: OnceLock<[(UnitShape, UnitShape); 2]> = OnceLock::new();
    PADS.get_or_init(|| {
        let pad = |index: usize| {
            let tilt = pad_tilt(index);
            let inset = [TRACKPAD_SIZE[0] - 0.028, TRACKPAD_SIZE[1] - 0.028];
            (
                UnitShape::rounded_rect(TRACKPADS[index], TRACKPAD_SIZE, TRACKPAD_CORNER, tilt),
                UnitShape::rounded_rect(TRACKPADS[index], inset, TRACKPAD_CORNER * 0.75, tilt),
            )
        };
        [pad(0), pad(1)]
    })
}

/// The same pad geometry, centered in a standalone unit square for focused
/// instruction and diagnostic views outside the full controller drawing.
pub(crate) fn trackpad_surface_shapes() -> &'static [(UnitShape, UnitShape); 2] {
    static PADS: OnceLock<[(UnitShape, UnitShape); 2]> = OnceLock::new();
    PADS.get_or_init(|| {
        let pad = |index: usize| {
            let corner = TRACKPAD_CORNER / TRACKPAD_SIZE[0];
            let inset_size = 1.0 - 0.028 / TRACKPAD_SIZE[0];
            (
                UnitShape::rounded_rect([0.5, 0.5], [1.0, 1.0], corner, pad_tilt(index)),
                UnitShape::rounded_rect(
                    [0.5, 0.5],
                    [inset_size, inset_size],
                    corner * 0.75,
                    pad_tilt(index),
                ),
            )
        };
        [pad(0), pad(1)]
    })
}

/// The cant applied to one pad. The two pads mirror each other.
pub(crate) fn pad_tilt(index: usize) -> f32 {
    if index == 0 {
        TRACKPAD_TILT
    } else {
        -TRACKPAD_TILT
    }
}

pub(crate) fn shoulder_shapes() -> &'static [UnitShape; 4] {
    static SHOULDER: OnceLock<[UnitShape; 4]> = OnceLock::new();
    SHOULDER.get_or_init(|| {
        SHOULDERS.map(|(_, center, size, rotation)| {
            UnitShape::rounded_rect(center, size, SHOULDER_CORNER, rotation)
        })
    })
}

pub(crate) fn dpad_shape() -> &'static UnitShape {
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

/// One D-pad arm in unit space, as (centre, size).
///
/// The arms run from the outer edge of the centre square to the tip, so the
/// four are disjoint: the shared centre square belongs to none of them and is
/// never lit on its own.
pub(crate) fn dpad_arm(control: Control) -> ([f32; 2], [f32; 2]) {
    let (arm, thick) = (DPAD_ARM, DPAD_THICKNESS);
    let length = arm - thick;
    let offset = (arm + thick) * 0.5;
    match control {
        Control::DpadUp => (
            [DPAD_CENTER[0], DPAD_CENTER[1] - offset],
            [thick * 2.0, length],
        ),
        Control::DpadDown => (
            [DPAD_CENTER[0], DPAD_CENTER[1] + offset],
            [thick * 2.0, length],
        ),
        Control::DpadLeft => (
            [DPAD_CENTER[0] - offset, DPAD_CENTER[1]],
            [length, thick * 2.0],
        ),
        _ => (
            [DPAD_CENTER[0] + offset, DPAD_CENTER[1]],
            [length, thick * 2.0],
        ),
    }
}

/// The shoulder shape index for a rear control, if it has one.
pub(crate) const fn shoulder_index(control: Control) -> Option<usize> {
    match control {
        Control::RightTrigger => Some(0),
        Control::RightBumper => Some(1),
        Control::LeftTrigger => Some(2),
        Control::LeftBumper => Some(3),
        _ => None,
    }
}

#[must_use]
pub(crate) fn normalized_point_impl(rect: egui::Rect, point: [f32; 2]) -> egui::Pos2 {
    egui::pos2(
        egui::lerp(rect.x_range(), point[0]),
        egui::lerp(rect.y_range(), point[1]),
    )
}

/// Maps a point in unit-square coordinates onto the screen.
#[must_use]
pub fn normalized_point(rect: egui::Rect, point: [f32; 2]) -> egui::Pos2 {
    normalized_point_impl(rect, point)
}

/// A centred rect in unit-square coordinates, mapped onto the screen.
///
/// Built from the centre rather than from two mapped corners. The two are
/// mathematically equal, but they round differently, and this form is what the
/// artwork was drawn against.
#[must_use]
pub fn unit_rect(view: egui::Rect, center: [f32; 2], size: [f32; 2]) -> egui::Rect {
    egui::Rect::from_center_size(
        normalized_point(view, center),
        egui::vec2(size[0] * view.width(), size[1] * view.height()),
    )
}

/// The unit-square view whose silhouette lands exactly on `body`.
///
/// Debug builds assert that `body` already has the silhouette's aspect ratio;
/// a mismatch means the caller sized the rect without going through
/// [`view_for_available`].
#[must_use]
pub fn view_for_body(body: egui::Rect) -> egui::Rect {
    let bounds = body_bounds();
    debug_assert!(
        (body.width() / body.height() - bounds.width() / bounds.height()).abs() < 0.01,
        "body rect must carry the silhouette's aspect ratio"
    );
    let side = body.width() / bounds.width();
    egui::Rect::from_min_size(
        egui::pos2(
            bounds.left().mul_add(-side, body.left()),
            bounds.top().mul_add(-side, body.top()),
        ),
        egui::vec2(side, side),
    )
}

/// Aspect-fits the silhouette inside `available`, centred, and returns the
/// unit-square view the drawing functions take.
#[must_use]
pub fn view_for_available(available: egui::Rect) -> egui::Rect {
    let bounds = body_bounds();
    let side = (available.width() / bounds.width()).min(available.height() / bounds.height());
    let body = egui::Rect::from_center_size(
        available.center(),
        egui::vec2(side * bounds.width(), side * bounds.height()),
    );
    view_for_body(body)
}

/// The silhouette's bounding box on screen, which is what captions and labels
/// align to.
#[must_use]
pub fn body_rect(view: egui::Rect) -> egui::Rect {
    let bounds = body_bounds();
    egui::Rect::from_min_max(
        normalized_point(view, [bounds.left(), bounds.top()]),
        normalized_point(view, [bounds.right(), bounds.bottom()]),
    )
}

/// Where a control is drawn, so the artwork, the hit area and any callout
/// leader can never drift apart.
///
/// `view` must be the view for the control's own [`Control::face`], except for
/// the bumpers, which are drawn on both.
#[must_use]
pub fn control_rect(view: egui::Rect, control: Control) -> egui::Rect {
    let scale = view.width();
    let circle = |center: [f32; 2], radius: f32| {
        egui::Rect::from_center_size(
            normalized_point(view, center),
            egui::Vec2::splat(radius * 2.0 * scale),
        )
    };

    let face = |offset: [f32; 2]| {
        circle(
            [FACE_BUTTONS[0] + offset[0], FACE_BUTTONS[1] + offset[1]],
            FACE_BUTTON_RADIUS,
        )
    };
    let wing = |index: usize| {
        let bounds = shoulder_shapes()[index].bounds();
        egui::Rect::from_min_max(
            normalized_point(view, [bounds.left(), bounds.top()]),
            normalized_point(view, [bounds.right(), bounds.bottom()]),
        )
    };
    let paddle = |index: usize| {
        let (_, center, size) = GRIP_PADDLES[index];
        unit_rect(view, center, size)
    };

    // Exhaustive on purpose: a new `Control` should fail to compile here rather
    // than fall through to some default rect. Total, so no `# Panics`.
    match control {
        Control::Y => face([0.0, -FACE_BUTTON_OFFSET]),
        Control::X => face([-FACE_BUTTON_OFFSET, 0.0]),
        Control::B => face([FACE_BUTTON_OFFSET, 0.0]),
        Control::A => face([0.0, FACE_BUTTON_OFFSET]),
        Control::View => unit_rect(view, OPTION_BUTTONS[0], OPTION_SIZE),
        Control::Menu => unit_rect(view, OPTION_BUTTONS[1], OPTION_SIZE),
        Control::R4 => paddle(0),
        Control::R5 => paddle(1),
        Control::L4 => paddle(2),
        Control::L5 => paddle(3),
        Control::DpadUp | Control::DpadDown | Control::DpadLeft | Control::DpadRight => {
            let (center, size) = dpad_arm(control);
            unit_rect(view, center, size)
        }
        Control::Steam => circle(STEAM_BUTTON, STEAM_RADIUS),
        Control::QuickAccess => unit_rect(view, QUICK_ACCESS, QUICK_ACCESS_SIZE),
        Control::LeftStick => circle(STICKS[0], STICK_RADIUS),
        Control::RightStick => circle(STICKS[1], STICK_RADIUS),
        Control::LeftPad => pad_rect(view, 0),
        Control::RightPad => pad_rect(view, 1),
        // Bumpers report their rear wing, matching `Control::face`. The front
        // cap is a second drawing of the same control and is painted from
        // `BUMPERS` directly; it is not addressable here, because a single
        // `view` cannot mean both faces at once.
        Control::RightBumper => wing(1),
        Control::LeftBumper => wing(3),
        Control::RightTrigger => wing(0),
        Control::LeftTrigger => wing(2),
    }
}

/// The screen point for a position inside a control's own `-1..=1` space,
/// with `y` pointing up. Honours a trackpad's cant and a stick's round well.
#[must_use]
pub fn locus_point(view: egui::Rect, control: Control, locus: [f32; 2]) -> egui::Pos2 {
    let clamp = |value: f32| {
        if value.is_finite() {
            value.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    };
    let (x, y) = (clamp(locus[0]), clamp(locus[1]));
    match control {
        Control::LeftStick | Control::RightStick => {
            let center = STICKS[usize::from(control == Control::RightStick)];
            // A stick well is round, so the magnitude has to be capped, not
            // each axis: clamping x and y independently still lets a diagonal
            // out to sqrt(2) and puts the dot outside the ring. Capping along
            // the direction is what the mapper's radial dead zone does too.
            let (x, y) = clamp_to_unit_circle(x, y);
            // Screen y grows downward, so an upward deflection subtracts.
            let reach = STICK_RADIUS - DOT_RADIUS;
            normalized_point(
                view,
                [x.mul_add(reach, center[0]), (-y).mul_add(reach, center[1])],
            )
        }
        Control::LeftPad | Control::RightPad => {
            let index = usize::from(control == Control::RightPad);
            let (sin, cos) = pad_tilt(index).sin_cos();
            let reach = [
                TRACKPAD_SIZE[0] * 0.5 - DOT_RADIUS,
                TRACKPAD_SIZE[1] * 0.5 - DOT_RADIUS,
            ];
            let local = [x * reach[0], -y * reach[1]];
            normalized_point(
                view,
                [
                    local[0].mul_add(cos, TRACKPADS[index][0]) - local[1] * sin,
                    local[0].mul_add(sin, TRACKPADS[index][1]) + local[1] * cos,
                ],
            )
        }
        _ => normalized_point(view, unit_center(control)),
    }
}

/// Caps a position at the unit circle without turning it.
///
/// A trackpad is square, so per-axis clamping is right for it. A stick well is
/// round, and there the direction has to be preserved while the magnitude is
/// capped — otherwise a full diagonal lands `sqrt(2)` out and draws outside the
/// well it belongs to.
#[must_use]
pub fn clamp_to_unit_circle(x: f32, y: f32) -> (f32, f32) {
    let (x, y) = (
        if x.is_finite() { x } else { 0.0 },
        if y.is_finite() { y } else { 0.0 },
    );
    let magnitude = x.hypot(y);
    if magnitude <= 1.0 || magnitude == 0.0 {
        return (x, y);
    }
    (x / magnitude, y / magnitude)
}

/// Radius of the live position dot, in unit-square units.
pub(crate) const DOT_RADIUS: f32 = 0.012;

/// A control's centre in unit-square coordinates.
pub(crate) fn unit_center(control: Control) -> [f32; 2] {
    match control {
        Control::LeftStick => STICKS[0],
        Control::RightStick => STICKS[1],
        Control::LeftPad => TRACKPADS[0],
        Control::RightPad => TRACKPADS[1],
        Control::Steam => STEAM_BUTTON,
        _ => QUICK_ACCESS,
    }
}

/// The screen rect of one trackpad, for hit testing.
///
/// This is the bounding box of the *tilted* polygon, not of the untilted
/// square, so it covers everything actually drawn.
#[must_use]
pub fn trackpad_rect(view: egui::Rect, side: PadSide) -> egui::Rect {
    pad_rect(view, side.index())
}

/// Maps a pad-local position onto a standalone tilted pad surface.
///
/// `locus` uses the controller convention: both axes span `-1..=1`, with
/// positive Y pointing upward. `surface` is the pad's square before its
/// physical cant is applied, so callers should leave room around it.
#[must_use]
pub fn trackpad_surface_point(surface: egui::Rect, side: PadSide, locus: [f32; 2]) -> egui::Pos2 {
    let clamp = |value: f32| {
        if value.is_finite() {
            value.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    };
    let local = egui::vec2(
        clamp(locus[0]) * surface.width() * 0.5,
        -clamp(locus[1]) * surface.height() * 0.5,
    );
    let (sin, cos) = pad_tilt(side.index()).sin_cos();
    surface.center()
        + egui::vec2(
            local.x.mul_add(cos, -local.y * sin),
            local.x.mul_add(sin, local.y * cos),
        )
}

fn pad_rect(view: egui::Rect, index: usize) -> egui::Rect {
    let mut bounds = egui::Rect::NOTHING;
    for point in &trackpad_shapes()[index].0.points {
        bounds.extend_with(normalized_point(view, *point));
    }
    bounds
}
