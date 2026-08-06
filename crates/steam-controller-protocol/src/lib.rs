//! Panic-free decoding for Steam Controller 2 host-facing HID reports.
//!
//! The layouts follow `OpenPuck`'s protocol documentation and implementation.

use serde::{Deserialize, Serialize};

pub const INPUT_REPORT_ID: u8 = 0x45;
pub const EXTENDED_INPUT_REPORT_ID: u8 = 0x42;
pub const LIZARD_MOUSE_REPORT_ID: u8 = 0x40;
pub const LIZARD_KEYBOARD_REPORT_ID: u8 = 0x41;
pub const BATTERY_REPORT_ID: u8 = 0x43;
pub const AUX_STATUS_REPORT_ID: u8 = 0x44;
pub const CONNECTION_REPORT_ID: u8 = 0x79;
pub const PERIODIC_STATUS_REPORT_ID: u8 = 0x7b;
pub const INPUT_REPORT_SIZE: usize = 46;
pub const EXTENDED_INPUT_REPORT_SIZE: usize = 54;
pub const LIZARD_MOUSE_REPORT_SIZE: usize = 6;
pub const LIZARD_KEYBOARD_REPORT_SIZE: usize = 9;
pub const FEATURE_REPORT_SIZE: usize = 64;
pub const RUMBLE_OUTPUT_REPORT_SIZE: usize = 10;
pub const RUMBLE_OUTPUT_REPORT_ID: u8 = 0x80;
pub const PAD_HAPTIC_OUTPUT_REPORT_SIZE: usize = 4;
pub const PAD_HAPTIC_OUTPUT_REPORT_ID: u8 = 0x82;
pub const POWER_OFF_COMMAND: u8 = 0x9f;

const FEATURE_REPORT_ID: u8 = 0x01;
const SET_SETTINGS_VALUES: u8 = 0x87;
const CONTROLLER_SETTING_SIZE: u8 = 3;
const SETTING_LIZARD_MODE: u8 = 9;
const LIZARD_MODE_OFF: u16 = 0;
const POWER_OFF_PAYLOAD: [u8; 4] = *b"off!";

/// Builds the fixed lizard-suppression feature report this project permits.
///
/// This matches SDL's Steam Controller 2/Triton lizard-mode suppression
/// command. The report ID is included in the returned 64-byte buffer.
#[must_use]
pub const fn lizard_mode_off_feature_report() -> [u8; FEATURE_REPORT_SIZE] {
    let mut report = [0_u8; FEATURE_REPORT_SIZE];
    report[0] = FEATURE_REPORT_ID;
    report[1] = SET_SETTINGS_VALUES;
    report[2] = CONTROLLER_SETTING_SIZE;
    report[3] = SETTING_LIZARD_MODE;
    let value = LIZARD_MODE_OFF.to_le_bytes();
    report[4] = value[0];
    report[5] = value[1];
    report
}

/// Builds the sole controller power-management feature report this project
/// permits.
///
/// The fixed `0x9f` command and `off!` payload are used by the official
/// controller protocol and observed by `OpenPuck`. The report ID is included in
/// the returned zero-padded 64-byte buffer.
#[must_use]
pub const fn power_off_feature_report() -> [u8; FEATURE_REPORT_SIZE] {
    let mut report = [0_u8; FEATURE_REPORT_SIZE];
    report[0] = FEATURE_REPORT_ID;
    report[1] = POWER_OFF_COMMAND;
    report[2] = 4;
    report[3] = POWER_OFF_PAYLOAD[0];
    report[4] = POWER_OFF_PAYLOAD[1];
    report[5] = POWER_OFF_PAYLOAD[2];
    report[6] = POWER_OFF_PAYLOAD[3];
    report
}

/// Builds the SDL-compatible Steam Controller 2 dual-rumble output report.
///
/// The returned packet includes report ID `0x80`. Xbox low-frequency rumble
/// drives the left actuator and high-frequency rumble drives the right.
#[must_use]
pub const fn rumble_output_report(
    low_frequency: u16,
    high_frequency: u16,
) -> [u8; RUMBLE_OUTPUT_REPORT_SIZE] {
    let mut report = [0_u8; RUMBLE_OUTPUT_REPORT_SIZE];
    let low = low_frequency.to_le_bytes();
    let high = high_frequency.to_le_bytes();
    report[0] = RUMBLE_OUTPUT_REPORT_ID;
    report[3] = low[0];
    report[4] = low[1];
    report[6] = high[0];
    report[7] = high[1];
    report
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PadHapticSide {
    Left = 0x01,
    Right = 0x02,
    Both = 0x03,
}

/// Builds the SDL Triton discrete trackpad-tick output report.
///
/// The returned packet includes report ID `0x82`, the selected side mask,
/// command `0x01` (tick), and a signed dB gain. A tick is finite and must not
/// be followed by an artificial stop report.
#[must_use]
pub const fn pad_haptic_tick_output_report(
    side: PadHapticSide,
    gain_db: i8,
) -> [u8; PAD_HAPTIC_OUTPUT_REPORT_SIZE] {
    [
        PAD_HAPTIC_OUTPUT_REPORT_ID,
        side as u8,
        0x01,
        gain_db.cast_unsigned(),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum SteamButton {
    A = 0,
    B = 1,
    X = 2,
    Y = 3,
    QuickAccess = 4,
    RightStickPress = 5,
    View = 6,
    RightGrip4 = 7,
    RightGrip5 = 8,
    RightShoulder = 9,
    DpadDown = 10,
    DpadRight = 11,
    DpadLeft = 12,
    DpadUp = 13,
    Menu = 14,
    LeftStickPress = 15,
    Steam = 16,
    LeftGrip4 = 17,
    LeftGrip5 = 18,
    LeftShoulder = 19,
    RightStickTouch = 20,
    RightPadTouch = 21,
    RightPadClick = 22,
    RightTriggerClick = 23,
    LeftStickTouch = 24,
    LeftPadTouch = 25,
    LeftPadClick = 26,
    LeftTriggerClick = 27,
    RightGripTouch = 28,
    LeftGripTouch = 29,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SteamButtons(pub u32);

impl SteamButtons {
    #[must_use]
    pub const fn contains(self, button: SteamButton) -> bool {
        self.0 & (1_u32 << button as u8) != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GyroState {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AccelerationState {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // Independent physical touch/click sensors, not a state machine.
pub struct SteamControllerState {
    pub report_id: u8,
    pub sequence: u8,
    pub buttons: SteamButtons,
    pub left_trigger: u16,
    pub right_trigger: u16,
    pub left_stick_x: i16,
    pub left_stick_y: i16,
    pub right_stick_x: i16,
    pub right_stick_y: i16,
    pub left_pad_x: i16,
    pub left_pad_y: i16,
    pub left_pad_pressure: i16,
    pub left_pad_touched: bool,
    pub left_pad_pressed: bool,
    pub right_pad_x: i16,
    pub right_pad_y: i16,
    pub right_pad_pressure: i16,
    pub right_pad_touched: bool,
    pub right_pad_pressed: bool,
    pub left_grip_touched: bool,
    pub right_grip_touched: bool,
    pub imu_timestamp: u32,
    pub gyro: Option<GyroState>,
    pub acceleration: Option<AccelerationState>,
    /// Complete validated report, including extended or unresolved bytes.
    pub raw_report: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Disconnected,
    Connected,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryStatus {
    pub charge_state: u8,
    pub percent: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecodedReport {
    ControllerState(SteamControllerState),
    LizardMouse {
        raw_report: Vec<u8>,
    },
    LizardKeyboard {
        raw_report: Vec<u8>,
    },
    Connection(ConnectionState),
    Battery {
        status: BatteryStatus,
        raw_report: Vec<u8>,
    },
    AuxiliaryStatus {
        raw_report: Vec<u8>,
    },
    PeriodicStatus {
        signal_strength_dbm: i8,
        raw_report: Vec<u8>,
    },
}

#[derive(Debug, Default)]
pub struct SteamControllerDecoder;

impl SteamControllerDecoder {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Decodes one complete Steam Controller 2 HID input report.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the report ID is unknown, the separate ID
    /// disagrees with the first byte, or the report has the wrong fixed size.
    pub fn decode(&mut self, report_id: u8, data: &[u8]) -> Result<DecodedReport, DecodeError> {
        if data.first().copied() != Some(report_id) {
            return Err(DecodeError::ReportIdMismatch {
                metadata: report_id,
                data: data.first().copied(),
            });
        }
        match report_id {
            LIZARD_MOUSE_REPORT_ID => {
                exact_size(report_id, data, LIZARD_MOUSE_REPORT_SIZE)?;
                Ok(DecodedReport::LizardMouse {
                    raw_report: data.to_vec(),
                })
            }
            LIZARD_KEYBOARD_REPORT_ID => {
                exact_size(report_id, data, LIZARD_KEYBOARD_REPORT_SIZE)?;
                Ok(DecodedReport::LizardKeyboard {
                    raw_report: data.to_vec(),
                })
            }
            INPUT_REPORT_ID => {
                exact_size(report_id, data, INPUT_REPORT_SIZE)?;
                Ok(DecodedReport::ControllerState(decode_state(data)))
            }
            EXTENDED_INPUT_REPORT_ID => {
                exact_size(report_id, data, EXTENDED_INPUT_REPORT_SIZE)?;
                Ok(DecodedReport::ControllerState(decode_state(data)))
            }
            BATTERY_REPORT_ID => {
                exact_size(report_id, data, 15)?;
                Ok(DecodedReport::Battery {
                    status: BatteryStatus {
                        charge_state: data[1],
                        percent: data[2],
                    },
                    raw_report: data.to_vec(),
                })
            }
            AUX_STATUS_REPORT_ID => {
                exact_size(report_id, data, 6)?;
                Ok(DecodedReport::AuxiliaryStatus {
                    raw_report: data.to_vec(),
                })
            }
            CONNECTION_REPORT_ID => {
                exact_size(report_id, data, 2)?;
                Ok(DecodedReport::Connection(match data[1] {
                    1 => ConnectionState::Disconnected,
                    2 => ConnectionState::Connected,
                    other => ConnectionState::Unknown(other),
                }))
            }
            PERIODIC_STATUS_REPORT_ID => {
                exact_size(report_id, data, 13)?;
                Ok(DecodedReport::PeriodicStatus {
                    signal_strength_dbm: i8::from_ne_bytes([data[9]]),
                    raw_report: data.to_vec(),
                })
            }
            _ => Err(DecodeError::UnknownReportId(report_id)),
        }
    }
}

fn decode_state(data: &[u8]) -> SteamControllerState {
    let buttons = SteamButtons(le_u32(data, 2));
    SteamControllerState {
        report_id: data[0],
        sequence: data[1],
        buttons,
        left_trigger: le_u16(data, 6),
        right_trigger: le_u16(data, 8),
        left_stick_x: symmetric_i16(data, 10),
        left_stick_y: symmetric_i16(data, 12),
        right_stick_x: symmetric_i16(data, 14),
        right_stick_y: symmetric_i16(data, 16),
        left_pad_x: symmetric_i16(data, 18),
        left_pad_y: symmetric_i16(data, 20),
        left_pad_pressure: le_i16(data, 22),
        left_pad_touched: buttons.contains(SteamButton::LeftPadTouch),
        left_pad_pressed: buttons.contains(SteamButton::LeftPadClick),
        right_pad_x: symmetric_i16(data, 24),
        right_pad_y: symmetric_i16(data, 26),
        right_pad_pressure: le_i16(data, 28),
        right_pad_touched: buttons.contains(SteamButton::RightPadTouch),
        right_pad_pressed: buttons.contains(SteamButton::RightPadClick),
        left_grip_touched: buttons.contains(SteamButton::LeftGripTouch),
        right_grip_touched: buttons.contains(SteamButton::RightGripTouch),
        imu_timestamp: le_u32(data, 30),
        acceleration: Some(AccelerationState {
            x: le_i16(data, 34),
            y: le_i16(data, 36),
            z: le_i16(data, 38),
        }),
        gyro: Some(GyroState {
            x: le_i16(data, 40),
            y: le_i16(data, 42),
            z: le_i16(data, 44),
        }),
        raw_report: data.to_vec(),
    }
}

fn exact_size(report_id: u8, data: &[u8], expected: usize) -> Result<(), DecodeError> {
    if data.len() == expected {
        Ok(())
    } else {
        Err(DecodeError::InvalidReportSize {
            report_id,
            expected,
            actual: data.len(),
        })
    }
}

fn le_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn le_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn le_i16(data: &[u8], offset: usize) -> i16 {
    i16::from_le_bytes([data[offset], data[offset + 1]])
}

fn symmetric_i16(data: &[u8], offset: usize) -> i16 {
    le_i16(data, offset).max(-32767)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    UnknownReportId(u8),
    ReportIdMismatch {
        metadata: u8,
        data: Option<u8>,
    },
    InvalidReportSize {
        report_id: u8,
        expected: usize,
        actual: usize,
    },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownReportId(id) => {
                write!(f, "unknown Steam Controller 2 report ID 0x{id:02x}")
            }
            Self::ReportIdMismatch { metadata, data } => write!(
                f,
                "report ID metadata 0x{metadata:02x} does not match first byte {data:?}"
            ),
            Self::InvalidReportSize {
                report_id,
                expected,
                actual,
            } => write!(
                f,
                "report 0x{report_id:02x} has size {actual}; expected {expected}"
            ),
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lizard_mode_off_feature_report_matches_sdl_golden_vector() {
        let report = lizard_mode_off_feature_report();
        assert_eq!(report.len(), FEATURE_REPORT_SIZE);
        assert_eq!(&report[..6], &[0x01, 0x87, 0x03, 0x09, 0x00, 0x00]);
        assert!(report[6..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn rumble_output_report_matches_sdl_golden_vectors() {
        assert_eq!(
            rumble_output_report(0x1234, 0xabcd),
            [0x80, 0, 0, 0x34, 0x12, 0, 0xcd, 0xab, 0, 0]
        );
        assert_eq!(
            rumble_output_report(0, 0),
            [0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            rumble_output_report(u16::MAX, u16::MAX),
            [0x80, 0, 0, 0xff, 0xff, 0, 0xff, 0xff, 0, 0]
        );
    }

    #[test]
    fn pad_haptic_tick_output_report_matches_sdl_golden_vectors() {
        for (gain_db, gain_byte) in [(-36, 0xdc), (-30, 0xe2), (-24, 0xe8)] {
            for (side, side_byte) in [
                (PadHapticSide::Left, 0x01),
                (PadHapticSide::Right, 0x02),
                (PadHapticSide::Both, 0x03),
            ] {
                assert_eq!(
                    pad_haptic_tick_output_report(side, gain_db),
                    [0x82, side_byte, 0x01, gain_byte]
                );
            }
        }
    }

    #[test]
    fn power_off_feature_report_matches_golden_vector() {
        let report = power_off_feature_report();
        assert_eq!(report.len(), FEATURE_REPORT_SIZE);
        assert_eq!(&report[..7], &[0x01, 0x9f, 0x04, b'o', b'f', b'f', b'!']);
        assert!(report[7..].iter().all(|byte| *byte == 0));
    }

    fn input_report(report_id: u8) -> Vec<u8> {
        let size = if report_id == INPUT_REPORT_ID {
            INPUT_REPORT_SIZE
        } else {
            EXTENDED_INPUT_REPORT_SIZE
        };
        let mut report = vec![0_u8; size];
        report[0] = report_id;
        report
    }

    fn put_i16(report: &mut [u8], offset: usize, value: i16) {
        report[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn decodes_all_thirty_documented_button_bits() {
        for bit in 0..30 {
            let mut report = input_report(INPUT_REPORT_ID);
            report[2 + bit / 8] = 1 << (bit % 8);
            let DecodedReport::ControllerState(state) = SteamControllerDecoder::new()
                .decode(INPUT_REPORT_ID, &report)
                .unwrap()
            else {
                panic!("controller state expected");
            };
            assert_eq!(state.buttons.0, 1 << bit);
        }
    }

    #[test]
    fn decodes_every_axis_trigger_pressure_and_motion_field() {
        let mut report = input_report(INPUT_REPORT_ID);
        report[1] = 0x7a;
        report[6..10].copy_from_slice(&[0x00, 0x00, 0xff, 0xff]);
        for (offset, value) in [
            (10, -32768),
            (12, 32767),
            (14, -3),
            (16, 4),
            (18, -5),
            (20, 6),
            (22, 7),
            (24, -8),
            (26, 9),
            (28, 10),
            (34, 11),
            (36, -12),
            (38, 13),
            (40, -14),
            (42, 15),
            (44, -16),
        ] {
            put_i16(&mut report, offset, value);
        }
        report[30..34].copy_from_slice(&0x7856_3412_u32.to_le_bytes());
        let DecodedReport::ControllerState(state) = SteamControllerDecoder::new()
            .decode(INPUT_REPORT_ID, &report)
            .unwrap()
        else {
            panic!("controller state expected");
        };
        assert_eq!(state.sequence, 0x7a);
        assert_eq!((state.left_trigger, state.right_trigger), (0, u16::MAX));
        assert_eq!((state.left_stick_x, state.left_stick_y), (-32767, 32767));
        assert_eq!((state.right_stick_x, state.right_stick_y), (-3, 4));
        assert_eq!(
            (state.left_pad_x, state.left_pad_y, state.left_pad_pressure),
            (-5, 6, 7)
        );
        assert_eq!(
            (
                state.right_pad_x,
                state.right_pad_y,
                state.right_pad_pressure
            ),
            (-8, 9, 10)
        );
        assert_eq!(state.imu_timestamp, 0x7856_3412);
        assert_eq!(
            state.acceleration,
            Some(AccelerationState {
                x: 11,
                y: -12,
                z: 13
            })
        );
        assert_eq!(
            state.gyro,
            Some(GyroState {
                x: -14,
                y: 15,
                z: -16
            })
        );
        assert_eq!(state.raw_report, report);
    }

    #[test]
    fn touch_and_click_flags_are_independent() {
        let mut report = input_report(INPUT_REPORT_ID);
        let buttons = (1_u32 << SteamButton::LeftPadTouch as u8)
            | (1_u32 << SteamButton::RightPadClick as u8)
            | (1_u32 << SteamButton::LeftGripTouch as u8);
        report[2..6].copy_from_slice(&buttons.to_le_bytes());
        let DecodedReport::ControllerState(state) = SteamControllerDecoder::new()
            .decode(INPUT_REPORT_ID, &report)
            .unwrap()
        else {
            panic!("controller state expected");
        };
        assert!(state.left_pad_touched && !state.left_pad_pressed);
        assert!(!state.right_pad_touched && state.right_pad_pressed);
        assert!(state.left_grip_touched && !state.right_grip_touched);
    }

    #[test]
    fn extended_42_uses_same_front_layout_and_preserves_tail() {
        let mut report = input_report(EXTENDED_INPUT_REPORT_ID);
        report[1] = 9;
        report[46] = 0xff;
        report[47] = 0x7f;
        let DecodedReport::ControllerState(state) = SteamControllerDecoder::new()
            .decode(EXTENDED_INPUT_REPORT_ID, &report)
            .unwrap()
        else {
            panic!("controller state expected");
        };
        assert_eq!(state.report_id, EXTENDED_INPUT_REPORT_ID);
        assert_eq!(state.sequence, 9);
        assert_eq!(&state.raw_report[46..48], &[0xff, 0x7f]);
    }

    #[test]
    fn decodes_connection_battery_and_periodic_status() {
        assert_eq!(
            SteamControllerDecoder::new().decode(CONNECTION_REPORT_ID, &[0x79, 2]),
            Ok(DecodedReport::Connection(ConnectionState::Connected))
        );
        let mut battery = vec![0_u8; 15];
        battery[0] = BATTERY_REPORT_ID;
        battery[1] = 1;
        battery[2] = 75;
        assert!(matches!(
            SteamControllerDecoder::new().decode(BATTERY_REPORT_ID, &battery),
            Ok(DecodedReport::Battery {
                status: BatteryStatus {
                    charge_state: 1,
                    percent: 75
                },
                ..
            })
        ));
        let mut auxiliary = vec![0_u8; 6];
        auxiliary[0] = AUX_STATUS_REPORT_ID;
        auxiliary[5] = 0xa5;
        assert!(matches!(
            SteamControllerDecoder::new().decode(AUX_STATUS_REPORT_ID, &auxiliary),
            Ok(DecodedReport::AuxiliaryStatus { raw_report }) if raw_report[5] == 0xa5
        ));
        let mut periodic = vec![0_u8; 13];
        periodic[0] = PERIODIC_STATUS_REPORT_ID;
        periodic[9] = 0xdd;
        assert!(matches!(
            SteamControllerDecoder::new().decode(PERIODIC_STATUS_REPORT_ID, &periodic),
            Ok(DecodedReport::PeriodicStatus {
                signal_strength_dbm: -35,
                ..
            })
        ));
    }

    #[test]
    fn observed_bluetooth_vectors_use_the_existing_state_and_battery_layouts() {
        // Anonymized packets captured from the supported 28de:1303,
        // ff00:0001, interface -1 Bluetooth collection. They intentionally
        // remain transport-agnostic decoder inputs.
        let state = [
            0x45, 0x78, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xcd, 0x00, 0xb4, 0x01,
            0x5a, 0x00, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xcf, 0x9e, 0x1c, 0x0f, 0x88, 0x00, 0x6c, 0xf4, 0x7d, 0x3f, 0x00, 0x00,
            0x02, 0x00, 0x01, 0x00,
        ];
        let DecodedReport::ControllerState(decoded) = SteamControllerDecoder::new()
            .decode(INPUT_REPORT_ID, &state)
            .expect("Bluetooth 0x45 state")
        else {
            panic!("controller state expected");
        };
        assert_eq!(decoded.sequence, 0x78);
        assert_eq!(decoded.buttons, SteamButtons(0));
        assert_eq!((decoded.left_stick_x, decoded.left_stick_y), (205, 436));
        assert_eq!((decoded.right_stick_x, decoded.right_stick_y), (90, 260));
        assert_eq!(decoded.raw_report, state);

        let battery = [
            0x43, 0x01, 0x61, 0x21, 0x10, 0x40, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3d,
            0x6c,
        ];
        assert!(matches!(
            SteamControllerDecoder::new().decode(BATTERY_REPORT_ID, &battery),
            Ok(DecodedReport::Battery {
                status: BatteryStatus {
                    charge_state: 1,
                    percent: 97
                },
                ..
            })
        ));
    }

    #[test]
    fn decodes_lizard_mouse_and_keyboard_reports() {
        let mouse = [LIZARD_MOUSE_REPORT_ID, 1, 2, 3, 4, 5];
        assert_eq!(
            SteamControllerDecoder::new().decode(LIZARD_MOUSE_REPORT_ID, &mouse),
            Ok(DecodedReport::LizardMouse {
                raw_report: mouse.to_vec()
            })
        );

        let keyboard = [LIZARD_KEYBOARD_REPORT_ID, 1, 0, 4, 5, 6, 7, 8, 9];
        assert_eq!(
            SteamControllerDecoder::new().decode(LIZARD_KEYBOARD_REPORT_ID, &keyboard),
            Ok(DecodedReport::LizardKeyboard {
                raw_report: keyboard.to_vec()
            })
        );

        assert!(matches!(
            SteamControllerDecoder::new().decode(LIZARD_MOUSE_REPORT_ID, &mouse[..5]),
            Err(DecodeError::InvalidReportSize { .. })
        ));
        assert!(matches!(
            SteamControllerDecoder::new().decode(LIZARD_KEYBOARD_REPORT_ID, &keyboard[..8]),
            Err(DecodeError::InvalidReportSize { .. })
        ));
    }

    #[test]
    fn malformed_reports_return_errors_and_never_panic() {
        let mut decoder = SteamControllerDecoder::new();
        assert_eq!(
            decoder.decode(0xaa, &[0xaa]),
            Err(DecodeError::UnknownReportId(0xaa))
        );
        assert!(matches!(
            decoder.decode(INPUT_REPORT_ID, &[INPUT_REPORT_ID; 3]),
            Err(DecodeError::InvalidReportSize { .. })
        ));
        assert!(matches!(
            decoder.decode(INPUT_REPORT_ID, &[]),
            Err(DecodeError::ReportIdMismatch { .. })
        ));
        assert!(matches!(
            decoder.decode(
                INPUT_REPORT_ID,
                &[EXTENDED_INPUT_REPORT_ID; INPUT_REPORT_SIZE]
            ),
            Err(DecodeError::ReportIdMismatch { .. })
        ));
        for size in 0..128 {
            let _ = decoder.decode(INPUT_REPORT_ID, &vec![INPUT_REPORT_ID; size]);
        }
    }

    #[test]
    fn arbitrary_byte_streams_for_every_report_id_never_panic() {
        let mut decoder = SteamControllerDecoder::new();
        let mut value = 0x1234_5678_u32;
        for report_id in [
            LIZARD_MOUSE_REPORT_ID,
            LIZARD_KEYBOARD_REPORT_ID,
            EXTENDED_INPUT_REPORT_ID,
            BATTERY_REPORT_ID,
            AUX_STATUS_REPORT_ID,
            INPUT_REPORT_ID,
            CONNECTION_REPORT_ID,
            PERIODIC_STATUS_REPORT_ID,
            0xaa,
        ] {
            for size in 0..80 {
                let mut bytes: Vec<u8> = (0..size)
                    .map(|_| {
                        value = value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        (value >> 24) as u8
                    })
                    .collect();
                if let Some(first) = bytes.first_mut() {
                    *first = report_id;
                }
                let _ = decoder.decode(report_id, &bytes);
            }
        }
    }
}
