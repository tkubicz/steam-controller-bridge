use super::{
    egui, OverlayState, ACCENT, ARC_STEPS, HUB_RADIUS, LABEL_RADIUS_FRACTION, MUTED_TEXT,
    ON_ACCENT, SCRIM, SURFACE, SURFACE_RAISED, TEXT, WEDGE_GAP, WHEEL_RADIUS,
};

pub(super) fn paint_wheel(painter: &egui::Painter, rect: egui::Rect, state: &OverlayState) {
    let Some((selected, page)) = state.open else {
        return;
    };
    let (entries, offset) = state.page_entries(page);
    if entries.is_empty() {
        // A roster update can outrun the picker's page clamp by one report;
        // painting nothing beats dimming the game under a wheel-less scrim.
        return;
    }
    painter.rect_filled(rect, 0.0, SCRIM);
    let center = rect.center();

    let sectors = entries.len();
    let selected = selected.min(sectors - 1);
    for (index, name) in entries.iter().enumerate() {
        let chosen = index == selected;
        paint_wedge(painter, center, index, sectors, chosen);
        paint_label(
            painter,
            center,
            index,
            sectors,
            name,
            chosen,
            state.active == Some(offset + index),
        );
    }

    // The hub covers the wedges' shared apex, turning the pie into a ring and
    // leaving room to name what is about to be applied.
    painter.circle_filled(center, HUB_RADIUS, SURFACE);
    painter.text(
        center - egui::vec2(0.0, 14.0),
        egui::Align2::CENTER_CENTER,
        entries[selected].as_str(),
        egui::FontId::proportional(17.0),
        TEXT,
    );
    painter.text(
        center + egui::vec2(0.0, 10.0),
        egui::Align2::CENTER_CENTER,
        "A apply",
        egui::FontId::proportional(12.0),
        MUTED_TEXT,
    );
    painter.text(
        center + egui::vec2(0.0, 26.0),
        egui::Align2::CENTER_CENTER,
        "B cancel",
        egui::FontId::proportional(12.0),
        MUTED_TEXT,
    );

    let pages = state.page_count();
    if pages > 1 {
        painter.text(
            center + egui::vec2(0.0, WHEEL_RADIUS + 28.0),
            egui::Align2::CENTER_CENTER,
            format!("L1 / R1   page {} of {pages}", page + 1),
            egui::FontId::proportional(13.0),
            MUTED_TEXT,
        );
    }
}

pub(super) fn paint_wedge(
    painter: &egui::Painter,
    center: egui::Pos2,
    index: usize,
    sectors: usize,
    selected: bool,
) {
    let radius = if selected {
        WHEEL_RADIUS + 10.0
    } else {
        WHEEL_RADIUS
    };
    let fill = if selected { ACCENT } else { SURFACE_RAISED };
    if sectors <= 1 {
        // A lone entry — the short last page of a roster like 9-of-8 — owns
        // the whole wheel. Its "wedge" swept nearly a full turn, which is not
        // convex and mistessellates; a disc is the same shape drawn honestly.
        painter.circle_filled(center, radius, fill);
        return;
    }
    let arc = std::f32::consts::TAU / sectors_as_f32(sectors);
    let middle = arc * sectors_as_f32(index);
    let (start, end) = (
        middle - arc / 2.0 + WEDGE_GAP,
        middle + arc / 2.0 - WEDGE_GAP,
    );

    // A pie wedge shares the centre, so it stays convex for any sector count of
    // two or more and tessellates correctly. The hub hides the apex afterwards.
    let mut points = Vec::with_capacity(ARC_STEPS + 2);
    points.push(center);
    for step in 0..=ARC_STEPS {
        let t = sectors_as_f32(step) / sectors_as_f32(ARC_STEPS);
        points.push(point_on_wheel(center, start + (end - start) * t, radius));
    }
    painter.add(egui::Shape::convex_polygon(
        points,
        fill,
        egui::Stroke::NONE,
    ));
}

pub(super) fn paint_label(
    painter: &egui::Painter,
    center: egui::Pos2,
    index: usize,
    sectors: usize,
    name: &str,
    selected: bool,
    active: bool,
) {
    let arc = std::f32::consts::TAU / sectors_as_f32(sectors);
    let position = point_on_wheel(
        center,
        arc * sectors_as_f32(index),
        WHEEL_RADIUS * LABEL_RADIUS_FRACTION,
    );
    painter.text(
        position,
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(14.0),
        if selected { ON_ACCENT } else { TEXT },
    );
    if active {
        // A dot marks the profile that is already in use, so the user can see
        // what a cancel would leave them with.
        painter.circle_filled(
            position + egui::vec2(0.0, 15.0),
            3.0,
            if selected { ON_ACCENT } else { ACCENT },
        );
    }
}

/// Sector zero sits at twelve o'clock and they run clockwise, matching
/// `profile_picker::sector_for`. Screen y grows downwards, hence the negation.
pub(super) fn point_on_wheel(center: egui::Pos2, angle: f32, radius: f32) -> egui::Pos2 {
    center + egui::vec2(angle.sin() * radius, -angle.cos() * radius)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "sector and step counts are small enough to be exact in f32"
)]
pub(super) fn sectors_as_f32(value: usize) -> f32 {
    value as f32
}
