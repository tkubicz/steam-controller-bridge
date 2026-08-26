use crate::bridge_transport::BridgeTransportError;

#[cfg(target_os = "linux")]
mod linux;
pub(crate) mod linux_usb;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod portable;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use portable as platform;

/// Board-neutral USB product marker used for zero-configuration serial
/// discovery. Vendor, product, and manufacturer IDs remain unrestricted for
/// independent protocol implementations.
pub const BRIDGE_DEVICE_USB_PRODUCT: &str = "Steam Controller Bridge";
pub use linux_bridge_usb::{
    Locator as LinuxUsbLocator, MANUFACTURER as OFFICIAL_BRIDGE_USB_MANUFACTURER,
    PRODUCT_ID as OFFICIAL_BRIDGE_USB_PRODUCT_ID, VENDOR_ID as OFFICIAL_BRIDGE_USB_VENDOR_ID,
};
pub const DEFAULT_BRIDGE_BAUD_RATE: u32 = 115_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeTransportKind {
    SerialPort,
    LinuxUsb,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeUsbIdentity {
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeEndpointLocator {
    SerialPort { path: String, baud_rate: u32 },
    LinuxUsb(LinuxUsbLocator),
}

#[derive(Clone, PartialEq, Eq)]
pub struct BridgeEndpoint {
    locator: BridgeEndpointLocator,
    stable_id: Option<String>,
    display_label: String,
    usb_identity: Option<BridgeUsbIdentity>,
}

impl std::fmt::Debug for BridgeEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BridgeEndpoint")
            .field("kind", &self.kind())
            .field("locator", &self.locator)
            .field("stable_id_present", &self.stable_id.is_some())
            .field("display_label", &self.display_label)
            .field("usb_identity", &self.usb_identity)
            .finish()
    }
}

impl BridgeEndpoint {
    #[must_use]
    pub fn serial_port(path: impl Into<String>, baud_rate: u32) -> Self {
        let path = path.into();
        Self {
            display_label: path.clone(),
            locator: BridgeEndpointLocator::SerialPort { path, baud_rate },
            stable_id: None,
            usb_identity: None,
        }
    }

    #[must_use]
    pub fn with_stable_id(mut self, stable_id: impl Into<String>) -> Self {
        self.stable_id = Some(stable_id.into());
        self
    }

    #[must_use]
    pub fn official_linux_usb(locator: LinuxUsbLocator, stable_id: impl Into<String>) -> Self {
        let usb_identity = BridgeUsbIdentity {
            vendor_id: OFFICIAL_BRIDGE_USB_VENDOR_ID,
            product_id: OFFICIAL_BRIDGE_USB_PRODUCT_ID,
            manufacturer: Some(OFFICIAL_BRIDGE_USB_MANUFACTURER.to_owned()),
            product: Some(BRIDGE_DEVICE_USB_PRODUCT.to_owned()),
        };
        Self {
            locator: BridgeEndpointLocator::LinuxUsb(locator),
            stable_id: Some(stable_id.into()),
            display_label: format!("{BRIDGE_DEVICE_USB_PRODUCT} (USB)"),
            usb_identity: Some(usb_identity),
        }
    }

    #[must_use]
    pub fn with_usb_identity(mut self, usb_identity: BridgeUsbIdentity) -> Self {
        self.usb_identity = Some(usb_identity);
        self
    }

    #[must_use]
    pub fn from_serial_device(device: SerialDeviceInfo, baud_rate: u32) -> Option<Self> {
        if !device.is_callout_port() {
            return None;
        }
        let usb_identity =
            device
                .vendor_id
                .zip(device.product_id)
                .map(|(vendor_id, product_id)| BridgeUsbIdentity {
                    vendor_id,
                    product_id,
                    manufacturer: device.manufacturer,
                    product: device.product,
                });
        Some(Self {
            display_label: device.path.clone(),
            locator: BridgeEndpointLocator::SerialPort {
                path: device.path,
                baud_rate,
            },
            stable_id: device.serial_number,
            usb_identity,
        })
    }

    #[must_use]
    pub const fn kind(&self) -> BridgeTransportKind {
        match self.locator {
            BridgeEndpointLocator::SerialPort { .. } => BridgeTransportKind::SerialPort,
            BridgeEndpointLocator::LinuxUsb(_) => BridgeTransportKind::LinuxUsb,
        }
    }

    #[must_use]
    pub fn stable_id(&self) -> Option<&str> {
        self.stable_id.as_deref()
    }

    #[must_use]
    pub fn display_label(&self) -> &str {
        &self.display_label
    }

    #[must_use]
    pub const fn usb_identity(&self) -> Option<&BridgeUsbIdentity> {
        self.usb_identity.as_ref()
    }

    #[must_use]
    pub const fn locator(&self) -> &BridgeEndpointLocator {
        &self.locator
    }

    #[must_use]
    pub fn serial_path(&self) -> Option<&str> {
        match &self.locator {
            BridgeEndpointLocator::SerialPort { path, .. } => Some(path),
            BridgeEndpointLocator::LinuxUsb(_) => None,
        }
    }

    #[must_use]
    pub fn with_serial_baud_rate(mut self, baud_rate: u32) -> Self {
        if let BridgeEndpointLocator::SerialPort {
            baud_rate: endpoint_baud_rate,
            ..
        } = &mut self.locator
        {
            *endpoint_baud_rate = baud_rate;
        }
        self
    }

    #[must_use]
    pub fn is_bridge_device(&self) -> bool {
        self.usb_identity
            .as_ref()
            .and_then(|identity| identity.product.as_deref())
            == Some(BRIDGE_DEVICE_USB_PRODUCT)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialDeviceInfo {
    pub path: String,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub serial_number: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
}

impl SerialDeviceInfo {
    /// Returns whether the host backend considers the endpoint safe for an
    /// outgoing connection. macOS accepts `/dev/cu.*`; Linux accepts numbered
    /// `/dev/ttyACM<N>` and `/dev/ttyUSB<N>` endpoints.
    #[must_use]
    pub fn is_callout_port(&self) -> bool {
        !self.path.is_empty() && platform::is_callout_port(&self.path)
    }

    #[must_use]
    pub fn is_bridge_device(&self) -> bool {
        self.is_callout_port() && self.product.as_deref() == Some(BRIDGE_DEVICE_USB_PRODUCT)
    }
}

pub(crate) fn open_error(path: &str, error: serialport::Error) -> BridgeTransportError {
    platform::open_error(path, error)
}

fn generic_open_error(error: serialport::Error) -> BridgeTransportError {
    error.into()
}

/// Enumerates native serial port names.
///
/// # Errors
/// Returns an error when the native backend cannot enumerate ports.
pub fn available_serial_ports() -> Result<Vec<String>, BridgeTransportError> {
    available_serial_devices().map(|ports| ports.into_iter().map(|port| port.path).collect())
}

/// Enumerates native serial ports with USB identity metadata.
///
/// # Errors
/// Returns an error when the native backend cannot enumerate ports.
pub fn available_serial_devices() -> Result<Vec<SerialDeviceInfo>, BridgeTransportError> {
    let mut devices = serialport::available_ports()?
        .into_iter()
        .map(device_info)
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(devices)
}

/// Enumerates native bridge transport endpoints.
///
/// # Errors
/// Returns an error when a native transport backend cannot enumerate endpoints.
pub fn available_bridge_endpoints() -> Result<Vec<BridgeEndpoint>, BridgeTransportError> {
    let serial = available_serial_endpoints()?;
    linux_usb::with_official_usb_endpoints(serial)
}

/// Enumerates native serial bridge transport endpoints.
///
/// # Errors
/// Returns an error when the native serial backend cannot enumerate endpoints.
pub fn available_serial_endpoints() -> Result<Vec<BridgeEndpoint>, BridgeTransportError> {
    available_serial_devices().map(|devices| {
        devices
            .into_iter()
            .filter_map(|device| {
                BridgeEndpoint::from_serial_device(device, DEFAULT_BRIDGE_BAUD_RATE)
            })
            .collect()
    })
}

fn device_info(port: serialport::SerialPortInfo) -> SerialDeviceInfo {
    let (vendor_id, product_id, serial_number, manufacturer, product) = match port.port_type {
        serialport::SerialPortType::UsbPort(usb) => (
            Some(usb.vid),
            Some(usb.pid),
            usb.serial_number,
            usb.manufacturer,
            usb.product,
        ),
        _ => (None, None, None, None, None),
    };
    SerialDeviceInfo {
        path: port.port_name,
        vendor_id,
        product_id,
        serial_number,
        manufacturer,
        product,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge_device(product: &str) -> SerialDeviceInfo {
        SerialDeviceInfo {
            path: platform::TEST_CALLOUT_PORT.to_owned(),
            vendor_id: Some(0x1209),
            product_id: Some(0x0001),
            serial_number: Some("TESTSERIAL0000".to_owned()),
            manufacturer: Some("Independent implementer".to_owned()),
            product: Some(product.to_owned()),
        }
    }

    #[test]
    fn bridge_device_filter_uses_product_marker() {
        assert!(bridge_device(BRIDGE_DEVICE_USB_PRODUCT).is_bridge_device());
        assert!(!bridge_device("Steam Controller Puck").is_bridge_device());
    }

    #[test]
    fn empty_paths_are_never_eligible_endpoints() {
        let mut device = bridge_device(BRIDGE_DEVICE_USB_PRODUCT);
        device.path.clear();

        assert!(!device.is_callout_port());
        assert!(!device.is_bridge_device());
    }

    #[test]
    fn typed_endpoint_separates_serial_locator_and_stable_identity() {
        let endpoint = BridgeEndpoint::from_serial_device(
            bridge_device(BRIDGE_DEVICE_USB_PRODUCT),
            DEFAULT_BRIDGE_BAUD_RATE,
        )
        .unwrap();

        assert_eq!(endpoint.kind(), BridgeTransportKind::SerialPort);
        assert_eq!(endpoint.stable_id(), Some("TESTSERIAL0000"));
        assert_eq!(endpoint.display_label(), platform::TEST_CALLOUT_PORT);
        assert!(endpoint.is_bridge_device());
        assert_eq!(
            endpoint.locator(),
            &BridgeEndpointLocator::SerialPort {
                path: platform::TEST_CALLOUT_PORT.to_owned(),
                baud_rate: DEFAULT_BRIDGE_BAUD_RATE,
            }
        );
    }

    #[test]
    fn endpoint_debug_omits_the_stable_identity() {
        let endpoint = BridgeEndpoint::from_serial_device(
            bridge_device(BRIDGE_DEVICE_USB_PRODUCT),
            DEFAULT_BRIDGE_BAUD_RATE,
        )
        .unwrap();

        let debug = format!("{endpoint:?}");

        assert!(debug.contains("stable_id_present: true"));
        assert!(!debug.contains("TESTSERIAL0000"));
    }

    #[test]
    fn raw_usb_endpoint_keeps_identity_separate_from_its_ephemeral_locator() {
        let locator = LinuxUsbLocator {
            bus_number: 3,
            device_address: 4,
            control_interface: 0,
            data_interface: 1,
            xinput_interface: 2,
            bulk_in_endpoint: 0x82,
            bulk_out_endpoint: 0x01,
        };
        let endpoint = BridgeEndpoint::official_linux_usb(locator, "TESTSERIAL0000")
            .with_serial_baud_rate(9_600);

        assert_eq!(endpoint.kind(), BridgeTransportKind::LinuxUsb);
        assert_eq!(endpoint.stable_id(), Some("TESTSERIAL0000"));
        assert_eq!(
            endpoint.locator(),
            &BridgeEndpointLocator::LinuxUsb(locator)
        );
        assert_eq!(endpoint.serial_path(), None);
        assert!(endpoint.is_bridge_device());
    }
}
