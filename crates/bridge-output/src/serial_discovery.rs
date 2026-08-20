use crate::serial::SerialError;

#[cfg(target_os = "linux")]
mod linux;
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

pub(crate) fn open_error(path: &str, error: serialport::Error) -> SerialError {
    platform::open_error(path, error)
}

fn generic_open_error(error: serialport::Error) -> SerialError {
    error.into()
}

/// Enumerates native serial port names.
///
/// # Errors
/// Returns an error when the native backend cannot enumerate ports.
pub fn available_serial_ports() -> Result<Vec<String>, SerialError> {
    available_serial_devices().map(|ports| ports.into_iter().map(|port| port.path).collect())
}

/// Enumerates native serial ports with USB identity metadata.
///
/// # Errors
/// Returns an error when the native backend cannot enumerate ports.
pub fn available_serial_devices() -> Result<Vec<SerialDeviceInfo>, SerialError> {
    let mut devices = serialport::available_ports()?
        .into_iter()
        .map(device_info)
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(devices)
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
}
