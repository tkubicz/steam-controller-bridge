//! Versioned JSONL recording and deterministic/timed replay.

use std::io::{self, BufRead, Write};
use std::thread;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use bridge_output::{GamepadOutput, OutputError};
use gamepad_state::{GamepadButtons, GamepadState, HatState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use steam_controller_protocol::SteamControllerState;

pub const FORMAT_VERSION: u32 = 1;
pub const KIND_DEVICE_CONNECTED: &str = "device_connected";
pub const KIND_DEVICE_DISCONNECTED: &str = "device_disconnected";
pub const KIND_RAW_HID: &str = "raw_hid";
pub const KIND_DECODED_STEAM_STATE: &str = "decoded_steam_state";
pub const KIND_MAPPED_GAMEPAD_STATE: &str = "mapped_gamepad_state";
pub const KIND_WARNING: &str = "warning";
pub const KIND_ERROR: &str = "error";
pub const KIND_MARKER: &str = "marker";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingEvent {
    pub version: u32,
    pub timestamp_us: u64,
    pub kind: String,
    pub payload: Value,
}

impl RecordingEvent {
    #[must_use]
    pub fn new(timestamp_us: u64, kind: impl Into<String>, payload: Value) -> Self {
        Self {
            version: FORMAT_VERSION,
            timestamp_us,
            kind: kind.into(),
            payload,
        }
    }

    /// Constructs a raw HID report event with base64-encoded bytes.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError`] if the typed payload cannot be represented as JSON.
    pub fn raw_hid(timestamp_us: u64, report_id: u8, bytes: &[u8]) -> Result<Self, RecordingError> {
        Self::raw_hid_with_metadata(timestamp_us, report_id, bytes, None, None, 0)
    }

    /// Constructs a raw HID event including source and loss diagnostics.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError`] if the typed payload cannot be represented as JSON.
    pub fn raw_hid_with_metadata(
        timestamp_us: u64,
        report_id: u8,
        bytes: &[u8],
        source_device_id: Option<&str>,
        transport: Option<&str>,
        dropped_reports: u64,
    ) -> Result<Self, RecordingError> {
        Self::from_payload(
            timestamp_us,
            KIND_RAW_HID,
            &RawHidPayload {
                report_id,
                bytes: BASE64.encode(bytes),
                source_device_id: source_device_id.map(str::to_owned),
                transport: transport.map(str::to_owned),
                dropped_reports,
            },
        )
    }

    /// Constructs a final mapped gamepad state event.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError`] if the state is invalid or serialization fails.
    pub fn mapped_gamepad_state(
        timestamp_us: u64,
        state: &GamepadState,
    ) -> Result<Self, RecordingError> {
        state.validate().map_err(RecordingError::InvalidState)?;
        Self::from_payload(
            timestamp_us,
            KIND_MAPPED_GAMEPAD_STATE,
            &GamepadPayload::from(*state),
        )
    }

    /// Constructs a decoded Steam Controller 2 state event.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError`] if serialization fails.
    pub fn decoded_steam_state(
        timestamp_us: u64,
        state: &SteamControllerState,
    ) -> Result<Self, RecordingError> {
        Self::from_payload(timestamp_us, KIND_DECODED_STEAM_STATE, state)
    }

    /// Constructs an event from any serializable typed payload.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError`] if payload serialization fails.
    pub fn from_payload<T: Serialize>(
        timestamp_us: u64,
        kind: impl Into<String>,
        payload: &T,
    ) -> Result<Self, RecordingError> {
        Ok(Self::new(
            timestamp_us,
            kind,
            serde_json::to_value(payload)?,
        ))
    }

    /// Decodes this event as a raw HID payload.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError`] when the event kind or payload is invalid.
    pub fn decode_raw_hid(&self) -> Result<(u8, Vec<u8>), RecordingError> {
        if self.kind != KIND_RAW_HID {
            return Err(RecordingError::UnexpectedKind(self.kind.clone()));
        }
        let payload: RawHidPayload = serde_json::from_value(self.payload.clone())?;
        let bytes = BASE64
            .decode(payload.bytes)
            .map_err(|error| RecordingError::InvalidPayload(error.to_string()))?;
        Ok((payload.report_id, bytes))
    }

    /// Decodes this event as a mapped gamepad state.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError`] when the event kind, fields, or ranges are invalid.
    pub fn decode_gamepad_state(&self) -> Result<GamepadState, RecordingError> {
        if self.kind != KIND_MAPPED_GAMEPAD_STATE {
            return Err(RecordingError::UnexpectedKind(self.kind.clone()));
        }
        let payload: GamepadPayload = serde_json::from_value(self.payload.clone())?;
        payload.try_into()
    }

    /// Decodes this event as a Steam Controller 2 state.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError`] when the event kind or fields are invalid.
    pub fn decode_steam_state(&self) -> Result<SteamControllerState, RecordingError> {
        if self.kind != KIND_DECODED_STEAM_STATE {
            return Err(RecordingError::UnexpectedKind(self.kind.clone()));
        }
        Ok(serde_json::from_value(self.payload.clone())?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RawHidPayload {
    report_id: u8,
    bytes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transport: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    dropped_reports: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde skip predicates receive references.
const fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct GamepadPayload {
    buttons: u32,
    hat: u8,
    left_x: f32,
    left_y: f32,
    right_x: f32,
    right_y: f32,
    left_trigger: f32,
    right_trigger: f32,
}

impl From<GamepadState> for GamepadPayload {
    fn from(state: GamepadState) -> Self {
        Self {
            buttons: state.buttons.0,
            hat: state.hat as u8,
            left_x: state.left_x,
            left_y: state.left_y,
            right_x: state.right_x,
            right_y: state.right_y,
            left_trigger: state.left_trigger,
            right_trigger: state.right_trigger,
        }
    }
}

impl TryFrom<GamepadPayload> for GamepadState {
    type Error = RecordingError;

    fn try_from(payload: GamepadPayload) -> Result<Self, Self::Error> {
        let state = Self {
            buttons: GamepadButtons(payload.buttons),
            hat: HatState::try_from(payload.hat)
                .map_err(|_| RecordingError::InvalidPayload("invalid hat".to_owned()))?,
            left_x: payload.left_x,
            left_y: payload.left_y,
            right_x: payload.right_x,
            right_y: payload.right_y,
            left_trigger: payload.left_trigger,
            right_trigger: payload.right_trigger,
        };
        state.validate().map_err(RecordingError::InvalidState)?;
        Ok(state)
    }
}

pub struct RecordingWriter<W: Write> {
    writer: W,
    started: Instant,
    last_timestamp_us: Option<u64>,
}

impl<W: Write> RecordingWriter<W> {
    #[must_use]
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            started: Instant::now(),
            last_timestamp_us: None,
        }
    }

    #[must_use]
    pub fn elapsed_us(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    /// Writes and flushes one JSONL event.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError`] for unsupported versions, decreasing timestamps,
    /// serialization failures, or writer I/O failures.
    pub fn write_event(&mut self, event: &RecordingEvent) -> Result<(), RecordingError> {
        if event.version != FORMAT_VERSION {
            return Err(RecordingError::UnsupportedVersion(event.version));
        }
        if self
            .last_timestamp_us
            .is_some_and(|previous| event.timestamp_us < previous)
        {
            return Err(RecordingError::OutOfOrderTimestamp {
                previous: self.last_timestamp_us.unwrap_or(0),
                current: event.timestamp_us,
            });
        }
        serde_json::to_writer(&mut self.writer, event)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        self.last_timestamp_us = Some(event.timestamp_us);
        Ok(())
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}

pub struct RecordingOutput<W: Write> {
    recording: RecordingWriter<W>,
}

impl<W: Write> RecordingOutput<W> {
    #[must_use]
    pub fn new(writer: W) -> Self {
        Self {
            recording: RecordingWriter::new(writer),
        }
    }

    pub fn into_inner(self) -> W {
        self.recording.into_inner()
    }
}

impl<W: Write> GamepadOutput for RecordingOutput<W> {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        let event = RecordingEvent::mapped_gamepad_state(self.recording.elapsed_us(), state)
            .map_err(recording_as_output_error)?;
        self.recording
            .write_event(&event)
            .map_err(recording_as_output_error)
    }
}

fn recording_as_output_error(error: RecordingError) -> OutputError {
    OutputError::Io(io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Parses an entire JSONL stream while preserving unknown event kinds.
///
/// # Errors
///
/// Returns [`RecordingError`] with the failing line number for malformed JSON,
/// unsupported versions, or decreasing timestamps.
pub fn read_events<R: BufRead>(reader: R) -> Result<Vec<RecordingEvent>, RecordingError> {
    let mut events = Vec::new();
    let mut previous = None;
    for (index, line) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line.map_err(RecordingError::Io)?;
        if line.trim().is_empty() {
            continue;
        }
        let event: RecordingEvent =
            serde_json::from_str(&line).map_err(|source| RecordingError::MalformedLine {
                line: line_number,
                source,
            })?;
        if event.version != FORMAT_VERSION {
            return Err(RecordingError::UnsupportedVersion(event.version));
        }
        if previous.is_some_and(|timestamp| event.timestamp_us < timestamp) {
            return Err(RecordingError::OutOfOrderTimestamp {
                previous: previous.unwrap_or(0),
                current: event.timestamp_us,
            });
        }
        previous = Some(event.timestamp_us);
        events.push(event);
    }
    Ok(events)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayTiming {
    Immediate,
    RealTime,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplayOptions {
    pub timing: ReplayTiming,
    pub speed: f64,
    pub seek_timestamp_us: u64,
}

impl Default for ReplayOptions {
    fn default() -> Self {
        Self {
            timing: ReplayTiming::RealTime,
            speed: 1.0,
            seek_timestamp_us: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReplayStats {
    pub events_processed: usize,
    pub states_sent: usize,
    pub events_ignored: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplaySession {
    events: Vec<RecordingEvent>,
}

impl ReplaySession {
    /// Loads a replay session from JSONL.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError`] when the recording cannot be parsed or validated.
    pub fn read<R: BufRead>(reader: R) -> Result<Self, RecordingError> {
        Ok(Self {
            events: read_events(reader)?,
        })
    }

    #[must_use]
    pub fn events(&self) -> &[RecordingEvent] {
        &self.events
    }

    #[must_use]
    pub fn seek_index(&self, timestamp_us: u64) -> usize {
        self.events
            .partition_point(|event| event.timestamp_us < timestamp_us)
    }

    /// Replays mapped gamepad events once through an output backend.
    ///
    /// # Errors
    ///
    /// Returns [`RecordingError`] for invalid options, malformed gamepad payloads,
    /// or output backend failures.
    pub fn play_once<O: GamepadOutput + ?Sized>(
        &self,
        output: &mut O,
        options: ReplayOptions,
    ) -> Result<ReplayStats, RecordingError> {
        if !options.speed.is_finite() || options.speed <= 0.0 {
            return Err(RecordingError::InvalidSpeed(options.speed));
        }
        let events = &self.events[self.seek_index(options.seek_timestamp_us)..];
        let mut stats = ReplayStats::default();
        let mut previous_timestamp = events.first().map(|event| event.timestamp_us);
        for event in events {
            if options.timing == ReplayTiming::RealTime {
                let delta_us = event
                    .timestamp_us
                    .saturating_sub(previous_timestamp.unwrap_or(event.timestamp_us));
                let scaled = Duration::from_secs_f64(
                    Duration::from_micros(delta_us).as_secs_f64() / options.speed,
                );
                if !scaled.is_zero() {
                    thread::sleep(scaled);
                }
            }
            previous_timestamp = Some(event.timestamp_us);
            stats.events_processed += 1;
            if event.kind == KIND_MAPPED_GAMEPAD_STATE {
                output.send_state(&event.decode_gamepad_state()?)?;
                stats.states_sent += 1;
            } else {
                stats.events_ignored += 1;
            }
        }
        Ok(stats)
    }
}

#[derive(Debug)]
pub enum RecordingError {
    Io(io::Error),
    Json(serde_json::Error),
    MalformedLine {
        line: usize,
        source: serde_json::Error,
    },
    UnsupportedVersion(u32),
    OutOfOrderTimestamp {
        previous: u64,
        current: u64,
    },
    UnexpectedKind(String),
    InvalidPayload(String),
    InvalidState(gamepad_state::InvalidState),
    InvalidSpeed(f64),
    Output(OutputError),
}

impl std::fmt::Display for RecordingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "recording I/O failed: {error}"),
            Self::Json(error) => write!(f, "recording JSON failed: {error}"),
            Self::MalformedLine { line, source } => {
                write!(f, "malformed recording line {line}: {source}")
            }
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported recording version {version}")
            }
            Self::OutOfOrderTimestamp { previous, current } => write!(
                f,
                "recording timestamp decreased from {previous} to {current}"
            ),
            Self::UnexpectedKind(kind) => write!(f, "unexpected recording event kind '{kind}'"),
            Self::InvalidPayload(detail) => write!(f, "invalid recording payload: {detail}"),
            Self::InvalidState(error) => write!(f, "invalid recorded gamepad state: {error}"),
            Self::InvalidSpeed(speed) => write!(f, "invalid replay speed {speed}"),
            Self::Output(error) => write!(f, "replay output failed: {error}"),
        }
    }
}

impl std::error::Error for RecordingError {}
impl From<io::Error> for RecordingError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<serde_json::Error> for RecordingError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}
impl From<OutputError> for RecordingError {
    fn from(value: OutputError) -> Self {
        Self::Output(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_output::MockOutput;
    use gamepad_state::Button;
    use serde_json::json;
    use std::io::Cursor;
    use steam_controller_protocol::{
        DecodedReport, SteamControllerDecoder, INPUT_REPORT_ID, INPUT_REPORT_SIZE,
    };

    #[test]
    fn event_round_trip_preserves_raw_and_gamepad_payloads() {
        let raw = RecordingEvent::raw_hid(7, 66, &[0, 1, 254, 255]).unwrap();
        assert_eq!(raw.decode_raw_hid().unwrap(), (66, vec![0, 1, 254, 255]));
        let raw_with_metadata = RecordingEvent::raw_hid_with_metadata(
            7,
            66,
            &[1, 2],
            Some("collection-1"),
            Some("USB"),
            3,
        )
        .unwrap();
        assert_eq!(
            raw_with_metadata.payload["source_device_id"],
            "collection-1"
        );
        assert_eq!(raw_with_metadata.payload["transport"], "USB");
        assert_eq!(raw_with_metadata.payload["dropped_reports"], 3);
        let mut state = GamepadState {
            left_x: -1.0,
            right_trigger: 1.0,
            ..GamepadState::neutral()
        };
        state.buttons.set(Button::South, true);
        let mapped = RecordingEvent::mapped_gamepad_state(8, &state).unwrap();
        assert_eq!(mapped.decode_gamepad_state().unwrap(), state);

        let mut writer = RecordingWriter::new(Vec::new());
        writer.write_event(&raw).unwrap();
        writer.write_event(&mapped).unwrap();
        let parsed = read_events(Cursor::new(writer.into_inner())).unwrap();
        assert_eq!(parsed, vec![raw, mapped]);
    }

    #[test]
    fn writer_and_reader_reject_decreasing_timestamps() {
        let late = RecordingEvent::new(10, KIND_MARKER, json!({"name":"late"}));
        let early = RecordingEvent::new(9, KIND_MARKER, json!({"name":"early"}));
        let mut writer = RecordingWriter::new(Vec::new());
        writer.write_event(&late).unwrap();
        assert!(matches!(
            writer.write_event(&early),
            Err(RecordingError::OutOfOrderTimestamp { .. })
        ));

        let text = format!(
            "{}\n{}\n",
            serde_json::to_string(&late).unwrap(),
            serde_json::to_string(&early).unwrap()
        );
        assert!(matches!(
            read_events(Cursor::new(text)),
            Err(RecordingError::OutOfOrderTimestamp { .. })
        ));
    }

    #[test]
    fn unknown_events_are_preserved_and_ignored_during_replay() {
        let unknown = RecordingEvent::new(0, "future_event", json!({"new":true}));
        let mapped = RecordingEvent::mapped_gamepad_state(1, &GamepadState::neutral()).unwrap();
        let session = ReplaySession {
            events: vec![unknown.clone(), mapped],
        };
        assert_eq!(session.events()[0], unknown);
        let mut output = MockOutput::default();
        let stats = session
            .play_once(
                &mut output,
                ReplayOptions {
                    timing: ReplayTiming::Immediate,
                    ..ReplayOptions::default()
                },
            )
            .unwrap();
        assert_eq!(
            stats,
            ReplayStats {
                events_processed: 2,
                states_sent: 1,
                events_ignored: 1
            }
        );
        assert_eq!(output.states, vec![GamepadState::neutral()]);
    }

    #[test]
    fn deterministic_replay_and_seek_are_stable() {
        let a = RecordingEvent::mapped_gamepad_state(10, &GamepadState::neutral()).unwrap();
        let mut changed = GamepadState::neutral();
        changed.left_trigger = 1.0;
        let b = RecordingEvent::mapped_gamepad_state(20, &changed).unwrap();
        let session = ReplaySession { events: vec![a, b] };
        assert_eq!(session.seek_index(20), 1);
        let mut output = MockOutput::default();
        session
            .play_once(
                &mut output,
                ReplayOptions {
                    timing: ReplayTiming::Immediate,
                    seek_timestamp_us: 20,
                    speed: 10.0,
                },
            )
            .unwrap();
        assert_eq!(output.states, vec![changed]);
    }

    #[test]
    fn truncated_and_unsupported_recordings_fail_cleanly() {
        assert!(matches!(
            read_events(Cursor::new("{\"version\":1")),
            Err(RecordingError::MalformedLine { line: 1, .. })
        ));
        let unsupported = "{\"version\":2,\"timestamp_us\":0,\"kind\":\"marker\",\"payload\":{}}\n";
        assert!(matches!(
            read_events(Cursor::new(unsupported)),
            Err(RecordingError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn recording_output_can_be_replayed_identically() {
        let mut recording = RecordingOutput::new(Vec::new());
        let states = [
            GamepadState::neutral(),
            GamepadState {
                right_x: 0.5,
                ..GamepadState::neutral()
            },
        ];
        for state in &states {
            recording.send_state(state).unwrap();
        }
        let session = ReplaySession::read(Cursor::new(recording.into_inner())).unwrap();
        let mut output = MockOutput::default();
        session
            .play_once(
                &mut output,
                ReplayOptions {
                    timing: ReplayTiming::Immediate,
                    ..ReplayOptions::default()
                },
            )
            .unwrap();
        assert_eq!(output.states, states);
    }

    #[test]
    fn decoded_steam_state_round_trips_as_typed_payload() {
        let mut report = vec![0_u8; INPUT_REPORT_SIZE];
        report[0] = INPUT_REPORT_ID;
        let DecodedReport::ControllerState(state) = SteamControllerDecoder::new()
            .decode(INPUT_REPORT_ID, &report)
            .unwrap()
        else {
            panic!("controller state expected");
        };
        let event = RecordingEvent::decoded_steam_state(12, &state).unwrap();
        assert_eq!(event.decode_steam_state().unwrap(), state);
    }
}
