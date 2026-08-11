use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use bridge_protocol::{Frame, Message, StreamDecoder, WireGamepadState, PROTOCOL_VERSION};
use gamepad_state::GamepadState;

use crate::{GamepadOutput, OutputDiagnostics, OutputError, OutputFeedback};

pub const XIAO_USB_VENDOR_ID: u16 = 0x045e;
pub const XIAO_USB_PRODUCT_ID: u16 = 0x028e;
pub const XIAO_USB_MANUFACTURER: &str = "Lynxware";
pub const XIAO_USB_PRODUCT: &str = "Steam Controller Bridge";
/// Oldest firmware revision the host considers current. Hand-maintained:
/// raise it only when the bridge depends on newer firmware behavior, so a
/// working older board is not nagged to reflash after app-only releases.
pub const MINIMUM_FIRMWARE_REVISION: u16 = 1;
/// How long after Ready the firmware gets to deliver its `DeviceInfo` report
/// before the connection is classified as pre-versioning firmware.
const FIRMWARE_REPORT_GRACE: Duration = Duration::from_secs(2);
const DEVICE_INFO_FORMAT: u8 = 1;
const SERIAL_SERVICE_MIN_INTERVAL: Duration = Duration::from_millis(10);
const HANDSHAKE_POLL_INTERVAL: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialDeviceInfo {
    pub path: String,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub serial_number: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
}

impl SerialDeviceInfo {
    #[must_use]
    pub fn is_xiao_bridge(&self) -> bool {
        let is_callout = !cfg!(target_os = "macos") || self.path.starts_with("/dev/cu.");
        is_callout
            && self.vendor_id == Some(XIAO_USB_VENDOR_ID)
            && self.product_id == Some(XIAO_USB_PRODUCT_ID)
            && self.manufacturer.as_deref() == Some(XIAO_USB_MANUFACTURER)
            && self.product.as_deref() == Some(XIAO_USB_PRODUCT)
    }
}

pub trait ByteTransport {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerialConfig {
    pub queue_capacity: usize,
    pub handshake_timeout: Duration,
    pub ping_interval: Duration,
    pub pong_timeout: Duration,
    pub state_refresh_interval: Duration,
    pub packet_logging: bool,
}

impl Default for SerialConfig {
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
pub enum SerialStatus {
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

impl FirmwareVersion {
    #[must_use]
    pub const fn revision(self) -> Option<u16> {
        match self {
            Self::Reported(revision) => Some(revision),
            Self::Pending | Self::UnsupportedFormat(_) | Self::Malformed | Self::Unreported => None,
        }
    }

    #[must_use]
    pub const fn update_recommended(self) -> bool {
        match self {
            Self::Unreported | Self::Malformed => true,
            Self::Reported(revision) => revision < MINIMUM_FIRMWARE_REVISION,
            Self::Pending | Self::UnsupportedFormat(_) => false,
        }
    }
}

fn parse_device_info(payload: &[u8]) -> FirmwareVersion {
    match payload {
        // Trailing bytes are future extensions and deliberately ignored.
        [DEVICE_INFO_FORMAT, low, high, ..] => {
            FirmwareVersion::Reported(u16::from_le_bytes([*low, *high]))
        }
        [DEVICE_INFO_FORMAT, ..] | [] => FirmwareVersion::Malformed,
        [format, ..] => FirmwareVersion::UnsupportedFormat(*format),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SerialMetrics {
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
pub enum SerialError {
    Io(io::Error),
    Protocol(bridge_protocol::ProtocolError),
    InvalidState(gamepad_state::InvalidState),
    InvalidConfig(&'static str),
    HandshakeTimeout,
    VersionRejected(u8),
    PongTimeout,
    NotReady,
}

impl std::fmt::Display for SerialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "serial I/O failed: {error}"),
            Self::Protocol(error) => write!(f, "serial protocol failed: {error}"),
            Self::InvalidState(error) => write!(f, "invalid gamepad state: {error}"),
            Self::InvalidConfig(field) => write!(f, "invalid serial configuration field {field}"),
            Self::HandshakeTimeout => write!(f, "serial hello handshake timed out"),
            Self::VersionRejected(version) => write!(
                f,
                "firmware selected unsupported protocol version {version}"
            ),
            Self::PongTimeout => write!(f, "serial pong timed out"),
            Self::NotReady => write!(f, "serial session is not ready"),
        }
    }
}

impl std::error::Error for SerialError {}
impl From<io::Error> for SerialError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<bridge_protocol::ProtocolError> for SerialError {
    fn from(value: bridge_protocol::ProtocolError) -> Self {
        Self::Protocol(value)
    }
}
impl From<gamepad_state::InvalidState> for SerialError {
    fn from(value: gamepad_state::InvalidState) -> Self {
        Self::InvalidState(value)
    }
}

pub struct SerialConnection<T> {
    transport: T,
    config: SerialConfig,
    status: SerialStatus,
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
    firmware: FirmwareVersion,
    metrics: SerialMetrics,
}

impl<T: ByteTransport> SerialConnection<T> {
    /// Starts a session and immediately transmits the version-negotiation hello.
    ///
    /// # Errors
    /// Returns an error for invalid configuration, framing, or transport writes.
    pub fn new(transport: T, config: SerialConfig, now: Duration) -> Result<Self, SerialError> {
        if config.queue_capacity == 0 {
            return Err(SerialError::InvalidConfig("queue_capacity"));
        }
        if config.handshake_timeout.is_zero() {
            return Err(SerialError::InvalidConfig("handshake_timeout"));
        }
        if config.state_refresh_interval.is_zero() {
            return Err(SerialError::InvalidConfig("state_refresh_interval"));
        }
        let mut connection = Self {
            transport,
            config,
            status: SerialStatus::Handshaking,
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
            firmware: FirmwareVersion::default(),
            metrics: SerialMetrics::default(),
        };
        connection.write_message(Message::Hello {
            minimum_version: PROTOCOL_VERSION,
            maximum_version: PROTOCOL_VERSION,
        })?;
        Ok(connection)
    }

    #[must_use]
    pub const fn status(&self) -> SerialStatus {
        self.status
    }
    #[must_use]
    pub const fn firmware(&self) -> FirmwareVersion {
        self.firmware
    }
    #[must_use]
    pub const fn metrics(&self) -> SerialMetrics {
        self.metrics
    }
    pub fn into_inner(self) -> T {
        self.transport
    }

    pub fn take_feedback(&mut self) -> Option<OutputFeedback> {
        self.pending_feedback.take()
    }

    /// Queues a validated state, dropping the oldest at the capacity limit.
    ///
    /// # Errors
    /// Returns an error when the state cannot be represented on the wire.
    pub fn queue_state(&mut self, state: GamepadState) -> Result<(), SerialError> {
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
    pub fn poll(&mut self, now: Duration) -> Result<(), SerialError> {
        let mut bytes = [0_u8; 512];
        match self.transport.read(&mut bytes) {
            Ok(0) => {}
            Ok(count) => {
                if self.config.packet_logging {
                    eprintln!("serial rx: {}", hex_bytes(&bytes[..count]));
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
                self.status = SerialStatus::Disconnected;
                return Err(error.into());
            }
        }
        if self.status == SerialStatus::Handshaking
            && now.saturating_sub(self.started) >= self.config.handshake_timeout
        {
            self.status = SerialStatus::Disconnected;
            return Err(SerialError::HandshakeTimeout);
        }
        if self.firmware == FirmwareVersion::Pending && self.status == SerialStatus::Ready {
            if let Some(ready_at) = self.ready_at {
                if now.saturating_sub(ready_at) >= FIRMWARE_REPORT_GRACE {
                    self.firmware = FirmwareVersion::Unreported;
                }
            }
        }
        if let Some((_, sent)) = self.pending_ping {
            if now.saturating_sub(sent) >= self.config.pong_timeout {
                self.status = SerialStatus::Unhealthy;
                return Err(SerialError::PongTimeout);
            }
        }
        if self.status == SerialStatus::Ready
            && self.pending_ping.is_none()
            && now.saturating_sub(self.last_ping) >= self.config.ping_interval
        {
            let nonce = u32::from(self.sequence) | (u32::from(self.sequence) << 16);
            self.write_message(Message::Ping { nonce })?;
            self.pending_ping = Some((nonce, now));
            self.last_ping = now;
        }
        self.flush_states(now)?;
        if self.status == SerialStatus::Ready
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
    /// Returns [`SerialError::NotReady`] or a protocol/transport failure.
    pub fn send_neutral_now(&mut self) -> Result<(), SerialError> {
        if self.status != SerialStatus::Ready {
            return Err(SerialError::NotReady);
        }
        self.queued.clear();
        self.last_state = None;
        self.last_state_sent = None;
        self.write_message(Message::Neutral)
    }

    fn handle_message(&mut self, message: &Message, now: Duration) -> Result<(), SerialError> {
        match message {
            Message::HelloResponse { selected_version }
                if *selected_version == PROTOCOL_VERSION =>
            {
                self.status = SerialStatus::Ready;
                self.ready_at = Some(now);
            }
            Message::HelloResponse { selected_version } => {
                self.status = SerialStatus::Disconnected;
                return Err(SerialError::VersionRejected(*selected_version));
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
            Message::Rumble {
                low_frequency,
                high_frequency,
            } if self.status == SerialStatus::Ready => {
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

    fn flush_states(&mut self, now: Duration) -> Result<(), SerialError> {
        if self.status != SerialStatus::Ready {
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

    fn write_message(&mut self, message: Message) -> Result<(), SerialError> {
        let bytes = Frame::new(self.sequence, message).encode()?;
        if self.config.packet_logging {
            eprintln!("serial tx: {}", hex_bytes(&bytes));
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

struct NativeTransport(Box<dyn serialport::SerialPort>);
impl ByteTransport for NativeTransport {
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

pub struct SerialOutput {
    path: String,
    baud_rate: u32,
    config: SerialConfig,
    connection: Option<SerialConnection<NativeTransport>>,
    clock: Instant,
    completed: SerialMetrics,
    connected_once: bool,
    desired_state: Option<GamepadState>,
    last_poll: Option<Duration>,
}

impl SerialOutput {
    /// Opens a native serial port and completes the protocol hello handshake.
    ///
    /// # Errors
    /// Returns an error when opening, negotiation, framing, or I/O fails.
    pub fn open(path: &str, baud_rate: u32, config: SerialConfig) -> Result<Self, SerialError> {
        let mut output = Self {
            path: path.to_owned(),
            baud_rate,
            config,
            connection: None,
            clock: Instant::now(),
            completed: SerialMetrics::default(),
            connected_once: false,
            desired_state: None,
            last_poll: None,
        };
        output.connect()?;
        Ok(output)
    }

    /// Advances input parsing and connection health checks.
    ///
    /// # Errors
    /// Returns protocol, transport, handshake, or health-check failures.
    pub fn poll(&mut self) -> Result<(), SerialError> {
        if self.connection.is_none() {
            self.connect()?;
        }
        let now = self.clock.elapsed();
        let Some(connection) = self.connection.as_mut() else {
            return Err(SerialError::NotReady);
        };
        let result = connection.poll(now);
        self.last_poll = Some(now);
        if result.is_err() {
            self.disconnect();
        }
        result
    }
    #[must_use]
    pub fn status(&self) -> SerialStatus {
        self.connection
            .as_ref()
            .map_or(SerialStatus::Disconnected, SerialConnection::status)
    }
    #[must_use]
    pub fn metrics(&self) -> SerialMetrics {
        let active = self
            .connection
            .as_ref()
            .map_or(SerialMetrics::default(), SerialConnection::metrics);
        SerialMetrics {
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

    fn connect(&mut self) -> Result<(), SerialError> {
        let port = serialport::new(&self.path, self.baud_rate)
            // Idle reads must not consume a large part of the host service
            // cadence; otherwise state refreshes can approach the firmware's
            // 100 ms controller-data watchdog after USB/CDC scheduling.
            .timeout(Duration::from_millis(1))
            .open()
            .map_err(|error| SerialError::Io(io::Error::other(error.to_string())))?;
        self.clock = Instant::now();
        self.last_poll = None;
        let mut connection =
            SerialConnection::new(NativeTransport(port), self.config, Duration::ZERO)?;
        while connection.status() == SerialStatus::Handshaking {
            connection.poll(self.clock.elapsed())?;
            if connection.status() == SerialStatus::Handshaking {
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

impl GamepadOutput for SerialOutput {
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
                return Err(OutputError::Transport(SerialError::NotReady.to_string()));
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
        self.desired_state = None;
        self.connection
            .as_mut()
            .ok_or_else(|| OutputError::Transport(SerialError::NotReady.to_string()))?
            .send_neutral_now()
            .map_err(|error| OutputError::Transport(error.to_string()))
    }
    fn service(&mut self) -> Result<(), OutputError> {
        if !serial_service_due(self.clock.elapsed(), self.last_poll) {
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
            .and_then(SerialConnection::take_feedback)
    }

    fn firmware_version(&self) -> Option<FirmwareVersion> {
        self.connection.as_ref().map(SerialConnection::firmware)
    }

    fn diagnostics(&self) -> OutputDiagnostics {
        let metrics = self.metrics();
        OutputDiagnostics {
            serial_reconnects: metrics.reconnects,
            framing_failures: metrics.framing_failures,
            checksum_failures: metrics.checksum_failures,
            state_refreshes: metrics.state_refreshes,
            rumble_commands_received: metrics.rumble_commands_received,
            rumble_commands_coalesced: metrics.rumble_commands_coalesced,
        }
    }
}

impl Drop for SerialOutput {
    fn drop(&mut self) {
        if let Some(connection) = &mut self.connection {
            let _ = connection.send_neutral_now();
        }
    }
}

fn serial_service_due(now: Duration, last_poll: Option<Duration>) -> bool {
    last_poll.is_none_or(|last_poll| now.saturating_sub(last_poll) >= SERIAL_SERVICE_MIN_INTERVAL)
}

/// Enumerates native serial port names.
///
/// # Errors
/// Returns an error when the native backend cannot enumerate ports.
pub fn available_serial_ports() -> Result<Vec<String>, SerialError> {
    available_serial_devices().map(|ports| ports.into_iter().map(|port| port.path).collect())
}

/// Enumerates native serial ports with USB identity metadata.
///
/// # Errors
/// Returns an error when the native backend cannot enumerate ports.
pub fn available_serial_devices() -> Result<Vec<SerialDeviceInfo>, SerialError> {
    let mut devices = serialport::available_ports()
        .map_err(|error| SerialError::Io(io::Error::other(error.to_string())))?
        .into_iter()
        .map(|port| {
            let (vendor_id, product_id, serial_number, manufacturer, product) = match port.port_type
            {
                serialport::SerialPortType::UsbPort(usb) => (
                    Some(usb.vid),
                    Some(usb.pid),
                    usb.serial_number,
                    usb.manufacturer,
                    usb.product,
                ),
                _ => (None, None, None, None, None),
            };
            SerialDeviceInfo {
                path: port.port_name,
                vendor_id,
                product_id,
                serial_number,
                manufacturer,
                product,
            }
        })
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn xiao_filter_uses_complete_usb_identity_and_rejects_the_puck() {
        let xiao = SerialDeviceInfo {
            path: if cfg!(target_os = "macos") {
                "/dev/cu.usbmodem11201".to_owned()
            } else {
                "/dev/ttyACM0".to_owned()
            },
            vendor_id: Some(XIAO_USB_VENDOR_ID),
            product_id: Some(XIAO_USB_PRODUCT_ID),
            serial_number: Some("TESTSERIAL0000".to_owned()),
            manufacturer: Some(XIAO_USB_MANUFACTURER.to_owned()),
            product: Some(XIAO_USB_PRODUCT.to_owned()),
        };
        assert!(xiao.is_xiao_bridge());

        let mut puck = xiao.clone();
        puck.path = if cfg!(target_os = "macos") {
            "/dev/cu.usbmodemFXB9961501D831".to_owned()
        } else {
            "/dev/ttyACM1".to_owned()
        };
        puck.vendor_id = Some(0x28de);
        puck.product_id = Some(0x1304);
        puck.manufacturer = Some("Valve Software".to_owned());
        puck.product = Some("Steam Controller Puck".to_owned());
        assert!(!puck.is_xiao_bridge());

        let mut dialin = xiao;
        dialin.path = "/dev/tty.usbmodem11201".to_owned();
        assert_eq!(dialin.is_xiao_bridge(), !cfg!(target_os = "macos"));
    }

    #[test]
    fn serial_service_cadence_avoids_redundant_polls_without_missing_deadlines() {
        assert!(serial_service_due(Duration::ZERO, None));
        assert!(!serial_service_due(
            SERIAL_SERVICE_MIN_INTERVAL
                .checked_sub(Duration::from_nanos(1))
                .unwrap(),
            Some(Duration::ZERO)
        ));
        assert!(serial_service_due(
            SERIAL_SERVICE_MIN_INTERVAL,
            Some(Duration::ZERO)
        ));
        assert!(!serial_service_due(
            Duration::from_millis(105),
            Some(Duration::from_millis(100))
        ));
        assert!(serial_service_due(
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

    #[test]
    fn handshake_flushes_only_the_latest_queued_snapshot() {
        let transport = MockTransport {
            reads: VecDeque::from([response(Message::HelloResponse {
                selected_version: 1,
            })]),
            writes: Vec::new(),
        };
        let mut connection =
            SerialConnection::new(transport, SerialConfig::default(), Duration::ZERO).unwrap();
        connection.queue_state(GamepadState::neutral()).unwrap();
        let mut stale = GamepadState::neutral();
        stale.left_y = -1.0;
        connection.queue_state(stale).unwrap();
        let mut latest = GamepadState::neutral();
        latest.left_y = 1.0;
        connection.queue_state(latest).unwrap();
        connection.poll(Duration::from_millis(1)).unwrap();
        assert_eq!(connection.status(), SerialStatus::Ready);
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
            SerialConnection::new(transport, SerialConfig::default(), Duration::ZERO).unwrap();
        assert!(matches!(
            connection.poll(Duration::ZERO),
            Err(SerialError::VersionRejected(2))
        ));
        let mut connection = SerialConnection::new(
            MockTransport::default(),
            SerialConfig::default(),
            Duration::ZERO,
        )
        .unwrap();
        assert!(matches!(
            connection.poll(Duration::from_secs(1)),
            Err(SerialError::HandshakeTimeout)
        ));
    }

    #[test]
    fn bounded_queue_drops_oldest_and_health_check_requires_matching_pong() {
        let config = SerialConfig {
            queue_capacity: 1,
            ping_interval: Duration::from_millis(10),
            pong_timeout: Duration::from_millis(20),
            ..SerialConfig::default()
        };
        let transport = MockTransport {
            reads: VecDeque::from([response(Message::HelloResponse {
                selected_version: 1,
            })]),
            writes: Vec::new(),
        };
        let mut connection = SerialConnection::new(transport, config, Duration::ZERO).unwrap();
        connection.queue_state(GamepadState::neutral()).unwrap();
        let mut active = GamepadState::neutral();
        active.left_x = 1.0;
        connection.queue_state(active).unwrap();
        connection.poll(Duration::ZERO).unwrap();
        connection.poll(Duration::from_millis(10)).unwrap();
        assert_eq!(connection.metrics().states_dropped, 1);
        assert!(matches!(
            connection.poll(Duration::from_millis(30)),
            Err(SerialError::PongTimeout)
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
            SerialConnection::new(transport, SerialConfig::default(), Duration::ZERO).unwrap();
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
            SerialConnection::new(transport, SerialConfig::default(), Duration::ZERO).unwrap();
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
            SerialConnection::new(transport, SerialConfig::default(), Duration::ZERO).unwrap();
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
    fn firmware_reports_parse_and_tolerate_trailing_bytes() {
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
            SerialConnection::new(transport, SerialConfig::default(), Duration::ZERO).unwrap();
        assert_eq!(connection.firmware(), FirmwareVersion::Pending);
        connection.poll(Duration::ZERO).unwrap();
        connection.poll(Duration::ZERO).unwrap();
        assert_eq!(connection.firmware(), FirmwareVersion::Reported(3));
        assert_eq!(connection.firmware().revision(), Some(3));
        assert!(!connection.firmware().update_recommended());

        assert_eq!(
            parse_device_info(&[1, 7, 1, 0xaa, 0xbb]),
            FirmwareVersion::Reported(263)
        );
    }

    #[test]
    fn unsupported_and_malformed_device_info_remain_distinct() {
        assert_eq!(
            parse_device_info(&[2, 9, 9]),
            FirmwareVersion::UnsupportedFormat(2)
        );
        assert_eq!(
            parse_device_info(&[2]),
            FirmwareVersion::UnsupportedFormat(2)
        );
        assert_eq!(parse_device_info(&[1]), FirmwareVersion::Malformed);
        assert_eq!(parse_device_info(&[]), FirmwareVersion::Malformed);
        assert!(!FirmwareVersion::UnsupportedFormat(2).update_recommended());
        assert!(FirmwareVersion::Malformed.update_recommended());
        assert!(!FirmwareVersion::Pending.update_recommended());
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
            SerialConnection::new(transport, SerialConfig::default(), Duration::ZERO).unwrap();
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
        assert!(connection.firmware().update_recommended());
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
            SerialConnection::new(transport, SerialConfig::default(), Duration::ZERO).unwrap();
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
    fn the_minimum_revision_bounds_the_update_recommendation() {
        assert!(
            FirmwareVersion::Reported(MINIMUM_FIRMWARE_REVISION.saturating_sub(1))
                .update_recommended()
                || MINIMUM_FIRMWARE_REVISION == 0
        );
        assert!(!FirmwareVersion::Reported(MINIMUM_FIRMWARE_REVISION).update_recommended());
        assert!(!FirmwareVersion::Reported(MINIMUM_FIRMWARE_REVISION + 1).update_recommended());
    }

    #[test]
    fn rejects_zero_refresh_interval() {
        let config = SerialConfig {
            state_refresh_interval: Duration::ZERO,
            ..SerialConfig::default()
        };
        assert!(matches!(
            SerialConnection::new(MockTransport::default(), config, Duration::ZERO),
            Err(SerialError::InvalidConfig("state_refresh_interval"))
        ));
    }
}
