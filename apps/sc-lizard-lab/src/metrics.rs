#![allow(clippy::cast_precision_loss)] // Bounded capture coordinates become f64 metrics.

use crate::trace::{Motion, Trace};

pub(crate) const STATIONARY_SPEED_COUNTS_PER_SECOND: f64 = 1_000.0;
pub(crate) const FAST_SPEED_COUNTS_PER_SECOND: f64 = 15_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpeedBand {
    Stationary,
    Slow,
    Fast,
}

impl SpeedBand {
    pub(crate) fn classify(speed: f64) -> Self {
        if speed < STATIONARY_SPEED_COUNTS_PER_SECOND {
            Self::Stationary
        } else if speed < FAST_SPEED_COUNTS_PER_SECOND {
            Self::Slow
        } else {
            Self::Fast
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PadInterval {
    pub(crate) start_us: u64,
    pub(crate) end_us: u64,
    pub(crate) distance_counts: f64,
    pub(crate) speed_counts_per_second: f64,
}

pub(crate) fn right_pad_intervals(trace: &Trace) -> impl Iterator<Item = PadInterval> + '_ {
    trace.states.windows(2).filter_map(|pair| {
        if !pair[0].value.right_pad_touched || !pair[1].value.right_pad_touched {
            return None;
        }
        let elapsed_us = pair[1].timestamp_us.saturating_sub(pair[0].timestamp_us);
        if elapsed_us == 0 {
            return None;
        }
        let dx = f64::from(pair[1].value.right_pad_x) - f64::from(pair[0].value.right_pad_x);
        let dy = f64::from(pair[1].value.right_pad_y) - f64::from(pair[0].value.right_pad_y);
        let distance_counts = dx.hypot(dy);
        Some(PadInterval {
            start_us: pair[0].timestamp_us,
            end_us: pair[1].timestamp_us,
            distance_counts,
            speed_counts_per_second: distance_counts * 1_000_000.0 / elapsed_us as f64,
        })
    })
}

pub(crate) fn click_windows(trace: &Trace) -> impl Iterator<Item = (u64, u64)> + '_ {
    trace.states.windows(2).filter_map(|pair| {
        (!pair[0].value.right_pad_pressed && pair[1].value.right_pad_pressed).then_some((
            pair[1].timestamp_us.saturating_sub(50_000),
            pair[1].timestamp_us.saturating_add(150_000),
        ))
    })
}

pub(crate) struct MotionTimeline<'a> {
    motion: &'a [Motion],
    path_prefix: Vec<u64>,
}

impl<'a> MotionTimeline<'a> {
    pub(crate) fn new(motion: &'a [Motion]) -> Self {
        let mut path_prefix = Vec::with_capacity(motion.len() + 1);
        path_prefix.push(0_u64);
        for item in motion {
            let magnitude = u64::from(item.x.unsigned_abs()) + u64::from(item.y.unsigned_abs());
            path_prefix.push(path_prefix.last().copied().unwrap_or(0) + magnitude);
        }
        Self {
            motion,
            path_prefix,
        }
    }

    pub(crate) fn slice(&self, start_us: u64, end_us: u64) -> &'a [Motion] {
        let start = self
            .motion
            .partition_point(|item| item.timestamp_us < start_us);
        let end = self
            .motion
            .partition_point(|item| item.timestamp_us <= end_us);
        &self.motion[start..end]
    }

    pub(crate) fn magnitude(&self, start_us: u64, end_us: u64) -> u64 {
        let start = self
            .motion
            .partition_point(|item| item.timestamp_us < start_us);
        let end = self
            .motion
            .partition_point(|item| item.timestamp_us <= end_us);
        self.path_prefix[end] - self.path_prefix[start]
    }

    pub(crate) fn total_magnitude(&self) -> u64 {
        self.path_prefix.last().copied().unwrap_or(0)
    }
}

pub(crate) fn angle_error(first: (i64, i64), second: (i64, i64)) -> Option<f64> {
    if first == second && first != (0, 0) {
        return Some(0.0);
    }
    let first_length = (first.0 as f64).hypot(first.1 as f64);
    let second_length = (second.0 as f64).hypot(second.1 as f64);
    if first_length == 0.0 || second_length == 0.0 {
        return None;
    }
    let dot = first.0 as f64 * second.0 as f64 + first.1 as f64 * second.1 as f64;
    let cosine = (dot / (first_length * second_length)).clamp(-1.0, 1.0);
    Some(cosine.acos().to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_band_boundaries_are_exclusive_and_consistent() {
        assert_eq!(SpeedBand::classify(999.0), SpeedBand::Stationary);
        assert_eq!(SpeedBand::classify(1_000.0), SpeedBand::Slow);
        assert_eq!(SpeedBand::classify(15_000.0), SpeedBand::Fast);
    }

    #[test]
    fn motion_timeline_uses_inclusive_ranges_and_prefix_sums() {
        let motion = [
            Motion {
                timestamp_us: 10,
                x: 2,
                y: -3,
            },
            Motion {
                timestamp_us: 20,
                x: 4,
                y: 0,
            },
            Motion {
                timestamp_us: 30,
                x: -1,
                y: 1,
            },
        ];
        let timeline = MotionTimeline::new(&motion);
        assert_eq!(timeline.magnitude(10, 20), 9);
        assert_eq!(timeline.slice(11, 30), &motion[1..]);
        assert_eq!(timeline.total_magnitude(), 11);
    }
}
