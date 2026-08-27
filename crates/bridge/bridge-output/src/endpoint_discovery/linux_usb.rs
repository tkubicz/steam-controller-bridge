use crate::bridge_transport::{BridgeTransportError, ByteTransport};
use crate::endpoint_discovery::{BridgeEndpoint, BridgeEndpointDiscovery};

pub use linux_bridge_usb::{
    Locator as LinuxUsbLocator, MANUFACTURER as OFFICIAL_BRIDGE_USB_MANUFACTURER,
    PRODUCT as OFFICIAL_BRIDGE_USB_PRODUCT, PRODUCT_ID, VENDOR_ID,
};

pub(crate) fn with_official_usb_endpoints(
    serial_result: Result<Vec<BridgeEndpoint>, BridgeTransportError>,
) -> Result<BridgeEndpointDiscovery, BridgeTransportError> {
    with_official_usb_endpoints_with(serial_result, linux_bridge_usb::discover)
}

fn with_official_usb_endpoints_with(
    serial_result: Result<Vec<BridgeEndpoint>, BridgeTransportError>,
    discover: impl FnOnce() -> Result<linux_bridge_usb::Discovery, linux_bridge_usb::Error>,
) -> Result<BridgeEndpointDiscovery, BridgeTransportError> {
    let (serial_endpoints, serial_error) = match serial_result {
        Ok(endpoints) => (endpoints, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    match discover() {
        Ok(discovery) => {
            let mut device_errors = discovery.errors;
            if discovery.devices.is_empty() && serial_endpoints.is_empty() {
                if !device_errors.is_empty() {
                    return Err(map_error(device_errors.remove(0)));
                }
                if let Some(error) = serial_error {
                    return Err(error);
                }
            }
            let mut warnings = device_errors.into_iter().map(map_error).collect::<Vec<_>>();
            if let Some(error) = serial_error {
                warnings.push(error);
            }
            Ok(BridgeEndpointDiscovery {
                endpoints: merge_official_usb_endpoints(serial_endpoints, discovery.devices),
                warnings,
            })
        }
        Err(error) if serial_endpoints.is_empty() => {
            Err(serial_error.unwrap_or_else(|| map_error(error)))
        }
        Err(error) => Ok(BridgeEndpointDiscovery {
            endpoints: serial_endpoints,
            warnings: vec![map_error(error)],
        }),
    }
}

fn merge_official_usb_endpoints(
    mut serial_endpoints: Vec<BridgeEndpoint>,
    devices: Vec<linux_bridge_usb::DeviceInfo>,
) -> Vec<BridgeEndpoint> {
    for device in devices {
        serial_endpoints.push(BridgeEndpoint::official_linux_usb(
            device.locator.bus_number,
            device.locator.device_address,
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
        linux_bridge_usb::Error::GamepadUnavailable(reason) => {
            BridgeTransportError::GamepadUnavailable(reason)
        }
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
    fn matching_serial_endpoint_keeps_raw_usb_as_an_open_fallback() {
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
        };

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
        );

        assert_eq!(endpoints.len(), 3);
        assert_eq!(
            endpoints
                .iter()
                .filter(|endpoint| endpoint.stable_id() == Some("same"))
                .count(),
            2
        );
        assert!(endpoints.iter().any(|endpoint| {
            endpoint.stable_id() == Some("other") && endpoint.serial_path().is_none()
        }));
    }

    #[test]
    fn a_raw_discovery_failure_does_not_discard_serial_fallbacks() {
        let serial = BridgeEndpoint::serial_port("serial:fallback", 115_200);

        let discovery = with_official_usb_endpoints_with(Ok(vec![serial.clone()]), || {
            Err(linux_bridge_usb::Error::Io(std::io::Error::other(
                "USB enumeration failed",
            )))
        })
        .unwrap();

        assert_eq!(discovery.endpoints, vec![serial]);
        assert_eq!(discovery.warnings.len(), 1);
        assert!(discovery.warnings[0]
            .to_string()
            .contains("USB enumeration failed"));
    }

    #[test]
    fn one_bad_raw_device_does_not_hide_a_good_raw_device() {
        let locator = linux_bridge_usb::Locator {
            bus_number: 3,
            device_address: 4,
        };

        let discovery = with_official_usb_endpoints_with(Ok(Vec::new()), || {
            Ok(linux_bridge_usb::Discovery {
                devices: vec![linux_bridge_usb::DeviceInfo {
                    locator,
                    stable_id: "good".to_owned(),
                }],
                errors: vec![linux_bridge_usb::Error::InvalidTopology(
                    "bad prototype".to_owned(),
                )],
            })
        })
        .unwrap();

        assert_eq!(discovery.endpoints.len(), 1);
        assert_eq!(discovery.endpoints[0].stable_id(), Some("good"));
        assert_eq!(discovery.warnings.len(), 1);
        assert!(discovery.warnings[0].to_string().contains("bad prototype"));
    }
}
