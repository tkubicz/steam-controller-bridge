use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use desktop_bindings::DesktopInputSnapshot;
use recording::{
    read_events, CaptureMetadata, HostPointerEvent, KIND_CAPTURE_METADATA,
    KIND_DECODED_LIZARD_MOUSE, KIND_DECODED_STEAM_STATE, KIND_HOST_POINTER, KIND_MARKER,
    KIND_RAW_HID,
};
use steam_controller_protocol::{
    DecodedReport, LizardMouseReport, SteamControllerDecoder, SteamControllerState,
    EXTENDED_INPUT_REPORT_ID, INPUT_REPORT_ID, LIZARD_MOUSE_REPORT_ID,
};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Timed<T> {
    pub(crate) timestamp_us: u64,
    pub(crate) value: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Motion {
    pub(crate) timestamp_us: u64,
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Marker {
    pub(crate) timestamp_us: u64,
    pub(crate) name: String,
    pub(crate) phase: Option<MarkerPhase>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkerPhase {
    Start,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuidedStage {
    pub(crate) name: String,
    pub(crate) start_us: u64,
    pub(crate) end_us: u64,
}

impl GuidedStage {
    pub(crate) fn timed_slice<'a, T>(&self, items: &'a [Timed<T>]) -> &'a [Timed<T>] {
        let start = items.partition_point(|item| item.timestamp_us < self.start_us);
        let end = items.partition_point(|item| item.timestamp_us <= self.end_us);
        &items[start..end]
    }
}

#[derive(Debug, Default)]
pub(crate) struct Trace {
    pub(crate) states: Vec<Timed<SteamControllerState>>,
    pub(crate) lizard: Vec<Timed<LizardMouseReport>>,
    pub(crate) host_pointer: Vec<Timed<HostPointerEvent>>,
    pub(crate) metadata: Vec<Timed<CaptureMetadata>>,
    pub(crate) markers: Vec<Marker>,
    pub(crate) event_count: usize,
    pub(crate) unknown_event_count: usize,
}

impl Trace {
    pub(crate) fn read(path: &Path) -> Result<Self, String> {
        let file = File::open(path)
            .map_err(|error| format!("cannot open capture '{}': {error}", path.display()))?;
        let events = read_events(BufReader::new(file)).map_err(|error| error.to_string())?;
        let has_typed_states = events
            .iter()
            .any(|event| event.kind == KIND_DECODED_STEAM_STATE);
        let has_typed_lizard = events
            .iter()
            .any(|event| event.kind == KIND_DECODED_LIZARD_MOUSE);
        let mut trace = Self {
            event_count: events.len(),
            ..Self::default()
        };
        let mut decoder = SteamControllerDecoder::new();
        for event in events {
            match event.kind.as_str() {
                KIND_DECODED_STEAM_STATE => trace.states.push(Timed {
                    timestamp_us: event.timestamp_us,
                    value: event
                        .decode_steam_state()
                        .map_err(|error| error.to_string())?,
                }),
                KIND_DECODED_LIZARD_MOUSE => trace.lizard.push(Timed {
                    timestamp_us: event.timestamp_us,
                    value: event
                        .decode_lizard_mouse()
                        .map_err(|error| error.to_string())?,
                }),
                KIND_HOST_POINTER => trace.host_pointer.push(Timed {
                    timestamp_us: event.timestamp_us,
                    value: event
                        .decode_host_pointer()
                        .map_err(|error| error.to_string())?,
                }),
                KIND_CAPTURE_METADATA => trace.metadata.push(Timed {
                    timestamp_us: event.timestamp_us,
                    value: event
                        .decode_capture_metadata()
                        .map_err(|error| error.to_string())?,
                }),
                KIND_MARKER => trace.markers.push(Marker {
                    timestamp_us: event.timestamp_us,
                    name: event
                        .payload
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unnamed")
                        .to_owned(),
                    phase: match event
                        .payload
                        .get("phase")
                        .and_then(serde_json::Value::as_str)
                    {
                        Some("start") => Some(MarkerPhase::Start),
                        Some("end") => Some(MarkerPhase::End),
                        _ => None,
                    },
                }),
                KIND_RAW_HID => {
                    let (report_id, bytes) =
                        event.decode_raw_hid().map_err(|error| error.to_string())?;
                    let needs_fallback = (!has_typed_states
                        && matches!(report_id, INPUT_REPORT_ID | EXTENDED_INPUT_REPORT_ID))
                        || (!has_typed_lizard && report_id == LIZARD_MOUSE_REPORT_ID);
                    if needs_fallback {
                        match decoder.decode(report_id, &bytes).map_err(|error| {
                            format!("raw report at {} us: {error}", event.timestamp_us)
                        })? {
                            DecodedReport::ControllerState(value) => trace.states.push(Timed {
                                timestamp_us: event.timestamp_us,
                                value,
                            }),
                            DecodedReport::LizardMouse(value) => trace.lizard.push(Timed {
                                timestamp_us: event.timestamp_us,
                                value,
                            }),
                            _ => {}
                        }
                    }
                }
                "device_connected"
                | "device_disconnected"
                | "mapped_gamepad_state"
                | "warning"
                | "error" => {}
                _ => trace.unknown_event_count += 1,
            }
        }
        trace.states.sort_by_key(|item| item.timestamp_us);
        trace.lizard.sort_by_key(|item| item.timestamp_us);
        trace.host_pointer.sort_by_key(|item| item.timestamp_us);
        trace.markers.sort_by_key(|item| item.timestamp_us);
        Ok(trace)
    }

    pub(crate) fn guided_stages(&self) -> Vec<GuidedStage> {
        let mut open = BTreeMap::new();
        let mut stages = Vec::new();
        for marker in &self.markers {
            match marker.phase {
                Some(MarkerPhase::Start) => {
                    open.insert(marker.name.clone(), marker.timestamp_us);
                }
                Some(MarkerPhase::End) => {
                    if let Some(start_us) = open.remove(&marker.name) {
                        stages.push(GuidedStage {
                            name: marker.name.clone(),
                            start_us,
                            end_us: marker.timestamp_us,
                        });
                    }
                }
                None => {}
            }
        }
        stages.sort_by_key(|stage| stage.start_us);
        stages
    }

    pub(crate) fn reference_motion(&self) -> Vec<Motion> {
        self.lizard
            .iter()
            .filter_map(|item| {
                let x = i32::from(item.value.x);
                let y = i32::from(item.value.y);
                (x != 0 || y != 0).then_some(Motion {
                    timestamp_us: item.timestamp_us,
                    x,
                    y,
                })
            })
            .collect()
    }
}

pub(crate) fn snapshot(state: &SteamControllerState) -> DesktopInputSnapshot {
    DesktopInputSnapshot::from(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use recording::{RecordingEvent, RecordingWriter};
    use std::fs::{self, File};
    use std::process;

    #[test]
    fn guided_stages_pair_named_start_and_end_markers() {
        let trace = Trace {
            markers: vec![
                Marker {
                    timestamp_us: 10,
                    name: "hold".to_owned(),
                    phase: Some(MarkerPhase::Start),
                },
                Marker {
                    timestamp_us: 20,
                    name: "ignored".to_owned(),
                    phase: None,
                },
                Marker {
                    timestamp_us: 30,
                    name: "hold".to_owned(),
                    phase: Some(MarkerPhase::End),
                },
            ],
            ..Trace::default()
        };
        assert_eq!(
            trace.guided_stages(),
            [GuidedStage {
                name: "hold".to_owned(),
                start_us: 10,
                end_us: 30,
            }]
        );
    }

    #[test]
    fn old_raw_v1_recording_falls_back_to_state_and_lizard_decoding() {
        let path = std::env::temp_dir().join(format!(
            "sc-lizard-lab-trace-{}-{}.jsonl",
            process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let mut writer = RecordingWriter::new(File::create(&path).unwrap());
        let mut state = vec![0_u8; 54];
        state[0] = EXTENDED_INPUT_REPORT_ID;
        let mouse = [LIZARD_MOUSE_REPORT_ID, 1, 0xff, 2, 3, 0xfc];
        writer
            .write_event(&RecordingEvent::raw_hid(1, EXTENDED_INPUT_REPORT_ID, &state).unwrap())
            .unwrap();
        writer
            .write_event(&RecordingEvent::raw_hid(2, LIZARD_MOUSE_REPORT_ID, &mouse).unwrap())
            .unwrap();
        drop(writer);

        let trace = Trace::read(&path).unwrap();
        assert_eq!(trace.states.len(), 1);
        assert_eq!(trace.lizard.len(), 1);
        assert_eq!((trace.lizard[0].value.x, trace.lizard[0].value.y), (-1, 2));
        assert_eq!(trace.lizard[0].value.horizontal_wheel, -4);
        fs::remove_file(path).unwrap();
    }
}
