#![allow(clippy::cast_precision_loss)] // Cumulative integer coordinates become f64 metrics.

use std::fs;
use std::path::Path;
use std::time::Duration;

use desktop_bindings::{
    load_store, BindingEngine, BindingProfile, ControlBindings, DesktopInputSink, KeyboardKey,
    Modifier, MouseButton,
};
use serde::Serialize;

use crate::lizard::metrics::{
    angle_error, click_windows, right_pad_intervals, MotionTimeline, SpeedBand,
    FAST_SPEED_COUNTS_PER_SECOND, STATIONARY_SPEED_COUNTS_PER_SECOND,
};
use crate::lizard::trace::{snapshot, Motion, Trace};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Leakage {
    pub(crate) reference_pixels: u64,
    pub(crate) bridge_pixels: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ComparisonReport {
    pub(crate) tool_version: &'static str,
    pub(crate) profile_name: String,
    pub(crate) state_count: usize,
    pub(crate) reference_motion_count: usize,
    pub(crate) bridge_motion_count: usize,
    pub(crate) endpoint_error: PointError,
    pub(crate) rms_path_error_pixels: f64,
    pub(crate) angular_error_degrees: Option<f64>,
    pub(crate) latency_error_us: Option<i64>,
    pub(crate) stationary_leakage: Leakage,
    pub(crate) click_leakage: Leakage,
    pub(crate) speed_bin_response: Vec<SpeedBin>,
    pub(crate) guided_summary: Option<GuidedComparisonSummary>,
    pub(crate) guided_stages: Vec<GuidedStageComparison>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct PointError {
    pub(crate) x: i64,
    pub(crate) y: i64,
    pub(crate) distance_pixels: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SpeedBin {
    pub(crate) label: &'static str,
    pub(crate) minimum_counts_per_second: f64,
    pub(crate) maximum_counts_per_second: Option<f64>,
    pub(crate) samples: usize,
    pub(crate) reference_pixels: u64,
    pub(crate) bridge_pixels: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct GuidedStageComparison {
    pub(crate) name: String,
    pub(crate) state_samples: usize,
    pub(crate) reference_motion_reports: usize,
    pub(crate) bridge_motion_reports: usize,
    pub(crate) reference_path_pixels: u64,
    pub(crate) bridge_path_pixels: u64,
    pub(crate) bridge_to_reference_ratio: Option<f64>,
    pub(crate) endpoint_error: PointError,
    pub(crate) rms_path_error_pixels: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct GuidedComparisonSummary {
    pub(crate) stage_count: usize,
    pub(crate) state_samples: usize,
    pub(crate) reference_motion_reports: usize,
    pub(crate) bridge_motion_reports: usize,
    pub(crate) reference_path_pixels: u64,
    pub(crate) bridge_path_pixels: u64,
    pub(crate) bridge_to_reference_ratio: Option<f64>,
    pub(crate) rms_path_error_pixels: f64,
}

pub(crate) fn write_report(
    trace: &Trace,
    output: &Path,
    profile_path: Option<&Path>,
    profile_name: Option<&str>,
) -> Result<(), String> {
    if trace.states.is_empty() {
        return Err("capture has no decoded controller states".to_owned());
    }
    if trace.lizard.is_empty() {
        return Err("capture has no decoded 0x40 lizard mouse reports".to_owned());
    }
    let profile = load_profile(profile_path, profile_name)?;
    let candidate = bridge_motion(trace, profile.clone())?;
    let report = build_report(trace, &candidate, profile.name);
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    fs::write(output, bytes)
        .map_err(|error| format!("cannot write report '{}': {error}", output.display()))?;
    if let Some(guided) = &report.guided_summary {
        println!(
            "Compared {} states and {} reference reports; guided RMS error {:.3} px across {} stages.",
            report.state_count,
            report.reference_motion_count,
            guided.rms_path_error_pixels,
            guided.stage_count
        );
    } else {
        println!(
            "Compared {} states and {} reference reports; RMS error {:.3} px.",
            report.state_count, report.reference_motion_count, report.rms_path_error_pixels
        );
    }
    Ok(())
}

pub(crate) fn load_profile(
    profile_path: Option<&Path>,
    profile_name: Option<&str>,
) -> Result<BindingProfile, String> {
    let profile = if let Some(path) = profile_path {
        let store = load_store(path)?;
        let name = profile_name.ok_or("--profile-name is required with --profile")?;
        store
            .profile_by_name(name)
            .cloned()
            .ok_or_else(|| format!("profile {name:?} does not exist in '{}'", path.display()))?
    } else {
        BindingProfile::default()
    };
    Ok(mouse_only_profile(profile))
}

pub(crate) fn mouse_only_profile(mut profile: BindingProfile) -> BindingProfile {
    profile.bindings = ControlBindings::default();
    profile.pads.left_scroll.enabled = false;
    profile.pads.right_mouse.enabled = true;
    profile.pads.left_scroll.feedback.enabled = false;
    profile.pads.right_mouse.feedback.enabled = false;
    profile
}

pub(crate) fn bridge_motion(trace: &Trace, profile: BindingProfile) -> Result<Vec<Motion>, String> {
    let mut sink = MotionSink::default();
    let mut engine = BindingEngine::new(profile);
    for item in &trace.states {
        sink.timestamp_us = item.timestamp_us;
        engine.observe_snapshot(
            snapshot(&item.value),
            Duration::from_micros(item.timestamp_us),
            &mut sink,
        )?;
    }
    Ok(sink.motion)
}

pub(crate) fn compare_with_profile(
    trace: &Trace,
    profile: BindingProfile,
) -> Result<(ComparisonReport, Vec<Motion>), String> {
    let profile_name = profile.name.clone();
    let candidate = bridge_motion(trace, profile)?;
    let report = build_report(trace, &candidate, profile_name);
    Ok((report, candidate))
}

#[derive(Default)]
struct MotionSink {
    timestamp_us: u64,
    motion: Vec<Motion>,
}

impl DesktopInputSink for MotionSink {
    fn key(&mut self, _key: KeyboardKey, _pressed: bool) -> Result<(), String> {
        Ok(())
    }

    fn modifier(&mut self, _modifier: Modifier, _pressed: bool) -> Result<(), String> {
        Ok(())
    }

    fn mouse_button(&mut self, _button: MouseButton, _pressed: bool) -> Result<(), String> {
        Ok(())
    }

    fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
        if x != 0 || y != 0 {
            self.motion.push(Motion {
                timestamp_us: self.timestamp_us,
                x,
                y,
            });
        }
        Ok(())
    }

    fn scroll(&mut self, _x: i32, _y: i32) -> Result<(), String> {
        Ok(())
    }
}

pub(crate) fn build_report(
    trace: &Trace,
    candidate: &[Motion],
    profile_name: String,
) -> ComparisonReport {
    let reference = trace.reference_motion();
    let sample_times: Vec<_> = trace
        .states
        .iter()
        .map(|state| state.timestamp_us)
        .collect();
    let metrics = trajectory_metrics(&sample_times, &reference, candidate);
    let reference_timeline = MotionTimeline::new(&reference);
    let candidate_timeline = MotionTimeline::new(candidate);
    let guided_stages = guided_stage_comparison(trace, &reference_timeline, &candidate_timeline);
    let guided_summary = guided_summary(&guided_stages);
    ComparisonReport {
        tool_version: env!("CARGO_PKG_VERSION"),
        profile_name,
        state_count: trace.states.len(),
        reference_motion_count: reference.len(),
        bridge_motion_count: candidate.len(),
        endpoint_error: metrics.endpoint_error,
        rms_path_error_pixels: metrics.rms_path_error_pixels,
        angular_error_degrees: metrics.angular_error_degrees,
        latency_error_us: latency_error(trace, &reference, candidate),
        stationary_leakage: leakage_for_stationary(trace, &reference_timeline, &candidate_timeline),
        click_leakage: leakage_for_clicks(trace, &reference_timeline, &candidate_timeline),
        speed_bin_response: speed_bins(trace, &reference_timeline, &candidate_timeline),
        guided_summary,
        guided_stages,
    }
}

fn guided_summary(stages: &[GuidedStageComparison]) -> Option<GuidedComparisonSummary> {
    if stages.is_empty() {
        return None;
    }
    let state_samples = stages
        .iter()
        .map(|stage| stage.state_samples)
        .sum::<usize>();
    let reference_path_pixels = stages
        .iter()
        .map(|stage| stage.reference_path_pixels)
        .sum::<u64>();
    let bridge_path_pixels = stages
        .iter()
        .map(|stage| stage.bridge_path_pixels)
        .sum::<u64>();
    let squared_error = stages
        .iter()
        .map(|stage| stage.rms_path_error_pixels.powi(2) * stage.state_samples as f64)
        .sum::<f64>();
    Some(GuidedComparisonSummary {
        stage_count: stages.len(),
        state_samples,
        reference_motion_reports: stages
            .iter()
            .map(|stage| stage.reference_motion_reports)
            .sum(),
        bridge_motion_reports: stages.iter().map(|stage| stage.bridge_motion_reports).sum(),
        reference_path_pixels,
        bridge_path_pixels,
        bridge_to_reference_ratio: (reference_path_pixels > 0)
            .then_some(bridge_path_pixels as f64 / reference_path_pixels as f64),
        rms_path_error_pixels: if state_samples == 0 {
            0.0
        } else {
            (squared_error / state_samples as f64).sqrt()
        },
    })
}

fn guided_stage_comparison(
    trace: &Trace,
    reference: &MotionTimeline<'_>,
    candidate: &MotionTimeline<'_>,
) -> Vec<GuidedStageComparison> {
    trace
        .guided_stages()
        .into_iter()
        .map(|stage| {
            let sample_times: Vec<_> = stage
                .timed_slice(&trace.states)
                .iter()
                .map(|item| item.timestamp_us)
                .collect();
            let reference_motion = reference.slice(stage.start_us, stage.end_us);
            let candidate_motion = candidate.slice(stage.start_us, stage.end_us);
            let reference_path_pixels = reference.magnitude(stage.start_us, stage.end_us);
            let bridge_path_pixels = candidate.magnitude(stage.start_us, stage.end_us);
            let metrics = trajectory_metrics(&sample_times, reference_motion, candidate_motion);
            GuidedStageComparison {
                name: stage.name,
                state_samples: sample_times.len(),
                reference_motion_reports: reference_motion.len(),
                bridge_motion_reports: candidate_motion.len(),
                reference_path_pixels,
                bridge_path_pixels,
                bridge_to_reference_ratio: (reference_path_pixels > 0)
                    .then_some(bridge_path_pixels as f64 / reference_path_pixels as f64),
                endpoint_error: metrics.endpoint_error,
                rms_path_error_pixels: metrics.rms_path_error_pixels,
            }
        })
        .collect()
}

struct TrajectoryMetrics {
    endpoint_error: PointError,
    rms_path_error_pixels: f64,
    angular_error_degrees: Option<f64>,
}

fn trajectory_metrics(
    sample_times: &[u64],
    reference: &[Motion],
    candidate: &[Motion],
) -> TrajectoryMetrics {
    let mut reference_index = 0;
    let mut candidate_index = 0;
    let mut reference_point = (0_i64, 0_i64);
    let mut candidate_point = (0_i64, 0_i64);
    let mut squared_error = 0.0;
    for timestamp in sample_times {
        accumulate_to(
            reference,
            &mut reference_index,
            *timestamp,
            &mut reference_point,
        );
        accumulate_to(
            candidate,
            &mut candidate_index,
            *timestamp,
            &mut candidate_point,
        );
        let dx = candidate_point.0 - reference_point.0;
        let dy = candidate_point.1 - reference_point.1;
        squared_error += (dx * dx + dy * dy) as f64;
    }
    if let Some(last) = sample_times.last() {
        accumulate_to(reference, &mut reference_index, *last, &mut reference_point);
        accumulate_to(candidate, &mut candidate_index, *last, &mut candidate_point);
    }
    let dx = candidate_point.0 - reference_point.0;
    let dy = candidate_point.1 - reference_point.1;
    TrajectoryMetrics {
        endpoint_error: PointError {
            x: dx,
            y: dy,
            distance_pixels: (dx as f64).hypot(dy as f64),
        },
        rms_path_error_pixels: if sample_times.is_empty() {
            0.0
        } else {
            (squared_error / sample_times.len() as f64).sqrt()
        },
        angular_error_degrees: angle_error(reference_point, candidate_point),
    }
}

fn accumulate_to(motion: &[Motion], index: &mut usize, timestamp_us: u64, point: &mut (i64, i64)) {
    while motion
        .get(*index)
        .is_some_and(|item| item.timestamp_us <= timestamp_us)
    {
        let item = &motion[*index];
        point.0 += i64::from(item.x);
        point.1 += i64::from(item.y);
        *index += 1;
    }
}

fn latency_error(trace: &Trace, reference: &[Motion], candidate: &[Motion]) -> Option<i64> {
    let touch_start = trace
        .states
        .windows(2)
        .find(|pair| !pair[0].value.right_pad_touched && pair[1].value.right_pad_touched)
        .map(|pair| pair[1].timestamp_us)
        .or_else(|| {
            trace
                .states
                .first()?
                .value
                .right_pad_touched
                .then_some(trace.states[0].timestamp_us)
        })?;
    let reference_start = reference
        .iter()
        .find(|item| item.timestamp_us >= touch_start)?;
    let candidate_start = candidate
        .iter()
        .find(|item| item.timestamp_us >= touch_start)?;
    let delta = i128::from(candidate_start.timestamp_us) - i128::from(reference_start.timestamp_us);
    i64::try_from(delta).ok()
}

fn leakage_for_stationary(
    trace: &Trace,
    reference: &MotionTimeline<'_>,
    candidate: &MotionTimeline<'_>,
) -> Leakage {
    let windows = right_pad_intervals(trace)
        .filter(|interval| {
            SpeedBand::classify(interval.speed_counts_per_second) == SpeedBand::Stationary
        })
        .map(|interval| (interval.start_us, interval.end_us));
    leakage_for_windows(windows, reference, candidate)
}

fn leakage_for_clicks(
    trace: &Trace,
    reference: &MotionTimeline<'_>,
    candidate: &MotionTimeline<'_>,
) -> Leakage {
    leakage_for_windows(click_windows(trace), reference, candidate)
}

fn leakage_for_windows(
    windows: impl Iterator<Item = (u64, u64)>,
    reference: &MotionTimeline<'_>,
    candidate: &MotionTimeline<'_>,
) -> Leakage {
    let mut leakage = Leakage::default();
    for (start, end) in windows {
        leakage.reference_pixels += reference.magnitude(start, end);
        leakage.bridge_pixels += candidate.magnitude(start, end);
    }
    leakage
}

fn speed_bins(
    trace: &Trace,
    reference: &MotionTimeline<'_>,
    candidate: &MotionTimeline<'_>,
) -> Vec<SpeedBin> {
    let mut bins = vec![
        SpeedBin {
            label: "stationary_precision",
            minimum_counts_per_second: 0.0,
            maximum_counts_per_second: Some(STATIONARY_SPEED_COUNTS_PER_SECOND),
            samples: 0,
            reference_pixels: 0,
            bridge_pixels: 0,
        },
        SpeedBin {
            label: "slow",
            minimum_counts_per_second: STATIONARY_SPEED_COUNTS_PER_SECOND,
            maximum_counts_per_second: Some(FAST_SPEED_COUNTS_PER_SECOND),
            samples: 0,
            reference_pixels: 0,
            bridge_pixels: 0,
        },
        SpeedBin {
            label: "fast",
            minimum_counts_per_second: FAST_SPEED_COUNTS_PER_SECOND,
            maximum_counts_per_second: None,
            samples: 0,
            reference_pixels: 0,
            bridge_pixels: 0,
        },
    ];
    for interval in right_pad_intervals(trace) {
        let index = match SpeedBand::classify(interval.speed_counts_per_second) {
            SpeedBand::Stationary => 0,
            SpeedBand::Slow => 1,
            SpeedBand::Fast => 2,
        };
        let bin = &mut bins[index];
        bin.samples += 1;
        bin.reference_pixels += reference.magnitude(interval.start_us, interval.end_us);
        bin.bridge_pixels += candidate.magnitude(interval.start_us, interval.end_us);
    }
    bins
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_traces_have_zero_error() {
        let motion = vec![
            Motion {
                timestamp_us: 10,
                x: 2,
                y: 3,
            },
            Motion {
                timestamp_us: 20,
                x: -1,
                y: 4,
            },
        ];
        let metrics = trajectory_metrics(&[10, 20], &motion, &motion);
        assert_eq!(metrics.endpoint_error.x, 0);
        assert_eq!(metrics.endpoint_error.y, 0);
        assert!(metrics.rms_path_error_pixels.abs() < f64::EPSILON);
        assert_eq!(metrics.angular_error_degrees, Some(0.0));
    }

    #[test]
    fn controlled_offset_has_exact_endpoint_and_rms_error() {
        let reference = vec![Motion {
            timestamp_us: 10,
            x: 2,
            y: 0,
        }];
        let candidate = vec![Motion {
            timestamp_us: 10,
            x: 5,
            y: 4,
        }];
        let metrics = trajectory_metrics(&[10, 20], &reference, &candidate);
        assert_eq!((metrics.endpoint_error.x, metrics.endpoint_error.y), (3, 4));
        assert!((metrics.endpoint_error.distance_pixels - 5.0).abs() < f64::EPSILON);
        assert!((metrics.rms_path_error_pixels - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn guided_summary_resets_trajectory_at_stage_boundaries() {
        let trace = Trace {
            states: vec![],
            markers: vec![
                crate::lizard::trace::Marker {
                    timestamp_us: 5,
                    name: "first".to_owned(),
                    phase: Some(crate::lizard::trace::MarkerPhase::Start),
                    protocol: None,
                    trial_id: None,
                    attempt: None,
                },
                crate::lizard::trace::Marker {
                    timestamp_us: 15,
                    name: "first".to_owned(),
                    phase: Some(crate::lizard::trace::MarkerPhase::End),
                    protocol: None,
                    trial_id: None,
                    attempt: None,
                },
            ],
            ..Trace::default()
        };
        let reference = [Motion {
            timestamp_us: 10,
            x: 2,
            y: 0,
        }];
        let candidate = [Motion {
            timestamp_us: 10,
            x: 2,
            y: 0,
        }];
        let stages = guided_stage_comparison(
            &trace,
            &MotionTimeline::new(&reference),
            &MotionTimeline::new(&candidate),
        );
        let summary = guided_summary(&stages).unwrap();
        assert_eq!(summary.stage_count, 1);
        assert_eq!(summary.reference_path_pixels, 2);
        assert_eq!(summary.bridge_path_pixels, 2);
        assert!(summary.rms_path_error_pixels.abs() < f64::EPSILON);
    }
}
