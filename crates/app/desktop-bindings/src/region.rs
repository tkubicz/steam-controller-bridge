//! Resolving a pad coordinate to one of a profile's regions.
//!
//! The pad is a rounded square with each axis independently full-scale, so
//! extent is the Chebyshev distance `max(|x|, |y|)` - the metric
//! `engine::position_aware_threshold` also uses. A Euclidean radius would
//! describe the inscribed disc instead, putting the corners at ~141%.

use crate::model::{
    PadRegion, PadRegionShape, REGION_HYSTERESIS_DEGREES, REGION_HYSTERESIS_PERCENT,
};

/// How far outside its bounds a region is still treated as occupied.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RegionMargin {
    percent: f32,
    degrees: f32,
}

impl RegionMargin {
    pub(crate) const NONE: Self = Self {
        percent: 0.0,
        degrees: 0.0,
    };
    /// Grown bounds, for the region the finger currently occupies.
    pub(crate) const HELD: Self = Self {
        percent: REGION_HYSTERESIS_PERCENT,
        degrees: REGION_HYSTERESIS_DEGREES,
    };
}

/// One coordinate prepared for checks against every region on a pad. Bearing is
/// lazy because a whole-pad shape needs only the cheaper extent check.
struct RegionPoint {
    x: f32,
    y: f32,
    extent: f32,
    degrees: Option<f32>,
}

impl RegionPoint {
    fn new(x: i16, y: i16) -> Self {
        let x = f32::from(x);
        let y = f32::from(y);
        Self {
            x,
            y,
            extent: (x.abs().max(y.abs()) / f32::from(i16::MAX) * 100.0).min(100.0),
            degrees: None,
        }
    }

    /// Bearing in `[0, 360)`, clockwise from twelve o'clock. The `atan2(x, y)`
    /// argument order matches `profile_picker::sector_for`.
    fn degrees(&mut self) -> f32 {
        *self.degrees.get_or_insert_with(|| {
            let degrees = self.x.atan2(self.y).to_degrees();
            if degrees < 0.0 {
                degrees + 360.0
            } else {
                degrees
            }
        })
    }
}

fn point_in_shape(shape: PadRegionShape, point: &mut RegionPoint, margin: RegionMargin) -> bool {
    let inner = f32::from(shape.inner_percent) - margin.percent;
    let outer = f32::from(shape.outer_percent) + margin.percent;
    if point.extent < inner || point.extent > outer {
        return false;
    }
    let sweep = f32::from(shape.sweep_degrees) + margin.degrees * 2.0;
    if sweep >= 360.0 {
        return true;
    }
    // Rotating into the sector's own frame removes the 360-degree wrap case.
    let start = f32::from(shape.start_degrees) - margin.degrees;
    let offset = (point.degrees() - start).rem_euclid(360.0);
    offset < sweep
}

/// Reports whether a coordinate falls inside a shape grown by `margin`.
#[cfg(test)]
pub(crate) fn shape_contains(shape: PadRegionShape, x: i16, y: i16, margin: RegionMargin) -> bool {
    point_in_shape(shape, &mut RegionPoint::new(x, y), margin)
}

/// First region containing the coordinate, preferring the one already occupied
/// so a fingertip resting on a seam does not chatter between two actions.
pub(crate) fn resolve_region(
    regions: &[PadRegion],
    (x, y): (i16, i16),
    held: Option<usize>,
) -> Option<usize> {
    let mut point = RegionPoint::new(x, y);
    if let Some(index) = held {
        if regions
            .get(index)
            .is_some_and(|region| point_in_shape(region.shape, &mut point, RegionMargin::HELD))
        {
            return Some(index);
        }
    }
    regions
        .iter()
        .position(|region| point_in_shape(region.shape, &mut point, RegionMargin::NONE))
}

#[cfg(test)]
mod tests {
    use super::{resolve_region, shape_contains, RegionMargin};
    use crate::model::{PadRegion, PadRegionShape};

    /// A coordinate at `degrees` clockwise from twelve o'clock, `percent` of the
    /// way from the centre to the pad's square edge.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "float-to-int casts saturate, and full scale is exactly i16::MAX"
    )]
    fn at(degrees: f32, percent: f32) -> (i16, i16) {
        let radians = degrees.to_radians();
        let (dx, dy) = (radians.sin(), radians.cos());
        let scale = f32::from(i16::MAX) * percent / 100.0 / dx.abs().max(dy.abs());
        ((dx * scale) as i16, (dy * scale) as i16)
    }

    fn regions(shapes: &[PadRegionShape]) -> Vec<PadRegion> {
        shapes
            .iter()
            .map(|shape| PadRegion::new("R", *shape))
            .collect()
    }

    #[test]
    fn the_whole_shape_contains_every_coordinate_including_the_exact_center() {
        for (x, y) in [
            at(0.0, 0.0),
            at(0.0, 100.0),
            at(180.0, 50.0),
            at(90.0, 99.0),
        ] {
            assert!(shape_contains(
                PadRegionShape::WHOLE,
                x,
                y,
                RegionMargin::NONE
            ));
        }
    }

    #[test]
    fn compass_presets_place_each_cardinal_sample_in_its_named_sector() {
        let eight = PadRegion::eight_way();
        for (bearing, name) in [
            (0.0, "Top"),
            (45.0, "Top Right"),
            (90.0, "Right"),
            (135.0, "Bottom Right"),
            (180.0, "Bottom"),
            (225.0, "Bottom Left"),
            (270.0, "Left"),
            (315.0, "Top Left"),
        ] {
            let index = resolve_region(&eight, at(bearing, 70.0), None)
                .unwrap_or_else(|| panic!("{bearing} degrees matched no region"));
            assert_eq!(eight[index].name, name, "at {bearing} degrees");
        }
    }

    #[test]
    fn a_sector_spanning_twelve_oclock_matches_on_both_sides_of_the_wrap() {
        let shape = PadRegionShape {
            start_degrees: 350,
            sweep_degrees: 20,
            inner_percent: 0,
            outer_percent: 100,
        };
        for bearing in [351.0, 359.0, 0.0, 5.0, 9.0] {
            let (x, y) = at(bearing, 60.0);
            assert!(
                shape_contains(shape, x, y, RegionMargin::NONE),
                "{bearing} degrees should be inside"
            );
        }
        for bearing in [30.0, 180.0, 340.0] {
            let (x, y) = at(bearing, 60.0);
            assert!(
                !shape_contains(shape, x, y, RegionMargin::NONE),
                "{bearing} degrees should be outside"
            );
        }
    }

    #[test]
    fn an_earlier_center_region_shadows_a_later_whole_pad_region() {
        let layout = regions(&[
            PadRegionShape {
                inner_percent: 0,
                outer_percent: 30,
                ..PadRegionShape::WHOLE
            },
            PadRegionShape::WHOLE,
        ]);
        assert_eq!(resolve_region(&layout, at(0.0, 10.0), None), Some(0));
        assert_eq!(resolve_region(&layout, at(0.0, 80.0), None), Some(1));
        // A Euclidean radius would read this diagonal corner as 42% and fall
        // through to the whole-pad region.
        assert_eq!(resolve_region(&layout, at(45.0, 29.0), None), Some(0));
    }

    #[test]
    fn a_coordinate_outside_every_region_resolves_to_nothing() {
        let layout = regions(&[PadRegionShape {
            inner_percent: 60,
            outer_percent: 100,
            ..PadRegionShape::WHOLE
        }]);
        assert_eq!(resolve_region(&layout, at(0.0, 20.0), None), None);
        // An edge band is a frame, not a ring: it holds the edge midpoints and
        // excludes the diagonal middle.
        assert_eq!(resolve_region(&layout, at(0.0, 90.0), None), Some(0));
        assert_eq!(resolve_region(&layout, at(45.0, 90.0), None), Some(0));
        assert_eq!(resolve_region(&layout, at(45.0, 50.0), None), None);
    }

    #[test]
    fn the_pad_corner_is_full_extent_rather_than_beyond_the_pad() {
        // A Euclidean radius would call a corner 141% and clamp, so no band
        // short of the whole pad could contain it.
        for bearing in [45.0, 135.0, 225.0, 315.0] {
            let (x, y) = at(bearing, 100.0);
            // Both axes land on full scale, to within the helper's own f32
            // rounding.
            assert!(x.unsigned_abs() >= 32_766, "at {bearing} degrees: {x}");
            assert!(y.unsigned_abs() >= 32_766, "at {bearing} degrees: {y}");
            assert!(shape_contains(
                PadRegionShape {
                    inner_percent: 95,
                    outer_percent: 100,
                    ..PadRegionShape::WHOLE
                },
                x,
                y,
                RegionMargin::NONE
            ));
        }
    }

    #[test]
    fn a_finger_resting_just_past_a_seam_stays_in_the_region_it_already_holds() {
        let layout = PadRegion::four_way();
        // The Top/Right seam is at 45 degrees.
        let top = resolve_region(&layout, at(30.0, 70.0), None).expect("top sector");
        assert_eq!(layout[top].name, "Top");
        let just_past = at(47.0, 70.0);
        assert_eq!(resolve_region(&layout, just_past, Some(top)), Some(top));
        // With no held region, or once travel clears the margin, the neighbour
        // wins.
        let right = resolve_region(&layout, just_past, None).expect("right sector");
        assert_eq!(layout[right].name, "Right");
        assert_eq!(
            resolve_region(&layout, at(60.0, 70.0), Some(top)),
            Some(right)
        );
    }

    #[test]
    fn a_stale_held_index_is_ignored_rather_than_panicking() {
        let layout = PadRegion::four_way();
        assert_eq!(resolve_region(&layout, at(0.0, 70.0), Some(99)), Some(0));
    }
}
