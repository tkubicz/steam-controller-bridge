//! Rust client and helper contract for the experimental macOS virtual gamepad.

mod client;
pub mod contract;
pub mod helper;
mod platform;

use std::path::PathBuf;
use std::time::Duration;

pub use client::VirtualHidOutput;
pub use contract::{
    encode_input_report, DEFAULT_PRODUCT_ID, DEFAULT_VENDOR_ID, GAMEPAD_REPORT_DESCRIPTOR,
    HELPER_PROTOCOL_VERSION, INPUT_REPORT_LEN, NEUTRAL_INPUT_REPORT,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualHidConfig {
    pub helper_path: PathBuf,
    pub queue_capacity: usize,
    pub startup_timeout: Duration,
    pub acknowledgement_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub dry_run: bool,
    pub vendor_id: u16,
    pub product_id: u16,
}

impl VirtualHidConfig {
    #[must_use]
    pub fn new(helper_path: PathBuf) -> Self {
        Self {
            helper_path,
            queue_capacity: 32,
            startup_timeout: Duration::from_secs(5),
            acknowledgement_timeout: Duration::from_secs(1),
            shutdown_timeout: Duration::from_secs(2),
            dry_run: false,
            vendor_id: DEFAULT_VENDOR_ID,
            product_id: DEFAULT_PRODUCT_ID,
        }
    }

    #[must_use]
    pub fn dry_run(helper_path: PathBuf) -> Self {
        Self {
            dry_run: true,
            ..Self::new(helper_path)
        }
    }

    /// Overrides the USB vendor and product IDs while preserving the fixed,
    /// proven gamepad descriptor and report format.
    #[must_use]
    pub fn with_identity(mut self, vendor_id: u16, product_id: u16) -> Self {
        self.vendor_id = vendor_id;
        self.product_id = product_id;
        self
    }

    /// Validates queue and lifecycle limits before the helper is spawned.
    ///
    /// # Errors
    ///
    /// Returns an invalid-configuration error for a zero-sized queue or any
    /// zero lifecycle timeout.
    pub fn validate(&self) -> Result<(), VirtualHidError> {
        if self.queue_capacity == 0 {
            return Err(VirtualHidError::new(
                VirtualHidErrorClass::InvalidConfiguration,
                "virtual HID queue capacity must be greater than zero",
            ));
        }
        if self.startup_timeout.is_zero()
            || self.acknowledgement_timeout.is_zero()
            || self.shutdown_timeout.is_zero()
        {
            return Err(VirtualHidError::new(
                VirtualHidErrorClass::InvalidConfiguration,
                "virtual HID lifecycle timeouts must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VirtualHidHelperMetadata {
    pub protocol_version: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    pub bundle_identifier: Option<String>,
    pub signing_identifier: Option<String>,
    pub entitlement_present: Option<bool>,
    pub dry_run: bool,
}

/// Parses a decimal or `0x`-prefixed hexadecimal USB identifier.
///
/// # Errors
///
/// Returns an error for an empty value, invalid digits, or a value larger than
/// 16 bits.
pub fn parse_usb_id(value: &str) -> Result<u16, String> {
    let value = value.trim();
    let (digits, radix) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or((value, 10), |digits| (digits, 16));
    if digits.is_empty() {
        return Err("USB identifier cannot be empty".to_owned());
    }
    u16::from_str_radix(digits, radix)
        .map_err(|_| format!("invalid 16-bit USB identifier: {value}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VirtualHidErrorClass {
    UnsupportedPlatform,
    MissingHelper,
    InvalidConfiguration,
    SpawnFailed,
    StartupTimeout,
    HelperExited,
    ProtocolMismatch,
    ProtocolViolation,
    EntitlementMissing,
    EntitlementRejected,
    DeviceCreationFailed,
    DispatchFailed,
    AcknowledgementTimeout,
    QueueOverflow,
    CancellationTimeout,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{class:?}: {message}")]
pub struct VirtualHidError {
    class: VirtualHidErrorClass,
    message: String,
}

impl VirtualHidError {
    #[must_use]
    pub fn new(class: VirtualHidErrorClass, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > 1_024 {
            let mut boundary = 1_024;
            while !message.is_char_boundary(boundary) {
                boundary -= 1;
            }
            message.truncate(boundary);
        }
        Self { class, message }
    }

    #[must_use]
    pub const fn class(&self) -> VirtualHidErrorClass {
        self.class
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub const fn is_permanent_configuration_failure(&self) -> bool {
        matches!(
            self.class,
            VirtualHidErrorClass::UnsupportedPlatform
                | VirtualHidErrorClass::MissingHelper
                | VirtualHidErrorClass::InvalidConfiguration
                | VirtualHidErrorClass::ProtocolMismatch
                | VirtualHidErrorClass::ProtocolViolation
                | VirtualHidErrorClass::EntitlementMissing
                | VirtualHidErrorClass::EntitlementRejected
                | VirtualHidErrorClass::DeviceCreationFailed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_bounded_and_failure_classes_are_stable() {
        let error = VirtualHidError::new(VirtualHidErrorClass::ProtocolMismatch, "x".repeat(2_000));
        assert_eq!(error.message().len(), 1_024);
        let unicode =
            VirtualHidError::new(VirtualHidErrorClass::ProtocolViolation, "é".repeat(600));
        assert!(unicode.message().len() <= 1_024);
        assert!(unicode.message().chars().all(|character| character == 'é'));
        assert!(error.is_permanent_configuration_failure());
        assert!(
            !VirtualHidError::new(VirtualHidErrorClass::HelperExited, "gone")
                .is_permanent_configuration_failure()
        );
    }

    #[test]
    fn virtual_output_defaults_to_the_proven_compatibility_identity() {
        let config = VirtualHidConfig::new(PathBuf::from("helper"));
        assert_eq!(config.vendor_id, 0x045e);
        assert_eq!(config.product_id, 0x028e);
        let custom = config.with_identity(0xcafe, 0x4001);
        assert_eq!((custom.vendor_id, custom.product_id), (0xcafe, 0x4001));
    }

    #[test]
    fn usb_identifiers_accept_decimal_and_explicit_hexadecimal() {
        assert_eq!(parse_usb_id("1118").unwrap(), 0x045e);
        assert_eq!(parse_usb_id("0x045e").unwrap(), 0x045e);
        assert_eq!(parse_usb_id("0XCAFE").unwrap(), 0xcafe);
        assert!(parse_usb_id("").is_err());
        assert!(parse_usb_id("cafe").is_err());
        assert!(parse_usb_id("0x10000").is_err());
    }
}
