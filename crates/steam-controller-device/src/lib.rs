//! HID discovery and raw report capture, with macOS access isolated behind cfg.

use std::time::Duration;

pub const LIZARD_MODE_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
pub const PROTEUS_VENDOR_ID: u16 = 0x28de;
pub const PROTEUS_PRODUCT_ID: u16 = 0x1304;
pub const STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID: u16 = 0x1303;
pub const STEAM_USAGE_PAGE: u16 = 0xff00;
pub const STEAM_CONTROLLER_USAGE: u16 = 0x0001;
pub const FIRST_PROTEUS_SLOT_INTERFACE: i32 = 2;
pub const LAST_PROTEUS_SLOT_INTERFACE: i32 = 5;
pub const BLUETOOTH_CONTROLLER_INTERFACE: i32 = -1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerTransport {
    Puck,
    Bluetooth,
}

impl std::fmt::Display for ControllerTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Puck => f.write_str("Puck"),
            Self::Bluetooth => f.write_str("Bluetooth"),
        }
    }
}

/// Formats a hardware serial for display, keeping only the last four characters.
///
/// A Bluetooth HID serial is the controller's MAC address, and users are asked to
/// paste diagnostics into public issue reports, so no display path may print the
/// whole value. Comparisons that identify a specific device keep using the field.
#[must_use]
pub fn masked_serial(value: Option<&str>) -> String {
    match value.filter(|serial| !serial.is_empty()) {
        None => "<none>".to_owned(),
        // Fewer than five characters cannot be shortened without revealing most of
        // the value, so nothing is kept.
        Some(serial) if serial.chars().count() <= 4 => "****".to_owned(),
        Some(serial) => {
            let tail: String = serial
                .chars()
                .skip(serial.chars().count().saturating_sub(4))
                .collect();
            format!("****{tail}")
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
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

/// Deliberately lossy: `serial_number` is masked so that any `{:?}` of this type,
/// or of anything containing it, stays safe to paste into a bug report. Read the
/// field directly when the real value is needed.
impl std::fmt::Debug for HidDeviceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HidDeviceInfo")
            .field("id", &self.id)
            .field("path", &self.path)
            .field("vendor_id", &format_args!("{:#06x}", self.vendor_id))
            .field("product_id", &format_args!("{:#06x}", self.product_id))
            .field("usage_page", &format_args!("{:#06x}", self.usage_page))
            .field("usage", &format_args!("{:#06x}", self.usage))
            .field("interface_number", &self.interface_number)
            .field(
                "serial_number",
                &masked_serial(self.serial_number.as_deref()),
            )
            .field("manufacturer", &self.manufacturer)
            .field("product", &self.product)
            .field("transport", &self.transport)
            .finish()
    }
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

    /// Classifies an exact supported Steam Controller 2 input collection.
    #[must_use]
    pub fn controller_transport(&self) -> Option<ControllerTransport> {
        if self.vendor_id != PROTEUS_VENDOR_ID
            || self.usage_page != STEAM_USAGE_PAGE
            || self.usage != STEAM_CONTROLLER_USAGE
        {
            return None;
        }
        if self.product_id == PROTEUS_PRODUCT_ID
            && self.transport == "USB"
            && self.interface_number >= FIRST_PROTEUS_SLOT_INTERFACE
            && self.interface_number <= LAST_PROTEUS_SLOT_INTERFACE
        {
            return Some(ControllerTransport::Puck);
        }
        if self.product_id == STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID
            && self.transport == "Bluetooth"
            && self.interface_number == BLUETOOTH_CONTROLLER_INTERFACE
        {
            return Some(ControllerTransport::Bluetooth);
        }
        None
    }

    /// Returns whether this collection is an exact supported Steam Controller
    /// 2 Puck slot or direct Bluetooth vendor collection.
    #[must_use]
    pub fn is_supported_controller_source(&self) -> bool {
        self.controller_transport().is_some()
    }

    /// Returns whether the narrow lizard-mode command may be sent.
    #[must_use]
    pub fn supports_lizard_mode_suppression(&self) -> bool {
        self.is_supported_controller_source()
    }

    /// Returns whether the narrow dual-rumble output may be sent.
    #[must_use]
    pub fn supports_rumble(&self) -> bool {
        self.is_supported_controller_source()
    }

    /// Returns whether the narrow Triton pad-tick output may be sent.
    #[must_use]
    pub fn supports_pad_haptics(&self) -> bool {
        self.is_supported_controller_source()
    }

    /// Returns whether the narrow controller power-off command may be sent.
    #[must_use]
    pub fn supports_power_off(&self) -> bool {
        self.is_supported_controller_source()
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
    UnsupportedPadHapticsTarget {
        vendor_id: u16,
        product_id: u16,
        usage_page: u16,
        usage: u16,
        interface_number: i32,
    },
    UnsupportedPowerOffTarget {
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
            } => write_unsupported_target(
                f,
                "lizard-mode",
                *vendor_id,
                *product_id,
                *usage_page,
                *usage,
                *interface_number,
            ),
            Self::UnsupportedRumbleTarget {
                vendor_id,
                product_id,
                usage_page,
                usage,
                interface_number,
            } => write_unsupported_target(
                f,
                "rumble",
                *vendor_id,
                *product_id,
                *usage_page,
                *usage,
                *interface_number,
            ),
            Self::UnsupportedPadHapticsTarget {
                vendor_id,
                product_id,
                usage_page,
                usage,
                interface_number,
            } => write_unsupported_target(
                f,
                "pad-haptic",
                *vendor_id,
                *product_id,
                *usage_page,
                *usage,
                *interface_number,
            ),
            Self::UnsupportedPowerOffTarget {
                vendor_id,
                product_id,
                usage_page,
                usage,
                interface_number,
            } => write_unsupported_target(
                f,
                "power-off",
                *vendor_id,
                *product_id,
                *usage_page,
                *usage,
                *interface_number,
            ),
            Self::UnsupportedPlatform => {
                write!(f, "live HID access is currently implemented only on macOS")
            }
        }
    }
}

fn write_unsupported_target(
    f: &mut std::fmt::Formatter<'_>,
    operation: &str,
    vendor_id: u16,
    product_id: u16,
    usage_page: u16,
    usage: u16,
    interface_number: i32,
) -> std::fmt::Result {
    write!(
        f,
        "refusing {operation} write to unsupported collection \
         {vendor_id:04x}:{product_id:04x} usage \
         {usage_page:04x}:{usage:04x} interface {interface_number}; \
         select either an active 28de:1304 ff00:0001 USB Puck slot on \
         interface 2-5 or the 28de:1303 ff00:0001 Bluetooth collection \
         on interface -1"
    )
}

impl std::error::Error for DeviceError {}

#[cfg(target_os = "macos")]
mod platform;

#[cfg(target_os = "macos")]
pub use platform::{enumerate, ControllerEnumerator, HidSession};

/// Non-macOS live-HID stub. Every fallible stub in this section returns
/// [`DeviceError::UnsupportedPlatform`] without touching hardware.
#[allow(
    clippy::missing_errors_doc,
    reason = "documented by the non-macOS stub contract"
)]
#[cfg(not(target_os = "macos"))]
pub fn enumerate() -> Result<Vec<HidDeviceInfo>, DeviceError> {
    Err(DeviceError::UnsupportedPlatform)
}

#[cfg(not(target_os = "macos"))]
pub struct ControllerEnumerator;

#[cfg(not(target_os = "macos"))]
#[allow(
    clippy::missing_errors_doc,
    reason = "documented by the non-macOS stub contract"
)]
impl ControllerEnumerator {
    pub fn new() -> Result<Self, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    pub fn enumerate(&mut self) -> Result<Vec<HidDeviceInfo>, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    pub fn enumerate_all(&mut self) -> Result<Vec<HidDeviceInfo>, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    pub fn open(&self, _info: &HidDeviceInfo) -> Result<HidSession, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }
}

#[cfg(not(target_os = "macos"))]
pub struct HidSession;

#[cfg(not(target_os = "macos"))]
#[allow(
    clippy::missing_errors_doc,
    reason = "documented by the non-macOS stub contract"
)]
impl HidSession {
    pub fn open_index(_index: usize) -> Result<Self, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    pub fn open_info(_info: &HidDeviceInfo) -> Result<Self, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    pub fn poll(&mut self, _timeout: Duration) -> Result<Option<DeviceEvent>, DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    pub fn suppress_lizard_mode(&self) -> Result<(), DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    pub fn set_rumble(&self, _low_frequency: u16, _high_frequency: u16) -> Result<(), DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    pub fn pad_haptic_tick(
        &self,
        _side: steam_controller_protocol::PadHapticSide,
        _gain: steam_controller_protocol::PadHapticGain,
    ) -> Result<(), DeviceError> {
        Err(DeviceError::UnsupportedPlatform)
    }

    pub fn power_off(&self) -> Result<(), DeviceError> {
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

    fn puck_target() -> HidDeviceInfo {
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
    fn puck_target_is_exact_and_interface_bounded() {
        let target = puck_target();
        assert_eq!(
            target.controller_transport(),
            Some(ControllerTransport::Puck)
        );
        assert!(target.is_supported_controller_source());
        assert!(target.supports_lizard_mode_suppression());
        assert!(target.supports_rumble());
        assert!(target.supports_pad_haptics());
        assert!(target.supports_power_off());

        for mutate in [
            |info: &mut HidDeviceInfo| info.vendor_id = 0,
            |info: &mut HidDeviceInfo| info.product_id = 0,
            |info: &mut HidDeviceInfo| info.usage_page = 0,
            |info: &mut HidDeviceInfo| info.usage = 0,
            |info: &mut HidDeviceInfo| info.interface_number = 1,
            |info: &mut HidDeviceInfo| info.interface_number = 6,
            |info: &mut HidDeviceInfo| info.transport = "Bluetooth".to_owned(),
        ] {
            let mut other = target.clone();
            mutate(&mut other);
            assert_eq!(other.controller_transport(), None);
            assert!(!other.is_supported_controller_source());
            assert!(!other.supports_lizard_mode_suppression());
            assert!(!other.supports_rumble());
            assert!(!other.supports_pad_haptics());
            assert!(!other.supports_power_off());
        }
    }

    #[test]
    fn bluetooth_target_is_exact_and_ignores_names() {
        let mut target = HidDeviceInfo {
            id: "bluetooth-controller".to_owned(),
            path: "bluetooth-controller".to_owned(),
            vendor_id: PROTEUS_VENDOR_ID,
            product_id: STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID,
            usage_page: STEAM_USAGE_PAGE,
            usage: STEAM_CONTROLLER_USAGE,
            interface_number: BLUETOOTH_CONTROLLER_INTERFACE,
            serial_number: Some("redacted".to_owned()),
            manufacturer: None,
            product: None,
            transport: "Bluetooth".to_owned(),
        };
        assert_eq!(
            target.controller_transport(),
            Some(ControllerTransport::Bluetooth)
        );
        assert!(target.supports_lizard_mode_suppression());
        assert!(target.supports_rumble());
        assert!(target.supports_pad_haptics());
        assert!(target.supports_power_off());

        target.product_id = PROTEUS_PRODUCT_ID;
        assert_eq!(target.controller_transport(), None);
        target.product_id = STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID;
        target.interface_number = 0;
        assert_eq!(target.controller_transport(), None);
        target.interface_number = BLUETOOTH_CONTROLLER_INTERFACE;
        target.transport = "USB".to_owned();
        assert_eq!(target.controller_transport(), None);
        target.transport = "Bluetooth".to_owned();
        target.usage = 2;
        assert_eq!(target.controller_transport(), None);
    }

    #[test]
    fn serials_are_masked_to_the_last_four_characters() {
        assert_eq!(masked_serial(None), "<none>");
        assert_eq!(masked_serial(Some("")), "<none>");
        assert_eq!(masked_serial(Some("ab")), "****");
        assert_eq!(masked_serial(Some("abcd")), "****");
        assert_eq!(masked_serial(Some("abcde")), "****bcde");
        assert_eq!(masked_serial(Some("5E6EF905E5468F85")), "****8F85");
        // A Bluetooth serial is a MAC address; only its tail survives.
        assert_eq!(masked_serial(Some("a1:b2:c3:d4:e5:f6")), "****5:f6");
    }

    #[test]
    fn debug_output_never_carries_a_whole_serial() {
        let mut info = puck_target();
        info.serial_number = Some("5E6EF905E5468F85".to_owned());
        let rendered = format!("{info:?}");
        assert!(!rendered.contains("5E6EF905E5468F85"));
        assert!(rendered.contains("****8F85"));
        // Non-identifying fields must still be readable for diagnostics.
        assert!(rendered.contains("Steam Controller Puck"));
        assert!(rendered.contains("0x28de"));
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
