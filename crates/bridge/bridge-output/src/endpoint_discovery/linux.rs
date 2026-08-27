use std::io;

#[cfg(test)]
use super::SerialDeviceInfo;
use super::{generic_open_error, BridgeTransportError};

pub(super) fn is_callout_port(path: &str) -> bool {
    numbered_device(path, "/dev/ttyACM") || numbered_device(path, "/dev/ttyUSB")
}

fn numbered_device(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

pub(super) fn open_error(path: &str, error: serialport::Error) -> BridgeTransportError {
    match error.kind() {
        serialport::ErrorKind::Io(io::ErrorKind::PermissionDenied) => {
            BridgeTransportError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "cannot open Linux serial endpoint {path}: permission denied; grant this device read/write access with a narrowly matched udev rule or the distribution's serial-access group (commonly dialout). Reconnect after changing udev rules, or start a new login or service after changing groups"
                ),
            ))
        }
        serialport::ErrorKind::NoDevice
        | serialport::ErrorKind::Io(io::ErrorKind::NotFound) => BridgeTransportError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "cannot open Linux serial endpoint {path}: the device is unavailable; it may have disconnected or another process may hold exclusive access. Reconnect it, close serial monitors, or, if ModemManager is probing this endpoint, configure a narrowly matched ID_MM_DEVICE_IGNORE rule, then retry"
            ),
        )),
        _ => generic_open_error(error),
    }
}

#[cfg(test)]
pub(super) const TEST_CALLOUT_PORT: &str = "/dev/ttyACM0";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint_discovery::BRIDGE_DEVICE_USB_PRODUCT;

    fn device(path: &str, product: &str) -> SerialDeviceInfo {
        SerialDeviceInfo {
            path: path.to_owned(),
            vendor_id: Some(0x1209),
            product_id: Some(0x0001),
            serial_number: Some("THIRDPARTY0001".to_owned()),
            manufacturer: Some("Independent implementer".to_owned()),
            product: Some(product.to_owned()),
        }
    }

    #[test]
    fn recognizes_only_numbered_usb_serial_device_names() {
        assert!(device("/dev/ttyACM0", "ignored").is_callout_port());
        assert!(device("/dev/ttyUSB12", "ignored").is_callout_port());
        assert!(!device("/dev/ttyS0", "ignored").is_callout_port());
        assert!(!device("/dev/ttyACM", "ignored").is_callout_port());
        assert!(!device("/dev/ttyACM0x", "ignored").is_callout_port());
        assert!(!device("/dev/cu.usbmodem1", "ignored").is_callout_port());
    }

    #[test]
    fn marked_non_usb_serial_endpoints_are_not_automatic_candidates() {
        assert!(device("/dev/ttyACM0", BRIDGE_DEVICE_USB_PRODUCT).is_bridge_device());
        assert!(!device("/dev/ttyS0", BRIDGE_DEVICE_USB_PRODUCT).is_bridge_device());
        assert!(!device("/dev/ttyACM0", "Steam Controller Puck").is_bridge_device());
    }

    #[test]
    fn open_failures_explain_permissions_and_unavailable_devices() {
        let permission = open_error(
            "/dev/ttyACM0",
            serialport::Error::new(
                serialport::ErrorKind::Io(io::ErrorKind::PermissionDenied),
                "Permission denied",
            ),
        );
        let BridgeTransportError::Io(permission) = permission else {
            panic!("permission failure must remain an I/O error");
        };
        assert_eq!(permission.kind(), io::ErrorKind::PermissionDenied);
        assert!(permission.to_string().contains("udev rule"));
        assert!(permission.to_string().contains("serial-access group"));
        assert!(permission.to_string().contains("new login or service"));

        for error in [
            serialport::Error::new(serialport::ErrorKind::NoDevice, "Device or resource busy"),
            serialport::Error::new(
                serialport::ErrorKind::Io(io::ErrorKind::NotFound),
                "No such file or directory",
            ),
        ] {
            let unavailable = open_error("/dev/ttyUSB2", error);
            let BridgeTransportError::Io(unavailable) = unavailable else {
                panic!("unavailable device must remain an I/O error");
            };
            assert_eq!(unavailable.kind(), io::ErrorKind::NotFound);
            assert!(unavailable.to_string().contains("disconnected"));
            assert!(unavailable.to_string().contains("exclusive access"));
            assert!(unavailable.to_string().contains("ModemManager"));
            assert!(unavailable.to_string().contains("narrowly matched"));
        }

        assert_eq!(
            open_error(
                "/dev/ttyACM0",
                serialport::Error::new(serialport::ErrorKind::InvalidInput, "invalid baud rate"),
            )
            .to_string(),
            "bridge transport I/O failed: invalid baud rate"
        );
    }
}
