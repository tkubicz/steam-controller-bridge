use std::io::BufRead;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const MAX_UPDATE_IPC_LINE_BYTES: usize = 4 * 1024;
const UPDATE_PROTOCOL_VERSION: u32 = 1;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCenterPage {
    About,
    Changelog,
    Updates,
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
        version: UPDATE_PROTOCOL_VERSION,
        message,
    })
    .map_err(|error| error.to_string())?;
    encoded.push(b'\n');
    if encoded.len() > MAX_UPDATE_IPC_LINE_BYTES {
        return Err("Update Center IPC message exceeds its bound".to_owned());
    }
    Ok(encoded)
}

pub fn read<T: DeserializeOwned>(reader: &mut impl BufRead) -> Result<Option<T>, String> {
    let mut line = Vec::with_capacity(MAX_UPDATE_IPC_LINE_BYTES);
    loop {
        let available = reader.fill_buf().map_err(|error| error.to_string())?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err("Update Center IPC ended inside a message".to_owned())
            };
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if line.len() + newline > MAX_UPDATE_IPC_LINE_BYTES {
                return Err("Update Center IPC message exceeds its bound".to_owned());
            }
            line.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            let envelope: Envelope<T> =
                serde_json::from_slice(&line).map_err(|error| error.to_string())?;
            if envelope.version != UPDATE_PROTOCOL_VERSION {
                return Err(format!(
                    "unsupported Update Center IPC version {}",
                    envelope.version
                ));
            }
            return Ok(Some(envelope.message));
        }
        if line.len() + available.len() > MAX_UPDATE_IPC_LINE_BYTES {
            return Err("Update Center IPC message exceeds its bound".to_owned());
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
        let oversized = vec![b'x'; MAX_UPDATE_IPC_LINE_BYTES + 1];
        assert!(read::<UpdateRequest>(&mut Cursor::new(oversized)).is_err());
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
