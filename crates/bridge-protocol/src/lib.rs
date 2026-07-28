//! Versioned framing for the host-to-firmware byte stream.

use gamepad_state::{GamepadButtons, GamepadState, HatState, InvalidState};

pub const MAGIC: [u8; 2] = *b"SC";
pub const PROTOCOL_VERSION: u8 = 1;
pub const HEADER_SIZE: usize = 8;
pub const CHECKSUM_SIZE: usize = 2;
pub const MAX_PAYLOAD_SIZE: usize = 256;
pub const GAMEPAD_PAYLOAD_SIZE: usize = 18;
pub const RUMBLE_PAYLOAD_SIZE: usize = 4;
pub const GAMEPAD_FRAME_SIZE: usize = HEADER_SIZE + GAMEPAD_PAYLOAD_SIZE + CHECKSUM_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Hello = 1,
    HelloResponse = 2,
    GamepadState = 3,
    Neutral = 4,
    Ping = 5,
    Pong = 6,
    DeviceInfo = 7,
    Rumble = 8,
    Error = 255,
}

impl TryFrom<u8> for MessageType {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, ()> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::HelloResponse),
            3 => Ok(Self::GamepadState),
            4 => Ok(Self::Neutral),
            5 => Ok(Self::Ping),
            6 => Ok(Self::Pong),
            7 => Ok(Self::DeviceInfo),
            8 => Ok(Self::Rumble),
            255 => Ok(Self::Error),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Hello {
        minimum_version: u8,
        maximum_version: u8,
    },
    HelloResponse {
        selected_version: u8,
    },
    GamepadState(WireGamepadState),
    Neutral,
    Ping {
        nonce: u32,
    },
    Pong {
        nonce: u32,
    },
    DeviceInfo(Vec<u8>),
    Rumble {
        low_frequency: u16,
        high_frequency: u16,
    },
    Error {
        code: u16,
        detail: Vec<u8>,
    },
    Unknown {
        message_type: u8,
        payload: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireGamepadState {
    pub buttons: u32,
    pub hat: u8,
    pub flags: u8,
    pub left_x: i16,
    pub left_y: i16,
    pub right_x: i16,
    pub right_y: i16,
    pub left_trigger: u16,
    pub right_trigger: u16,
}

impl TryFrom<GamepadState> for WireGamepadState {
    type Error = InvalidState;

    fn try_from(state: GamepadState) -> Result<Self, Self::Error> {
        state.validate()?;
        Ok(Self {
            buttons: state.buttons.0,
            hat: state.hat as u8,
            flags: 0,
            left_x: encode_stick(state.left_x),
            left_y: encode_stick(state.left_y),
            right_x: encode_stick(state.right_x),
            right_y: encode_stick(state.right_y),
            left_trigger: encode_trigger(state.left_trigger),
            right_trigger: encode_trigger(state.right_trigger),
        })
    }
}

impl TryFrom<WireGamepadState> for GamepadState {
    type Error = ProtocolError;

    fn try_from(state: WireGamepadState) -> Result<Self, Self::Error> {
        if state.left_x == i16::MIN
            || state.left_y == i16::MIN
            || state.right_x == i16::MIN
            || state.right_y == i16::MIN
        {
            return Err(ProtocolError::ReservedAxisValue);
        }
        Ok(Self {
            buttons: GamepadButtons(state.buttons),
            hat: HatState::try_from(state.hat).map_err(|_| ProtocolError::InvalidHat(state.hat))?,
            left_x: decode_stick(state.left_x),
            left_y: decode_stick(state.left_y),
            right_x: decode_stick(state.right_x),
            right_y: decode_stick(state.right_y),
            left_trigger: decode_trigger(state.left_trigger),
            right_trigger: decode_trigger(state.right_trigger),
        })
    }
}

#[allow(clippy::cast_possible_truncation)] // Validated input is bounded; rounding is the wire contract.
fn encode_stick(value: f32) -> i16 {
    (value * 32767.0).round() as i16
}
fn decode_stick(value: i16) -> f32 {
    f32::from(value) / 32767.0
}
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Validated input is 0..=1.
fn encode_trigger(value: f32) -> u16 {
    (value * 65535.0).round() as u16
}
fn decode_trigger(value: u16) -> f32 {
    f32::from(value) / 65535.0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub version: u8,
    pub sequence: u16,
    pub message: Message,
}

impl Frame {
    #[must_use]
    pub fn new(sequence: u16, message: Message) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            sequence,
            message,
        }
    }

    /// Encodes the frame using the explicit v1 wire layout.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError`] for an unsupported version or oversized payload.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        if self.version != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(self.version));
        }
        let (message_type, payload) = encode_message(&self.message);
        if payload.len() > MAX_PAYLOAD_SIZE {
            return Err(ProtocolError::PayloadTooLarge(payload.len()));
        }
        let payload_len = u16::try_from(payload.len())
            .map_err(|_| ProtocolError::PayloadTooLarge(payload.len()))?;
        let mut bytes = Vec::with_capacity(HEADER_SIZE + payload.len() + CHECKSUM_SIZE);
        bytes.extend_from_slice(&MAGIC);
        bytes.push(self.version);
        bytes.push(message_type);
        bytes.extend_from_slice(&payload_len.to_le_bytes());
        bytes.extend_from_slice(&self.sequence.to_le_bytes());
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&crc16_ccitt_false(&bytes).to_le_bytes());
        Ok(bytes)
    }
}

fn encode_message(message: &Message) -> (u8, Vec<u8>) {
    match message {
        Message::Hello {
            minimum_version,
            maximum_version,
        } => (
            MessageType::Hello as u8,
            vec![*minimum_version, *maximum_version],
        ),
        Message::HelloResponse { selected_version } => {
            (MessageType::HelloResponse as u8, vec![*selected_version])
        }
        Message::GamepadState(state) => (MessageType::GamepadState as u8, encode_gamepad(*state)),
        Message::Neutral => (MessageType::Neutral as u8, Vec::new()),
        Message::Ping { nonce } => (MessageType::Ping as u8, nonce.to_le_bytes().to_vec()),
        Message::Pong { nonce } => (MessageType::Pong as u8, nonce.to_le_bytes().to_vec()),
        Message::DeviceInfo(info) => (MessageType::DeviceInfo as u8, info.clone()),
        Message::Rumble {
            low_frequency,
            high_frequency,
        } => {
            let mut payload = Vec::with_capacity(RUMBLE_PAYLOAD_SIZE);
            payload.extend_from_slice(&low_frequency.to_le_bytes());
            payload.extend_from_slice(&high_frequency.to_le_bytes());
            (MessageType::Rumble as u8, payload)
        }
        Message::Error { code, detail } => {
            let mut payload = code.to_le_bytes().to_vec();
            payload.extend_from_slice(detail);
            (MessageType::Error as u8, payload)
        }
        Message::Unknown {
            message_type,
            payload,
        } => (*message_type, payload.clone()),
    }
}

fn encode_gamepad(state: WireGamepadState) -> Vec<u8> {
    let mut payload = Vec::with_capacity(GAMEPAD_PAYLOAD_SIZE);
    payload.extend_from_slice(&state.buttons.to_le_bytes());
    payload.push(state.hat);
    payload.push(state.flags);
    for axis in [state.left_x, state.left_y, state.right_x, state.right_y] {
        payload.extend_from_slice(&axis.to_le_bytes());
    }
    payload.extend_from_slice(&state.left_trigger.to_le_bytes());
    payload.extend_from_slice(&state.right_trigger.to_le_bytes());
    payload
}

fn parse_message(message_type: u8, payload: &[u8]) -> Result<Message, ProtocolError> {
    let known = MessageType::try_from(message_type);
    match known {
        Ok(MessageType::Hello) => {
            exact_len(payload, 2)?;
            Ok(Message::Hello {
                minimum_version: payload[0],
                maximum_version: payload[1],
            })
        }
        Ok(MessageType::HelloResponse) => {
            exact_len(payload, 1)?;
            Ok(Message::HelloResponse {
                selected_version: payload[0],
            })
        }
        Ok(MessageType::GamepadState) => {
            exact_len(payload, GAMEPAD_PAYLOAD_SIZE)?;
            Ok(Message::GamepadState(parse_gamepad(payload)?))
        }
        Ok(MessageType::Neutral) => {
            exact_len(payload, 0)?;
            Ok(Message::Neutral)
        }
        Ok(MessageType::Ping) => {
            exact_len(payload, 4)?;
            Ok(Message::Ping {
                nonce: u32::from_le_bytes(payload.try_into().expect("length checked")),
            })
        }
        Ok(MessageType::Pong) => {
            exact_len(payload, 4)?;
            Ok(Message::Pong {
                nonce: u32::from_le_bytes(payload.try_into().expect("length checked")),
            })
        }
        Ok(MessageType::DeviceInfo) => Ok(Message::DeviceInfo(payload.to_vec())),
        Ok(MessageType::Rumble) => {
            exact_len(payload, RUMBLE_PAYLOAD_SIZE)?;
            Ok(Message::Rumble {
                low_frequency: u16::from_le_bytes([payload[0], payload[1]]),
                high_frequency: u16::from_le_bytes([payload[2], payload[3]]),
            })
        }
        Ok(MessageType::Error) => {
            if payload.len() < 2 {
                return Err(ProtocolError::InvalidPayloadLength {
                    expected: 2,
                    actual: payload.len(),
                });
            }
            Ok(Message::Error {
                code: u16::from_le_bytes([payload[0], payload[1]]),
                detail: payload[2..].to_vec(),
            })
        }
        Err(()) => Ok(Message::Unknown {
            message_type,
            payload: payload.to_vec(),
        }),
    }
}

fn parse_gamepad(p: &[u8]) -> Result<WireGamepadState, ProtocolError> {
    let i16_at = |i| i16::from_le_bytes([p[i], p[i + 1]]);
    let state = WireGamepadState {
        buttons: u32::from_le_bytes(p[0..4].try_into().expect("length checked")),
        hat: p[4],
        flags: p[5],
        left_x: i16_at(6),
        left_y: i16_at(8),
        right_x: i16_at(10),
        right_y: i16_at(12),
        left_trigger: u16::from_le_bytes([p[14], p[15]]),
        right_trigger: u16::from_le_bytes([p[16], p[17]]),
    };
    let _ = GamepadState::try_from(state)?;
    Ok(state)
}

fn exact_len(payload: &[u8], expected: usize) -> Result<(), ProtocolError> {
    if payload.len() == expected {
        Ok(())
    } else {
        Err(ProtocolError::InvalidPayloadLength {
            expected,
            actual: payload.len(),
        })
    }
}

/// CRC-16/CCITT-FALSE: poly 0x1021, init 0xffff, refin false, refout false, xorout 0.
#[must_use]
pub fn crc16_ccitt_false(bytes: &[u8]) -> u16 {
    let mut crc = 0xffff_u16;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    UnsupportedVersion(u8),
    PayloadTooLarge(usize),
    ChecksumMismatch { expected: u16, actual: u16 },
    InvalidPayloadLength { expected: usize, actual: usize },
    InvalidHat(u8),
    ReservedAxisValue,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ProtocolError {}

#[derive(Debug, Default)]
pub struct StreamDecoder {
    buffer: Vec<u8>,
}

impl StreamDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds bytes and returns every complete frame or recoverable error found.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<Frame, ProtocolError>> {
        self.buffer.extend_from_slice(bytes);
        let mut output = Vec::new();
        loop {
            let Some(magic_at) = self.buffer.windows(2).position(|w| w == MAGIC) else {
                if self.buffer.last() == Some(&MAGIC[0]) {
                    self.buffer.drain(..self.buffer.len() - 1);
                } else {
                    self.buffer.clear();
                }
                break;
            };
            if magic_at > 0 {
                self.buffer.drain(..magic_at);
            }
            if self.buffer.len() < HEADER_SIZE {
                break;
            }
            let payload_len = usize::from(u16::from_le_bytes([self.buffer[4], self.buffer[5]]));
            if payload_len > MAX_PAYLOAD_SIZE {
                output.push(Err(ProtocolError::PayloadTooLarge(payload_len)));
                self.buffer.drain(..1);
                continue;
            }
            let frame_len = HEADER_SIZE + payload_len + CHECKSUM_SIZE;
            if self.buffer.len() < frame_len {
                break;
            }
            let candidate = &self.buffer[..frame_len];
            let actual = u16::from_le_bytes([candidate[frame_len - 2], candidate[frame_len - 1]]);
            let expected = crc16_ccitt_false(&candidate[..frame_len - 2]);
            if actual != expected {
                output.push(Err(ProtocolError::ChecksumMismatch { expected, actual }));
                self.buffer.drain(..1);
                continue;
            }
            let version = candidate[2];
            if version != PROTOCOL_VERSION {
                output.push(Err(ProtocolError::UnsupportedVersion(version)));
                self.buffer.drain(..frame_len);
                continue;
            }
            let sequence = u16::from_le_bytes([candidate[6], candidate[7]]);
            let parsed =
                parse_message(candidate[3], &candidate[8..8 + payload_len]).map(|message| Frame {
                    version,
                    sequence,
                    message,
                });
            output.push(parsed);
            self.buffer.drain(..frame_len);
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages() -> Vec<Message> {
        vec![
            Message::Hello {
                minimum_version: 1,
                maximum_version: 1,
            },
            Message::HelloResponse {
                selected_version: 1,
            },
            Message::GamepadState(WireGamepadState {
                buttons: 0xa55a,
                hat: 7,
                flags: 1,
                left_x: -32767,
                left_y: 0,
                right_x: 32767,
                right_y: -1,
                left_trigger: 0,
                right_trigger: 65535,
            }),
            Message::Neutral,
            Message::Ping { nonce: 0x1234_5678 },
            Message::Pong { nonce: 9 },
            Message::DeviceInfo(b"xiao".to_vec()),
            Message::Rumble {
                low_frequency: 0x1234,
                high_frequency: 0xabcd,
            },
            Message::Error {
                code: 42,
                detail: b"bad".to_vec(),
            },
            Message::Unknown {
                message_type: 99,
                payload: vec![1, 2, 3],
            },
        ]
    }

    #[test]
    fn round_trips_every_message_type() {
        for message in messages() {
            let frame = Frame::new(65535, message);
            let decoded = StreamDecoder::new()
                .push(&frame.encode().unwrap())
                .pop()
                .unwrap()
                .unwrap();
            assert_eq!(decoded, frame);
        }
    }

    #[test]
    fn rumble_has_a_stable_little_endian_golden_vector() {
        let frame = Frame::new(
            0x5678,
            Message::Rumble {
                low_frequency: 0x1234,
                high_frequency: 0xabcd,
            },
        )
        .encode()
        .unwrap();
        assert_eq!(
            &frame[..12],
            &[b'S', b'C', 1, 8, 4, 0, 0x78, 0x56, 0x34, 0x12, 0xcd, 0xab]
        );
        assert!(matches!(
            parse_message(MessageType::Rumble as u8, &[0, 0, 0]),
            Err(ProtocolError::InvalidPayloadLength {
                expected: 4,
                actual: 3
            })
        ));
    }

    #[test]
    fn parses_partial_and_multiple_frames() {
        let a = Frame::new(1, Message::Neutral).encode().unwrap();
        let b = Frame::new(2, Message::Ping { nonce: 3 }).encode().unwrap();
        let mut decoder = StreamDecoder::new();
        assert!(decoder.push(&a[..3]).is_empty());
        let mut rest = a[3..].to_vec();
        rest.extend_from_slice(&b);
        let result = decoder.push(&rest);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].as_ref().unwrap().sequence, 1);
        assert_eq!(result[1].as_ref().unwrap().sequence, 2);
    }

    #[test]
    fn rejects_corruption_and_recovers_after_garbage() {
        let mut bad = Frame::new(1, Message::Ping { nonce: 7 }).encode().unwrap();
        bad[8] ^= 0xff;
        let good = Frame::new(2, Message::Neutral).encode().unwrap();
        let mut input = b"garbage".to_vec();
        input.extend_from_slice(&bad);
        input.extend_from_slice(&good);
        let result = StreamDecoder::new().push(&input);
        assert!(matches!(
            result[0],
            Err(ProtocolError::ChecksumMismatch { .. })
        ));
        assert_eq!(result.last().unwrap().as_ref().unwrap().sequence, 2);
    }

    #[test]
    fn rejects_version_and_oversized_payload_then_recovers() {
        let mut version = Frame::new(1, Message::Neutral).encode().unwrap();
        version[2] = 2;
        let crc = crc16_ccitt_false(&version[..version.len() - 2]).to_le_bytes();
        let len = version.len();
        version[len - 2..].copy_from_slice(&crc);
        assert_eq!(
            StreamDecoder::new().push(&version),
            vec![Err(ProtocolError::UnsupportedVersion(2))]
        );

        let mut oversized = vec![b'S', b'C', 1, 4, 1, 1, 0, 0];
        oversized.extend_from_slice(&Frame::new(3, Message::Neutral).encode().unwrap());
        let result = StreamDecoder::new().push(&oversized);
        assert!(matches!(
            result[0],
            Err(ProtocolError::PayloadTooLarge(257))
        ));
        assert_eq!(result.last().unwrap().as_ref().unwrap().sequence, 3);
    }

    #[test]
    fn truncated_frame_waits_without_error() {
        let frame = Frame::new(1, Message::Neutral).encode().unwrap();
        assert!(StreamDecoder::new()
            .push(&frame[..frame.len() - 1])
            .is_empty());
    }

    #[test]
    fn axis_extremes_round_trip_and_min_is_rejected() {
        let state = GamepadState {
            left_x: -1.0,
            left_y: 1.0,
            right_x: 0.0,
            right_y: -1.0,
            left_trigger: 0.0,
            right_trigger: 1.0,
            ..GamepadState::neutral()
        };
        let wire = WireGamepadState::try_from(state).unwrap();
        assert_eq!(
            (
                wire.left_x,
                wire.left_y,
                wire.left_trigger,
                wire.right_trigger
            ),
            (-32767, 32767, 0, 65535)
        );
        let decoded = GamepadState::try_from(wire).unwrap();
        assert_eq!(decoded, state);
        let mut invalid = wire;
        invalid.left_x = i16::MIN;
        assert_eq!(
            GamepadState::try_from(invalid),
            Err(ProtocolError::ReservedAxisValue)
        );
    }

    #[test]
    fn crc_standard_check_and_static_neutral_vector() {
        assert_eq!(crc16_ccitt_false(b"123456789"), 0x29b1);
        assert_eq!(
            Frame::new(0, Message::Neutral).encode().unwrap(),
            vec![0x53, 0x43, 0x01, 0x04, 0x00, 0x00, 0x00, 0x00, 0xe7, 0xfb]
        );
    }

    #[test]
    fn arbitrary_byte_chunks_never_panic() {
        let mut decoder = StreamDecoder::new();
        let mut value = 0x1234_5678_u32;
        for length in 0..512 {
            let bytes: Vec<u8> = (0..length)
                .map(|_| {
                    value = value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (value >> 24) as u8
                })
                .collect();
            let _ = decoder.push(&bytes);
        }
    }
}
