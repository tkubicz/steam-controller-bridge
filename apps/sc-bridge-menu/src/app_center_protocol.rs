use std::io::BufRead;

use clap::ValueEnum;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use bridge_runtime::{FirmwareInfo, FirmwareInstallSource, FirmwareInstallState, FirmwareVersion};

#[cfg(test)]
use bridge_runtime::FirmwareCapabilities;

use crate::line_protocol::read_bounded_line;

pub const MAX_IPC_LINE_BYTES: usize = 4 * 1024;
const IPC_PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    version: u32,
    message: T,
}

pub type RequestId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRequest {
    pub id: RequestId,
    pub operation: UpdateOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateOperation {
    SuspendBridge,
    ResumeBridge,
    QuitForReplacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCenterPage {
    About,
    Changelog,
    Updates,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "value")]
pub enum FirmwareStatus {
    #[default]
    Pending,
    Reported(u16),
    UnsupportedFormat(u8),
    Malformed,
    Unreported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirmwareReceiptSource {
    AppCenter,
    FirstObserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirmwareReceiptStatus {
    pub installed_at: u64,
    pub install_id: [u8; 16],
    pub source: FirmwareReceiptSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "receipt")]
pub enum FirmwareInstallStatus {
    #[default]
    Unsupported,
    Pending,
    Recorded(FirmwareReceiptStatus),
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FirmwareDetails {
    pub version: FirmwareStatus,
    pub capabilities: u32,
    pub install: FirmwareInstallStatus,
}

impl From<FirmwareInfo> for FirmwareDetails {
    fn from(value: FirmwareInfo) -> Self {
        Self {
            version: value.version.into(),
            capabilities: value.capabilities.bits(),
            install: match value.install_state {
                FirmwareInstallState::Unsupported => FirmwareInstallStatus::Unsupported,
                FirmwareInstallState::Pending => FirmwareInstallStatus::Pending,
                FirmwareInstallState::Invalid => FirmwareInstallStatus::Invalid,
                FirmwareInstallState::Recorded(receipt) => {
                    FirmwareInstallStatus::Recorded(FirmwareReceiptStatus {
                        installed_at: receipt.installed_at,
                        install_id: receipt.install_id,
                        source: match receipt.source {
                            FirmwareInstallSource::AppCenter => FirmwareReceiptSource::AppCenter,
                            FirmwareInstallSource::FirstObserved => {
                                FirmwareReceiptSource::FirstObserved
                            }
                        },
                    })
                }
            },
        }
    }
}

impl From<FirmwareVersion> for FirmwareStatus {
    fn from(value: FirmwareVersion) -> Self {
        match value {
            FirmwareVersion::Pending => Self::Pending,
            FirmwareVersion::Reported(revision) => Self::Reported(revision),
            FirmwareVersion::UnsupportedFormat(format) => Self::UnsupportedFormat(format),
            FirmwareVersion::Malformed => Self::Malformed,
            FirmwareVersion::Unreported => Self::Unreported,
        }
    }
}

impl From<FirmwareStatus> for FirmwareVersion {
    fn from(value: FirmwareStatus) -> Self {
        match value {
            FirmwareStatus::Pending => Self::Pending,
            FirmwareStatus::Reported(revision) => Self::Reported(revision),
            FirmwareStatus::UnsupportedFormat(format) => Self::UnsupportedFormat(format),
            FirmwareStatus::Malformed => Self::Malformed,
            FirmwareStatus::Unreported => Self::Unreported,
        }
    }
}

impl std::fmt::Display for FirmwareDetails {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let encoded = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        formatter.write_str(&encoded)
    }
}

impl std::str::FromStr for FirmwareDetails {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(value).map_err(|error| error.to_string())
    }
}

impl AppCenterPage {
    /// The `--tab` value the child parses back into this page. Reading it from
    /// the same `ValueEnum` the child parses keeps one list of tab names.
    pub fn argument(self) -> String {
        self.to_possible_value()
            .expect("every page is a selectable tab")
            .get_name()
            .to_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCenterCommand {
    Close,
    Navigate {
        page: AppCenterPage,
        firmware: FirmwareDetails,
    },
    FirmwareVersion {
        firmware: FirmwareDetails,
    },
    UpdateResponse(UpdateResponse),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateResponse {
    pub id: RequestId,
    pub result: UpdateResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateResult {
    Suspended,
    Resumed,
    Quitting,
    Error { message: String },
}

pub fn encode<T: Serialize>(message: T) -> Result<Vec<u8>, String> {
    let mut encoded = serde_json::to_vec(&Envelope {
        version: IPC_PROTOCOL_VERSION,
        message,
    })
    .map_err(|error| error.to_string())?;
    encoded.push(b'\n');
    if encoded.len() > MAX_IPC_LINE_BYTES {
        return Err("app window IPC message exceeds its bound".to_owned());
    }
    Ok(encoded)
}

pub fn read<T: DeserializeOwned>(reader: &mut impl BufRead) -> Result<Option<T>, String> {
    let Some(line) = read_bounded_line(reader, MAX_IPC_LINE_BYTES)? else {
        return Ok(None);
    };
    let envelope: Envelope<T> = serde_json::from_slice(&line).map_err(|error| error.to_string())?;
    if envelope.version != IPC_PROTOCOL_VERSION {
        return Err(format!(
            "unsupported app window IPC version {}",
            envelope.version
        ));
    }
    Ok(Some(envelope.message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn protocol_is_versioned_and_bounded_before_allocation_grows() {
        let request = UpdateRequest {
            id: 7,
            operation: UpdateOperation::SuspendBridge,
        };
        let encoded = encode(request).unwrap();
        assert!(matches!(
            read::<UpdateRequest>(&mut Cursor::new(encoded)).unwrap(),
            Some(decoded) if decoded == request
        ));
        let oversized = vec![b'x'; MAX_IPC_LINE_BYTES + 1];
        assert!(read::<UpdateRequest>(&mut Cursor::new(oversized)).is_err());
    }

    #[test]
    fn every_page_survives_its_launch_argument() {
        for page in AppCenterPage::value_variants() {
            assert_eq!(
                AppCenterPage::from_str(&page.argument(), true).ok(),
                Some(*page)
            );
        }
    }

    #[test]
    fn host_commands_distinguish_navigation_from_update_responses() {
        let firmware = FirmwareDetails {
            version: FirmwareStatus::Reported(7),
            ..FirmwareDetails::default()
        };
        let encoded = encode(AppCenterCommand::Navigate {
            page: AppCenterPage::Updates,
            firmware,
        })
        .unwrap();
        assert_eq!(
            read(&mut Cursor::new(encoded)).unwrap(),
            Some(AppCenterCommand::Navigate {
                page: AppCenterPage::Updates,
                firmware,
            })
        );

        let response = AppCenterCommand::UpdateResponse(UpdateResponse {
            id: 42,
            result: UpdateResult::Resumed,
        });
        assert_eq!(
            read::<AppCenterCommand>(&mut Cursor::new(encode(&response).unwrap())).unwrap(),
            Some(response)
        );

        assert_eq!(
            read::<AppCenterCommand>(&mut Cursor::new(encode(AppCenterCommand::Close).unwrap()))
                .unwrap(),
            Some(AppCenterCommand::Close)
        );
    }

    #[test]
    fn every_firmware_detail_survives_a_launch_argument() {
        for details in [
            FirmwareDetails::default(),
            FirmwareDetails {
                version: FirmwareStatus::Reported(7),
                capabilities: (FirmwareCapabilities::ENTER_UF2_BOOTLOADER
                    | FirmwareCapabilities::INSTALL_RECEIPT)
                    .bits(),
                install: FirmwareInstallStatus::Pending,
            },
            FirmwareDetails {
                version: FirmwareStatus::Reported(2),
                capabilities: (FirmwareCapabilities::ENTER_UF2_BOOTLOADER
                    | FirmwareCapabilities::INSTALL_RECEIPT)
                    .bits(),
                install: FirmwareInstallStatus::Recorded(FirmwareReceiptStatus {
                    installed_at: 1_786_456_920,
                    install_id: [0xa5; 16],
                    source: FirmwareReceiptSource::AppCenter,
                }),
            },
        ] {
            assert_eq!(details.to_string().parse(), Ok(details));
        }
    }
}
