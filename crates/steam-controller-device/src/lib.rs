//! HID discovery and raw report capture, with macOS access isolated behind cfg.

use std::time::Duration;

pub const LIZARD_MODE_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
pub const PROTEUS_VENDOR_ID: u16 = 0x28de;
pub const PROTEUS_PRODUCT_ID: u16 = 0x1304;
pub const STEAM_USAGE_PAGE: u16 = 0xff00;
pub const STEAM_CONTROLLER_USAGE: u16 = 0x0001;
pub const FIRST_PROTEUS_SLOT_INTERFACE: i32 = 2;
pub const LAST_PROTEUS_SLOT_INTERFACE: i32 = 5;

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

    /// Returns whether this collection is a supported Steam Controller 2 Puck
    /// slot on which the narrow lizard-mode command may be sent.
    #[must_use]
    pub const fn supports_lizard_mode_suppression(&self) -> bool {
        self.vendor_id == PROTEUS_VENDOR_ID
            && self.product_id == PROTEUS_PRODUCT_ID
            && self.usage_page == STEAM_USAGE_PAGE
            && self.usage == STEAM_CONTROLLER_USAGE
            && self.interface_number >= FIRST_PROTEUS_SLOT_INTERFACE
            && self.interface_number <= LAST_PROTEUS_SLOT_INTERFACE
    }

    /// Returns whether this collection is a supported Steam Controller 2 Puck
    /// slot on which the narrow dual-rumble output may be sent.
    #[must_use]
    pub const fn supports_rumble(&self) -> bool {
        self.supports_lizard_mode_suppression()
    }
}

/// Portable scheduling state for the SDL-compatible lizard-off heartbeat.
#[derive(Debug, Default)]
pub struct LizardModeHeartbeat {
    connected: bool,
    last_refresh: Option<Duration>,
}

impl LizardModeHeartbeat {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            connected: false,
            last_refresh: None,
        }
    }

    pub fn connected(&mut self) {
        self.connected = true;
        self.last_refresh = None;
    }

    pub fn disconnected(&mut self) {
        self.connected = false;
        self.last_refresh = None;
    }

    #[must_use]
    pub fn refresh_due(&self, now: Duration) -> bool {
        self.connected
            && self
                .last_refresh
                .is_none_or(|last| now.saturating_sub(last) >= LIZARD_MODE_REFRESH_INTERVAL)
    }

    pub fn refreshed(&mut self, now: Duration) {
        self.last_refresh = Some(now);
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.connected && self.last_refresh.is_some()
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
    NotConnected,
    OwnershipConflict {
        interface_number: i32,
    },
    UnsupportedLizardSuppressionTarget {
        vendor_id: u16,
        product_id: u16,
        usage_page: u16,
        usage: u16,
        interface_number: i32,
    },
    UnsupportedRumbleTarget {
        vendor_id: u16,
        product_id: u16,
        usage_page: u16,
        usage: u16,
        interface_number: i32,
    },
    UnsupportedPlatform,
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(message) => write!(f, "HID backend failed: {message}"),
            Self::InvalidIndex(index) => write!(f, "HID device index {index} does not exist"),
            Self::NotConnected => write!(f, "the selected HID collection is not connected"),
            Self::OwnershipConflict { interface_number } => write!(
                f,
                "HID interface {interface_number} is already owned by another \
                 steam-controller-bridge tool; stop the other sc-bridge, \
                 sc-probe, or sc-visualizer process and retry"
            ),
            Self::UnsupportedLizardSuppressionTarget {
                vendor_id,
                product_id,
                usage_page,
                usage,
                interface_number,
            } => write!(
                f,
                "refusing lizard-mode write to unsupported collection \
                 {vendor_id:04x}:{product_id:04x} usage \
                 {usage_page:04x}:{usage:04x} interface {interface_number}; \
                 select the active 28de:1304 ff00:0001 interface 2-5 Puck slot"
            ),
            Self::UnsupportedRumbleTarget {
                vendor_id,
                product_id,
                usage_page,
                usage,
                interface_number,
            } => write!(
                f,
                "refusing rumble write to unsupported collection \
                 {vendor_id:04x}:{product_id:04x} usage \
                 {usage_page:04x}:{usage:04x} interface {interface_number}; \
                 select the active 28de:1304 ff00:0001 interface 2-5 Puck slot"
            ),
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

/// Returns an unsupported-platform error on non-macOS hosts.
///
/// # Errors
///
/// Always returns [`DeviceError::UnsupportedPlatform`].
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
    pub fn open_info(_info: &HidDeviceInfo) -> Result<Self, DeviceError> {
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

    /// Returns an unsupported-platform error without attempting a feature
    /// write.
    ///
    /// # Errors
    ///
    /// Always returns [`DeviceError::UnsupportedPlatform`].
    pub fn suppress_lizard_mode(&self) -> Result<(), DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    /// Returns an unsupported-platform error without attempting an output
    /// write.
    ///
    /// # Errors
    ///
    /// Always returns [`DeviceError::UnsupportedPlatform`].
    pub fn set_rumble(&self, _low_frequency: u16, _high_frequency: u16) -> Result<(), DeviceError> {
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

    fn suppression_target() -> HidDeviceInfo {
        HidDeviceInfo {
            id: "slot".to_owned(),
            path: "slot".to_owned(),
            vendor_id: PROTEUS_VENDOR_ID,
            product_id: PROTEUS_PRODUCT_ID,
            usage_page: STEAM_USAGE_PAGE,
            usage: STEAM_CONTROLLER_USAGE,
            interface_number: FIRST_PROTEUS_SLOT_INTERFACE,
            serial_number: Some("abc".to_owned()),
            manufacturer: Some("Valve Software".to_owned()),
            product: Some("Steam Controller Puck".to_owned()),
            transport: "USB".to_owned(),
        }
    }

    #[test]
    fn suppression_target_is_exact_and_interface_bounded() {
        let target = suppression_target();
        assert!(target.supports_lizard_mode_suppression());
        assert!(target.supports_rumble());

        for mutate in [
            |info: &mut HidDeviceInfo| info.vendor_id = 0,
            |info: &mut HidDeviceInfo| info.product_id = 0,
            |info: &mut HidDeviceInfo| info.usage_page = 0,
            |info: &mut HidDeviceInfo| info.usage = 0,
            |info: &mut HidDeviceInfo| info.interface_number = 1,
            |info: &mut HidDeviceInfo| info.interface_number = 6,
        ] {
            let mut other = target.clone();
            mutate(&mut other);
            assert!(!other.supports_lizard_mode_suppression());
            assert!(!other.supports_rumble());
        }
    }

    #[test]
    fn heartbeat_is_immediate_periodic_and_stops_when_disconnected() {
        let mut heartbeat = LizardModeHeartbeat::new();
        assert!(!heartbeat.refresh_due(Duration::ZERO));

        heartbeat.connected();
        assert!(heartbeat.refresh_due(Duration::ZERO));
        heartbeat.refreshed(Duration::ZERO);
        assert!(heartbeat.is_active());
        assert!(!heartbeat.refresh_due(Duration::from_millis(2_999)));
        assert!(heartbeat.refresh_due(Duration::from_secs(3)));

        heartbeat.refreshed(Duration::from_secs(3));
        heartbeat.disconnected();
        assert!(!heartbeat.is_active());
        assert!(!heartbeat.refresh_due(Duration::from_secs(30)));

        heartbeat.connected();
        assert!(heartbeat.refresh_due(Duration::from_secs(30)));
    }
}
