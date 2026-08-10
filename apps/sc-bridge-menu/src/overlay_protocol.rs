//! The line protocol between the menu app and its profile-wheel overlay.
//!
//! The overlay runs as a second process of this same binary because
//! `eframe::run_native` owns an event loop and the menu app's is already
//! committed to the status item. The parent writes one JSON object per line to
//! the child's stdin; the child never writes back. Every decision — what is
//! selected, what was chosen — is made by the runtime, so the overlay is a pure
//! display and needs no input of its own.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Identifies the overlay's window so it can be found in `NSApp.windows()` and
/// given the level and collection behaviour that float it over a fullscreen
/// game. Nothing shows this string to the user: the window has no title bar.
#[cfg(feature = "overlay")]
pub const OVERLAY_WINDOW_TITLE: &str = "Steam Controller Bridge Profile Wheel";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverlayMessage {
    /// The profiles to draw. Sent at startup and whenever the store changes.
    Roster {
        names: Vec<String>,
        active: Option<usize>,
        sectors_per_page: usize,
    },
    /// Show the wheel with this sector highlighted, or move the highlight
    /// while it is up. There is no message to hide the wheel: a closed wheel
    /// is a killed process, so the child only ever learns of a close by its
    /// stdin reaching end of file.
    Open { selected: usize, page: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayEnvelope {
    pub v: u32,
    #[serde(flatten)]
    pub message: OverlayMessage,
}

impl OverlayEnvelope {
    #[must_use]
    pub const fn new(message: OverlayMessage) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            message,
        }
    }

    /// Renders one newline-terminated line for the child's stdin.
    ///
    /// # Errors
    /// Returns an error if the message cannot be serialized.
    pub fn to_line(&self) -> Result<String, String> {
        serde_json::to_string(self)
            .map(|line| format!("{line}\n"))
            .map_err(|error| error.to_string())
    }

    /// Parses one line from the parent.
    ///
    /// # Errors
    /// Returns an error for malformed JSON or a version this build cannot read.
    #[cfg(any(feature = "overlay", test))]
    pub fn from_line(line: &str) -> Result<Self, String> {
        let envelope: Self = serde_json::from_str(line).map_err(|error| error.to_string())?;
        if envelope.v != PROTOCOL_VERSION {
            return Err(format!(
                "unsupported overlay protocol version {}",
                envelope.v
            ));
        }
        Ok(envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_survives_a_round_trip() {
        for message in [
            OverlayMessage::Roster {
                names: vec!["Default".to_owned(), "Gaming".to_owned()],
                active: Some(1),
                sectors_per_page: 8,
            },
            OverlayMessage::Roster {
                names: Vec::new(),
                active: None,
                sectors_per_page: 8,
            },
            OverlayMessage::Open {
                selected: 3,
                page: 1,
            },
            OverlayMessage::Open {
                selected: 0,
                page: 0,
            },
        ] {
            let envelope = OverlayEnvelope::new(message);
            let line = envelope.to_line().unwrap();
            assert!(line.ends_with('\n'), "lines must be newline delimited");
            assert!(
                !line[..line.len() - 1].contains('\n'),
                "a message must occupy exactly one line"
            );
            assert_eq!(OverlayEnvelope::from_line(&line).unwrap(), envelope);
        }
    }

    #[test]
    fn a_profile_name_with_newlines_cannot_desynchronize_the_stream() {
        // Profile names come from a user-editable file, so a name carrying a
        // newline must not be able to split one message into two.
        let envelope = OverlayEnvelope::new(OverlayMessage::Roster {
            names: vec!["one\ntwo".to_owned()],
            active: None,
            sectors_per_page: 8,
        });
        let line = envelope.to_line().unwrap();
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(OverlayEnvelope::from_line(&line).unwrap(), envelope);
    }

    #[test]
    fn a_future_protocol_version_is_refused_rather_than_misread() {
        let line = r#"{"v":99,"kind":"open","selected":0,"page":0}"#;
        assert!(OverlayEnvelope::from_line(line)
            .unwrap_err()
            .contains("unsupported overlay protocol version"));
    }

    #[test]
    fn malformed_input_is_an_error_and_not_a_panic() {
        for line in ["", "{", "null", r#"{"v":1}"#, r#"{"v":1,"kind":"nope"}"#] {
            assert!(OverlayEnvelope::from_line(line).is_err(), "{line:?}");
        }
    }
}
