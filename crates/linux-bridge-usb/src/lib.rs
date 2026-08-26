use std::collections::BTreeSet;
use std::io;

#[cfg(target_os = "linux")]
mod backend;
#[cfg(not(target_os = "linux"))]
mod portable;

#[cfg(target_os = "linux")]
use backend as platform;
#[cfg(not(target_os = "linux"))]
use portable as platform;

pub const VENDOR_ID: u16 = 0x045e;
pub const PRODUCT_ID: u16 = 0x028e;
pub const MANUFACTURER: &str = "Lynxware";
pub const PRODUCT: &str = "Steam Controller Bridge";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locator {
    pub bus_number: u8,
    pub device_address: u8,
    pub control_interface: u8,
    pub data_interface: u8,
    pub xinput_interface: u8,
    pub bulk_in_endpoint: u8,
    pub bulk_out_endpoint: u8,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub locator: Locator,
    pub stable_id: String,
}

impl std::fmt::Debug for DeviceInfo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceInfo")
            .field("locator", &self.locator)
            .field("stable_id_present", &!self.stable_id.is_empty())
            .finish()
    }
}

#[derive(Debug)]
pub enum Error {
    AccessDenied(String),
    Busy(String),
    InvalidTopology(String),
    Disconnected(String),
    Unsupported,
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccessDenied(endpoint) => write!(formatter, "access denied: {endpoint}"),
            Self::Busy(endpoint) => write!(formatter, "endpoint busy: {endpoint}"),
            Self::InvalidTopology(reason) => write!(formatter, "invalid USB topology: {reason}"),
            Self::Disconnected(endpoint) => write!(formatter, "disconnected: {endpoint}"),
            Self::Unsupported => formatter.write_str("Linux USB is unsupported on this platform"),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::AccessDenied(_)
            | Self::Busy(_)
            | Self::InvalidTopology(_)
            | Self::Disconnected(_)
            | Self::Unsupported => None,
        }
    }
}

/// Enumerates exact official bridge USB devices and validates their topology.
///
/// # Errors
/// Returns an access, ownership, descriptor, disconnection, or host I/O error.
pub fn discover(excluded_stable_ids: &BTreeSet<String>) -> Result<Vec<DeviceInfo>, Error> {
    platform::discover(excluded_stable_ids)
}

#[cfg(any(target_os = "linux", test))]
fn is_excluded(serial: Option<&str>, excluded_stable_ids: &BTreeSet<String>) -> bool {
    serial.is_some_and(|serial| excluded_stable_ids.contains(serial))
}

/// Opens the exact official bridge identified by its stable USB serial.
///
/// # Errors
/// Returns an access, ownership, descriptor, disconnection, or host I/O error.
pub fn open(stable_id: &str) -> Result<UsbTransport, Error> {
    platform::open(stable_id).map(|inner| UsbTransport { inner })
}

pub struct UsbTransport {
    inner: platform::UsbTransport,
}

impl UsbTransport {
    /// Writes one complete protocol frame.
    ///
    /// # Errors
    /// Returns an I/O error when the bulk transfer fails or is incomplete.
    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.inner.write_all(bytes)
    }

    /// Reads one currently completed bulk transfer.
    ///
    /// # Errors
    /// Returns `WouldBlock` when no transfer is ready, or an I/O error.
    pub fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.inner.read(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_debug_never_exposes_the_stable_serial() {
        let device = DeviceInfo {
            locator: Locator {
                bus_number: 3,
                device_address: 4,
                control_interface: 0,
                data_interface: 1,
                xinput_interface: 2,
                bulk_in_endpoint: 0x82,
                bulk_out_endpoint: 0x01,
            },
            stable_id: "TESTSERIAL0000".to_owned(),
        };

        let debug = format!("{device:?}");

        assert!(debug.contains("stable_id_present: true"));
        assert!(!debug.contains("TESTSERIAL0000"));
    }

    #[test]
    fn serial_fallback_excludes_only_matching_stable_raw_devices() {
        let excluded = BTreeSet::from(["SERIAL-FALLBACK".to_owned()]);

        assert!(is_excluded(Some("SERIAL-FALLBACK"), &excluded));
        assert!(!is_excluded(Some("RAW-ONLY"), &excluded));
        assert!(!is_excluded(None, &excluded));
    }
}
