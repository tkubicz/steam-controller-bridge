#![allow(clippy::cast_precision_loss)] // Bounded capture counters become JSON f64 metrics.

use std::fs;
use std::path::Path;

use recording::HostPointerEventKind;
use serde::Serialize;

use crate::lizard::metrics::{
    angle_error, click_windows, right_pad_intervals, MotionTimeline, SpeedBand,
    FAST_SPEED_COUNTS_PER_SECOND, STATIONARY_SPEED_COUNTS_PER_SECOND,
};
use crate::lizard::trace::Trace;

#[derive(Debug, Serialize)]
pub(crate) struct AnalysisReport {
    pub(crate) tool_version: &'static str,
    pub(crate) format_event_count: usize,
    pub(crate) unknown_event_count: usize,
    pub(crate) decoded_state_count: usize,
    pub(crate) lizard_mouse_report_count: usize,
    pub(crate) host_pointer_event_count: usize,
    pub(crate) marker_count: usize,
    pub(crate) duration_seconds: f64,
    pub(crate) cadence: Cadence,
    pub(crate) touch_session_count: usize,
    pub(crate) response_latency_us: Distribution,
    pub(crate) stationary_leakage_pixels: u64,
    pub(crate) click_displacement_pixels: u64,
    pub(crate) output_speed_curve: Vec<SpeedResponse>,
    pub(crate) directional_response: DirectionalResponse,
    pub(crate) raw_to_screen: RawToScreen,
    pub(crate) guided_stages: Vec<GuidedStageAnalysis>,
    pub(crate) capture_validity: CaptureValidity,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct Cadence {
    pub(crate) state_hz: f64,
    pub(crate) lizard_hz: f64,
    pub(crate) state_median_interval_us: Option<u64>,
    pub(crate) lizard_median_interval_us: Option<u64>,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct Distribution {
    pub(crate) samples: usize,
    pub(crate) minimum: Option<u64>,
    pub(crate) median: Option<u64>,
    pub(crate) maximum: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SpeedResponse {
    pub(crate) label: &'static str,
    pub(crate) minimum_counts_per_second: f64,
    pub(crate) maximum_counts_per_second: Option<f64>,
    pub(crate) samples: usize,
    pub(crate) input_counts: f64,
    pub(crate) output_pixels: u64,
    pub(crate) pixels_per_thousand_input_counts: Option<f64>,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct DirectionalResponse {
    pub(crate) measured_sessions: usize,
    pub(crate) mean_angular_error_degrees: Option<f64>,
    pub(crate) maximum_angular_error_degrees: Option<f64>,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct RawToScreen {
    pub(crate) available: bool,
    pub(crate) raw_lizard_pixels: u64,
    pub(crate) host_delta_pixels: u64,
    pub(crate) host_to_raw_ratio: Option<f64>,
    pub(crate) unmatched_host_events: usize,
    pub(crate) cursor_edge_clipping_events: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct GuidedStageAnalysis {
    pub(crate) name: String,
    pub(crate) duration_seconds: f64,
    pub(crate) touched_state_samples: usize,
    pub(crate) click_presses: usize,
    pub(crate) input_path_counts: f64,
    pub(crate) reference_motion_reports: usize,
    pub(crate) reference_path_pixels: u64,
    pub(crate) reference_net_x: i64,
    pub(crate) reference_net_y: i64,
    pub(crate) input_counts_per_reference_pixel: Option<f64>,
    pub(crate) host_pointer_events: usize,
    pub(crate) host_path_pixels: u64,
    pub(crate) host_to_reference_ratio: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CaptureValidity {
    pub(crate) metadata_valid: Option<bool>,
    pub(crate) invalid_reason: Option<String>,
    pub(crate) has_state_reports: bool,
    pub(crate) has_lizard_mouse_reports: bool,
    pub(crate) has_host_pointer_events: bool,
}

#[derive(Debug, Clone, Copy)]
struct Session {
    start_us: u64,
    end_us: u64,
    first_x: i16,
    first_y: i16,
    last_x: i16,
    last_y: i16,
}

pub(crate) fn write_report(trace: &Trace, output: &Path) -> Result<(), String> {
    let report = analyze(trace);
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    fs::write(output, bytes)
        .map_err(|error| format!("cannot write report '{}': {error}", output.display()))?;
    println!(
        "Analyzed {} decoded states, {} lizard mouse reports, and {} touch sessions.",
        report.decoded_state_count, report.lizard_mouse_report_count, report.touch_session_count
    );
    if !report.capture_validity.has_host_pointer_events {
        println!("No host_pointer events: raw-to-screen acceleration is unavailable.");
    }
    Ok(())
}

pub(crate) fn analyze(trace: &Trace) -> AnalysisReport {
    let sessions = sessions(trace);
    let motion = trace.reference_motion();
    let timeline = MotionTimeline::new(&motion);
    let start = first_timestamp(trace).unwrap_or(0);
    let end = last_timestamp(trace).unwrap_or(start);
    let duration_seconds = end.saturating_sub(start) as f64 / 1_000_000.0;
    let latencies = sessions
        .iter()
        .filter_map(|session| {
            motion
                .iter()
                .find(|item| (session.start_us..=session.end_us).contains(&item.timestamp_us))
                .map(|item| item.timestamp_us.saturating_sub(session.start_us))
        })
        .collect();
    let final_metadata = trace.metadata.last().map(|item| &item.value);
    AnalysisReport {
        tool_version: env!("CARGO_PKG_VERSION"),
        format_event_count: trace.event_count,
        unknown_event_count: trace.unknown_event_count,
        decoded_state_count: trace.states.len(),
        lizard_mouse_report_count: trace.lizard.len(),
        host_pointer_event_count: trace.host_pointer.len(),
        marker_count: trace.markers.len(),
        duration_seconds,
        cadence: Cadence {
            state_hz: rate(trace.states.len(), duration_seconds),
            lizard_hz: rate(trace.lizard.len(), duration_seconds),
            state_median_interval_us: median_interval(
                &trace
                    .states
                    .iter()
                    .map(|item| item.timestamp_us)
                    .collect::<Vec<_>>(),
            ),
            lizard_median_interval_us: median_interval(
                &trace
                    .lizard
                    .iter()
                    .map(|item| item.timestamp_us)
                    .collect::<Vec<_>>(),
            ),
        },
        touch_session_count: sessions.len(),
        response_latency_us: distribution(latencies),
        stationary_leakage_pixels: stationary_leakage(trace, &timeline),
        click_displacement_pixels: click_displacement(trace, &timeline),
        output_speed_curve: speed_curve(trace, &timeline),
        directional_response: directional_response(&sessions, &timeline),
        raw_to_screen: raw_to_screen(trace, &sessions, &timeline),
        guided_stages: guided_stage_analysis(trace, &timeline),
        capture_validity: CaptureValidity {
            metadata_valid: final_metadata.and_then(|metadata| metadata.valid),
            invalid_reason: final_metadata.and_then(|metadata| metadata.invalid_reason.clone()),
            has_state_reports: !trace.states.is_empty(),
            has_lizard_mouse_reports: !trace.lizard.is_empty(),
            has_host_pointer_events: !trace.host_pointer.is_empty(),
        },
    }
}

fn guided_stage_analysis(trace: &Trace, timeline: &MotionTimeline<'_>) -> Vec<GuidedStageAnalysis> {
    trace
        .guided_stages()
        .into_iter()
        .map(|stage| {
            let states = stage.timed_slice(&trace.states);
            let input_path_counts = states
                .windows(2)
                .filter(|pair| pair[0].value.right_pad_touched && pair[1].value.right_pad_touched)
                .map(|pair| {
                    let dx =
                        f64::from(pair[1].value.right_pad_x) - f64::from(pair[0].value.right_pad_x);
                    let dy =
                        f64::from(pair[1].value.right_pad_y) - f64::from(pair[0].value.right_pad_y);
                    dx.hypot(dy)
                })
                .sum::<f64>();
            let click_presses = states
                .windows(2)
                .filter(|pair| !pair[0].value.right_pad_pressed && pair[1].value.right_pad_pressed)
                .count();
            let reference = timeline.slice(stage.start_us, stage.end_us);
            let reference_path_pixels = timeline.magnitude(stage.start_us, stage.end_us);
            let (reference_net_x, reference_net_y) =
                reference.iter().fold((0_i64, 0_i64), |(x, y), item| {
                    (x + i64::from(item.x), y + i64::from(item.y))
                });
            let host = stage.timed_slice(&trace.host_pointer);
            let host_path_pixels = host
                .iter()
                .map(|item| item.value.delta_x.unsigned_abs() + item.value.delta_y.unsigned_abs())
                .sum();
            GuidedStageAnalysis {
                name: stage.name,
                duration_seconds: stage.end_us.saturating_sub(stage.start_us) as f64 / 1_000_000.0,
                touched_state_samples: states
                    .iter()
                    .filter(|item| item.value.right_pad_touched)
                    .count(),
                click_presses,
                input_path_counts,
                reference_motion_reports: reference.len(),
                reference_path_pixels,
                reference_net_x,
                reference_net_y,
                input_counts_per_reference_pixel: (reference_path_pixels > 0)
                    .then_some(input_path_counts / reference_path_pixels as f64),
                host_pointer_events: host.len(),
                host_path_pixels,
                host_to_reference_ratio: (reference_path_pixels > 0)
                    .then_some(host_path_pixels as f64 / reference_path_pixels as f64),
            }
        })
        .collect()
}

fn sessions(trace: &Trace) -> Vec<Session> {
    let mut result = Vec::new();
    let mut open: Option<Session> = None;
    for state in &trace.states {
        if state.value.right_pad_touched {
            let session = open.get_or_insert(Session {
                start_us: state.timestamp_us,
                end_us: state.timestamp_us,
                first_x: state.value.right_pad_x,
                first_y: state.value.right_pad_y,
                last_x: state.value.right_pad_x,
                last_y: state.value.right_pad_y,
            });
            session.end_us = state.timestamp_us;
            session.last_x = state.value.right_pad_x;
            session.last_y = state.value.right_pad_y;
        } else if let Some(mut session) = open.take() {
            session.end_us = state.timestamp_us;
            result.push(session);
        }
    }
    if let Some(session) = open {
        result.push(session);
    }
    result
}

fn stationary_leakage(trace: &Trace, timeline: &MotionTimeline<'_>) -> u64 {
    right_pad_intervals(trace)
        .filter(|interval| {
            SpeedBand::classify(interval.speed_counts_per_second) == SpeedBand::Stationary
        })
        .map(|interval| timeline.magnitude(interval.start_us, interval.end_us))
        .sum()
}

fn click_displacement(trace: &Trace, timeline: &MotionTimeline<'_>) -> u64 {
    click_windows(trace)
        .map(|(start, end)| timeline.magnitude(start, end))
        .sum()
}

fn speed_curve(trace: &Trace, timeline: &MotionTimeline<'_>) -> Vec<SpeedResponse> {
    let mut bins = vec![
        SpeedResponse {
            label: "stationary_precision",
            minimum_counts_per_second: 0.0,
            maximum_counts_per_second: Some(STATIONARY_SPEED_COUNTS_PER_SECOND),
            samples: 0,
            input_counts: 0.0,
            output_pixels: 0,
            pixels_per_thousand_input_counts: None,
        },
        SpeedResponse {
            label: "slow",
            minimum_counts_per_second: STATIONARY_SPEED_COUNTS_PER_SECOND,
            maximum_counts_per_second: Some(FAST_SPEED_COUNTS_PER_SECOND),
            samples: 0,
            input_counts: 0.0,
            output_pixels: 0,
            pixels_per_thousand_input_counts: None,
        },
        SpeedResponse {
            label: "fast",
            minimum_counts_per_second: FAST_SPEED_COUNTS_PER_SECOND,
            maximum_counts_per_second: None,
            samples: 0,
            input_counts: 0.0,
            output_pixels: 0,
            pixels_per_thousand_input_counts: None,
        },
    ];
    for interval in right_pad_intervals(trace) {
        let index = match SpeedBand::classify(interval.speed_counts_per_second) {
            SpeedBand::Stationary => 0,
            SpeedBand::Slow => 1,
            SpeedBand::Fast => 2,
        };
        bins[index].samples += 1;
        bins[index].input_counts += interval.distance_counts;
        bins[index].output_pixels += timeline.magnitude(interval.start_us, interval.end_us);
    }
    for bin in &mut bins {
        if bin.input_counts > 0.0 {
            bin.pixels_per_thousand_input_counts =
                Some(bin.output_pixels as f64 * 1_000.0 / bin.input_counts);
        }
    }
    bins
}

fn directional_response(
    sessions: &[Session],
    timeline: &MotionTimeline<'_>,
) -> DirectionalResponse {
    let mut errors = Vec::new();
    for session in sessions {
        let input = (
            i64::from(session.last_x) - i64::from(session.first_x),
            i64::from(session.last_y) - i64::from(session.first_y),
        );
        let output = timeline
            .slice(session.start_us, session.end_us)
            .iter()
            .fold((0_i64, 0_i64), |point, item| {
                (point.0 + i64::from(item.x), point.1 + i64::from(item.y))
            });
        if let Some(error) = angle_error(input, output) {
            errors.push(error);
        }
    }
    DirectionalResponse {
        measured_sessions: errors.len(),
        mean_angular_error_degrees: (!errors.is_empty())
            .then(|| errors.iter().sum::<f64>() / errors.len() as f64),
        maximum_angular_error_degrees: errors.into_iter().reduce(f64::max),
    }
}

fn raw_to_screen(
    trace: &Trace,
    sessions: &[Session],
    timeline: &MotionTimeline<'_>,
) -> RawToScreen {
    if trace.host_pointer.is_empty() {
        return RawToScreen::default();
    }
    let raw = timeline.total_magnitude();
    let host = trace
        .host_pointer
        .iter()
        .map(|item| item.value.delta_x.unsigned_abs() + item.value.delta_y.unsigned_abs())
        .sum();
    let unmatched = trace
        .host_pointer
        .iter()
        .filter(|item| {
            !sessions
                .iter()
                .any(|session| (session.start_us..=session.end_us).contains(&item.timestamp_us))
        })
        .count();
    let clipping = trace
        .host_pointer
        .windows(2)
        .filter(|pair| {
            let current = &pair[1].value;
            matches!(
                current.event_kind,
                HostPointerEventKind::Moved
                    | HostPointerEventKind::LeftDragged
                    | HostPointerEventKind::RightDragged
                    | HostPointerEventKind::OtherDragged
            ) && (current.delta_x != 0 || current.delta_y != 0)
                && (current.location_x - pair[0].value.location_x).abs() <= f64::EPSILON
                && (current.location_y - pair[0].value.location_y).abs() <= f64::EPSILON
        })
        .count();
    RawToScreen {
        available: true,
        raw_lizard_pixels: raw,
        host_delta_pixels: host,
        host_to_raw_ratio: (raw > 0).then_some(host as f64 / raw as f64),
        unmatched_host_events: unmatched,
        cursor_edge_clipping_events: clipping,
    }
}

fn rate(count: usize, duration_seconds: f64) -> f64 {
    if duration_seconds > 0.0 {
        count as f64 / duration_seconds
    } else {
        0.0
    }
}

fn median_interval(timestamps: &[u64]) -> Option<u64> {
    let mut intervals: Vec<_> = timestamps
        .windows(2)
        .map(|pair| pair[1].saturating_sub(pair[0]))
        .collect();
    intervals.sort_unstable();
    intervals.get(intervals.len() / 2).copied()
}

fn distribution(mut values: Vec<u64>) -> Distribution {
    values.sort_unstable();
    Distribution {
        samples: values.len(),
        minimum: values.first().copied(),
        median: values.get(values.len() / 2).copied(),
        maximum: values.last().copied(),
    }
}

fn first_timestamp(trace: &Trace) -> Option<u64> {
    trace
        .states
        .first()
        .map(|item| item.timestamp_us)
        .into_iter()
        .chain(trace.lizard.first().map(|item| item.timestamp_us))
        .chain(trace.host_pointer.first().map(|item| item.timestamp_us))
        .min()
}

fn last_timestamp(trace: &Trace) -> Option<u64> {
    trace
        .states
        .last()
        .map(|item| item.timestamp_us)
        .into_iter()
        .chain(trace.lizard.last().map(|item| item.timestamp_us))
        .chain(trace.host_pointer.last().map(|item| item.timestamp_us))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lizard::trace::Motion;
    use recording::HostPointerEvent;
    use steam_controller_protocol::{SteamButtons, SteamControllerState};

    fn state(
        timestamp_us: u64,
        touched: bool,
        x: i16,
        pressed: bool,
    ) -> crate::lizard::trace::Timed<SteamControllerState> {
        crate::lizard::trace::Timed {
            timestamp_us,
            value: SteamControllerState {
                report_id: 0x42,
                sequence: 0,
                buttons: SteamButtons::default(),
                left_trigger: 0,
                right_trigger: 0,
                left_stick_x: 0,
                left_stick_y: 0,
                right_stick_x: 0,
                right_stick_y: 0,
                left_pad_x: 0,
                left_pad_y: 0,
                left_pad_pressure: 0,
                left_pad_touched: false,
                left_pad_pressed: false,
                right_pad_x: x,
                right_pad_y: 0,
                right_pad_pressure: 0,
                right_pad_touched: touched,
                right_pad_pressed: pressed,
                left_grip_touched: false,
                right_grip_touched: false,
                imu_timestamp: 0,
                gyro: None,
                acceleration: None,
                raw_report: Vec::new(),
            },
        }
    }

    #[test]
    fn segments_touch_sessions_and_measures_click_window() {
        let trace = Trace {
            states: vec![
                state(0, false, 0, false),
                state(10_000, true, 0, false),
                state(20_000, true, 100, true),
                state(30_000, false, 0, false),
                state(40_000, true, 0, false),
            ],
            ..Trace::default()
        };
        assert_eq!(sessions(&trace).len(), 2);
        let motion = vec![Motion {
            timestamp_us: 20_000,
            x: 3,
            y: -4,
        }];
        assert_eq!(click_displacement(&trace, &MotionTimeline::new(&motion)), 7);
    }

    #[test]
    fn host_analysis_flags_external_motion_and_cursor_edge_clipping() {
        let trace = Trace {
            states: vec![state(10, true, 0, false), state(20, false, 0, false)],
            host_pointer: vec![
                crate::lizard::trace::Timed {
                    timestamp_us: 5,
                    value: HostPointerEvent {
                        event_kind: HostPointerEventKind::Moved,
                        delta_x: 4,
                        delta_y: 0,
                        location_x: 100.0,
                        location_y: 100.0,
                        scroll_x: 0,
                        scroll_y: 0,
                    },
                },
                crate::lizard::trace::Timed {
                    timestamp_us: 15,
                    value: HostPointerEvent {
                        event_kind: HostPointerEventKind::Moved,
                        delta_x: 3,
                        delta_y: 0,
                        location_x: 100.0,
                        location_y: 100.0,
                        scroll_x: 0,
                        scroll_y: 0,
                    },
                },
            ],
            ..Trace::default()
        };
        let result = raw_to_screen(&trace, &sessions(&trace), &MotionTimeline::new(&[]));
        assert_eq!(result.unmatched_host_events, 1);
        assert_eq!(result.cursor_edge_clipping_events, 1);
    }

    #[test]
    fn stationary_noise_and_fast_swipe_land_in_distinct_speed_bins() {
        let trace = Trace {
            states: vec![
                state(0, true, 0, false),
                state(1_000_000, true, 500, false),
                state(2_000_000, true, 20_500, false),
            ],
            ..Trace::default()
        };
        let motion = vec![
            Motion {
                timestamp_us: 500_000,
                x: 2,
                y: 0,
            },
            Motion {
                timestamp_us: 1_500_000,
                x: 20,
                y: 0,
            },
        ];
        let timeline = MotionTimeline::new(&motion);
        assert_eq!(stationary_leakage(&trace, &timeline), 2);
        let bins = speed_curve(&trace, &timeline);
        assert_eq!(bins[0].samples, 1);
        assert_eq!(bins[0].output_pixels, 2);
        assert_eq!(bins[2].samples, 1);
        assert_eq!(bins[2].output_pixels, 20);
    }

    #[test]
    fn guided_analysis_uses_marker_boundaries() {
        let trace = Trace {
            states: vec![state(10, true, 0, false), state(20, true, 128, true)],
            host_pointer: vec![crate::lizard::trace::Timed {
                timestamp_us: 20,
                value: HostPointerEvent {
                    event_kind: HostPointerEventKind::Moved,
                    delta_x: 1,
                    delta_y: 0,
                    location_x: 101.0,
                    location_y: 100.0,
                    scroll_x: 0,
                    scroll_y: 0,
                },
            }],
            markers: vec![
                crate::lizard::trace::Marker {
                    timestamp_us: 5,
                    name: "precision".to_owned(),
                    phase: Some(crate::lizard::trace::MarkerPhase::Start),
                    protocol: None,
                    trial_id: None,
                    attempt: None,
                },
                crate::lizard::trace::Marker {
                    timestamp_us: 25,
                    name: "precision".to_owned(),
                    phase: Some(crate::lizard::trace::MarkerPhase::End),
                    protocol: None,
                    trial_id: None,
                    attempt: None,
                },
            ],
            ..Trace::default()
        };
        let motion = [Motion {
            timestamp_us: 20,
            x: 1,
            y: 0,
        }];
        let stages = guided_stage_analysis(&trace, &MotionTimeline::new(&motion));
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].name, "precision");
        assert_eq!(stages[0].click_presses, 1);
        assert!((stages[0].input_path_counts - 128.0).abs() < f64::EPSILON);
        assert_eq!(stages[0].reference_path_pixels, 1);
        assert_eq!(stages[0].host_path_pixels, 1);
    }
}
