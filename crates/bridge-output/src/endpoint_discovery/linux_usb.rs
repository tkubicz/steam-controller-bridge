use std::collections::BTreeSet;

use crate::bridge_transport::{BridgeTransportError, ByteTransport};
use crate::endpoint_discovery::BridgeEndpoint;

pub(crate) fn with_official_usb_endpoints(
    serial_endpoints: Vec<BridgeEndpoint>,
) -> Result<Vec<BridgeEndpoint>, BridgeTransportError> {
    let serial_ids = official_serial_ids(&serial_endpoints);
    linux_bridge_usb::discover(&serial_ids)
        .map(|devices| merge_official_usb_endpoints(serial_endpoints, devices, &serial_ids))
        .map_err(map_error)
}

fn merge_official_usb_endpoints(
    mut serial_endpoints: Vec<BridgeEndpoint>,
    devices: Vec<linux_bridge_usb::DeviceInfo>,
    serial_ids: &BTreeSet<String>,
) -> Vec<BridgeEndpoint> {
    for device in devices {
        if serial_ids.contains(&device.stable_id) {
            continue;
        }
        serial_endpoints.push(BridgeEndpoint::official_linux_usb(
            device.locator,
            device.stable_id,
        ));
    }
    serial_endpoints.sort_by(|left, right| {
        left.display_label()
            .cmp(right.display_label())
            .then_with(|| left.stable_id().cmp(&right.stable_id()))
    });
    serial_endpoints
}

fn official_serial_ids(serial_endpoints: &[BridgeEndpoint]) -> BTreeSet<String> {
    serial_endpoints
        .iter()
        .filter(|endpoint| endpoint.is_bridge_device())
        .filter_map(BridgeEndpoint::stable_id)
        .map(str::to_owned)
        .collect()
}

pub(crate) fn open(
    endpoint: &BridgeEndpoint,
) -> Result<linux_bridge_usb::UsbTransport, BridgeTransportError> {
    let stable_id = endpoint.stable_id().ok_or_else(|| {
        BridgeTransportError::InvalidTopology(
            "raw USB endpoints require a stable device serial".to_owned(),
        )
    })?;
    linux_bridge_usb::open(stable_id).map_err(map_error)
}

impl ByteTransport for linux_bridge_usb::UsbTransport {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        linux_bridge_usb::UsbTransport::write_all(self, bytes)
    }

    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        linux_bridge_usb::UsbTransport::read(self, bytes)
    }
}

fn map_error(error: linux_bridge_usb::Error) -> BridgeTransportError {
    match error {
        linux_bridge_usb::Error::AccessDenied(endpoint) => {
            BridgeTransportError::AccessDenied(endpoint)
        }
        linux_bridge_usb::Error::Busy(endpoint) => BridgeTransportError::DeviceBusy(endpoint),
        linux_bridge_usb::Error::InvalidTopology(reason) => {
            BridgeTransportError::InvalidTopology(reason)
        }
        linux_bridge_usb::Error::Disconnected(endpoint) => {
            BridgeTransportError::Disconnected(endpoint)
        }
        linux_bridge_usb::Error::Unsupported => {
            BridgeTransportError::UnsupportedTransport("Linux USB")
        }
        linux_bridge_usb::Error::Io(error) => BridgeTransportError::from(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint_discovery::{
        BridgeUsbIdentity, BRIDGE_DEVICE_USB_PRODUCT, OFFICIAL_BRIDGE_USB_MANUFACTURER,
        OFFICIAL_BRIDGE_USB_PRODUCT_ID, OFFICIAL_BRIDGE_USB_VENDOR_ID,
    };

    #[test]
    fn matching_serial_endpoint_deduplicates_the_raw_usb_candidate() {
        let serial = BridgeEndpoint::serial_port("/dev/ttyACM0", 115_200)
            .with_stable_id("same")
            .with_usb_identity(BridgeUsbIdentity {
                vendor_id: OFFICIAL_BRIDGE_USB_VENDOR_ID,
                product_id: OFFICIAL_BRIDGE_USB_PRODUCT_ID,
                manufacturer: Some(OFFICIAL_BRIDGE_USB_MANUFACTURER.to_owned()),
                product: Some(BRIDGE_DEVICE_USB_PRODUCT.to_owned()),
            });
        let locator = linux_bridge_usb::Locator {
            bus_number: 3,
            device_address: 4,
            control_interface: 0,
            data_interface: 1,
            xinput_interface: 2,
            bulk_in_endpoint: 0x82,
            bulk_out_endpoint: 0x01,
        };

        let serial_ids = official_serial_ids(std::slice::from_ref(&serial));
        let endpoints = merge_official_usb_endpoints(
            vec![serial],
            vec![
                linux_bridge_usb::DeviceInfo {
                    locator,
                    stable_id: "same".to_owned(),
                },
                linux_bridge_usb::DeviceInfo {
                    locator,
                    stable_id: "other".to_owned(),
                },
            ],
            &serial_ids,
        );

        assert_eq!(endpoints.len(), 2);
        assert_eq!(
            endpoints
                .iter()
                .filter(|endpoint| endpoint.stable_id() == Some("same"))
                .count(),
            1
        );
        assert!(endpoints.iter().any(|endpoint| {
            endpoint.stable_id() == Some("other") && endpoint.serial_path().is_none()
        }));
    }
}
