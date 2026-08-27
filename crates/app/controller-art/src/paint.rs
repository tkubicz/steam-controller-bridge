//! Painting the two faces from a caller-supplied per-control state.

use eframe::egui;
use ui_theme::{ACCENT, ACCENT_SUBTLE, DETAIL, INSET, OUTLINE, SURFACE};

use crate::geometry::{
    body_shape, control_rect, dpad_arm, dpad_shape, locus_point, normalized_point, shoulder_index,
    shoulder_shapes, trackpad_shapes, trackpad_surface_shapes, unit_rect, BUMPERS, BUMPER_SIZE,
    DOT_RADIUS, FACE_BUTTON_LAYOUT, FACE_BUTTON_RADIUS, GRIP_PADDLES, OPTION_LAYOUT,
    PUCK_CONNECTOR, PUCK_CONNECTOR_SIZE, QUICK_ACCESS, QUICK_ACCESS_SIZE, STEAM_RADIUS, STICKS,
    STICK_RADIUS, TOP_SEAM, TOP_SEAM_WIDTH, USB_PORT_SIZE,
};
use crate::{Analog, Control, ControlState, Highlight, PadSide};

/// Draws one pad at instruction-view scale using the same cant, corner radius,
/// inset, and resting colors as the full controller artwork.
pub fn draw_trackpad_surface(painter: &egui::Painter, surface: egui::Rect, side: PadSide) {
    let (pad, inset) = &trackpad_surface_shapes()[side.index()];
    pad.paint(painter, surface, INSET, egui::Stroke::new(2.0, OUTLINE));
    inset.outline(painter, surface, egui::Stroke::new(1.0, DETAIL));
}

/// Fill and stroke for one control, from its state and the face's resting
/// stroke.
///
/// Precedence runs marked, then pressed, then touched, then hovered. The editor
/// only ever sets the first and last; the visualizer only ever sets the middle
/// two. Neither has to know the other's states exist.
fn control_style(
    state: ControlState,
    default_stroke: egui::Stroke,
) -> (egui::Color32, egui::Stroke) {
    let touched = matches!(state.analog, Some(Analog::Position { touched: true, .. }));
    match state.highlight {
        Highlight::Active => (ACCENT_SUBTLE, egui::Stroke::new(2.4, ACCENT)),
        Highlight::Hover => (INSET, egui::Stroke::new(1.8, ACCENT.gamma_multiply(0.6))),
        Highlight::Idle if touched => (INSET, egui::Stroke::new(1.8, ACCENT.gamma_multiply(0.6))),
        Highlight::Idle => (INSET, default_stroke),
    }
}

/// The travel a trigger reports, clamped and made safe for non-finite input.
fn travel_of(state: ControlState) -> f32 {
    match state.analog {
        Some(Analog::Trigger { travel }) if travel.is_finite() => travel.clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Where to draw the live position dot, if there is one to draw.
fn locus_of(state: ControlState) -> Option<[f32; 2]> {
    match state.analog {
        Some(Analog::Position { offset, .. }) => offset,
        _ => None,
    }
}

/// The shell silhouette. Paint this before either face.
pub fn draw_body(painter: &egui::Painter, view: egui::Rect) {
    body_shape().paint(painter, view, SURFACE, egui::Stroke::new(1.8, OUTLINE));
}

/// The front face: bumpers, trackpads, sticks, D-pad, face buttons, options,
/// Steam and Quick Access.
pub fn draw_front(
    painter: &egui::Painter,
    view: egui::Rect,
    state: &impl Fn(Control) -> ControlState,
) {
    let scale = view.width();
    let detail = egui::Stroke::new(1.3, DETAIL);
    let outline = egui::Stroke::new(1.5, OUTLINE);

    bumpers(painter, view, state);
    trackpads(painter, view, state, detail, outline);
    sticks(painter, view, state, detail, outline);
    dpad(painter, view, state, detail);
    face_buttons(painter, view, state, detail);
    options(painter, view, state, detail);
    steam(painter, view, state, detail, scale);
    quick_access(painter, view, state, scale);
}

fn bumpers(painter: &egui::Painter, view: egui::Rect, state: &impl Fn(Control) -> ControlState) {
    // The bumpers sit on the shell's top corners, breaking the outline. This is
    // the front cap; the rear wing for the same control is drawn by `draw_rear`.
    for (index, center) in BUMPERS.into_iter().enumerate() {
        let control = if index == 0 {
            Control::LeftBumper
        } else {
            Control::RightBumper
        };
        let (fill, stroke) = control_style(state(control), egui::Stroke::new(1.6, OUTLINE));
        let idle = state(control).highlight == Highlight::Idle;
        let bumper = unit_rect(view, center, BUMPER_SIZE);
        painter.rect_filled(
            bumper,
            bumper.height() * 0.5,
            if idle { SURFACE } else { fill },
        );
        painter.rect_stroke(
            bumper,
            bumper.height() * 0.5,
            if idle {
                egui::Stroke::new(1.6, OUTLINE)
            } else {
                stroke
            },
            egui::StrokeKind::Inside,
        );
    }
}

fn trackpads(
    painter: &egui::Painter,
    view: egui::Rect,
    state: &impl Fn(Control) -> ControlState,
    detail: egui::Stroke,
    outline: egui::Stroke,
) {
    for (index, control) in [Control::LeftPad, Control::RightPad]
        .into_iter()
        .enumerate()
    {
        let (pad, inset) = &trackpad_shapes()[index];
        let current = state(control);
        let (fill, stroke) = control_style(current, outline);
        pad.paint(painter, view, fill, stroke);
        inset.outline(
            painter,
            view,
            egui::Stroke::new(detail.width, stroke.color.gamma_multiply(0.75)),
        );
        if let Some(offset) = locus_of(current) {
            painter.circle_filled(
                locus_point(view, control, offset),
                view.width() * DOT_RADIUS,
                ACCENT,
            );
        }
    }
}

fn sticks(
    painter: &egui::Painter,
    view: egui::Rect,
    state: &impl Fn(Control) -> ControlState,
    detail: egui::Stroke,
    outline: egui::Stroke,
) {
    let scale = view.width();
    for (index, control) in [Control::LeftStick, Control::RightStick]
        .into_iter()
        .enumerate()
    {
        let current = state(control);
        let (fill, stroke) = control_style(current, outline);
        let center = normalized_point(view, STICKS[index]);
        painter.circle_filled(center, scale * STICK_RADIUS, fill);
        painter.circle_stroke(center, scale * STICK_RADIUS, stroke);
        painter.circle_stroke(center, scale * (STICK_RADIUS - 0.014), detail);
        if let Some(offset) = locus_of(current) {
            painter.circle_filled(
                locus_point(view, control, offset),
                scale * DOT_RADIUS,
                ACCENT,
            );
        }
    }
}

fn dpad(
    painter: &egui::Painter,
    view: egui::Rect,
    state: &impl Fn(Control) -> ControlState,
    detail: egui::Stroke,
) {
    dpad_shape().paint(painter, view, INSET, detail);
    for control in [
        Control::DpadUp,
        Control::DpadDown,
        Control::DpadLeft,
        Control::DpadRight,
    ] {
        let current = state(control);
        if current.highlight == Highlight::Idle {
            continue;
        }
        let (fill, stroke) = control_style(current, detail);
        let (center, size) = dpad_arm(control);
        let arm = unit_rect(view, center, size);
        painter.rect_filled(arm, 0.0, fill);
        painter.rect_stroke(arm, 0.0, stroke, egui::StrokeKind::Inside);
    }
}

fn face_buttons(
    painter: &egui::Painter,
    view: egui::Rect,
    state: &impl Fn(Control) -> ControlState,
    detail: egui::Stroke,
) {
    let scale = view.width();
    for (control, _) in FACE_BUTTON_LAYOUT {
        let (fill, stroke) = control_style(state(control), detail);
        let rect = control_rect(view, control);
        painter.circle_filled(rect.center(), scale * FACE_BUTTON_RADIUS, fill);
        painter.circle_stroke(rect.center(), scale * FACE_BUTTON_RADIUS, stroke);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            control.label(),
            egui::FontId::proportional(scale * 0.026),
            stroke.color,
        );
    }
}

fn options(
    painter: &egui::Painter,
    view: egui::Rect,
    state: &impl Fn(Control) -> ControlState,
    detail: egui::Stroke,
) {
    let scale = view.width();
    for (control, _) in OPTION_LAYOUT {
        let (fill, stroke) = control_style(state(control), detail);
        let button = control_rect(view, control);
        painter.rect_filled(button, scale * 0.010, fill);
        painter.rect_stroke(button, scale * 0.010, stroke, egui::StrokeKind::Inside);
    }
}

fn steam(
    painter: &egui::Painter,
    view: egui::Rect,
    state: &impl Fn(Control) -> ControlState,
    detail: egui::Stroke,
    scale: f32,
) {
    let (fill, stroke) = control_style(state(Control::Steam), detail);
    let center = control_rect(view, Control::Steam).center();
    painter.circle_filled(center, scale * STEAM_RADIUS, fill);
    painter.circle_stroke(center, scale * STEAM_RADIUS, stroke);
    painter.circle_stroke(center, scale * (STEAM_RADIUS - 0.009), detail);
}

fn quick_access(
    painter: &egui::Painter,
    view: egui::Rect,
    state: &impl Fn(Control) -> ControlState,
    scale: f32,
) {
    let (fill, stroke) =
        control_style(state(Control::QuickAccess), egui::Stroke::new(1.5, OUTLINE));
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

/// The rear face: shell seam and USB port, the four shoulder wings, the puck
/// connector and the four grip paddles.
pub fn draw_rear(
    painter: &egui::Painter,
    view: egui::Rect,
    state: &impl Fn(Control) -> ControlState,
) {
    let scale = view.width();
    let detail = egui::Stroke::new(1.3, DETAIL);
    let outline = egui::Stroke::new(1.4, OUTLINE);

    seam_and_port(painter, view, detail, scale);
    shoulders(painter, view, state, outline);
    puck(painter, view, detail, scale);
    paddles(painter, view, state, scale);
}

fn seam_and_port(painter: &egui::Painter, view: egui::Rect, detail: egui::Stroke, scale: f32) {
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
}

fn shoulders(
    painter: &egui::Painter,
    view: egui::Rect,
    state: &impl Fn(Control) -> ControlState,
    outline: egui::Stroke,
) {
    // Painted trigger-first so each bumper rides over its own trigger's upper
    // edge, which is how the reference reads.
    for control in [
        Control::RightTrigger,
        Control::RightBumper,
        Control::LeftTrigger,
        Control::LeftBumper,
    ] {
        let Some(index) = shoulder_index(control) else {
            continue;
        };
        let shape = &shoulder_shapes()[index];
        let current = state(control);
        let (fill, stroke) = control_style(current, outline);
        shape.paint(painter, view, fill, stroke);

        let travel = travel_of(current);
        if travel > 0.0 {
            // A trigger fills from its lower edge upward. The clip runs along
            // screen axes, so on a wing canted by 0.24 rad the boundary is a
            // few degrees off the wing's own edge.
            let bounds = shape.bounds();
            let wing = egui::Rect::from_min_max(
                normalized_point(view, [bounds.left(), bounds.top()]),
                normalized_point(view, [bounds.right(), bounds.bottom()]),
            );
            let filled = egui::Rect::from_min_max(
                egui::pos2(wing.left(), wing.bottom() - wing.height() * travel),
                wing.max,
            );
            shape.paint(
                &painter.with_clip_rect(filled),
                view,
                ACCENT.gamma_multiply(0.75),
                stroke,
            );
        }
    }
}

fn puck(painter: &egui::Painter, view: egui::Rect, detail: egui::Stroke, scale: f32) {
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
}

fn paddles(
    painter: &egui::Painter,
    view: egui::Rect,
    state: &impl Fn(Control) -> ControlState,
    scale: f32,
) {
    for (control, center, size) in GRIP_PADDLES {
        let (fill, stroke) = control_style(state(control), egui::Stroke::new(1.5, OUTLINE));
        let center = normalized_point(view, center);
        let radius = egui::vec2(size[0] * 0.5 * scale, size[1] * 0.5 * scale);
        painter.add(egui::Shape::ellipse_filled(center, radius, fill));
        painter.add(egui::Shape::ellipse_stroke(center, radius, stroke));
    }
}
