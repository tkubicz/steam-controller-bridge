use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bridge_protocol::{
    Frame, InstallReceipt, InstallSource, Message, StreamDecoder, WireGamepadState,
    PROTOCOL_VERSION,
};
use gamepad_state::GamepadState;

use crate::{
    endpoint_discovery::{open_error, BridgeEndpointLocator},
    BridgeEndpoint, GamepadOutput, OutputDiagnostics, OutputError, OutputFeedback,
    BRIDGE_BUSY_ERROR_MARKER,
};
/// How long after Ready the firmware gets to deliver its `DeviceInfo` report
/// before the connection is classified as pre-versioning firmware.
const FIRMWARE_REPORT_GRACE: Duration = Duration::from_secs(2);
const DEVICE_INFO_FORMAT: u8 = 1;
const DEVICE_INFO_EXTENSION_SIZE: usize = 8;
const DEVICE_INFO_RECORDED_SIZE: usize = 33;
const FIRMWARE_TARGET_TLV: u8 = 1;
pub const MAX_FIRMWARE_TARGET_ID_LEN: usize = 64;
const TRANSPORT_SERVICE_MIN_INTERVAL: Duration = Duration::from_millis(10);
const HANDSHAKE_POLL_INTERVAL: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FirmwareTargetId {
    length: u8,
    bytes: [u8; MAX_FIRMWARE_TARGET_ID_LEN],
}

impl FirmwareTargetId {
    /// Creates a bounded target identifier suitable for the `DeviceInfo` wire
    /// extension and signed release catalog lookup.
    ///
    /// # Errors
    /// Returns an error unless the value is a non-empty lowercase ASCII slug
    /// containing only letters, digits, dots, and hyphens.
    pub fn new(value: &str) -> Result<Self, FirmwareTargetIdError> {
        if value.is_empty()
            || value.len() > MAX_FIRMWARE_TARGET_ID_LEN
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte)
            })
            || !value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !value
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
        {
            return Err(FirmwareTargetIdError);
        }
        let length = u8::try_from(value.len()).map_err(|_| FirmwareTargetIdError)?;
        let mut bytes = [0; MAX_FIRMWARE_TARGET_ID_LEN];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self { length, bytes })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.length)]).unwrap_or_default()
    }
}

impl std::fmt::Debug for FirmwareTargetId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("FirmwareTargetId")
            .field(&self.as_str())
            .finish()
    }
}

impl std::fmt::Display for FirmwareTargetId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<&str> for FirmwareTargetId {
    type Error = FirmwareTargetIdError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareTargetIdError;

impl std::fmt::Display for FirmwareTargetIdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("firmware target ID must be 1-64 lowercase ASCII slug characters")
    }
}

impl std::error::Error for FirmwareTargetIdError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FirmwareTarget {
    #[default]
    Unreported,
    Reported(FirmwareTargetId),
    Malformed,
}

pub trait ByteTransport: Send {
    /// Writes one complete protocol frame.
    ///
    /// # Errors
    /// Returns an I/O error when the transport cannot accept all bytes.
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()>;
    /// Reads currently available stream bytes.
    ///
    /// # Errors
    /// Returns an I/O error or a non-fatal timeout/would-block condition.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
}

impl<T: ByteTransport + ?Sized> ByteTransport for Box<T> {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        (**self).write_all(bytes)
    }

    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        (**self).read(buffer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeTransportConfig {
    pub queue_capacity: usize,
    pub handshake_timeout: Duration,
    pub ping_interval: Duration,
    pub pong_timeout: Duration,
    pub state_refresh_interval: Duration,
    pub packet_logging: bool,
}

impl Default for BridgeTransportConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 8,
            handshake_timeout: Duration::from_secs(1),
            ping_interval: Duration::from_secs(1),
            pong_timeout: Duration::from_secs(2),
            state_refresh_interval: Duration::from_millis(25),
            packet_logging: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeConnectionStatus {
    Handshaking,
    Ready,
    Unhealthy,
    Disconnected,
}

/// What the firmware has reported about itself on this connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FirmwareVersion {
    /// Still inside the post-handshake reporting grace window.
    #[default]
    Pending,
    /// The firmware's hand-maintained monotonic revision.
    Reported(u16),
    /// A device-info report arrived in a format this build does not
    /// understand - firmware newer than the host, never a firmware-update
    /// recommendation.
    UnsupportedFormat(u8),
    /// A report used the current format but omitted required fields.
    Malformed,
    /// The grace window elapsed without a report: the firmware predates
    /// version reporting.
    Unreported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FirmwareCapabilities(u32);

impl FirmwareCapabilities {
    pub const ENTER_UF2_BOOTLOADER: Self = Self(1 << 0);
    pub const INSTALL_RECEIPT: Self = Self(1 << 1);

    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, capability: Self) -> bool {
        self.0 & capability.0 == capability.0
    }
}

impl std::ops::BitOr for FirmwareCapabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareInstallSource {
    AppCenter,
    FirstObserved,
}

impl From<FirmwareInstallSource> for InstallSource {
    fn from(value: FirmwareInstallSource) -> Self {
        match value {
            FirmwareInstallSource::AppCenter => Self::AppCenter,
            FirmwareInstallSource::FirstObserved => Self::FirstObserved,
        }
    }
}

impl From<InstallSource> for FirmwareInstallSource {
    fn from(value: InstallSource) -> Self {
        match value {
            InstallSource::AppCenter => Self::AppCenter,
            InstallSource::FirstObserved => Self::FirstObserved,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareInstallReceipt {
    pub installed_at: u64,
    pub install_id: [u8; 16],
    pub source: FirmwareInstallSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareReceiptCreationError(String);

impl std::fmt::Display for FirmwareReceiptCreationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for FirmwareReceiptCreationError {}

/// Generates an installation receipt from the current UTC Unix time and OS randomness.
///
/// # Errors
/// Returns an error when the system clock predates the Unix epoch, the timestamp
/// exceeds the wire format's signed display range, or OS randomness is unavailable.
pub fn new_firmware_install_receipt(
    source: FirmwareInstallSource,
) -> Result<FirmwareInstallReceipt, FirmwareReceiptCreationError> {
    let installed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| FirmwareReceiptCreationError(error.to_string()))?
        .as_secs();
    if installed_at == 0 || i64::try_from(installed_at).is_err() {
        return Err(FirmwareReceiptCreationError(
            "system time is outside the installation receipt range".to_owned(),
        ));
    }
    let mut install_id = [0_u8; 16];
    getrandom::fill(&mut install_id)
        .map_err(|error| FirmwareReceiptCreationError(format!("OS randomness failed: {error}")))?;
    if install_id == [0; 16] {
        return Err(FirmwareReceiptCreationError(
            "OS randomness returned an empty installation ID".to_owned(),
        ));
    }
    Ok(FirmwareInstallReceipt {
        installed_at,
        install_id,
        source,
    })
}

/// Generates an unpredictable request identifier for firmware control messages.
///
/// # Errors
/// Returns an error when operating-system randomness is unavailable.
pub fn random_firmware_request_id() -> Result<u32, FirmwareReceiptCreationError> {
    let mut bytes = [0_u8; 4];
    getrandom::fill(&mut bytes)
        .map_err(|error| FirmwareReceiptCreationError(format!("OS randomness failed: {error}")))?;
    Ok(u32::from_le_bytes(bytes))
}

impl From<FirmwareInstallReceipt> for InstallReceipt {
    fn from(value: FirmwareInstallReceipt) -> Self {
        Self {
            installed_at: value.installed_at,
            install_id: value.install_id,
            source: value.source.into(),
        }
    }
}

impl From<InstallReceipt> for FirmwareInstallReceipt {
    fn from(value: InstallReceipt) -> Self {
        Self {
            installed_at: value.installed_at,
            install_id: value.install_id,
            source: value.source.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FirmwareInstallState {
    #[default]
    Unsupported,
    Pending,
    Recorded(FirmwareInstallReceipt),
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FirmwareInfo {
    pub target: FirmwareTarget,
    pub version: FirmwareVersion,
    pub capabilities: FirmwareCapabilities,
    pub install_state: FirmwareInstallState,
}

impl FirmwareInfo {
    #[must_use]
    pub const fn firmware_version(self) -> FirmwareVersion {
        self.version
    }
}

fn validate_receipt_response(
    expected_request_id: u32,
    expected_receipt: FirmwareInstallReceipt,
    actual_request_id: u32,
    actual_receipt: FirmwareInstallReceipt,
) -> Result<FirmwareInstallReceipt, BridgeTransportError> {
    if actual_request_id != expected_request_id {
        return Err(BridgeTransportError::RequestMismatch {
            expected: expected_request_id,
            actual: actual_request_id,
        });
    }
    if actual_receipt != expected_receipt {
        return Err(BridgeTransportError::ReceiptMismatch);
    }
    Ok(actual_receipt)
}

impl FirmwareVersion {
    #[must_use]
    pub const fn revision(self) -> Option<u16> {
        match self {
            Self::Reported(revision) => Some(revision),
            Self::Pending | Self::UnsupportedFormat(_) | Self::Malformed | Self::Unreported => None,
        }
    }
}

fn parse_device_info(payload: &[u8]) -> FirmwareInfo {
    match payload {
        [DEVICE_INFO_FORMAT, low, high] => FirmwareInfo {
            version: FirmwareVersion::Reported(u16::from_le_bytes([*low, *high])),
            ..FirmwareInfo::default()
        },
        [DEVICE_INFO_FORMAT, ..] if payload.len() < DEVICE_INFO_EXTENSION_SIZE => FirmwareInfo {
            version: FirmwareVersion::Malformed,
            ..FirmwareInfo::default()
        },
        [DEVICE_INFO_FORMAT, low, high, cap0, cap1, cap2, cap3, state, ..] => {
            let version = FirmwareVersion::Reported(u16::from_le_bytes([*low, *high]));
            let capabilities =
                FirmwareCapabilities::from_bits(u32::from_le_bytes([*cap0, *cap1, *cap2, *cap3]));
            let (install_state, extension_offset) = match *state {
                0 => (
                    FirmwareInstallState::Unsupported,
                    DEVICE_INFO_EXTENSION_SIZE,
                ),
                1 => (FirmwareInstallState::Pending, DEVICE_INFO_EXTENSION_SIZE),
                2 if payload.len() >= DEVICE_INFO_RECORDED_SIZE => {
                    let installed_at =
                        u64::from_le_bytes(payload[8..16].try_into().expect("length checked"));
                    let install_id: [u8; 16] = payload[16..32].try_into().expect("length checked");
                    let state = match InstallSource::try_from(payload[32]) {
                        Ok(source)
                            if installed_at > 0
                                && i64::try_from(installed_at).is_ok()
                                && install_id != [0; 16] =>
                        {
                            FirmwareInstallState::Recorded(FirmwareInstallReceipt {
                                installed_at,
                                install_id,
                                source: source.into(),
                            })
                        }
                        Ok(_) | Err(_) => FirmwareInstallState::Invalid,
                    };
                    (state, DEVICE_INFO_RECORDED_SIZE)
                }
                2 => (FirmwareInstallState::Invalid, payload.len()),
                3..=u8::MAX => (FirmwareInstallState::Invalid, DEVICE_INFO_EXTENSION_SIZE),
            };
            FirmwareInfo {
                target: parse_firmware_target(&payload[extension_offset..]),
                version,
                capabilities,
                install_state,
            }
        }
        [format, ..] if *format != DEVICE_INFO_FORMAT => FirmwareInfo {
            version: FirmwareVersion::UnsupportedFormat(*format),
            ..FirmwareInfo::default()
        },
        _ => FirmwareInfo {
            version: FirmwareVersion::Malformed,
            ..FirmwareInfo::default()
        },
    }
}

fn parse_firmware_target(mut extensions: &[u8]) -> FirmwareTarget {
    let mut target = FirmwareTarget::Unreported;
    while !extensions.is_empty() {
        if extensions.len() < 2 {
            return FirmwareTarget::Malformed;
        }
        let tag = extensions[0];
        let length = usize::from(extensions[1]);
        extensions = &extensions[2..];
        if extensions.len() < length {
            return FirmwareTarget::Malformed;
        }
        let value = &extensions[..length];
        extensions = &extensions[length..];
        if tag != FIRMWARE_TARGET_TLV {
            continue;
        }
        if !matches!(target, FirmwareTarget::Unreported) {
            return FirmwareTarget::Malformed;
        }
        let Ok(value) = std::str::from_utf8(value) else {
            return FirmwareTarget::Malformed;
        };
        let Ok(identifier) = FirmwareTargetId::new(value) else {
            return FirmwareTarget::Malformed;
        };
        target = FirmwareTarget::Reported(identifier);
    }
    target
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BridgeTransportMetrics {
    pub packets_sent: u64,
    pub packets_received: u64,
    pub framing_failures: u64,
    pub checksum_failures: u64,
    pub states_dropped: u64,
    pub reconnects: u64,
    pub state_refreshes: u64,
    pub rumble_commands_received: u64,
    pub rumble_commands_coalesced: u64,
}

#[derive(Debug)]
pub enum BridgeTransportError {
    Io(io::Error),
    AccessDenied(String),
    DeviceBusy(String),
    GamepadUnavailable(String),
    InvalidTopology(String),
    Disconnected(String),
    Protocol(bridge_protocol::ProtocolError),
    InvalidState(gamepad_state::InvalidState),
    InvalidConfig(&'static str),
    HandshakeTimeout,
    VersionRejected(u8),
    PongTimeout,
    NotReady,
    UnsupportedCapability(&'static str),
    ControlTimeout(&'static str),
    ControlRejected { request_id: u32, code: u16 },
    RequestMismatch { expected: u32, actual: u32 },
    ReceiptMismatch,
    UnsupportedTransport(&'static str),
}

impl std::fmt::Display for BridgeTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "bridge transport I/O failed: {error}"),
            Self::AccessDenied(endpoint) => write!(
                f,
                "access to bridge endpoint {endpoint} was denied; install the narrowly matched Linux device-access rule and reconnect the bridge"
            ),
            Self::DeviceBusy(endpoint) => write!(
                f,
                "{BRIDGE_BUSY_ERROR_MARKER} at {endpoint}; another process or driver may own it"
            ),
            Self::GamepadUnavailable(reason) => {
                write!(f, "bridge gamepad output is unavailable: {reason}")
            }
            Self::InvalidTopology(reason) => {
                write!(f, "official bridge USB topology is invalid: {reason}")
            }
            Self::Disconnected(endpoint) => {
                write!(f, "bridge transport disconnected: {endpoint}")
            }
            Self::Protocol(error) => write!(f, "bridge protocol failed: {error}"),
            Self::InvalidState(error) => write!(f, "invalid gamepad state: {error}"),
            Self::InvalidConfig(field) => {
                write!(f, "invalid bridge transport configuration field {field}")
            }
            Self::HandshakeTimeout => write!(f, "bridge hello handshake timed out"),
            Self::VersionRejected(version) => write!(
                f,
                "firmware selected unsupported protocol version {version}"
            ),
            Self::PongTimeout => write!(f, "bridge pong timed out"),
            Self::NotReady => write!(f, "bridge session is not ready"),
            Self::UnsupportedCapability(capability) => {
                write!(f, "firmware does not support {capability}")
            }
            Self::ControlTimeout(operation) => write!(f, "timed out while {operation}"),
            Self::ControlRejected { request_id, code } => write!(
                f,
                "firmware rejected request ID {request_id} with error code {code}"
            ),
            Self::RequestMismatch { expected, actual } => write!(
                f,
                "firmware response used request ID {actual}, expected {expected}"
            ),
            Self::ReceiptMismatch => {
                write!(f, "firmware acknowledged a different installation receipt")
            }
            Self::UnsupportedTransport(transport) => {
                write!(f, "bridge transport {transport} is unsupported on this platform")
            }
        }
    }
}

impl std::error::Error for BridgeTransportError {}
impl From<io::Error> for BridgeTransportError {
    fn from(value: io::Error) -> Self {
        if matches!(
            value.kind(),
            io::ErrorKind::NotConnected
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::UnexpectedEof
        ) {
            Self::Disconnected(value.to_string())
        } else {
            Self::Io(value)
        }
    }
}
impl From<serialport::Error> for BridgeTransportError {
    fn from(value: serialport::Error) -> Self {
        Self::Io(io::Error::other(value.to_string()))
    }
}
impl From<bridge_protocol::ProtocolError> for BridgeTransportError {
    fn from(value: bridge_protocol::ProtocolError) -> Self {
        Self::Protocol(value)
    }
}
impl From<gamepad_state::InvalidState> for BridgeTransportError {
    fn from(value: gamepad_state::InvalidState) -> Self {
        Self::InvalidState(value)
    }
}

pub struct BridgeConnection<T> {
    transport: T,
    config: BridgeTransportConfig,
    status: BridgeConnectionStatus,
    decoder: StreamDecoder,
    sequence: u16,
    queued: VecDeque<GamepadState>,
    started: Duration,
    last_ping: Duration,
    pending_ping: Option<(u32, Duration)>,
    last_state: Option<GamepadState>,
    last_state_sent: Option<Duration>,
    pending_feedback: Option<OutputFeedback>,
    ready_at: Option<Duration>,
    firmware: FirmwareInfo,
    uf2_bootloader_ready: Option<u32>,
    install_receipt_recorded: Option<(u32, FirmwareInstallReceipt)>,
    control_error: Option<(u32, u16)>,
    metrics: BridgeTransportMetrics,
}

impl<T: ByteTransport> BridgeConnection<T> {
    /// Starts a session and immediately transmits the version-negotiation hello.
    ///
    /// # Errors
    /// Returns an error for invalid configuration, framing, or transport writes.
    pub fn new(
        transport: T,
        config: BridgeTransportConfig,
        now: Duration,
    ) -> Result<Self, BridgeTransportError> {
        if config.queue_capacity == 0 {
            return Err(BridgeTransportError::InvalidConfig("queue_capacity"));
        }
        if config.handshake_timeout.is_zero() {
            return Err(BridgeTransportError::InvalidConfig("handshake_timeout"));
        }
        if config.state_refresh_interval.is_zero() {
            return Err(BridgeTransportError::InvalidConfig(
                "state_refresh_interval",
            ));
        }
        let mut connection = Self {
            transport,
            config,
            status: BridgeConnectionStatus::Handshaking,
            decoder: StreamDecoder::new(),
            sequence: 0,
            queued: VecDeque::new(),
            started: now,
            last_ping: now,
            pending_ping: None,
            last_state: None,
            last_state_sent: None,
            pending_feedback: None,
            ready_at: None,
            firmware: FirmwareInfo::default(),
            uf2_bootloader_ready: None,
            install_receipt_recorded: None,
            control_error: None,
            metrics: BridgeTransportMetrics::default(),
        };
        connection.write_message(Message::Hello {
            minimum_version: PROTOCOL_VERSION,
            maximum_version: PROTOCOL_VERSION,
        })?;
        Ok(connection)
    }

    #[must_use]
    pub const fn status(&self) -> BridgeConnectionStatus {
        self.status
    }
    #[must_use]
    pub const fn firmware(&self) -> FirmwareVersion {
        self.firmware.version
    }
    #[must_use]
    pub const fn firmware_info(&self) -> FirmwareInfo {
        self.firmware
    }
    #[must_use]
    pub const fn metrics(&self) -> BridgeTransportMetrics {
        self.metrics
    }
    pub fn into_inner(self) -> T {
        self.transport
    }

    pub fn take_feedback(&mut self) -> Option<OutputFeedback> {
        self.pending_feedback.take()
    }

    /// Requests a firmware-assisted transition to the UF2 bootloader.
    ///
    /// # Errors
    /// Returns an error unless negotiation is complete and the capability was reported.
    pub fn request_uf2_bootloader(&mut self, request_id: u32) -> Result<(), BridgeTransportError> {
        self.require_capability(
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER,
            "automatic UF2 bootloader entry",
        )?;
        self.uf2_bootloader_ready = None;
        self.control_error = None;
        self.write_message(Message::EnterUf2Bootloader { request_id })
    }

    pub fn take_uf2_bootloader_ready(&mut self) -> Option<u32> {
        self.uf2_bootloader_ready.take()
    }

    /// Requests that firmware commit the supplied installation receipt.
    ///
    /// # Errors
    /// Returns an error unless negotiation is complete and the capability was reported.
    pub fn record_install_receipt(
        &mut self,
        request_id: u32,
        receipt: FirmwareInstallReceipt,
    ) -> Result<(), BridgeTransportError> {
        self.require_capability(
            FirmwareCapabilities::INSTALL_RECEIPT,
            "installation receipts",
        )?;
        self.install_receipt_recorded = None;
        self.control_error = None;
        self.write_message(Message::RecordInstallReceipt {
            request_id,
            receipt: receipt.into(),
        })
    }

    pub fn take_install_receipt_recorded(&mut self) -> Option<(u32, FirmwareInstallReceipt)> {
        self.install_receipt_recorded.take()
    }

    fn take_control_error(&mut self, expected: u32) -> Option<BridgeTransportError> {
        self.control_error.take().map(|(request_id, code)| {
            if request_id == expected {
                BridgeTransportError::ControlRejected { request_id, code }
            } else {
                BridgeTransportError::RequestMismatch {
                    expected,
                    actual: request_id,
                }
            }
        })
    }

    fn require_capability(
        &self,
        capability: FirmwareCapabilities,
        name: &'static str,
    ) -> Result<(), BridgeTransportError> {
        if self.status != BridgeConnectionStatus::Ready {
            return Err(BridgeTransportError::NotReady);
        }
        if !self.firmware.capabilities.contains(capability) {
            return Err(BridgeTransportError::UnsupportedCapability(name));
        }
        Ok(())
    }

    /// Queues a validated state, dropping the oldest at the capacity limit.
    ///
    /// # Errors
    /// Returns an error when the state cannot be represented on the wire.
    pub fn queue_state(&mut self, state: GamepadState) -> Result<(), BridgeTransportError> {
        state.validate()?;
        if self.queued.len() == self.config.queue_capacity {
            self.queued.pop_front();
            self.metrics.states_dropped += 1;
        }
        self.queued.push_back(state);
        Ok(())
    }

    /// Processes input, deadlines, health checks, and queued states.
    ///
    /// # Errors
    /// Returns protocol, transport, handshake, or health-check failures.
    pub fn poll(&mut self, now: Duration) -> Result<(), BridgeTransportError> {
        let mut bytes = [0_u8; 512];
        match self.transport.read(&mut bytes) {
            Ok(0) => {}
            Ok(count) => {
                if self.config.packet_logging {
                    eprintln!("bridge rx: {}", hex_bytes(&bytes[..count]));
                }
                for decoded in self.decoder.push(&bytes[..count]) {
                    match decoded {
                        Ok(frame) => {
                            self.metrics.packets_received += 1;
                            self.handle_message(&frame.message, now)?;
                        }
                        Err(error) => {
                            self.metrics.framing_failures += 1;
                            if matches!(
                                error,
                                bridge_protocol::ProtocolError::ChecksumMismatch { .. }
                            ) {
                                self.metrics.checksum_failures += 1;
                            }
                        }
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                self.status = BridgeConnectionStatus::Disconnected;
                return Err(error.into());
            }
        }
        if self.status == BridgeConnectionStatus::Handshaking
            && now.saturating_sub(self.started) >= self.config.handshake_timeout
        {
            self.status = BridgeConnectionStatus::Disconnected;
            return Err(BridgeTransportError::HandshakeTimeout);
        }
        if self.firmware.version == FirmwareVersion::Pending
            && self.status == BridgeConnectionStatus::Ready
        {
            if let Some(ready_at) = self.ready_at {
                if now.saturating_sub(ready_at) >= FIRMWARE_REPORT_GRACE {
                    self.firmware.version = FirmwareVersion::Unreported;
                }
            }
        }
        if let Some((_, sent)) = self.pending_ping {
            if now.saturating_sub(sent) >= self.config.pong_timeout {
                self.status = BridgeConnectionStatus::Unhealthy;
                return Err(BridgeTransportError::PongTimeout);
            }
        }
        if self.status == BridgeConnectionStatus::Ready
            && self.pending_ping.is_none()
            && now.saturating_sub(self.last_ping) >= self.config.ping_interval
        {
            let nonce = u32::from(self.sequence) | (u32::from(self.sequence) << 16);
            self.write_message(Message::Ping { nonce })?;
            self.pending_ping = Some((nonce, now));
            self.last_ping = now;
        }
        self.flush_states(now)?;
        if self.status == BridgeConnectionStatus::Ready
            && self.queued.is_empty()
            && self
                .last_state_sent
                .is_some_and(|sent| now.saturating_sub(sent) >= self.config.state_refresh_interval)
        {
            if let Some(state) = self.last_state {
                self.write_message(Message::GamepadState(WireGamepadState::try_from(state)?))?;
                self.last_state_sent = Some(now);
                self.metrics.state_refreshes += 1;
            }
        }
        Ok(())
    }

    /// Immediately sends the dedicated neutral message on a ready connection.
    ///
    /// # Errors
    /// Returns [`BridgeTransportError::NotReady`] or a protocol/transport failure.
    pub fn send_neutral_now(&mut self) -> Result<(), BridgeTransportError> {
        if self.status != BridgeConnectionStatus::Ready {
            return Err(BridgeTransportError::NotReady);
        }
        self.queued.clear();
        self.last_state = None;
        self.last_state_sent = None;
        self.write_message(Message::Neutral)
    }

    fn handle_message(
        &mut self,
        message: &Message,
        now: Duration,
    ) -> Result<(), BridgeTransportError> {
        match message {
            Message::HelloResponse { selected_version }
                if *selected_version == PROTOCOL_VERSION =>
            {
                self.status = BridgeConnectionStatus::Ready;
                self.ready_at = Some(now);
            }
            Message::HelloResponse { selected_version } => {
                self.status = BridgeConnectionStatus::Disconnected;
                return Err(BridgeTransportError::VersionRejected(*selected_version));
            }
            Message::Ping { nonce } => self.write_message(Message::Pong { nonce: *nonce })?,
            Message::Pong { nonce }
                if self
                    .pending_ping
                    .is_some_and(|(expected, _)| expected == *nonce) =>
            {
                self.pending_ping = None;
            }
            Message::DeviceInfo(payload) => {
                self.firmware = parse_device_info(payload);
            }
            Message::Uf2BootloaderReady { request_id }
                if self.status == BridgeConnectionStatus::Ready =>
            {
                self.uf2_bootloader_ready = Some(*request_id);
            }
            Message::InstallReceiptRecorded {
                request_id,
                receipt,
            } if self.status == BridgeConnectionStatus::Ready => {
                let recorded: FirmwareInstallReceipt = (*receipt).into();
                self.install_receipt_recorded = Some((*request_id, recorded));
            }
            Message::Error { code, detail }
                if self.status == BridgeConnectionStatus::Ready && detail.len() == 4 =>
            {
                self.control_error = Some((
                    u32::from_le_bytes(detail[..4].try_into().expect("length checked")),
                    *code,
                ));
            }
            Message::Rumble {
                low_frequency,
                high_frequency,
            } if self.status == BridgeConnectionStatus::Ready => {
                self.metrics.rumble_commands_received += 1;
                if self
                    .pending_feedback
                    .replace(OutputFeedback::Rumble {
                        low_frequency: *low_frequency,
                        high_frequency: *high_frequency,
                    })
                    .is_some()
                {
                    self.metrics.rumble_commands_coalesced += 1;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn flush_states(&mut self, now: Duration) -> Result<(), BridgeTransportError> {
        if self.status != BridgeConnectionStatus::Ready {
            return Ok(());
        }
        let Some(state) = self.queued.pop_back() else {
            return Ok(());
        };
        self.metrics.states_dropped += self.queued.len() as u64;
        self.queued.clear();
        self.write_message(Message::GamepadState(WireGamepadState::try_from(state)?))?;
        if state == GamepadState::neutral() {
            self.last_state = None;
            self.last_state_sent = None;
        } else {
            self.last_state = Some(state);
            self.last_state_sent = Some(now);
        }
        Ok(())
    }

    fn write_message(&mut self, message: Message) -> Result<(), BridgeTransportError> {
        let bytes = Frame::new(self.sequence, message).encode()?;
        if self.config.packet_logging {
            eprintln!("bridge tx: {}", hex_bytes(&bytes));
        }
        self.transport.write_all(&bytes)?;
        self.sequence = self.sequence.wrapping_add(1);
        self.metrics.packets_sent += 1;
        Ok(())
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

struct SerialPortTransport(Box<dyn serialport::SerialPort>);
impl ByteTransport for SerialPortTransport {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        Write::write_all(&mut self.0, bytes)
    }
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        // The serialport crate implements reads as `poll(timeout)` followed by
        // `read`. During controller discovery there is normally no firmware
        // feedback, so entering that one-millisecond poll on every service
        // tick was measurable idle CPU. FIONREAD/TIOCINQ is nonblocking; keep
        // the existing timeout for writes and only call `read` when bytes are
        // already queued.
        let queued = self
            .0
            .bytes_to_read()
            .map_err(|error| io::Error::other(error.to_string()))?;
        if queued == 0 {
            return Ok(0);
        }
        Read::read(&mut self.0, buffer)
    }
}

trait TransportFactory: Send {
    fn open(
        &mut self,
        endpoint: &BridgeEndpoint,
    ) -> Result<Box<dyn ByteTransport>, BridgeTransportError>;
}

#[derive(Default)]
struct NativeTransportFactory;

impl TransportFactory for NativeTransportFactory {
    fn open(
        &mut self,
        endpoint: &BridgeEndpoint,
    ) -> Result<Box<dyn ByteTransport>, BridgeTransportError> {
        match endpoint.locator() {
            BridgeEndpointLocator::SerialPort { path, baud_rate } => {
                let port = serialport::new(path, *baud_rate)
                    .timeout(Duration::from_millis(1))
                    .open()
                    .map_err(|error| open_error(path, error))?;
                Ok(Box::new(SerialPortTransport(port)))
            }
            BridgeEndpointLocator::LinuxUsb(_) => {
                crate::endpoint_discovery::linux_usb::open(endpoint)
                    .map(|transport| Box::new(transport) as Box<dyn ByteTransport>)
            }
        }
    }
}

pub struct BridgeOutput {
    endpoint: BridgeEndpoint,
    factory: Box<dyn TransportFactory>,
    config: BridgeTransportConfig,
    connection: Option<BridgeConnection<Box<dyn ByteTransport>>>,
    clock: Instant,
    completed: BridgeTransportMetrics,
    connected_once: bool,
    desired_state: Option<GamepadState>,
    last_poll: Option<Duration>,
    bootloader_transition: bool,
}

impl BridgeOutput {
    /// Opens a native bridge endpoint and completes the protocol hello handshake.
    ///
    /// # Errors
    /// Returns an error when opening, negotiation, framing, or I/O fails.
    pub fn open(
        endpoint: BridgeEndpoint,
        config: BridgeTransportConfig,
    ) -> Result<Self, BridgeTransportError> {
        Self::open_with_factory(endpoint, config, Box::<NativeTransportFactory>::default())
    }

    /// Opens an explicitly selected native serial port.
    ///
    /// # Errors
    /// Returns an error when opening, negotiation, framing, or I/O fails.
    pub fn open_serial(
        path: &str,
        baud_rate: u32,
        config: BridgeTransportConfig,
    ) -> Result<Self, BridgeTransportError> {
        Self::open(BridgeEndpoint::serial_port(path, baud_rate), config)
    }

    /// Returns the endpoint used for the current session and reconnects.
    #[must_use]
    pub const fn endpoint(&self) -> &BridgeEndpoint {
        &self.endpoint
    }

    fn open_with_factory(
        endpoint: BridgeEndpoint,
        config: BridgeTransportConfig,
        factory: Box<dyn TransportFactory>,
    ) -> Result<Self, BridgeTransportError> {
        let mut output = Self {
            endpoint,
            factory,
            config,
            connection: None,
            clock: Instant::now(),
            completed: BridgeTransportMetrics::default(),
            connected_once: false,
            desired_state: None,
            last_poll: None,
            bootloader_transition: false,
        };
        output.connect()?;
        Ok(output)
    }

    /// Advances input parsing and connection health checks.
    ///
    /// # Errors
    /// Returns protocol, transport, handshake, or health-check failures.
    pub fn poll(&mut self) -> Result<(), BridgeTransportError> {
        if self.connection.is_none() {
            self.connect()?;
        }
        self.poll_existing()
    }

    /// Waits for the post-negotiation device information report.
    ///
    /// # Errors
    /// Returns an I/O, protocol, or timeout error.
    pub fn wait_for_firmware_info(
        &mut self,
        timeout: Duration,
    ) -> Result<FirmwareInfo, BridgeTransportError> {
        self.wait_for_control_response(timeout, "waiting for firmware information", |connection| {
            let info = connection.firmware_info();
            (info.version != FirmwareVersion::Pending).then_some(Ok(info))
        })
    }

    /// Requests automatic UF2 entry and waits for the correlated readiness response.
    ///
    /// # Errors
    /// Returns an I/O, capability, correlation, or timeout error.
    pub fn enter_uf2_bootloader(
        &mut self,
        request_id: u32,
        timeout: Duration,
    ) -> Result<(), BridgeTransportError> {
        self.connection
            .as_mut()
            .ok_or(BridgeTransportError::NotReady)?
            .request_uf2_bootloader(request_id)?;
        let actual = self.wait_for_control_response(
            timeout,
            "waiting for the UF2 bootloader response",
            |connection| {
                connection
                    .take_uf2_bootloader_ready()
                    .map(Ok)
                    .or_else(|| connection.take_control_error(request_id).map(Err))
            },
        )?;
        if actual != request_id {
            return Err(BridgeTransportError::RequestMismatch {
                expected: request_id,
                actual,
            });
        }
        self.bootloader_transition = true;
        Ok(())
    }

    /// Records a receipt and waits for the correlated committed receipt response.
    ///
    /// # Errors
    /// Returns an I/O, capability, receipt, correlation, or timeout error.
    pub fn record_install_receipt_and_wait(
        &mut self,
        request_id: u32,
        receipt: FirmwareInstallReceipt,
        timeout: Duration,
    ) -> Result<FirmwareInstallReceipt, BridgeTransportError> {
        self.connection
            .as_mut()
            .ok_or(BridgeTransportError::NotReady)?
            .record_install_receipt(request_id, receipt)?;
        let (actual, recorded) = self.wait_for_control_response(
            timeout,
            "waiting for installation receipt recording",
            |connection| {
                connection
                    .take_install_receipt_recorded()
                    .map(Ok)
                    .or_else(|| connection.take_control_error(request_id).map(Err))
            },
        )?;
        let recorded = validate_receipt_response(request_id, receipt, actual, recorded)?;
        if let Some(connection) = self.connection.as_mut() {
            connection.firmware.install_state = FirmwareInstallState::Recorded(recorded);
        }
        Ok(recorded)
    }
    #[must_use]
    pub fn status(&self) -> BridgeConnectionStatus {
        self.connection.as_ref().map_or(
            BridgeConnectionStatus::Disconnected,
            BridgeConnection::status,
        )
    }
    #[must_use]
    pub fn metrics(&self) -> BridgeTransportMetrics {
        let active = self
            .connection
            .as_ref()
            .map_or(BridgeTransportMetrics::default(), BridgeConnection::metrics);
        BridgeTransportMetrics {
            packets_sent: self.completed.packets_sent + active.packets_sent,
            packets_received: self.completed.packets_received + active.packets_received,
            framing_failures: self.completed.framing_failures + active.framing_failures,
            checksum_failures: self.completed.checksum_failures + active.checksum_failures,
            states_dropped: self.completed.states_dropped + active.states_dropped,
            reconnects: self.completed.reconnects,
            state_refreshes: self.completed.state_refreshes + active.state_refreshes,
            rumble_commands_received: self.completed.rumble_commands_received
                + active.rumble_commands_received,
            rumble_commands_coalesced: self.completed.rumble_commands_coalesced
                + active.rumble_commands_coalesced,
        }
    }

    fn connect(&mut self) -> Result<(), BridgeTransportError> {
        let transport = self.factory.open(&self.endpoint)?;
        self.clock = Instant::now();
        self.last_poll = None;
        let mut connection = BridgeConnection::new(transport, self.config, Duration::ZERO)?;
        while connection.status() == BridgeConnectionStatus::Handshaking {
            connection.poll(self.clock.elapsed())?;
            if connection.status() == BridgeConnectionStatus::Handshaking {
                // NativeTransport deliberately avoids a blocking read during
                // normal service. Yield here so an unresponsive exact-match
                // device cannot spin a core for the handshake timeout.
                std::thread::sleep(HANDSHAKE_POLL_INTERVAL);
            }
        }
        if let Some(state) = self.desired_state {
            connection.queue_state(state)?;
        }
        if self.connected_once {
            self.completed.reconnects += 1;
        }
        self.connected_once = true;
        self.connection = Some(connection);
        self.last_poll = Some(self.clock.elapsed());
        Ok(())
    }

    fn poll_existing(&mut self) -> Result<(), BridgeTransportError> {
        let now = self.clock.elapsed();
        let result = self
            .connection
            .as_mut()
            .ok_or(BridgeTransportError::NotReady)?
            .poll(now);
        self.last_poll = Some(now);
        if result.is_err() {
            self.disconnect();
        }
        result
    }

    fn wait_for_control_response<T>(
        &mut self,
        timeout: Duration,
        stage: &'static str,
        mut take_response: impl FnMut(
            &mut BridgeConnection<Box<dyn ByteTransport>>,
        ) -> Option<Result<T, BridgeTransportError>>,
    ) -> Result<T, BridgeTransportError> {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll_existing()?;
            if let Some(response) = self.connection.as_mut().and_then(&mut take_response) {
                return response;
            }
            if Instant::now() >= deadline {
                return Err(BridgeTransportError::ControlTimeout(stage));
            }
            std::thread::sleep(HANDSHAKE_POLL_INTERVAL);
        }
    }

    fn disconnect(&mut self) {
        if let Some(connection) = self.connection.take() {
            let metrics = connection.metrics();
            self.completed.packets_sent += metrics.packets_sent;
            self.completed.packets_received += metrics.packets_received;
            self.completed.framing_failures += metrics.framing_failures;
            self.completed.checksum_failures += metrics.checksum_failures;
            self.completed.states_dropped += metrics.states_dropped;
            self.completed.state_refreshes += metrics.state_refreshes;
            self.completed.rumble_commands_received += metrics.rumble_commands_received;
            self.completed.rumble_commands_coalesced += metrics.rumble_commands_coalesced;
        }
    }
}

impl GamepadOutput for BridgeOutput {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        state.validate()?;
        self.desired_state = Some(*state);
        for attempt in 0..2 {
            let mut connected_here = false;
            if self.connection.is_none() {
                if let Err(error) = self.connect() {
                    if attempt == 1 {
                        return Err(OutputError::Transport(error.to_string()));
                    }
                    continue;
                }
                connected_here = true;
            }
            let Some(connection) = self.connection.as_mut() else {
                return Err(OutputError::Transport(
                    BridgeTransportError::NotReady.to_string(),
                ));
            };
            if !connected_here {
                connection
                    .queue_state(*state)
                    .map_err(|error| OutputError::Transport(error.to_string()))?;
            }
            match self.poll() {
                Ok(()) => return Ok(()),
                Err(error) if attempt == 1 => {
                    return Err(OutputError::Transport(error.to_string()));
                }
                Err(_) => {}
            }
        }
        unreachable!("two reconnect attempts always return")
    }
    fn send_neutral(&mut self) -> Result<(), OutputError> {
        self.desired_state = Some(GamepadState::neutral());
        let result = self
            .connection
            .as_mut()
            .ok_or_else(|| OutputError::Transport(BridgeTransportError::NotReady.to_string()))?
            .send_neutral_now()
            .map_err(|error| OutputError::Transport(error.to_string()));
        if result.is_ok() {
            self.desired_state = None;
        }
        result
    }
    fn service(&mut self) -> Result<(), OutputError> {
        if !transport_service_due(self.clock.elapsed(), self.last_poll) {
            return Ok(());
        }
        for attempt in 0..2 {
            match self.poll() {
                Ok(()) => return Ok(()),
                Err(error) if attempt == 1 => {
                    return Err(OutputError::Transport(error.to_string()));
                }
                Err(_) => {}
            }
        }
        unreachable!("two service attempts always return")
    }

    fn take_feedback(&mut self) -> Option<OutputFeedback> {
        self.connection
            .as_mut()
            .and_then(BridgeConnection::take_feedback)
    }

    fn firmware_info(&self) -> Option<FirmwareInfo> {
        self.connection
            .as_ref()
            .map(BridgeConnection::firmware_info)
    }

    fn request_firmware_install_receipt(
        &mut self,
        request_id: u32,
        receipt: FirmwareInstallReceipt,
    ) -> Result<(), OutputError> {
        self.connection
            .as_mut()
            .ok_or_else(|| OutputError::Transport(BridgeTransportError::NotReady.to_string()))?
            .record_install_receipt(request_id, receipt)
            .map_err(|error| OutputError::Transport(error.to_string()))
    }

    fn poll_firmware_install_receipt(
        &mut self,
        request_id: u32,
        receipt: FirmwareInstallReceipt,
    ) -> Option<Result<FirmwareInstallReceipt, OutputError>> {
        let connection = self.connection.as_mut()?;
        if let Some((actual, recorded)) = connection.take_install_receipt_recorded() {
            return Some(
                validate_receipt_response(request_id, receipt, actual, recorded)
                    .inspect(|recorded| {
                        connection.firmware.install_state =
                            FirmwareInstallState::Recorded(*recorded);
                    })
                    .map_err(|error| OutputError::Transport(error.to_string())),
            );
        }
        connection
            .take_control_error(request_id)
            .map(|error| Err(OutputError::Transport(error.to_string())))
    }

    fn diagnostics(&self) -> OutputDiagnostics {
        let metrics = self.metrics();
        OutputDiagnostics {
            bridge_reconnects: metrics.reconnects,
            framing_failures: metrics.framing_failures,
            checksum_failures: metrics.checksum_failures,
            state_refreshes: metrics.state_refreshes,
            rumble_commands_received: metrics.rumble_commands_received,
            rumble_commands_coalesced: metrics.rumble_commands_coalesced,
            ..OutputDiagnostics::default()
        }
    }
}

impl Drop for BridgeOutput {
    fn drop(&mut self) {
        if !self.bootloader_transition && self.desired_state.is_some() {
            if let Some(connection) = &mut self.connection {
                let _ = connection.send_neutral_now();
            }
        }
    }
}

fn transport_service_due(now: Duration, last_poll: Option<Duration>) -> bool {
    last_poll
        .is_none_or(|last_poll| now.saturating_sub(last_poll) >= TRANSPORT_SERVICE_MIN_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_output_remains_send() {
        fn assert_send<T: Send>() {}

        assert_send::<BridgeOutput>();
    }

    #[test]
    fn bridge_busy_errors_keep_the_bridge_specific_classifier() {
        let error = BridgeTransportError::DeviceBusy("endpoint".to_owned()).to_string();

        assert!(error.contains(BRIDGE_BUSY_ERROR_MARKER));
        assert!(!error.contains("already owned"));
    }
    use std::sync::{Arc, Mutex};

    #[test]
    fn transport_disconnects_keep_a_distinct_error_class() {
        for kind in [
            io::ErrorKind::NotConnected,
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::UnexpectedEof,
        ] {
            assert!(matches!(
                BridgeTransportError::from(io::Error::from(kind)),
                BridgeTransportError::Disconnected(_)
            ));
        }
        assert!(matches!(
            BridgeTransportError::from(io::Error::from(io::ErrorKind::InvalidData)),
            BridgeTransportError::Io(_)
        ));
    }

    #[derive(Default)]
    struct MockTransport {
        reads: VecDeque<Vec<u8>>,
        writes: Vec<u8>,
    }
    impl ByteTransport for MockTransport {
        fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.writes.extend_from_slice(bytes);
            Ok(())
        }
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let Some(bytes) = self.reads.pop_front() else {
                return Err(io::ErrorKind::WouldBlock.into());
            };
            buffer[..bytes.len()].copy_from_slice(&bytes);
            Ok(bytes.len())
        }
    }
    fn response(message: Message) -> Vec<u8> {
        Frame::new(90, message).encode().unwrap()
    }

    #[derive(Debug, PartialEq)]
    enum LifecycleEvent {
        Open,
        Write(Message),
        ReadFailure,
        TransportDrop,
    }

    enum OpenStep {
        Fail(&'static str),
        Transport(VecDeque<io::Result<Vec<u8>>>),
        TransportWithWrites {
            reads: VecDeque<io::Result<Vec<u8>>>,
            writes: VecDeque<io::Result<()>>,
        },
    }

    struct ScriptedFactory {
        steps: VecDeque<OpenStep>,
        events: Arc<Mutex<Vec<LifecycleEvent>>>,
    }

    impl TransportFactory for ScriptedFactory {
        fn open(
            &mut self,
            _endpoint: &BridgeEndpoint,
        ) -> Result<Box<dyn ByteTransport>, BridgeTransportError> {
            self.events.lock().unwrap().push(LifecycleEvent::Open);
            match self.steps.pop_front().expect("scripted open step") {
                OpenStep::Fail(message) => Err(io::Error::other(message).into()),
                OpenStep::Transport(reads) => Ok(Box::new(LifecycleTransport {
                    reads,
                    writes: VecDeque::new(),
                    events: Arc::clone(&self.events),
                })),
                OpenStep::TransportWithWrites { reads, writes } => {
                    Ok(Box::new(LifecycleTransport {
                        reads,
                        writes,
                        events: Arc::clone(&self.events),
                    }))
                }
            }
        }
    }

    struct LifecycleTransport {
        reads: VecDeque<io::Result<Vec<u8>>>,
        writes: VecDeque<io::Result<()>>,
        events: Arc<Mutex<Vec<LifecycleEvent>>>,
    }

    impl ByteTransport for LifecycleTransport {
        fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            for message in messages(bytes) {
                self.events
                    .lock()
                    .unwrap()
                    .push(LifecycleEvent::Write(message));
            }
            self.writes.pop_front().unwrap_or(Ok(()))
        }

        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            match self.reads.pop_front() {
                Some(Ok(bytes)) => {
                    buffer[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
                Some(Err(error)) => {
                    self.events
                        .lock()
                        .unwrap()
                        .push(LifecycleEvent::ReadFailure);
                    Err(error)
                }
                None => Err(io::ErrorKind::WouldBlock.into()),
            }
        }
    }

    impl Drop for LifecycleTransport {
        fn drop(&mut self) {
            self.events
                .lock()
                .unwrap()
                .push(LifecycleEvent::TransportDrop);
        }
    }

    #[test]
    fn transport_service_cadence_avoids_redundant_polls_without_missing_deadlines() {
        assert!(transport_service_due(Duration::ZERO, None));
        assert!(!transport_service_due(
            TRANSPORT_SERVICE_MIN_INTERVAL
                .checked_sub(Duration::from_nanos(1))
                .unwrap(),
            Some(Duration::ZERO)
        ));
        assert!(transport_service_due(
            TRANSPORT_SERVICE_MIN_INTERVAL,
            Some(Duration::ZERO)
        ));
        assert!(!transport_service_due(
            Duration::from_millis(105),
            Some(Duration::from_millis(100))
        ));
        assert!(transport_service_due(
            Duration::from_millis(110),
            Some(Duration::from_millis(100))
        ));
    }

    fn messages(bytes: &[u8]) -> Vec<Message> {
        StreamDecoder::new()
            .push(bytes)
            .into_iter()
            .map(Result::unwrap)
            .map(|frame| frame.message)
            .collect()
    }

    fn output_with_script(
        steps: impl IntoIterator<Item = OpenStep>,
        events: Arc<Mutex<Vec<LifecycleEvent>>>,
    ) -> Result<BridgeOutput, BridgeTransportError> {
        BridgeOutput::open_with_factory(
            BridgeEndpoint::serial_port("fixture", 115_200),
            BridgeTransportConfig::default(),
            Box::new(ScriptedFactory {
                steps: steps.into_iter().collect(),
                events,
            }),
        )
    }

    #[test]
    fn endpoint_open_failure_is_returned_without_starting_a_session() {
        let events = Arc::new(Mutex::new(Vec::new()));

        let error =
            output_with_script([OpenStep::Fail("fixture open failed")], Arc::clone(&events))
                .err()
                .expect("open must fail");

        assert!(error.to_string().contains("fixture open failed"));
        assert_eq!(*events.lock().unwrap(), [LifecycleEvent::Open]);
    }

    #[test]
    fn reconnect_reopens_the_endpoint_and_replays_the_desired_state() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let handshake = || {
            Ok(response(Message::HelloResponse {
                selected_version: PROTOCOL_VERSION,
            }))
        };
        let mut output = output_with_script(
            [
                OpenStep::Transport(VecDeque::from([
                    handshake(),
                    Err(io::Error::new(io::ErrorKind::BrokenPipe, "disconnected")),
                ])),
                OpenStep::Transport(VecDeque::from([handshake()])),
            ],
            Arc::clone(&events),
        )
        .unwrap();
        let mut state = GamepadState::neutral();
        state.left_y = 1.0;

        output.send_state(&state).unwrap();

        assert_eq!(output.metrics().reconnects, 1);
        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, LifecycleEvent::Open))
                .count(),
            2
        );
        assert!(events.iter().any(|event| matches!(
            event,
            LifecycleEvent::Write(Message::GamepadState(wire))
                if *wire == WireGamepadState::try_from(state).unwrap()
        )));
    }

    #[test]
    fn shutdown_sends_neutral_after_active_output_before_releasing_the_transport() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut output = output_with_script(
            [OpenStep::Transport(VecDeque::from([Ok(response(
                Message::HelloResponse {
                    selected_version: PROTOCOL_VERSION,
                },
            ))]))],
            Arc::clone(&events),
        )
        .unwrap();
        let mut state = GamepadState::neutral();
        state.left_x = 1.0;
        output.send_state(&state).unwrap();

        drop(output);

        let events = events.lock().unwrap();
        let neutral = events
            .iter()
            .position(|event| matches!(event, LifecycleEvent::Write(Message::Neutral)))
            .expect("neutral write");
        let released = events
            .iter()
            .position(|event| matches!(event, LifecycleEvent::TransportDrop))
            .expect("transport drop");
        assert!(neutral < released);
    }

    #[test]
    fn shutdown_retries_a_failed_explicit_neutral_before_releasing_the_transport() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut output = output_with_script(
            [OpenStep::TransportWithWrites {
                reads: VecDeque::from([Ok(response(Message::HelloResponse {
                    selected_version: PROTOCOL_VERSION,
                }))]),
                writes: VecDeque::from([
                    Ok(()),
                    Ok(()),
                    Err(io::Error::new(io::ErrorKind::TimedOut, "neutral timeout")),
                    Ok(()),
                ]),
            }],
            Arc::clone(&events),
        )
        .unwrap();
        let mut state = GamepadState::neutral();
        state.left_x = 1.0;
        output.send_state(&state).unwrap();

        assert!(output.send_neutral().is_err());
        assert_eq!(output.desired_state, Some(GamepadState::neutral()));
        drop(output);

        let events = events.lock().unwrap();
        let neutral_writes = events
            .iter()
            .filter(|event| matches!(event, LifecycleEvent::Write(Message::Neutral)))
            .count();
        assert_eq!(neutral_writes, 2);
        assert_eq!(events.last(), Some(&LifecycleEvent::TransportDrop));
    }

    #[test]
    fn handshake_flushes_only_the_latest_queued_snapshot() {
        let transport = MockTransport {
            reads: VecDeque::from([response(Message::HelloResponse {
                selected_version: 1,
            })]),
            writes: Vec::new(),
        };
        let mut connection =
            BridgeConnection::new(transport, BridgeTransportConfig::default(), Duration::ZERO)
                .unwrap();
        connection.queue_state(GamepadState::neutral()).unwrap();
        let mut stale = GamepadState::neutral();
        stale.left_y = -1.0;
        connection.queue_state(stale).unwrap();
        let mut latest = GamepadState::neutral();
        latest.left_y = 1.0;
        connection.queue_state(latest).unwrap();
        connection.poll(Duration::from_millis(1)).unwrap();
        assert_eq!(connection.status(), BridgeConnectionStatus::Ready);
        assert_eq!(connection.metrics().states_dropped, 2);
        assert_eq!(
            messages(&connection.into_inner().writes),
            vec![
                Message::Hello {
                    minimum_version: 1,
                    maximum_version: 1
                },
                Message::GamepadState(WireGamepadState::try_from(latest).unwrap())
            ]
        );
    }

    #[test]
    fn rejects_version_and_handshake_timeout() {
        let transport = MockTransport {
            reads: VecDeque::from([response(Message::HelloResponse {
                selected_version: 2,
            })]),
            writes: Vec::new(),
        };
        let mut connection =
            BridgeConnection::new(transport, BridgeTransportConfig::default(), Duration::ZERO)
                .unwrap();
        assert!(matches!(
            connection.poll(Duration::ZERO),
            Err(BridgeTransportError::VersionRejected(2))
        ));
        let mut connection = BridgeConnection::new(
            MockTransport::default(),
            BridgeTransportConfig::default(),
            Duration::ZERO,
        )
        .unwrap();
        assert!(matches!(
            connection.poll(Duration::from_secs(1)),
            Err(BridgeTransportError::HandshakeTimeout)
        ));
    }

    #[test]
    fn bounded_queue_drops_oldest_and_health_check_requires_matching_pong() {
        let config = BridgeTransportConfig {
            queue_capacity: 1,
            ping_interval: Duration::from_millis(10),
            pong_timeout: Duration::from_millis(20),
            ..BridgeTransportConfig::default()
        };
        let transport = MockTransport {
            reads: VecDeque::from([response(Message::HelloResponse {
                selected_version: 1,
            })]),
            writes: Vec::new(),
        };
        let mut connection = BridgeConnection::new(transport, config, Duration::ZERO).unwrap();
        connection.queue_state(GamepadState::neutral()).unwrap();
        let mut active = GamepadState::neutral();
        active.left_x = 1.0;
        connection.queue_state(active).unwrap();
        connection.poll(Duration::ZERO).unwrap();
        connection.poll(Duration::from_millis(10)).unwrap();
        assert_eq!(connection.metrics().states_dropped, 1);
        assert!(matches!(
            connection.poll(Duration::from_millis(30)),
            Err(BridgeTransportError::PongTimeout)
        ));
    }

    #[test]
    fn responds_to_firmware_ping_and_counts_corrupt_frames() {
        let mut bad = response(Message::Neutral);
        let last = bad.len() - 1;
        bad[last] ^= 1;
        let transport = MockTransport {
            reads: VecDeque::from([response(Message::Ping { nonce: 42 }), bad]),
            writes: Vec::new(),
        };
        let mut connection =
            BridgeConnection::new(transport, BridgeTransportConfig::default(), Duration::ZERO)
                .unwrap();
        connection.poll(Duration::ZERO).unwrap();
        connection.poll(Duration::ZERO).unwrap();
        assert_eq!(connection.metrics().framing_failures, 1);
        assert_eq!(connection.metrics().checksum_failures, 1);
        assert!(messages(&connection.into_inner().writes).contains(&Message::Pong { nonce: 42 }));
    }

    #[test]
    fn stages_only_the_latest_ready_rumble_feedback() {
        let transport = MockTransport {
            reads: VecDeque::from([
                response(Message::Rumble {
                    low_frequency: 1,
                    high_frequency: 2,
                }),
                response(Message::HelloResponse {
                    selected_version: 1,
                }),
                response(Message::Rumble {
                    low_frequency: 3,
                    high_frequency: 4,
                }),
                response(Message::Rumble {
                    low_frequency: 5,
                    high_frequency: 6,
                }),
            ]),
            writes: Vec::new(),
        };
        let mut connection =
            BridgeConnection::new(transport, BridgeTransportConfig::default(), Duration::ZERO)
                .unwrap();
        connection.poll(Duration::ZERO).unwrap();
        assert_eq!(connection.take_feedback(), None);
        connection.poll(Duration::ZERO).unwrap();
        connection.poll(Duration::ZERO).unwrap();
        connection.poll(Duration::ZERO).unwrap();
        assert_eq!(
            connection.take_feedback(),
            Some(OutputFeedback::Rumble {
                low_frequency: 5,
                high_frequency: 6
            })
        );
        assert_eq!(connection.metrics().rumble_commands_received, 2);
        assert_eq!(connection.metrics().rumble_commands_coalesced, 1);
    }

    #[test]
    fn refreshes_unchanged_active_state_and_neutral_clears_refresh() {
        let transport = MockTransport {
            reads: VecDeque::from([response(Message::HelloResponse {
                selected_version: 1,
            })]),
            writes: Vec::new(),
        };
        let mut connection =
            BridgeConnection::new(transport, BridgeTransportConfig::default(), Duration::ZERO)
                .unwrap();
        let mut active = GamepadState::neutral();
        active.left_x = 0.5;
        connection.queue_state(active).unwrap();
        connection.poll(Duration::ZERO).unwrap();
        connection.poll(Duration::from_millis(24)).unwrap();
        assert_eq!(connection.metrics().state_refreshes, 0);
        connection.poll(Duration::from_millis(25)).unwrap();
        assert_eq!(connection.metrics().state_refreshes, 1);
        connection.send_neutral_now().unwrap();
        connection.poll(Duration::from_millis(500)).unwrap();
        assert_eq!(connection.metrics().state_refreshes, 1);

        let sent = messages(&connection.into_inner().writes);
        assert_eq!(
            sent.iter()
                .filter(|message| matches!(message, Message::GamepadState(_)))
                .count(),
            2
        );
        assert!(matches!(sent.last(), Some(Message::Neutral)));
    }

    #[test]
    fn old_and_extended_firmware_reports_parse_without_ambiguity() {
        let transport = MockTransport {
            reads: VecDeque::from([
                response(Message::HelloResponse {
                    selected_version: 1,
                }),
                response(Message::DeviceInfo(vec![1, 3, 0])),
            ]),
            writes: Vec::new(),
        };
        let mut connection =
            BridgeConnection::new(transport, BridgeTransportConfig::default(), Duration::ZERO)
                .unwrap();
        assert_eq!(connection.firmware(), FirmwareVersion::Pending);
        connection.poll(Duration::ZERO).unwrap();
        connection.poll(Duration::ZERO).unwrap();
        assert_eq!(connection.firmware(), FirmwareVersion::Reported(3));
        assert_eq!(connection.firmware().revision(), Some(3));

        assert_eq!(
            parse_device_info(&[1, 7, 1]),
            FirmwareInfo {
                target: FirmwareTarget::Unreported,
                version: FirmwareVersion::Reported(263),
                capabilities: FirmwareCapabilities::default(),
                install_state: FirmwareInstallState::Unsupported,
            }
        );

        let receipt = FirmwareInstallReceipt {
            installed_at: 1_786_456_920,
            install_id: [0xa5; 16],
            source: FirmwareInstallSource::AppCenter,
        };
        let mut extended = vec![1, 2, 0];
        extended.extend_from_slice(
            &(FirmwareCapabilities::ENTER_UF2_BOOTLOADER | FirmwareCapabilities::INSTALL_RECEIPT)
                .bits()
                .to_le_bytes(),
        );
        extended.push(2);
        extended.extend_from_slice(&receipt.installed_at.to_le_bytes());
        extended.extend_from_slice(&receipt.install_id);
        extended.push(InstallSource::AppCenter as u8);
        extended.extend_from_slice(&[0xaa, 2, 0xbb, 0xcc]);
        assert_eq!(
            parse_device_info(&extended),
            FirmwareInfo {
                target: FirmwareTarget::Unreported,
                version: FirmwareVersion::Reported(2),
                capabilities: FirmwareCapabilities::ENTER_UF2_BOOTLOADER
                    | FirmwareCapabilities::INSTALL_RECEIPT,
                install_state: FirmwareInstallState::Recorded(receipt),
            }
        );
        extended.extend_from_slice(&[FIRMWARE_TARGET_TLV, 19]);
        extended.extend_from_slice(b"seeed-xiao-nrf52840");
        assert_eq!(
            parse_device_info(&extended),
            FirmwareInfo {
                target: FirmwareTarget::Reported(
                    FirmwareTargetId::new("seeed-xiao-nrf52840").unwrap()
                ),
                version: FirmwareVersion::Reported(2),
                capabilities: FirmwareCapabilities::ENTER_UF2_BOOTLOADER
                    | FirmwareCapabilities::INSTALL_RECEIPT,
                install_state: FirmwareInstallState::Recorded(receipt),
            }
        );

        let mut targeted = vec![1, 3, 0, 0, 0, 0, 0, 1];
        targeted.extend_from_slice(&[FIRMWARE_TARGET_TLV, 19]);
        targeted.extend_from_slice(b"seeed-xiao-nrf52840");
        assert_eq!(
            parse_device_info(&targeted).target,
            FirmwareTarget::Reported(FirmwareTargetId::new("seeed-xiao-nrf52840").unwrap())
        );
    }

    #[test]
    fn firmware_target_extensions_fail_closed_without_invalidating_the_revision() {
        let payload = |extensions: &[u8]| {
            let mut payload = vec![1, 3, 0, 0, 0, 0, 0, 1];
            payload.extend_from_slice(extensions);
            payload
        };
        for malformed in [
            vec![FIRMWARE_TARGET_TLV],
            vec![FIRMWARE_TARGET_TLV, 4, b'x'],
            vec![FIRMWARE_TARGET_TLV, 1, b'X'],
            vec![FIRMWARE_TARGET_TLV, 1, b'x', FIRMWARE_TARGET_TLV, 1, b'y'],
        ] {
            let info = parse_device_info(&payload(&malformed));
            assert_eq!(info.version, FirmwareVersion::Reported(3));
            assert_eq!(info.target, FirmwareTarget::Malformed);
        }

        let info = parse_device_info(&payload(&[7, 2, 0xaa, 0xbb]));
        assert_eq!(info.target, FirmwareTarget::Unreported);
    }

    #[test]
    fn generated_receipts_use_valid_time_and_operating_system_randomness() {
        let receipt = new_firmware_install_receipt(FirmwareInstallSource::AppCenter).unwrap();
        assert!(receipt.installed_at > 0);
        assert!(i64::try_from(receipt.installed_at).is_ok());
        assert_ne!(receipt.install_id, [0; 16]);
        assert_eq!(receipt.source, FirmwareInstallSource::AppCenter);
        random_firmware_request_id().unwrap();
    }

    #[test]
    fn unsupported_and_malformed_device_info_remain_distinct() {
        assert_eq!(
            parse_device_info(&[2, 9, 9]).version,
            FirmwareVersion::UnsupportedFormat(2)
        );
        assert_eq!(
            parse_device_info(&[2]).version,
            FirmwareVersion::UnsupportedFormat(2)
        );
        assert_eq!(parse_device_info(&[1]).version, FirmwareVersion::Malformed);
        assert_eq!(parse_device_info(&[]).version, FirmwareVersion::Malformed);
        for partial_extension in [vec![1, 2, 0, 1], vec![1, 2, 0, 1, 0, 0, 0]] {
            assert_eq!(
                parse_device_info(&partial_extension).version,
                FirmwareVersion::Malformed
            );
        }
        assert_eq!(
            parse_device_info(&[1, 2, 0, 3, 0, 0, 0, 2]).install_state,
            FirmwareInstallState::Invalid
        );

        let recorded_payload = |installed_at: u64, install_id: [u8; 16], source: u8| {
            let mut payload = vec![1, 2, 0, 3, 0, 0, 0, 2];
            payload.extend_from_slice(&installed_at.to_le_bytes());
            payload.extend_from_slice(&install_id);
            payload.push(source);
            payload
        };
        for invalid in [
            recorded_payload(0, [1; 16], InstallSource::AppCenter as u8),
            recorded_payload(1, [0; 16], InstallSource::AppCenter as u8),
            recorded_payload(i64::MAX as u64 + 1, [1; 16], InstallSource::AppCenter as u8),
            recorded_payload(1, [1; 16], 99),
        ] {
            assert_eq!(
                parse_device_info(&invalid),
                FirmwareInfo {
                    target: FirmwareTarget::Unreported,
                    version: FirmwareVersion::Reported(2),
                    capabilities: FirmwareCapabilities::ENTER_UF2_BOOTLOADER
                        | FirmwareCapabilities::INSTALL_RECEIPT,
                    install_state: FirmwareInstallState::Invalid,
                }
            );
        }
    }

    #[test]
    fn control_requests_require_capabilities_and_keep_request_correlation() {
        let receipt = FirmwareInstallReceipt {
            installed_at: 123,
            install_id: [7; 16],
            source: FirmwareInstallSource::FirstObserved,
        };
        let mut extended = vec![1, 2, 0];
        extended.extend_from_slice(
            &(FirmwareCapabilities::ENTER_UF2_BOOTLOADER | FirmwareCapabilities::INSTALL_RECEIPT)
                .bits()
                .to_le_bytes(),
        );
        extended.push(1);
        let transport = MockTransport {
            reads: VecDeque::from([
                response(Message::HelloResponse {
                    selected_version: 1,
                }),
                response(Message::DeviceInfo(extended)),
                response(Message::Uf2BootloaderReady { request_id: 42 }),
                response(Message::InstallReceiptRecorded {
                    request_id: 43,
                    receipt: receipt.into(),
                }),
            ]),
            writes: Vec::new(),
        };
        let mut connection =
            BridgeConnection::new(transport, BridgeTransportConfig::default(), Duration::ZERO)
                .unwrap();
        connection.poll(Duration::ZERO).unwrap();
        connection.poll(Duration::ZERO).unwrap();
        connection.request_uf2_bootloader(42).unwrap();
        connection.poll(Duration::ZERO).unwrap();
        assert_eq!(connection.take_uf2_bootloader_ready(), Some(42));
        connection.record_install_receipt(43, receipt).unwrap();
        connection.poll(Duration::ZERO).unwrap();
        assert_eq!(
            connection.take_install_receipt_recorded(),
            Some((43, receipt))
        );

        let sent = messages(&connection.into_inner().writes);
        assert!(sent.contains(&Message::EnterUf2Bootloader { request_id: 42 }));
        assert!(sent.contains(&Message::RecordInstallReceipt {
            request_id: 43,
            receipt: receipt.into(),
        }));

        let transport = MockTransport {
            reads: VecDeque::from([
                response(Message::HelloResponse {
                    selected_version: 1,
                }),
                response(Message::DeviceInfo(vec![1, 1, 0])),
            ]),
            writes: Vec::new(),
        };
        let mut old =
            BridgeConnection::new(transport, BridgeTransportConfig::default(), Duration::ZERO)
                .unwrap();
        old.poll(Duration::ZERO).unwrap();
        old.poll(Duration::ZERO).unwrap();
        assert!(matches!(
            old.request_uf2_bootloader(1),
            Err(BridgeTransportError::UnsupportedCapability(_))
        ));
    }

    #[test]
    fn control_errors_are_correlated_before_they_are_reported() {
        let receipt = FirmwareInstallReceipt {
            installed_at: 123,
            install_id: [7; 16],
            source: FirmwareInstallSource::FirstObserved,
        };
        let mut extended = vec![1, 2, 0];
        extended.extend_from_slice(&FirmwareCapabilities::INSTALL_RECEIPT.bits().to_le_bytes());
        extended.push(1);
        let connection_with_error = |request_id: u32| {
            let transport = MockTransport {
                reads: VecDeque::from([
                    response(Message::HelloResponse {
                        selected_version: 1,
                    }),
                    response(Message::DeviceInfo(extended.clone())),
                    response(Message::Error {
                        code: bridge_protocol::ControlErrorCode::InstallReceiptRejected as u16,
                        detail: request_id.to_le_bytes().to_vec(),
                    }),
                ]),
                writes: Vec::new(),
            };
            BridgeConnection::new(transport, BridgeTransportConfig::default(), Duration::ZERO)
                .unwrap()
        };

        let mut matching = connection_with_error(42);
        matching.poll(Duration::ZERO).unwrap();
        matching.poll(Duration::ZERO).unwrap();
        matching.record_install_receipt(42, receipt).unwrap();
        matching.poll(Duration::ZERO).unwrap();
        assert!(matches!(
            matching.take_control_error(42),
            Some(BridgeTransportError::ControlRejected {
                request_id: 42,
                code
            }) if code == bridge_protocol::ControlErrorCode::InstallReceiptRejected as u16
        ));

        let mut stale = connection_with_error(41);
        stale.poll(Duration::ZERO).unwrap();
        stale.poll(Duration::ZERO).unwrap();
        stale.record_install_receipt(42, receipt).unwrap();
        stale.poll(Duration::ZERO).unwrap();
        assert!(matches!(
            stale.take_control_error(42),
            Some(BridgeTransportError::RequestMismatch {
                expected: 42,
                actual: 41
            })
        ));

        let different_receipt = FirmwareInstallReceipt {
            installed_at: receipt.installed_at + 1,
            ..receipt
        };
        assert!(matches!(
            validate_receipt_response(42, receipt, 42, different_receipt),
            Err(BridgeTransportError::ReceiptMismatch)
        ));
    }

    #[test]
    fn silence_after_ready_becomes_unreported_only_past_the_grace() {
        let transport = MockTransport {
            reads: VecDeque::from([response(Message::HelloResponse {
                selected_version: 1,
            })]),
            writes: Vec::new(),
        };
        let mut connection =
            BridgeConnection::new(transport, BridgeTransportConfig::default(), Duration::ZERO)
                .unwrap();
        // The grace clock starts at Ready, not at connection creation.
        connection.poll(Duration::from_millis(500)).unwrap();
        assert_eq!(connection.firmware(), FirmwareVersion::Pending);
        connection
            .poll(Duration::from_millis(499) + FIRMWARE_REPORT_GRACE)
            .unwrap();
        assert_eq!(connection.firmware(), FirmwareVersion::Pending);
        connection
            .poll(Duration::from_millis(500) + FIRMWARE_REPORT_GRACE)
            .unwrap();
        assert_eq!(connection.firmware(), FirmwareVersion::Unreported);
    }

    #[test]
    fn a_late_report_replaces_unreported() {
        let transport = MockTransport {
            reads: VecDeque::from([response(Message::HelloResponse {
                selected_version: 1,
            })]),
            writes: Vec::new(),
        };
        let mut connection =
            BridgeConnection::new(transport, BridgeTransportConfig::default(), Duration::ZERO)
                .unwrap();
        connection.poll(Duration::ZERO).unwrap();
        connection.poll(FIRMWARE_REPORT_GRACE).unwrap();
        assert_eq!(connection.firmware(), FirmwareVersion::Unreported);
        connection
            .transport
            .reads
            .push_back(response(Message::DeviceInfo(vec![1, 2, 0])));
        connection.poll(FIRMWARE_REPORT_GRACE).unwrap();
        assert_eq!(connection.firmware(), FirmwareVersion::Reported(2));
    }

    #[test]
    fn rejects_zero_refresh_interval() {
        let config = BridgeTransportConfig {
            state_refresh_interval: Duration::ZERO,
            ..BridgeTransportConfig::default()
        };
        assert!(matches!(
            BridgeConnection::new(MockTransport::default(), config, Duration::ZERO),
            Err(BridgeTransportError::InvalidConfig(
                "state_refresh_interval"
            ))
        ));
    }
}
