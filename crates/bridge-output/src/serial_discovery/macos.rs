pub(super) fn is_callout_port(path: &str) -> bool {
    path.starts_with("/dev/cu.")
}

#[cfg(test)]
pub(super) const TEST_CALLOUT_PORT: &str = "/dev/cu.usbmodem11201";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial_discovery::{SerialDeviceInfo, BRIDGE_DEVICE_USB_PRODUCT};

    #[test]
    fn excludes_dial_in_ports() {
        assert!(is_callout_port("/dev/cu.usbmodem11201"));
        assert!(!is_callout_port("/dev/tty.usbmodem11201"));
    }

    #[test]
    fn bridge_device_filter_rejects_a_marked_dial_in_port() {
        let device = SerialDeviceInfo {
            path: "/dev/tty.usbmodem11201".to_owned(),
            vendor_id: None,
            product_id: None,
            serial_number: None,
            manufacturer: None,
            product: Some(BRIDGE_DEVICE_USB_PRODUCT.to_owned()),
        };

        assert!(!device.is_bridge_device());
    }
}
