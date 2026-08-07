//! Testable decode, map, safety, and output orchestration for the host bridge.

use std::time::{Duration, Instant};

use bridge_output::{GamepadOutput, OutputError};
use controller_mapper::{ControllerMapper, MapperConfig, MappingError};
use gamepad_state::{GamepadState, OutputSuppression};
use steam_controller_protocol::{
    DecodeError, DecodedReport, SteamControllerDecoder, SteamControllerState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeConfig {
    pub input_timeout: Duration,
    pub decode_failure_limit: u32,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            input_timeout: Duration::from_millis(200),
            decode_failure_limit: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BridgeMetrics {
    pub input_reports: u64,
    pub dropped_input_reports: u64,
    pub decode_failures: u64,
    pub state_changes: u64,
    pub output_packets: u64,
    pub outputs_skipped_unchanged: u64,
    pub hid_reconnects: u64,
    pub decode_time_ns: u128,
    pub mapping_time_ns: u128,
    pub processing_time_ns: u128,
}

impl BridgeMetrics {
    #[must_use]
    pub fn average_decode_us(self) -> f64 {
        average_us(self.decode_time_ns, self.input_reports)
    }
    #[must_use]
    pub fn average_mapping_us(self) -> f64 {
        average_us(
            self.mapping_time_ns,
            self.state_changes + self.outputs_skipped_unchanged,
        )
    }
    #[must_use]
    pub fn average_processing_us(self) -> f64 {
        average_us(self.processing_time_ns, self.input_reports)
    }
}

#[allow(clippy::cast_precision_loss)] // Diagnostics tolerate sub-unit precision loss at huge counters.
fn average_us(nanoseconds: u128, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        nanoseconds as f64 / count as f64 / 1_000.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProcessOutcome {
    State {
        source: SteamControllerState,
        /// What the game sees: the mapped state with any output suppression
        /// applied. The dedupe, the metrics, and the recording all use this.
        mapped: GamepadState,
        /// The mapped state before suppression: what the user is doing with
        /// the controller. Activity tracking reads this, so operating the
        /// profile wheel — which pins `mapped` at neutral — does not count as
        /// idle time.
        unsuppressed: GamepadState,
        sent: bool,
    },
    Status(DecodedReport),
    Neutralized(NeutralReason),
    NoChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeutralReason {
    Disconnect,
    DecodeFailures,
    InputTimeout,
    Shutdown,
    Reset,
}

#[derive(Debug)]
pub enum BridgeError {
    InvalidConfig(&'static str),
    Mapping(MappingError),
    Decode(DecodeError),
    Output(OutputError),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(field) => write!(f, "invalid bridge configuration field {field}"),
            Self::Mapping(error) => write!(f, "mapping failed: {error}"),
            Self::Decode(error) => write!(f, "decode failed: {error}"),
            Self::Output(error) => write!(f, "output failed: {error}"),
        }
    }
}
impl std::error::Error for BridgeError {}
impl From<MappingError> for BridgeError {
    fn from(value: MappingError) -> Self {
        Self::Mapping(value)
    }
}
impl From<OutputError> for BridgeError {
    fn from(value: OutputError) -> Self {
        Self::Output(value)
    }
}

pub struct BridgeEngine {
    config: BridgeConfig,
    decoder: SteamControllerDecoder,
    mapper: ControllerMapper,
    metrics: BridgeMetrics,
    last_input: Option<Duration>,
    last_sent: Option<GamepadState>,
    consecutive_decode_failures: u32,
    connected_once: bool,
    neutralized: bool,
    suppression: Option<OutputSuppression>,
}

impl BridgeEngine {
    /// Creates a validated bridge pipeline.
    ///
    /// # Errors
    /// Returns an error for zero timeouts/limits or an invalid mapping profile.
    pub fn new(config: BridgeConfig, mapper: MapperConfig) -> Result<Self, BridgeError> {
        if config.input_timeout.is_zero() {
            return Err(BridgeError::InvalidConfig("input_timeout"));
        }
        if config.decode_failure_limit == 0 {
            return Err(BridgeError::InvalidConfig("decode_failure_limit"));
        }
        Ok(Self {
            config,
            decoder: SteamControllerDecoder::new(),
            mapper: ControllerMapper::new(mapper)?,
            metrics: BridgeMetrics::default(),
            last_input: None,
            last_sent: None,
            consecutive_decode_failures: 0,
            connected_once: false,
            neutralized: true,
            suppression: None,
        })
    }

    #[must_use]
    pub const fn metrics(&self) -> BridgeMetrics {
        self.metrics
    }

    /// Hides controls a host feature has taken over from the gamepad output.
    ///
    /// A host overlay that steers with the sticks would otherwise drive the
    /// game at the same time. Suppression is applied to the mapped state, so
    /// the unchanged-output dedupe and the metrics see exactly what the game
    /// sees. `None` restores full passthrough.
    pub fn set_output_suppression(&mut self, suppression: Option<OutputSuppression>) {
        self.suppression = suppression;
    }

    #[must_use]
    pub const fn output_suppression(&self) -> Option<OutputSuppression> {
        self.suppression
    }
    pub fn note_dropped_reports(&mut self, count: u64) {
        self.metrics.dropped_input_reports += count;
    }
    pub fn connected(&mut self) {
        if self.connected_once {
            self.metrics.hid_reconnects += 1;
        }
        self.connected_once = true;
        self.consecutive_decode_failures = 0;
    }

    /// Decodes, maps, and conditionally sends one HID report.
    ///
    /// # Errors
    /// Returns decode or output failures. Reaching the configured decode-failure
    /// limit first sends neutral and returns a neutralized outcome.
    pub fn process_report<O: GamepadOutput + ?Sized>(
        &mut self,
        report_id: u8,
        data: &[u8],
        now: Duration,
        output: &mut O,
    ) -> Result<ProcessOutcome, BridgeError> {
        let processing_started = Instant::now();
        self.metrics.input_reports += 1;
        let decode_started = Instant::now();
        let decoded = self.decoder.decode(report_id, data);
        self.metrics.decode_time_ns += decode_started.elapsed().as_nanos();
        let decoded = match decoded {
            Ok(value) => {
                self.consecutive_decode_failures = 0;
                value
            }
            Err(error) => {
                self.metrics.decode_failures += 1;
                self.consecutive_decode_failures += 1;
                self.metrics.processing_time_ns += processing_started.elapsed().as_nanos();
                if self.consecutive_decode_failures >= self.config.decode_failure_limit {
                    return self.force_neutral(output, NeutralReason::DecodeFailures);
                }
                return Err(BridgeError::Decode(error));
            }
        };
        let outcome = match decoded {
            DecodedReport::ControllerState(source) => {
                self.last_input = Some(now);
                self.neutralized = false;
                let mapping_started = Instant::now();
                let mut mapped = self.mapper.map(&source, 0.004);
                let unsuppressed = mapped;
                if let Some(suppression) = self.suppression {
                    suppression.apply(&mut mapped);
                }
                self.metrics.mapping_time_ns += mapping_started.elapsed().as_nanos();
                let sent = if self.last_sent == Some(mapped) {
                    self.metrics.outputs_skipped_unchanged += 1;
                    false
                } else {
                    output.send_state(&mapped)?;
                    self.last_sent = Some(mapped);
                    self.metrics.state_changes += 1;
                    self.metrics.output_packets += 1;
                    true
                };
                ProcessOutcome::State {
                    source,
                    mapped,
                    unsuppressed,
                    sent,
                }
            }
            status => ProcessOutcome::Status(status),
        };
        self.metrics.processing_time_ns += processing_started.elapsed().as_nanos();
        Ok(outcome)
    }

    /// Sends neutral once when the input deadline expires.
    ///
    /// # Errors
    /// Returns an output backend failure.
    pub fn tick<O: GamepadOutput + ?Sized>(
        &mut self,
        now: Duration,
        output: &mut O,
    ) -> Result<ProcessOutcome, BridgeError> {
        output.service()?;
        if !self.neutralized
            && self
                .last_input
                .is_some_and(|last| now.saturating_sub(last) >= self.config.input_timeout)
        {
            self.force_neutral(output, NeutralReason::InputTimeout)
        } else {
            Ok(ProcessOutcome::NoChange)
        }
    }

    /// Immediately sends neutral and clears mapper history after disconnect.
    ///
    /// # Errors
    /// Returns an output backend failure.
    pub fn disconnected<O: GamepadOutput + ?Sized>(
        &mut self,
        output: &mut O,
    ) -> Result<ProcessOutcome, BridgeError> {
        self.force_neutral(output, NeutralReason::Disconnect)
    }
    /// Immediately sends neutral for an explicit reset.
    ///
    /// # Errors
    /// Returns an output backend failure.
    pub fn reset<O: GamepadOutput + ?Sized>(
        &mut self,
        output: &mut O,
    ) -> Result<ProcessOutcome, BridgeError> {
        self.force_neutral(output, NeutralReason::Reset)
    }
    /// Sends final neutral before orderly shutdown.
    ///
    /// # Errors
    /// Returns an output backend failure.
    pub fn shutdown<O: GamepadOutput + ?Sized>(
        &mut self,
        output: &mut O,
    ) -> Result<ProcessOutcome, BridgeError> {
        self.force_neutral(output, NeutralReason::Shutdown)
    }

    fn force_neutral<O: GamepadOutput + ?Sized>(
        &mut self,
        output: &mut O,
        reason: NeutralReason,
    ) -> Result<ProcessOutcome, BridgeError> {
        output.send_neutral()?;
        self.metrics.output_packets += 1;
        self.last_sent = Some(GamepadState::neutral());
        self.last_input = None;
        self.mapper.reset();
        self.neutralized = true;
        Ok(ProcessOutcome::Neutralized(reason))
    }
}

impl Default for BridgeEngine {
    fn default() -> Self {
        Self::new(BridgeConfig::default(), MapperConfig::default())
            .expect("default bridge configuration is valid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_output::MockOutput;
    use gamepad_state::Button;
    use steam_controller_protocol::{
        SteamButton, INPUT_REPORT_ID, INPUT_REPORT_SIZE, LIZARD_MOUSE_REPORT_ID,
        LIZARD_MOUSE_REPORT_SIZE,
    };

    fn report(sequence: u8, x: i16) -> Vec<u8> {
        let mut bytes = vec![0; INPUT_REPORT_SIZE];
        bytes[0] = INPUT_REPORT_ID;
        bytes[1] = sequence;
        bytes[18..20].copy_from_slice(&x.to_le_bytes());
        bytes
    }

    /// A report that actually deflects the left stick, unlike `report`, which
    /// writes the left pad and so never reaches the mapped axes.
    fn stick_report(sequence: u8, stick_x: i16, buttons: &[SteamButton]) -> Vec<u8> {
        let mut bytes = vec![0; INPUT_REPORT_SIZE];
        bytes[0] = INPUT_REPORT_ID;
        bytes[1] = sequence;
        let mask = buttons
            .iter()
            .fold(0_u32, |mask, button| mask | 1 << *button as u8);
        bytes[2..6].copy_from_slice(&mask.to_le_bytes());
        bytes[10..12].copy_from_slice(&stick_x.to_le_bytes());
        bytes
    }

    fn mapped_state(outcome: &ProcessOutcome) -> GamepadState {
        match outcome {
            ProcessOutcome::State { mapped, .. } => *mapped,
            other => panic!("expected a controller state, got {other:?}"),
        }
    }

    #[test]
    fn suppression_parks_the_output_at_exactly_neutral() {
        let mut engine = BridgeEngine::default();
        let mut output = MockOutput::default();
        engine.connected();
        let pressed = [SteamButton::A, SteamButton::X];

        let outcome = engine
            .process_report(
                INPUT_REPORT_ID,
                &stick_report(1, 10_000, &pressed),
                Duration::ZERO,
                &mut output,
            )
            .unwrap();
        let passthrough = mapped_state(&outcome);
        assert!(passthrough.left_x > 0.0);
        assert!(passthrough.buttons.contains(Button::South));
        assert!(passthrough.buttons.contains(Button::West));

        engine.set_output_suppression(Some(OutputSuppression::Neutral));

        // The physical state is unchanged, but the suppressed view is not, so
        // this must reach the game rather than being deduped away.
        let outcome = engine
            .process_report(
                INPUT_REPORT_ID,
                &stick_report(2, 10_000, &pressed),
                Duration::from_millis(4),
                &mut output,
            )
            .unwrap();
        assert!(matches!(outcome, ProcessOutcome::State { sent: true, .. }));
        let hidden = mapped_state(&outcome);
        // Exactly neutral, so firmware disarms its controller-data watchdog and
        // a host stall while the overlay is up cannot fault the device.
        assert_eq!(hidden, GamepadState::NEUTRAL);
        assert_eq!(output.states.last(), Some(&GamepadState::NEUTRAL));

        // A suppressed run still dedupes; only the first frame is sent.
        let outcome = engine
            .process_report(
                INPUT_REPORT_ID,
                &stick_report(3, 10_000, &pressed),
                Duration::from_millis(8),
                &mut output,
            )
            .unwrap();
        assert!(matches!(outcome, ProcessOutcome::State { sent: false, .. }));

        engine.set_output_suppression(None);
        let outcome = engine
            .process_report(
                INPUT_REPORT_ID,
                &stick_report(4, 10_000, &pressed),
                Duration::from_millis(12),
                &mut output,
            )
            .unwrap();
        assert_eq!(mapped_state(&outcome), passthrough);
    }

    #[test]
    fn maps_changed_states_and_skips_duplicates() {
        let mut engine = BridgeEngine::default();
        let mut output = MockOutput::default();
        engine.connected();
        assert!(matches!(
            engine
                .process_report(
                    INPUT_REPORT_ID,
                    &report(1, 10000),
                    Duration::ZERO,
                    &mut output
                )
                .unwrap(),
            ProcessOutcome::State { sent: true, .. }
        ));
        assert!(matches!(
            engine
                .process_report(
                    INPUT_REPORT_ID,
                    &report(2, 10000),
                    Duration::from_millis(4),
                    &mut output
                )
                .unwrap(),
            ProcessOutcome::State { sent: false, .. }
        ));
        assert_eq!(output.states.len(), 1);
        assert_eq!(engine.metrics().outputs_skipped_unchanged, 1);
    }

    #[test]
    fn disconnect_timeout_failures_reset_and_shutdown_force_neutral() {
        let mut engine = BridgeEngine::default();
        let mut output = MockOutput::default();
        engine
            .process_report(
                INPUT_REPORT_ID,
                &report(1, 20000),
                Duration::ZERO,
                &mut output,
            )
            .unwrap();
        assert!(matches!(
            engine
                .tick(Duration::from_millis(200), &mut output)
                .unwrap(),
            ProcessOutcome::Neutralized(NeutralReason::InputTimeout)
        ));
        assert!(matches!(
            engine.disconnected(&mut output).unwrap(),
            ProcessOutcome::Neutralized(NeutralReason::Disconnect)
        ));
        assert!(matches!(
            engine.reset(&mut output).unwrap(),
            ProcessOutcome::Neutralized(NeutralReason::Reset)
        ));
        assert!(matches!(
            engine.shutdown(&mut output).unwrap(),
            ProcessOutcome::Neutralized(NeutralReason::Shutdown)
        ));
        assert!(output.states[1..]
            .iter()
            .all(|state| *state == GamepadState::neutral()));
    }

    #[test]
    fn repeated_decode_failures_neutralize_and_reconnects_are_counted() {
        let mut engine = BridgeEngine::default();
        let mut output = MockOutput::default();
        engine.connected();
        engine.connected();
        for _ in 0..2 {
            assert!(matches!(
                engine.process_report(0xff, &[0xff], Duration::ZERO, &mut output),
                Err(BridgeError::Decode(_))
            ));
        }
        assert!(matches!(
            engine
                .process_report(0xff, &[0xff], Duration::ZERO, &mut output)
                .unwrap(),
            ProcessOutcome::Neutralized(NeutralReason::DecodeFailures)
        ));
        assert_eq!(engine.metrics().hid_reconnects, 1);
        assert_eq!(engine.metrics().decode_failures, 3);
    }

    #[test]
    fn valid_lizard_reports_are_status_and_do_not_refresh_input_timeout() {
        let mut engine = BridgeEngine::default();
        let mut output = MockOutput::default();
        engine.connected();
        engine
            .process_report(
                INPUT_REPORT_ID,
                &report(1, 20_000),
                Duration::ZERO,
                &mut output,
            )
            .unwrap();

        let mut mouse = [0_u8; LIZARD_MOUSE_REPORT_SIZE];
        mouse[0] = LIZARD_MOUSE_REPORT_ID;
        assert!(matches!(
            engine
                .process_report(
                    LIZARD_MOUSE_REPORT_ID,
                    &mouse,
                    Duration::from_millis(199),
                    &mut output
                )
                .unwrap(),
            ProcessOutcome::Status(DecodedReport::LizardMouse { .. })
        ));
        assert_eq!(engine.metrics().decode_failures, 0);
        assert!(matches!(
            engine
                .tick(Duration::from_millis(200), &mut output)
                .unwrap(),
            ProcessOutcome::Neutralized(NeutralReason::InputTimeout)
        ));
    }

    #[test]
    fn valid_lizard_reports_clear_the_decode_error_streak() {
        let mut engine = BridgeEngine::default();
        let mut output = MockOutput::default();
        for _ in 0..2 {
            assert!(matches!(
                engine.process_report(0xff, &[0xff], Duration::ZERO, &mut output),
                Err(BridgeError::Decode(_))
            ));
        }

        let mut mouse = [0_u8; LIZARD_MOUSE_REPORT_SIZE];
        mouse[0] = LIZARD_MOUSE_REPORT_ID;
        assert!(engine
            .process_report(LIZARD_MOUSE_REPORT_ID, &mouse, Duration::ZERO, &mut output)
            .is_ok());

        for _ in 0..2 {
            assert!(matches!(
                engine.process_report(0xff, &[0xff], Duration::ZERO, &mut output),
                Err(BridgeError::Decode(_))
            ));
        }
        assert!(output.states.is_empty());
        assert_eq!(engine.metrics().decode_failures, 4);
    }
}
