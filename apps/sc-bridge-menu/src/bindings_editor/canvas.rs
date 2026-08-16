use super::{
    binding_side, body_bounds, egui, normalized_point, trackpad_rect, ControlCallout,
    ControllerView, EditorSelection, LabelSide, PadBindings, PadMotionMode, PadSide, ACCENT,
    MUTED_TEXT,
};

const LABEL_SIZE: [f32; 2] = [86.0, 40.0];
/// Gap between a label and the controller it points at.
const LABEL_LEAD: f32 = 12.0;
/// Gap between the front and rear groups.
const VIEW_GAP: f32 = 28.0;
/// Height of the FRONT / REAR caption row above each controller.
const CAPTION_ROW: f32 = 22.0;

/// Places both controller drawings and every callout label inside the canvas.
///
/// Each drawing is a square so the artwork below, which is expressed in a unit
/// square, keeps its proportions whatever the window size is. Everything is
/// laid out against the silhouette's bounding box rather than that square, so
/// the drawings are as large as the canvas allows and the labels sit a fixed
/// distance from the controller itself.
pub(super) struct CanvasLayout {
    pub(super) front: egui::Rect,
    pub(super) rear: egui::Rect,
    pub(super) caption_top: f32,
}

impl CanvasLayout {
    pub(super) fn new(canvas: egui::Rect) -> Self {
        let inner = canvas.shrink2(egui::vec2(16.0, 14.0));
        let bounds = body_bounds();
        // Width budget: front body, gap, label column, rear body, label column.
        // The rear grips are labelled on both sides.
        let label_columns = 2.0 * (LABEL_SIZE[0] + LABEL_LEAD);
        let by_width = (inner.width() - label_columns - VIEW_GAP) * 0.5 / bounds.width();
        // Height budget: caption row, body, the Quick Access label below the
        // front body.
        let by_height =
            (inner.height() - CAPTION_ROW - LABEL_LEAD - LABEL_SIZE[1]) / bounds.height();
        let side = by_width.min(by_height).max(180.0);

        let body = egui::vec2(side * bounds.width(), side * bounds.height());
        let group_height = CAPTION_ROW + body.y + LABEL_LEAD + LABEL_SIZE[1];
        let caption_top = inner.top() + (inner.height() - group_height).max(0.0) * 0.5;
        let body_top = caption_top + CAPTION_ROW;
        let total_width = 2.0f32.mul_add(body.x, label_columns + VIEW_GAP);
        let front_left = inner.left() + (inner.width() - total_width).max(0.0) * 0.5;
        let rear_left = front_left + body.x + VIEW_GAP + LABEL_SIZE[0] + LABEL_LEAD;
        // The drawing rect is the unit square that puts the silhouette exactly
        // where the budget above says it goes.
        let view_at = |body_left: f32| {
            egui::Rect::from_min_size(
                egui::pos2(
                    bounds.left().mul_add(-side, body_left),
                    bounds.top().mul_add(-side, body_top),
                ),
                egui::vec2(side, side),
            )
        };
        Self {
            front: view_at(front_left),
            rear: view_at(rear_left),
            caption_top,
        }
    }

    pub(super) fn view(&self, view: ControllerView) -> egui::Rect {
        match view {
            ControllerView::Front => self.front,
            ControllerView::Rear => self.rear,
        }
    }

    /// The silhouette's bounding box on screen, which is what labels and
    /// captions are aligned to.
    pub(super) fn body(&self, view: ControllerView) -> egui::Rect {
        let view = self.view(view);
        let bounds = body_bounds();
        egui::Rect::from_min_max(
            normalized_point(view, [bounds.left(), bounds.top()]),
            normalized_point(view, [bounds.right(), bounds.bottom()]),
        )
    }

    pub(super) fn label(&self, callout: ControlCallout) -> egui::Rect {
        let body = self.body(callout.view);
        let size = egui::vec2(LABEL_SIZE[0], LABEL_SIZE[1]);
        let row_top = callout
            .label_y
            .mul_add(body.height(), body.top() - size.y * 0.5);
        let min = match callout.side {
            LabelSide::Left => egui::pos2(body.left() - LABEL_LEAD - size.x, row_top),
            LabelSide::Right => egui::pos2(body.right() + LABEL_LEAD, row_top),
            LabelSide::Below => {
                egui::pos2(body.center().x - size.x * 0.5, body.bottom() + LABEL_LEAD)
            }
        };
        egui::Rect::from_min_size(min, size)
    }
}

/// Where a ray from the middle of `rect` towards `toward` leaves the rectangle.
pub(super) fn rect_edge_towards(rect: egui::Rect, toward: egui::Pos2) -> egui::Pos2 {
    let center = rect.center();
    let delta = toward - center;
    let half = rect.size() * 0.5;
    let reach = |distance: f32, half: f32| {
        if distance.abs() > f32::EPSILON {
            half / distance.abs()
        } else {
            f32::INFINITY
        }
    };
    let scale = reach(delta.x, half.x).min(reach(delta.y, half.y));
    if scale.is_finite() {
        center + delta * scale
    } else {
        center
    }
}

/// Summarizes each pad on the artwork: what its motion does, and how many
/// regions carry an action. Neither is fixed to a side any more, so the label
/// has to be read off the profile rather than baked into the drawing.
pub(super) fn draw_pad_labels(
    painter: &egui::Painter,
    view: egui::Rect,
    pads: &PadBindings,
    selected: EditorSelection,
) {
    for side in PadSide::ALL {
        let pad = pads.get(binding_side(side));
        let motion = match pad.motion {
            PadMotionMode::None => "NO MOTION",
            PadMotionMode::Pointer => "POINTER",
            PadMotionMode::Scroll => "SCROLL",
        };
        let bound = pad.bound_region_count();
        let rect = trackpad_rect(view, side);
        let color = if matches!(
            selected,
            EditorSelection::Pad(chosen) | EditorSelection::PadRegion(chosen, _) if chosen == side
        ) {
            ACCENT
        } else {
            MUTED_TEXT
        };
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{motion}\n{bound} bound"),
            egui::FontId::proportional(9.5),
            color,
        );
    }
}
