use super::{
    draw_trackpad_surface, egui, trackpad_surface_point, PadRegion, PadRegionShape, PadSide,
    ACCENT, DETAIL, MUTED_TEXT, ON_ACCENT,
};

/// Where the pad's square boundary turns a corner. Edges are straight between
/// them, so these plus a sector's own two ends draw the outline exactly.
const CORNER_BEARINGS: [f32; 4] = [45.0, 135.0, 225.0, 315.0];

/// Paints a pad's regions onto a drawing of that pad, in the engine's own
/// conventions: a rounded square, zero degrees up, bearings clockwise.
pub(super) fn draw_region_map(
    painter: &egui::Painter,
    rect: egui::Rect,
    side: PadSide,
    regions: &[PadRegion],
    selected: Option<usize>,
) {
    // The pad art is canted and needs room to rotate.
    let surface = egui::Rect::from_center_size(
        rect.center(),
        egui::Vec2::splat(rect.width().min(rect.height()) * 0.78),
    );
    draw_trackpad_surface(painter, surface, side);
    if regions.is_empty() {
        painter.text(
            surface.center(),
            egui::Align2::CENTER_CENTER,
            "No regions",
            egui::FontId::proportional(11.0),
            MUTED_TEXT,
        );
        return;
    }

    // Back to front, so an earlier region - which also wins resolution - is
    // drawn on top.
    for (index, region) in regions.iter().enumerate().rev() {
        let chosen = selected == Some(index);
        let fill = if chosen {
            ACCENT.gamma_multiply(0.55)
        } else if region.is_bound() {
            DETAIL.gamma_multiply(0.55)
        } else {
            egui::Color32::TRANSPARENT
        };
        let bearings = region_bearings(region.shape);
        // A region with a hole is concave, so it cannot go to `convex_polygon`.
        if fill != egui::Color32::TRANSPARENT {
            painter.add(egui::Shape::mesh(region_mesh(
                surface,
                side,
                region.shape,
                &bearings,
                fill,
            )));
        }
        painter.add(egui::Shape::closed_line(
            region_outline(surface, side, region.shape, &bearings),
            egui::Stroke::new(
                if chosen { 1.8 } else { 1.0 },
                if chosen { ACCENT } else { DETAIL },
            ),
        ));
    }
    for (index, region) in regions.iter().enumerate() {
        painter.text(
            region_label_anchor(surface, side, region.shape),
            egui::Align2::CENTER_CENTER,
            &region.name,
            egui::FontId::proportional(9.5),
            if selected == Some(index) {
                ON_ACCENT
            } else {
                MUTED_TEXT
            },
        );
    }
}

/// Pad-local position in the controller's `-1..=1` convention, Y upward.
/// Dividing by the larger direction component mirrors the engine's
/// `max(|x|, |y|)` extent, putting 100% on the edge in every direction.
fn pad_locus(degrees: f32, extent: f32) -> [f32; 2] {
    let radians = degrees.to_radians();
    let (dx, dy) = (radians.sin(), radians.cos());
    let reach = dx.abs().max(dy.abs()).max(f32::EPSILON);
    [dx / reach * extent, dy / reach * extent]
}

/// Screen position for a bearing and an extent, on the pad's own square.
fn pad_point(surface: egui::Rect, side: PadSide, degrees: f32, extent: f32) -> egui::Pos2 {
    trackpad_surface_point(surface, side, pad_locus(degrees, extent))
}

/// A region's two ends plus every pad corner strictly between them. A validated
/// sweep is at most one turn, so six is an exact fit.
#[derive(Debug, Clone, Copy)]
struct RegionBearings {
    values: [f32; 2 + CORNER_BEARINGS.len()],
    len: usize,
}

impl std::ops::Deref for RegionBearings {
    type Target = [f32];

    fn deref(&self) -> &Self::Target {
        &self.values[..self.len]
    }
}

fn region_bearings(shape: PadRegionShape) -> RegionBearings {
    let start = f32::from(shape.start_degrees);
    let end = start + f32::from(shape.sweep_degrees);
    let mut bearings = RegionBearings {
        values: [0.0; 2 + CORNER_BEARINGS.len()],
        len: 1,
    };
    bearings.values[0] = start;
    // A sweep can run past 360, so each corner is offered in both turns. The
    // length guard is unreachable for a validated shape; it stops a wider sweep
    // panicking rather than clipping.
    for corner in CORNER_BEARINGS {
        for turn in [0.0, 360.0] {
            let candidate = corner + turn;
            if candidate > start && candidate < end && bearings.len + 1 < bearings.values.len() {
                bearings.values[bearings.len] = candidate;
                bearings.len += 1;
            }
        }
    }
    bearings.values[..bearings.len].sort_by(f32::total_cmp);
    bearings.values[bearings.len] = end;
    bearings.len += 1;
    bearings
}

fn extents(shape: PadRegionShape) -> (f32, f32) {
    (
        f32::from(shape.inner_percent) / 100.0,
        f32::from(shape.outer_percent) / 100.0,
    )
}

/// A strip of triangles: the tessellator never fills a concave outline, and
/// repainting allocates no per-segment polygon.
fn region_mesh(
    surface: egui::Rect,
    side: PadSide,
    shape: PadRegionShape,
    bearings: &[f32],
    fill: egui::Color32,
) -> egui::Mesh {
    let (inner, outer) = extents(shape);
    let mut mesh = egui::Mesh::default();
    for pair in bearings.windows(2) {
        let base = u32::try_from(mesh.vertices.len()).expect("region mesh fits in u32 indices");
        mesh.colored_vertex(pad_point(surface, side, pair[0], outer), fill);
        mesh.colored_vertex(pad_point(surface, side, pair[1], outer), fill);
        if inner > 0.0 {
            mesh.colored_vertex(pad_point(surface, side, pair[1], inner), fill);
            mesh.colored_vertex(pad_point(surface, side, pair[0], inner), fill);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        } else {
            mesh.colored_vertex(surface.center(), fill);
            mesh.add_triangle(base, base + 1, base + 2);
        }
    }
    mesh
}

/// Outer edge forwards, inner edge back. With no hole it closes through the
/// middle; a full sweep closes on itself.
fn region_outline(
    surface: egui::Rect,
    side: PadSide,
    shape: PadRegionShape,
    bearings: &[f32],
) -> Vec<egui::Pos2> {
    let (inner, outer) = extents(shape);
    let mut points: Vec<_> = bearings
        .iter()
        .map(|degrees| pad_point(surface, side, *degrees, outer))
        .collect();
    if inner > 0.0 {
        points.extend(
            bearings
                .iter()
                .rev()
                .map(|degrees| pad_point(surface, side, *degrees, inner)),
        );
    } else if shape.sweep_degrees < 360 {
        points.push(surface.center());
    }
    points
}

fn region_label_anchor(surface: egui::Rect, side: PadSide, shape: PadRegionShape) -> egui::Pos2 {
    let middle = f32::from(shape.start_degrees) + f32::from(shape.sweep_degrees) * 0.5;
    let band = (f32::from(shape.inner_percent) + f32::from(shape.outer_percent)) / 200.0;
    pad_point(surface, side, middle, band)
}

#[cfg(test)]
mod tests {
    use super::{pad_locus, region_bearings, PadRegion, PadRegionShape};

    /// The drawing's extent must agree with the engine's `max(|x|, |y|)`.
    #[test]
    fn full_extent_reaches_the_pad_edge_at_every_bearing() {
        for step in 0..72_u8 {
            let degrees = f32::from(step) * 5.0;
            let [x, y] = pad_locus(degrees, 1.0);
            let extent = x.abs().max(y.abs());
            assert!(
                (extent - 1.0).abs() < 1e-4,
                "{degrees} degrees reached {extent}"
            );
        }
        // A corner is both axes at full scale.
        let [x, y] = pad_locus(45.0, 1.0);
        assert!((x - 1.0).abs() < 1e-4 && (y - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_sector_containing_a_pad_corner_samples_it_so_its_fill_follows_the_square() {
        let eight = PadRegion::eight_way();
        let top_right = eight
            .iter()
            .find(|region| region.name == "Top Right")
            .expect("preset has a top-right sector");
        assert!(region_bearings(top_right.shape).contains(&45.0));

        // A four-way sector runs corner to corner: one straight edge, no
        // interior sample.
        let four = PadRegion::four_way();
        let top = four
            .iter()
            .find(|region| region.name == "Top")
            .expect("preset has a top sector");
        assert_eq!(region_bearings(top.shape).len(), 2);
    }

    #[test]
    fn an_over_wide_sweep_is_clipped_rather_than_overrunning_the_bearing_buffer() {
        // Unreachable today; the buffer is an exact fit for the sweep cap, and
        // a future wider sweep must not panic.
        let bearings = region_bearings(PadRegionShape {
            start_degrees: 0,
            sweep_degrees: 360,
            inner_percent: 0,
            outer_percent: 100,
        });
        assert_eq!(bearings.len(), 6);
        let wider = region_bearings(PadRegionShape {
            start_degrees: 0,
            sweep_degrees: u16::MAX,
            inner_percent: 0,
            outer_percent: 100,
        });
        assert!(wider.len() <= 6);
        assert!(wider.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn a_whole_pad_region_traces_all_four_corners() {
        let bearings = region_bearings(PadRegionShape::WHOLE);
        for corner in super::CORNER_BEARINGS {
            assert!(
                bearings.contains(&corner),
                "the whole-pad outline skipped {corner} degrees"
            );
        }
        // Both ends plus four corners, no duplicates from the second turn.
        assert_eq!(bearings.len(), 6);
    }
}
