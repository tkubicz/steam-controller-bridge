use std::io::{self, BufRead, Read};

use gamepad_state::{Button, GamepadState, HatState};
use serde::{Deserialize, Serialize};

use crate::{VirtualHidError, VirtualHidErrorClass};

pub const HELPER_PROTOCOL_VERSION: u16 = 3;
pub const DEFAULT_VENDOR_ID: u16 = 0x045e;
pub const DEFAULT_PRODUCT_ID: u16 = 0x028e;
pub const DEVICE_SERIAL_NUMBER: &str = "SCBRIDGE-VIRTUAL-GAMEPAD-1";
pub const DEVICE_PHYSICAL_UNIQUE_ID: &str = "SCBRIDGE-VIRTUAL-GAMEPAD-1";
pub const DEVICE_LOCATION_ID: i32 = 0x5343_4201;
pub const INPUT_REPORT_LEN: usize = 20;
pub const MAX_LINE_LEN: usize = 65_536;
pub const MAX_RAW_REPORT_LEN: usize = 4_096;

/// The working HID descriptor published by Apple's Xbox 360 userspace driver.
pub const GAMEPAD_REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0x05, 0x01, 0x09, 0x3A, 0xA1, 0x02, 0x75, 0x08, 0x95, 0x02,
    0x05, 0x01, 0x09, 0x3F, 0x09, 0x3B, 0x81, 0x01, 0x75, 0x01, 0x15, 0x00, 0x25, 0x01, 0x35, 0x00,
    0x45, 0x01, 0x95, 0x04, 0x05, 0x09, 0x19, 0x0C, 0x29, 0x0F, 0x81, 0x02, 0x75, 0x01, 0x15, 0x00,
    0x25, 0x01, 0x35, 0x00, 0x45, 0x01, 0x95, 0x04, 0x05, 0x09, 0x09, 0x09, 0x09, 0x0A, 0x09, 0x07,
    0x09, 0x08, 0x81, 0x02, 0x75, 0x01, 0x15, 0x00, 0x25, 0x01, 0x35, 0x00, 0x45, 0x01, 0x95, 0x03,
    0x05, 0x09, 0x09, 0x05, 0x09, 0x06, 0x09, 0x0B, 0x81, 0x02, 0x75, 0x01, 0x95, 0x01, 0x81, 0x01,
    0x75, 0x01, 0x15, 0x00, 0x25, 0x01, 0x35, 0x00, 0x45, 0x01, 0x95, 0x04, 0x05, 0x09, 0x19, 0x01,
    0x29, 0x04, 0x81, 0x02, 0x75, 0x08, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x35, 0x00, 0x46, 0xFF, 0x00,
    0x95, 0x02, 0x05, 0x01, 0x09, 0x32, 0x09, 0x35, 0x81, 0x02, 0x75, 0x10, 0x16, 0x00, 0x80, 0x26,
    0xFF, 0x7F, 0x36, 0x00, 0x80, 0x46, 0xFF, 0x7F, 0x05, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x95, 0x02,
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x81, 0x02, 0xC0, 0x05, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x95,
    0x02, 0x05, 0x01, 0x09, 0x33, 0x09, 0x34, 0x81, 0x02, 0xC0, 0x75, 0x08, 0x95, 0x06, 0x81, 0x01,
    0x06, 0x00, 0xFF, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x85, 0x31, 0x09, 0x01, 0x75, 0x08, 0x95, 0x4D,
    0x91, 0x02, 0xC0, 0xC0,
];

pub const NEUTRAL_INPUT_REPORT: [u8; INPUT_REPORT_LEN] =
    [0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HelperRequest {
    Create {
        protocol: u16,
        vendor_id: u16,
        product_id: u16,
    },
    InputReport {
        protocol: u16,
        sequence: u64,
        report: Vec<u8>,
    },
    Shutdown {
        protocol: u16,
        sequence: u64,
    },
}

impl HelperRequest {
    #[must_use]
    pub const fn protocol(&self) -> u16 {
        match self {
            Self::Create { protocol, .. }
            | Self::InputReport { protocol, .. }
            | Self::Shutdown { protocol, .. } => *protocol,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HidReportType {
    Input,
    Output,
    Feature,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HelperResponse {
    Ready {
        protocol: u16,
        vendor_id: u16,
        product_id: u16,
        dry_run: bool,
        bundle_identifier: Option<String>,
        signing_identifier: Option<String>,
        entitlement_present: Option<bool>,
    },
    Applied {
        protocol: u16,
        sequence: u64,
    },
    SetReport {
        protocol: u16,
        event_sequence: u64,
        report_type: HidReportType,
        report_id: u32,
        report: Vec<u8>,
    },
    GetReport {
        protocol: u16,
        event_sequence: u64,
        report_type: HidReportType,
        report_id: u32,
        max_size: usize,
    },
    Fatal {
        protocol: u16,
        class: VirtualHidErrorClass,
        message: String,
    },
}

impl HelperResponse {
    #[must_use]
    pub const fn protocol(&self) -> u16 {
        match self {
            Self::Ready { protocol, .. }
            | Self::Applied { protocol, .. }
            | Self::SetReport { protocol, .. }
            | Self::GetReport { protocol, .. }
            | Self::Fatal { protocol, .. } => *protocol,
        }
    }
}

/// Encodes one validated bridge state into the pinned virtual-gamepad report.
///
/// # Errors
///
/// Returns an error when the state contains an out-of-range value or uses
/// buttons that do not fit the virtual device's 16-button contract.
pub fn encode_input_report(
    state: &GamepadState,
) -> Result<[u8; INPUT_REPORT_LEN], VirtualHidError> {
    validate_state(state)?;

    let mut buttons = match state.hat {
        HatState::North => 0x0001,
        HatState::NorthEast => 0x0001 | 0x0008,
        HatState::East => 0x0008,
        HatState::SouthEast => 0x0002 | 0x0008,
        HatState::South => 0x0002,
        HatState::SouthWest => 0x0002 | 0x0004,
        HatState::West => 0x0004,
        HatState::NorthWest => 0x0001 | 0x0004,
        HatState::Centered => 0,
    };
    for (button, mask) in [
        (Button::Start, 0x0010),
        (Button::Back, 0x0020),
        (Button::LeftStick, 0x0040),
        (Button::RightStick, 0x0080),
        (Button::LeftShoulder, 0x0100),
        (Button::RightShoulder, 0x0200),
        (Button::Guide, 0x0400),
        (Button::South, 0x1000),
        (Button::East, 0x2000),
        (Button::West, 0x4000),
        (Button::North, 0x8000),
    ] {
        if state.buttons.contains(button) {
            buttons |= mask;
        }
    }

    let mut report = NEUTRAL_INPUT_REPORT;
    report[2..4].copy_from_slice(&u16::to_le_bytes(buttons));
    report[4] = encode_trigger(state.left_trigger);
    report[5] = encode_trigger(state.right_trigger);
    for (offset, value) in [
        (6, state.left_x),
        (8, state.left_y),
        (10, state.right_x),
        (12, state.right_y),
    ] {
        report[offset..offset + 2].copy_from_slice(&encode_axis(value).to_le_bytes());
    }
    Ok(report)
}

fn validate_state(state: &GamepadState) -> Result<(), VirtualHidError> {
    state.validate().map_err(|error| {
        VirtualHidError::new(
            VirtualHidErrorClass::InvalidConfiguration,
            error.to_string(),
        )
    })?;
    if state.buttons.0 & !0xffff != 0 {
        return Err(VirtualHidError::new(
            VirtualHidErrorClass::InvalidConfiguration,
            "virtual HID input supports exactly 16 buttons",
        ));
    }
    Ok(())
}

#[allow(clippy::cast_possible_truncation)]
fn encode_axis(value: f32) -> i16 {
    // GamepadState::validate guarantees a finite value in -1.0..=1.0, so the
    // rounded product is exactly representable by i16.
    (value * 32_767.0).round() as i16
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn encode_trigger(value: f32) -> u8 {
    // GamepadState::validate guarantees a finite value in 0.0..=1.0, so the
    // rounded product is exactly representable by u8.
    (value * 255.0).round() as u8
}

/// Checks the length and fixed bytes of an input report.
///
/// # Errors
///
/// Returns a protocol violation for any report outside the pinned wire format.
pub fn validate_input_report(report: &[u8]) -> Result<(), VirtualHidError> {
    if report.len() != INPUT_REPORT_LEN {
        return Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            format!("expected {INPUT_REPORT_LEN}-byte Xbox 360 input report"),
        ));
    }
    if report[0] != 0 || report[1] != 20 {
        return Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            "Xbox 360 input report has invalid type or size bytes",
        ));
    }
    if report[14..].iter().any(|byte| *byte != 0) {
        return Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            "Xbox 360 input report has non-zero reserved bytes",
        ));
    }
    Ok(())
}

/// Reads one size-bounded, newline-terminated JSON protocol message.
///
/// # Errors
///
/// Returns a protocol violation for malformed, oversized, unterminated, or
/// unreadable input.
pub fn read_json_line<T: serde::de::DeserializeOwned>(
    reader: &mut impl BufRead,
) -> Result<Option<T>, VirtualHidError> {
    let mut line = Vec::new();
    let read = Read::by_ref(reader)
        .take((MAX_LINE_LEN + 1) as u64)
        .read_until(b'\n', &mut line)
        .map_err(|error| {
            VirtualHidError::new(VirtualHidErrorClass::ProtocolViolation, error.to_string())
        })?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > MAX_LINE_LEN || !line.ends_with(b"\n") {
        return Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            "helper protocol line is oversized or unterminated",
        ));
    }
    serde_json::from_slice(&line).map(Some).map_err(|error| {
        VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            format!("invalid helper protocol JSON: {error}"),
        )
    })
}

/// Writes and flushes one newline-terminated JSON protocol message.
///
/// # Errors
///
/// Returns an error when serialization or writing fails.
pub fn write_json_line<T: Serialize>(
    writer: &mut impl io::Write,
    value: &T,
) -> Result<(), VirtualHidError> {
    serde_json::to_writer(&mut *writer, value).map_err(|error| {
        VirtualHidError::new(VirtualHidErrorClass::ProtocolViolation, error.to_string())
    })?;
    writer.write_all(b"\n").map_err(|error| {
        VirtualHidError::new(VirtualHidErrorClass::HelperExited, error.to_string())
    })?;
    writer.flush().map_err(|error| {
        VirtualHidError::new(VirtualHidErrorClass::HelperExited, error.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gamepad_state::{Button, HatState};

    #[test]
    fn descriptor_is_pinned() {
        assert_eq!(DEVICE_SERIAL_NUMBER, "SCBRIDGE-VIRTUAL-GAMEPAD-1");
        assert_eq!(DEVICE_PHYSICAL_UNIQUE_ID, "SCBRIDGE-VIRTUAL-GAMEPAD-1");
        assert_eq!(DEVICE_LOCATION_ID, 0x5343_4201);
        assert_eq!(GAMEPAD_REPORT_DESCRIPTOR.len(), 212);
        assert_eq!(GAMEPAD_REPORT_DESCRIPTOR[0..6], [5, 1, 9, 5, 161, 1]);
        assert_eq!(GAMEPAD_REPORT_DESCRIPTOR.last(), Some(&0xc0));
    }

    #[test]
    fn neutral_report_is_exact() {
        assert_eq!(
            encode_input_report(&GamepadState::neutral()).unwrap(),
            NEUTRAL_INPUT_REPORT
        );
    }

    #[test]
    fn report_matches_the_xinput_packet_layout() {
        for (hat, mask) in [
            (HatState::North, 0x0001),
            (HatState::NorthEast, 0x0009),
            (HatState::East, 0x0008),
            (HatState::SouthEast, 0x000a),
            (HatState::South, 0x0002),
            (HatState::SouthWest, 0x0006),
            (HatState::West, 0x0004),
            (HatState::NorthWest, 0x0005),
            (HatState::Centered, 0x0000),
        ] {
            let state = GamepadState {
                hat,
                ..GamepadState::neutral()
            };
            let report = encode_input_report(&state).unwrap();
            assert_eq!(u16::from_le_bytes([report[2], report[3]]), mask);
        }

        let mapped_buttons = [
            (Button::Start, 0x0010),
            (Button::Back, 0x0020),
            (Button::LeftStick, 0x0040),
            (Button::RightStick, 0x0080),
            (Button::LeftShoulder, 0x0100),
            (Button::RightShoulder, 0x0200),
            (Button::Guide, 0x0400),
            (Button::South, 0x1000),
            (Button::East, 0x2000),
            (Button::West, 0x4000),
            (Button::North, 0x8000),
        ];
        for (button, mask) in mapped_buttons {
            let mut state = GamepadState::neutral();
            state.buttons.set(button, true);
            let report = encode_input_report(&state).unwrap();
            assert_eq!(u16::from_le_bytes([report[2], report[3]]), mask);
        }

        let state = GamepadState {
            left_x: -1.0,
            left_y: 1.0,
            right_x: -1.0,
            right_y: 1.0,
            left_trigger: 0.5,
            right_trigger: 1.0,
            ..GamepadState::neutral()
        };
        let report = encode_input_report(&state).unwrap();
        assert_eq!(report[0..2], [0, 20]);
        assert_eq!(report[4..6], [128, 255]);
        assert_eq!(i16::from_le_bytes([report[6], report[7]]), -32_767);
        assert_eq!(i16::from_le_bytes([report[8], report[9]]), 32_767);
        assert_eq!(report[14..], [0; 6]);
    }

    #[test]
    fn invalid_states_and_malformed_reports_are_rejected() {
        for state in [
            GamepadState {
                left_x: f32::NAN,
                ..GamepadState::neutral()
            },
            GamepadState {
                right_y: 1.01,
                ..GamepadState::neutral()
            },
            GamepadState {
                left_trigger: -0.01,
                ..GamepadState::neutral()
            },
        ] {
            assert!(encode_input_report(&state).is_err());
        }
        assert!(validate_input_report(&NEUTRAL_INPUT_REPORT[..19]).is_err());
        let mut wrong_header = NEUTRAL_INPUT_REPORT;
        wrong_header[1] = 19;
        assert!(validate_input_report(&wrong_header).is_err());
        let mut wrong_reserved = NEUTRAL_INPUT_REPORT;
        wrong_reserved[19] = 1;
        assert!(validate_input_report(&wrong_reserved).is_err());
    }

    #[test]
    fn strict_json_rejects_unknown_fields_and_bad_protocol_is_visible() {
        let mut unknown = io::Cursor::new(b"{\"type\":\"create\",\"protocol\":3,\"vendor_id\":1118,\"product_id\":654,\"extra\":true}\n");
        assert!(read_json_line::<HelperRequest>(&mut unknown).is_err());
        let mut wrong = io::Cursor::new(
            b"{\"type\":\"create\",\"protocol\":99,\"vendor_id\":1118,\"product_id\":654}\n",
        );
        let request = read_json_line::<HelperRequest>(&mut wrong)
            .unwrap()
            .unwrap();
        assert_eq!(request.protocol(), 99);
    }

    #[test]
    fn oversized_and_unterminated_lines_are_rejected() {
        let mut oversized = io::Cursor::new(vec![b'a'; MAX_LINE_LEN + 1]);
        assert!(read_json_line::<HelperRequest>(&mut oversized).is_err());
        let mut unterminated = io::Cursor::new(b"{}".to_vec());
        assert!(read_json_line::<HelperRequest>(&mut unterminated).is_err());
    }

    #[test]
    fn checked_in_json_fixtures_match_the_shared_contract() {
        let requests = [
            (
                include_str!("../fixtures/create.jsonl"),
                HelperRequest::Create {
                    protocol: HELPER_PROTOCOL_VERSION,
                    vendor_id: DEFAULT_VENDOR_ID,
                    product_id: DEFAULT_PRODUCT_ID,
                },
            ),
            (
                include_str!("../fixtures/input-neutral.jsonl"),
                HelperRequest::InputReport {
                    protocol: HELPER_PROTOCOL_VERSION,
                    sequence: 1,
                    report: NEUTRAL_INPUT_REPORT.to_vec(),
                },
            ),
            (
                include_str!("../fixtures/shutdown.jsonl"),
                HelperRequest::Shutdown {
                    protocol: HELPER_PROTOCOL_VERSION,
                    sequence: 2,
                },
            ),
        ];
        for (fixture, expected) in requests {
            assert_eq!(serde_json::to_string(&expected).unwrap() + "\n", fixture);
            assert_eq!(
                serde_json::from_str::<HelperRequest>(fixture).unwrap(),
                expected
            );
        }

        let ready = HelperResponse::Ready {
            protocol: HELPER_PROTOCOL_VERSION,
            vendor_id: DEFAULT_VENDOR_ID,
            product_id: DEFAULT_PRODUCT_ID,
            dry_run: true,
            bundle_identifier: None,
            signing_identifier: None,
            entitlement_present: None,
        };
        let fixture = include_str!("../fixtures/ready-dry-run.jsonl");
        assert_eq!(serde_json::to_string(&ready).unwrap() + "\n", fixture);
        assert_eq!(
            serde_json::from_str::<HelperResponse>(fixture).unwrap(),
            ready
        );
    }
}
