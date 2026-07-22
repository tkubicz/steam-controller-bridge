//! HID discovery and raw report capture, with macOS access isolated behind cfg.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidDeviceInfo {
    pub id: String,
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub interface_number: i32,
    pub serial_number: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub transport: String,
}

impl HidDeviceInfo {
    #[must_use]
    pub fn same_physical_device(&self, other: &Self) -> bool {
        if self.vendor_id != other.vendor_id || self.product_id != other.product_id {
            return false;
        }
        let left_serial = self
            .serial_number
            .as_deref()
            .filter(|value| !value.is_empty());
        let right_serial = other
            .serial_number
            .as_deref()
            .filter(|value| !value.is_empty());
        match (left_serial, right_serial) {
            (Some(left), Some(right)) => left == right,
            (None, None) => {
                self.manufacturer == other.manufacturer
                    && self.product == other.product
                    && self.transport == other.transport
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawHidReport {
    pub timestamp: Duration,
    pub report_id: u8,
    pub data: Vec<u8>,
    pub source_device_id: String,
    pub transport: String,
    pub dropped_reports: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    Connected(HidDeviceInfo),
    Disconnected,
    Report(RawHidReport),
}

#[derive(Debug)]
pub enum DeviceError {
    Backend(String),
    InvalidIndex(usize),
    UnsupportedPlatform,
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(message) => write!(f, "HID backend failed: {message}"),
            Self::InvalidIndex(index) => write!(f, "HID device index {index} does not exist"),
            Self::UnsupportedPlatform => {
                write!(f, "live HID access is currently implemented only on macOS")
            }
        }
    }
}

impl std::error::Error for DeviceError {}

#[cfg(target_os = "macos")]
mod platform;

#[cfg(target_os = "macos")]
pub use platform::{enumerate, HidSession};

#[cfg(not(target_os = "macos"))]
pub fn enumerate() -> Result<Vec<HidDeviceInfo>, DeviceError> {
    Err(DeviceError::UnsupportedPlatform)
}

#[cfg(not(target_os = "macos"))]
pub struct HidSession;

#[cfg(not(target_os = "macos"))]
impl HidSession {
    /// Returns an unsupported-platform error on non-macOS hosts.
    ///
    /// # Errors
    ///
    /// Always returns [`DeviceError::UnsupportedPlatform`].
    pub fn open_index(_index: usize) -> Result<Self, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    /// Returns an unsupported-platform error on non-macOS hosts.
    ///
    /// # Errors
    ///
    /// Always returns [`DeviceError::UnsupportedPlatform`].
    pub fn poll(&mut self, _timeout: Duration) -> Result<Option<DeviceEvent>, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(serial: Option<&str>, usage: u16) -> HidDeviceInfo {
        HidDeviceInfo {
            id: format!("device-{usage}"),
            path: format!("path-{usage}"),
            vendor_id: 0x28de,
            product_id: 0x1102,
            usage_page: 1,
            usage,
            interface_number: 0,
            serial_number: serial.map(str::to_owned),
            manufacturer: Some("Valve".to_owned()),
            product: Some("Controller".to_owned()),
            transport: "USB".to_owned(),
        }
    }

    #[test]
    fn collections_group_by_physical_identity_not_usage() {
        assert!(info(Some("abc"), 1).same_physical_device(&info(Some("abc"), 2)));
        assert!(!info(Some("abc"), 1).same_physical_device(&info(Some("def"), 1)));
        let mut different_product = info(None, 2);
        different_product.product = Some("Different".to_owned());
        assert!(!info(None, 1).same_physical_device(&different_product));
    }
}
