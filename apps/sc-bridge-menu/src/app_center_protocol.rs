use std::io::BufRead;

use clap::ValueEnum;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const MAX_IPC_LINE_BYTES: usize = 4 * 1024;
const IPC_PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    version: u32,
    message: T,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateRequest {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCenterCommand {
    Navigate {
        page: AppCenterPage,
        firmware_version: String,
    },
    FirmwareVersion {
        firmware_version: String,
    },
    UpdateResponse(UpdateResponse),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateResponse {
    Suspended { resume_after: bool },
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
    let mut line = Vec::with_capacity(MAX_IPC_LINE_BYTES);
    loop {
        let available = reader.fill_buf().map_err(|error| error.to_string())?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err("app window IPC ended inside a message".to_owned())
            };
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if line.len() + newline > MAX_IPC_LINE_BYTES {
                return Err("app window IPC message exceeds its bound".to_owned());
            }
            line.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            let envelope: Envelope<T> =
                serde_json::from_slice(&line).map_err(|error| error.to_string())?;
            if envelope.version != IPC_PROTOCOL_VERSION {
                return Err(format!(
                    "unsupported app window IPC version {}",
                    envelope.version
                ));
            }
            return Ok(Some(envelope.message));
        }
        if line.len() + available.len() > MAX_IPC_LINE_BYTES {
            return Err("app window IPC message exceeds its bound".to_owned());
        }
        line.extend_from_slice(available);
        let consumed = available.len();
        reader.consume(consumed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn protocol_is_versioned_and_bounded_before_allocation_grows() {
        let encoded = encode(UpdateRequest::SuspendBridge).unwrap();
        assert!(matches!(
            read(&mut Cursor::new(encoded)).unwrap(),
            Some(UpdateRequest::SuspendBridge)
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
        let encoded = encode(AppCenterCommand::Navigate {
            page: AppCenterPage::Updates,
            firmware_version: "7".to_owned(),
        })
        .unwrap();
        assert!(matches!(
            read(&mut Cursor::new(encoded)).unwrap(),
            Some(AppCenterCommand::Navigate {
                page: AppCenterPage::Updates,
                firmware_version
            }) if firmware_version == "7"
        ));
    }
}
