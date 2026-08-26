//! Hardware-independent gamepad output backends.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use bridge_protocol::{Frame, Message, WireGamepadState};
use gamepad_state::GamepadState;

mod bridge_transport;
mod endpoint_discovery;
pub use bridge_transport::{
    new_firmware_install_receipt, random_firmware_request_id, BridgeConnection,
    BridgeConnectionStatus, BridgeOutput, BridgeTransportConfig, BridgeTransportError,
    BridgeTransportMetrics, ByteTransport, FirmwareCapabilities, FirmwareInfo,
    FirmwareInstallReceipt, FirmwareInstallSource, FirmwareInstallState,
    FirmwareReceiptCreationError, FirmwareTarget, FirmwareTargetId, FirmwareTargetIdError,
    FirmwareVersion, MAX_FIRMWARE_TARGET_ID_LEN,
};
pub use endpoint_discovery::{
    available_bridge_endpoints, available_serial_devices, available_serial_endpoints,
    available_serial_ports, BridgeEndpoint, BridgeTransportKind, BridgeUsbIdentity,
    SerialDeviceInfo, BRIDGE_DEVICE_USB_PRODUCT, DEFAULT_BRIDGE_BAUD_RATE,
};

/// The rendered prefix of [`OutputError::Configuration`]. Callers that only
/// see a flattened error string classify against this constant, so the wording
/// and the classification cannot drift apart.
pub const CONFIGURATION_FAILURE_PREFIX: &str = "output configuration failed";
pub const BRIDGE_BUSY_ERROR_MARKER: &str = "bridge device or resource busy";

#[derive(Debug)]
pub enum OutputError {
    Io(io::Error),
    InvalidState(gamepad_state::InvalidState),
    Protocol(bridge_protocol::ProtocolError),
    Transport(String),
    /// A failure that reopening the same backend cannot clear: a missing or
    /// unauthorized helper, a rejected protocol, or invalid configuration.
    Configuration(String),
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "output I/O failed: {error}"),
            Self::InvalidState(error) => write!(f, "invalid gamepad state: {error}"),
            Self::Protocol(error) => write!(f, "protocol encoding failed: {error}"),
            Self::Transport(error) => write!(f, "output transport failed: {error}"),
            Self::Configuration(error) => write!(f, "{CONFIGURATION_FAILURE_PREFIX}: {error}"),
        }
    }
}

impl std::error::Error for OutputError {}
impl From<io::Error> for OutputError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<gamepad_state::InvalidState> for OutputError {
    fn from(value: gamepad_state::InvalidState) -> Self {
        Self::InvalidState(value)
    }
}
impl From<bridge_protocol::ProtocolError> for OutputError {
    fn from(value: bridge_protocol::ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

pub trait GamepadOutput {
    /// Sends one validated state.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError`] when validation, encoding, or backend I/O fails.
    fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError>;

    /// Sends a fully neutral state.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError`] when the backend cannot accept the state.
    fn send_neutral(&mut self) -> Result<(), OutputError> {
        self.send_state(&GamepadState::neutral())
    }

    /// Services backend I/O and time-based work while no state is changing.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError`] when backend health or I/O servicing fails.
    fn service(&mut self) -> Result<(), OutputError> {
        Ok(())
    }

    /// Takes the latest device-to-host feedback request, if the backend
    /// supports bidirectional output.
    fn take_feedback(&mut self) -> Option<OutputFeedback> {
        None
    }

    #[must_use]
    fn diagnostics(&self) -> OutputDiagnostics {
        OutputDiagnostics::default()
    }

    /// What the connected firmware has reported about itself, for backends
    /// with an identifiable device on the other end. `None` when the backend
    /// has no device or no live connection.
    #[must_use]
    fn firmware_version(&self) -> Option<FirmwareVersion> {
        self.firmware_info().map(FirmwareInfo::firmware_version)
    }

    /// Complete version, capability, and installation receipt information for
    /// the connected firmware. `None` when no identifiable device is live.
    #[must_use]
    fn firmware_info(&self) -> Option<FirmwareInfo> {
        None
    }

    /// Starts receipt recording without waiting for the device response.
    ///
    /// # Errors
    /// Returns [`OutputError`] when the backend cannot send the request.
    fn request_firmware_install_receipt(
        &mut self,
        _request_id: u32,
        _receipt: FirmwareInstallReceipt,
    ) -> Result<(), OutputError> {
        Err(OutputError::Transport(
            "output does not support asynchronous firmware installation receipts".to_owned(),
        ))
    }

    /// Takes a completed response for the expected receipt, if one is available.
    fn poll_firmware_install_receipt(
        &mut self,
        _request_id: u32,
        _receipt: FirmwareInstallReceipt,
    ) -> Option<Result<FirmwareInstallReceipt, OutputError>> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFeedback {
    Rumble {
        low_frequency: u16,
        high_frequency: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutputDiagnostics {
    pub bridge_reconnects: u64,
    pub framing_failures: u64,
    pub checksum_failures: u64,
    pub state_refreshes: u64,
    pub rumble_commands_received: u64,
    pub rumble_commands_coalesced: u64,
    pub virtual_reports_dispatched: u64,
    pub virtual_reports_coalesced: u64,
    pub virtual_helper_restarts: u64,
    pub virtual_protocol_failures: u64,
    pub virtual_set_reports_received: u64,
    pub virtual_get_reports_received: u64,
    /// Delegate diagnostics the virtual-HID helper dropped rather than block
    /// a host callback, recovered from gaps in its event sequence.
    pub virtual_delegate_reports_dropped: u64,
    pub virtual_fatal_errors: u64,
}

#[derive(Debug, Default)]
pub struct MockOutput {
    pub states: Vec<GamepadState>,
}

impl GamepadOutput for MockOutput {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        state.validate()?;
        self.states.push(*state);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpFormat {
    Compact,
    Pretty,
    Json,
    Raw,
}

pub struct DumpOutput<W: Write> {
    writer: W,
    format: DumpFormat,
    sequence: u16,
    previous: Option<GamepadState>,
}

impl<W: Write> DumpOutput<W> {
    pub fn new(writer: W, format: DumpFormat) -> Self {
        Self {
            writer,
            format,
            sequence: 0,
            previous: None,
        }
    }
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: Write> GamepadOutput for DumpOutput<W> {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        state.validate()?;
        if self.previous == Some(*state) {
            return Ok(());
        }
        match self.format {
            DumpFormat::Compact => writeln!(self.writer, "seq={} buttons={:#010x} hat={:?} lx={:.3} ly={:.3} rx={:.3} ry={:.3} lt={:.3} rt={:.3}", self.sequence, state.buttons.0, state.hat, state.left_x, state.left_y, state.right_x, state.right_y, state.left_trigger, state.right_trigger)?,
            DumpFormat::Pretty => writeln!(self.writer, "sequence: {}\n  buttons: {:#010x}\n  hat: {:?}\n  left: ({:.3}, {:.3})\n  right: ({:.3}, {:.3})\n  triggers: ({:.3}, {:.3})", self.sequence, state.buttons.0, state.hat, state.left_x, state.left_y, state.right_x, state.right_y, state.left_trigger, state.right_trigger)?,
            DumpFormat::Json => writeln!(self.writer, "{{\"sequence\":{},\"buttons\":{},\"hat\":{},\"left_x\":{},\"left_y\":{},\"right_x\":{},\"right_y\":{},\"left_trigger\":{},\"right_trigger\":{}}}", self.sequence, state.buttons.0, state.hat as u8, state.left_x, state.left_y, state.right_x, state.right_y, state.left_trigger, state.right_trigger)?,
            DumpFormat::Raw => {
                let bytes = state_frame(self.sequence, state)?;
                for byte in bytes { write!(self.writer, "{byte:02x}")?; }
                writeln!(self.writer)?;
            }
        }
        self.previous = Some(*state);
        self.sequence = self.sequence.wrapping_add(1);
        Ok(())
    }
}

pub struct FileOutput<W: Write> {
    writer: W,
    sequence: u16,
}

impl FileOutput<File> {
    /// Creates or truncates a binary frame file.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError`] when the file cannot be created.
    pub fn create(path: impl AsRef<Path>) -> Result<Self, OutputError> {
        Ok(Self::new(File::create(path)?))
    }
}

impl<W: Write> FileOutput<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            sequence: 0,
        }
    }
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: Write> GamepadOutput for FileOutput<W> {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        let bytes = state_frame(self.sequence, state)?;
        self.writer.write_all(&bytes)?;
        self.writer.flush()?;
        self.sequence = self.sequence.wrapping_add(1);
        Ok(())
    }
}

fn state_frame(sequence: u16, state: &GamepadState) -> Result<Vec<u8>, OutputError> {
    let wire = WireGamepadState::try_from(*state)?;
    Ok(Frame::new(sequence, Message::GamepadState(wire)).encode()?)
}

/// Wraps any backend and suppresses consecutive duplicate states.
pub struct ChangedOnly<O> {
    inner: O,
    previous: Option<GamepadState>,
}

impl<O> ChangedOnly<O> {
    pub fn new(inner: O) -> Self {
        Self {
            inner,
            previous: None,
        }
    }
    pub fn into_inner(self) -> O {
        self.inner
    }
}

impl<O: GamepadOutput> GamepadOutput for ChangedOnly<O> {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        state.validate()?;
        if self.previous == Some(*state) {
            return Ok(());
        }
        self.inner.send_state(state)?;
        self.previous = Some(*state);
        Ok(())
    }

    fn send_neutral(&mut self) -> Result<(), OutputError> {
        if self.previous == Some(GamepadState::neutral()) {
            return Ok(());
        }
        self.inner.send_neutral()?;
        self.previous = Some(GamepadState::neutral());
        Ok(())
    }
    fn service(&mut self) -> Result<(), OutputError> {
        self.inner.service()
    }
    fn take_feedback(&mut self) -> Option<OutputFeedback> {
        self.inner.take_feedback()
    }
    fn diagnostics(&self) -> OutputDiagnostics {
        self.inner.diagnostics()
    }
    fn firmware_info(&self) -> Option<FirmwareInfo> {
        self.inner.firmware_info()
    }
    fn request_firmware_install_receipt(
        &mut self,
        request_id: u32,
        receipt: FirmwareInstallReceipt,
    ) -> Result<(), OutputError> {
        self.inner
            .request_firmware_install_receipt(request_id, receipt)
    }
    fn poll_firmware_install_receipt(
        &mut self,
        request_id: u32,
        receipt: FirmwareInstallReceipt,
    ) -> Option<Result<FirmwareInstallReceipt, OutputError>> {
        self.inner
            .poll_firmware_install_receipt(request_id, receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_protocol::{StreamDecoder, GAMEPAD_FRAME_SIZE};
    use gamepad_state::Button;

    #[test]
    fn mock_validates_and_stores_states() {
        let mut output = MockOutput::default();
        output.send_neutral().unwrap();
        assert_eq!(output.states, vec![GamepadState::neutral()]);
        let mut invalid = GamepadState::neutral();
        invalid.left_x = f32::NAN;
        assert!(output.send_state(&invalid).is_err());
    }

    #[test]
    fn changed_only_suppresses_duplicates() {
        let mut output = ChangedOnly::new(MockOutput::default());
        output.send_neutral().unwrap();
        output.send_neutral().unwrap();
        let mut changed = GamepadState::neutral();
        changed.buttons.set(Button::South, true);
        output.send_state(&changed).unwrap();
        assert_eq!(output.into_inner().states.len(), 2);
    }

    #[test]
    fn file_output_writes_sequenced_decodable_frames() {
        let mut output = FileOutput::new(Vec::new());
        output.send_neutral().unwrap();
        output.send_neutral().unwrap();
        let bytes = output.into_inner();
        assert_eq!(bytes.len(), GAMEPAD_FRAME_SIZE * 2);
        let frames = StreamDecoder::new().push(&bytes);
        assert_eq!(frames[0].as_ref().unwrap().sequence, 0);
        assert_eq!(frames[1].as_ref().unwrap().sequence, 1);
    }

    #[test]
    fn dump_formats_emit_and_skip_unchanged() {
        for format in [
            DumpFormat::Compact,
            DumpFormat::Pretty,
            DumpFormat::Json,
            DumpFormat::Raw,
        ] {
            let mut output = DumpOutput::new(Vec::new(), format);
            output.send_neutral().unwrap();
            output.send_neutral().unwrap();
            let text = String::from_utf8(output.into_inner()).unwrap();
            assert_eq!(
                text.lines().count(),
                if format == DumpFormat::Pretty { 6 } else { 1 }
            );
        }
    }
}
