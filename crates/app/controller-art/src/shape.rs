//! Closed polygons in unit-square coordinates, and the triangulator behind them.

use eframe::egui;

use crate::geometry::normalized_point;

/// A closed polygon in unit-square coordinates, triangulated once so that
/// painting it is a plain mesh upload.
pub(crate) struct UnitShape {
    pub(crate) points: Vec<[f32; 2]>,
    pub(crate) triangles: Vec<[u32; 3]>,
}

impl UnitShape {
    pub(crate) fn new(points: Vec<[f32; 2]>) -> Self {
        let triangles = triangulate(&points);
        Self { points, triangles }
    }

    /// A rounded rectangle, optionally turned about its own centre.
    pub(crate) fn rounded_rect(
        center: [f32; 2],
        size: [f32; 2],
        corner: f32,
        rotation: f32,
    ) -> Self {
        const ARC_STEPS: usize = 5;
        let (half_x, half_y) = (size[0] * 0.5, size[1] * 0.5);
        let corner = corner.min(half_x).min(half_y);
        let (sin, cos) = rotation.sin_cos();
        let mut points = Vec::with_capacity(4 * (ARC_STEPS + 1));
        for (index, [sign_x, sign_y]) in [[1.0, -1.0], [1.0, 1.0], [-1.0, 1.0], [-1.0, -1.0]]
            .into_iter()
            .enumerate()
        {
            let pivot = [sign_x * (half_x - corner), sign_y * (half_y - corner)];
            #[allow(clippy::cast_precision_loss)]
            for step in 0..=ARC_STEPS {
                let angle = std::f32::consts::FRAC_PI_2
                    * (step as f32 / ARC_STEPS as f32 + index as f32 - 1.0);
                let (arc_sin, arc_cos) = angle.sin_cos();
                let local = [
                    corner.mul_add(arc_cos, pivot[0]),
                    corner.mul_add(arc_sin, pivot[1]),
                ];
                points.push([
                    local[0].mul_add(cos, center[0]) - local[1] * sin,
                    local[0].mul_add(sin, center[1]) + local[1] * cos,
                ]);
            }
        }
        Self::new(points)
    }

    /// Bounding box in unit-square coordinates.
    pub(crate) fn bounds(&self) -> egui::Rect {
        let mut bounds = egui::Rect::NOTHING;
        for [x, y] in self.points.iter().copied() {
            bounds.extend_with(egui::pos2(x, y));
        }
        bounds
    }

    pub(crate) fn screen_points(&self, view: egui::Rect) -> Vec<egui::Pos2> {
        self.points
            .iter()
            .map(|point| normalized_point(view, *point))
            .collect()
    }

    pub(crate) fn paint(
        &self,
        painter: &egui::Painter,
        view: egui::Rect,
        fill: egui::Color32,
        stroke: egui::Stroke,
    ) {
        let points = self.screen_points(view);
        // A mesh built from the triangulation fills concave silhouettes exactly.
        // egui's generic path fill leaks outside the outline around the grips.
        let mut mesh = egui::Mesh::default();
        for point in &points {
            mesh.colored_vertex(*point, fill);
        }
        for [a, b, c] in &self.triangles {
            mesh.add_triangle(*a, *b, *c);
        }
        painter.add(egui::Shape::mesh(mesh));
        painter.add(egui::Shape::closed_line(points, stroke));
    }

    pub(crate) fn outline(&self, painter: &egui::Painter, view: egui::Rect, stroke: egui::Stroke) {
        painter.add(egui::Shape::closed_line(self.screen_points(view), stroke));
    }
}

// ---------------------------------------------------------------------------
// Polygon helpers
// ---------------------------------------------------------------------------

pub(crate) fn cross(origin: [f32; 2], first: [f32; 2], second: [f32; 2]) -> f32 {
    (first[0] - origin[0]).mul_add(
        second[1] - origin[1],
        -((first[1] - origin[1]) * (second[0] - origin[0])),
    )
}

pub(crate) fn signed_area(points: &[[f32; 2]]) -> f32 {
    let mut area = 0.0;
    for (index, point) in points.iter().enumerate() {
        let next = points[(index + 1) % points.len()];
        area += point[0].mul_add(next[1], -(next[0] * point[1]));
    }
    area * 0.5
}

pub(crate) fn point_in_triangle(point: [f32; 2], triangle: [[f32; 2]; 3]) -> bool {
    let [a, b, c] = triangle;
    cross(a, b, point) >= 0.0 && cross(b, c, point) >= 0.0 && cross(c, a, point) >= 0.0
}

/// Ear-clipping triangulation of a simple polygon, concave parts included.
pub(crate) fn triangulate(points: &[[f32; 2]]) -> Vec<[u32; 3]> {
    let mut remaining: Vec<u32> = (0..points.len())
        .map(|index| u32::try_from(index).expect("an outline has far fewer than u32::MAX points"))
        .collect();
    if signed_area(points) < 0.0 {
        remaining.reverse();
    }
    let corner = |index: u32| points[index as usize];
    let mut triangles = Vec::with_capacity(remaining.len().saturating_sub(2));
    while remaining.len() > 3 {
        let ear = (0..remaining.len()).find_map(|index| {
            let triangle = [
                remaining[(index + remaining.len() - 1) % remaining.len()],
                remaining[index],
                remaining[(index + 1) % remaining.len()],
            ];
            let corners = triangle.map(corner);
            if cross(corners[0], corners[1], corners[2]) <= 0.0 {
                return None;
            }
            let empty = remaining.iter().all(|candidate| {
                triangle.contains(candidate) || !point_in_triangle(corner(*candidate), corners)
            });
            empty.then_some((index, triangle))
        });
        // A non-simple polygon has no ear left; stop instead of looping forever.
        let Some((index, triangle)) = ear else {
            break;
        };
        triangles.push(triangle);
        remaining.remove(index);
    }
    if remaining.len() == 3 {
        triangles.push([remaining[0], remaining[1], remaining[2]]);
    }
    triangles
}
