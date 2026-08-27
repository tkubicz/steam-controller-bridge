use std::fs;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use nusb::descriptors::{ConfigurationDescriptor, TransferType};
use nusb::transfer::{
    Buffer, Bulk, ControlOut, ControlType, Direction, In, Out, Recipient, TransferError,
};
use nusb::{Device, DeviceInfo as NusbDeviceInfo, Endpoint, Interface, MaybeFuture};
use rustix::fs::{Mode, OFlags};

use crate::{DeviceInfo, Discovery, Error, Locator, MANUFACTURER, PRODUCT, PRODUCT_ID, VENDOR_ID};

const USB_CLASS_COMMUNICATIONS: u8 = 0x02;
const USB_CLASS_CDC_DATA: u8 = 0x0a;
const USB_CLASS_VENDOR: u8 = 0xff;
const CDC_SUBCLASS_ACM: u8 = 0x02;
const XINPUT_SUBCLASS: u8 = 0x5d;
const XINPUT_PROTOCOL: u8 = 0x01;
const CONTROL_TIMEOUT: Duration = Duration::from_millis(250);
// Leave headroom below the firmware's 100 ms watchdog for the 25 ms state
// refresh interval and the runtime's 10 ms service cadence.
const BULK_OUT_TOTAL_TIMEOUT: Duration = Duration::from_millis(50);
const DTR_LOW_INTERVAL: Duration = Duration::from_millis(25);
const BULK_IN_BUFFER_SIZE: usize = 512;
const BULK_OUT_BUFFER_SIZE: usize = 512;
const XPAD_REQUIREMENT: &str = "the bridge's Xbox 360 USB interface requires xpad or xpad-noone; systems using xone must also install xpad-noone";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Topology {
    control_interface: u8,
    data_interface: u8,
    xinput_interface: u8,
    bulk_in: u8,
    bulk_out: u8,
}

pub(super) fn discover() -> Result<Discovery, Error> {
    let mut devices = nusb::list_devices()
        .wait()
        .map_err(|error| Error::Io(error.into()))?
        .filter(is_official_device)
        .collect::<Vec<_>>();
    devices.sort_by_key(|info| (info.busnum(), info.device_address()));
    let mut discovery = Discovery::default();
    for info in &devices {
        match discover_device(info) {
            Ok(device) => discovery.devices.push(device),
            Err(error) => discovery.errors.push(error),
        }
    }
    Ok(discovery)
}

fn discover_device(info: &NusbDeviceInfo) -> Result<DeviceInfo, Error> {
    let stable_id = required_serial(info)?.to_owned();
    preflight_topology(info)?;
    Ok(DeviceInfo {
        locator: Locator {
            bus_number: info.busnum(),
            device_address: info.device_address(),
        },
        stable_id,
    })
}

pub(super) fn open(stable_id: &str) -> Result<UsbTransport, Error> {
    let info = find_official_device(stable_id)?;
    let (device, topology) = prepare_device(&info)?;
    let control = device
        .claim_interface(topology.control_interface)
        .wait()
        .map_err(|error| map_nusb_error(error, PRODUCT))?;
    let data = device
        .claim_interface(topology.data_interface)
        .wait()
        .map_err(|error| map_nusb_error(error, PRODUCT))?;
    let bulk_in = data
        .endpoint::<Bulk, In>(topology.bulk_in)
        .map_err(|error| Error::InvalidTopology(error.to_string()))?;
    let bulk_out = data
        .endpoint::<Bulk, Out>(topology.bulk_out)
        .map_err(|error| Error::InvalidTopology(error.to_string()))?;
    let mut transport = UsbTransport {
        control,
        _data: data,
        bulk_in,
        bulk_out,
        bulk_out_buffer: Some(Buffer::new(BULK_OUT_BUFFER_SIZE)),
        bulk_out_poisoned: false,
    };
    set_line_coding(&transport.control, topology.control_interface).map_err(Error::Io)?;
    transport.set_dtr(false).map_err(Error::Io)?;
    thread::sleep(DTR_LOW_INTERVAL);
    transport.set_dtr(true).map_err(Error::Io)?;
    transport.bulk_in.submit(Buffer::new(BULK_IN_BUFFER_SIZE));
    Ok(transport)
}

pub(super) struct UsbTransport {
    control: Interface,
    _data: Interface,
    bulk_in: Endpoint<Bulk, In>,
    bulk_out: Endpoint<Bulk, Out>,
    bulk_out_buffer: Option<Buffer>,
    bulk_out_poisoned: bool,
}

impl UsbTransport {
    pub(super) fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.bulk_out_poisoned {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "bulk OUT transport is awaiting cancellation",
            ));
        }
        let mut buffer = self
            .bulk_out_buffer
            .take()
            .unwrap_or_else(|| Buffer::new(bytes.len().max(BULK_OUT_BUFFER_SIZE)));
        if buffer.capacity() < bytes.len() {
            buffer = Buffer::new(bytes.len());
        }
        buffer.clear();
        buffer.extend_from_slice(bytes);
        self.bulk_out.submit(buffer);
        let Some(mut completion) = self.bulk_out.wait_next_complete(BULK_OUT_TOTAL_TIMEOUT) else {
            self.bulk_out.cancel_all();
            self.bulk_out_poisoned = true;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "bulk OUT transfer timed out",
            ));
        };
        let status = completion.status;
        let actual_len = completion.actual_len;
        completion.buffer.clear();
        self.bulk_out_buffer = Some(completion.buffer);
        match status {
            Ok(()) | Err(TransferError::Cancelled) if actual_len == bytes.len() => Ok(()),
            Ok(()) => Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("bulk OUT transferred {actual_len} of {} bytes", bytes.len()),
            )),
            Err(error) => Err(map_transfer_error(error)),
        }
    }

    pub(super) fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let Some(mut completion) = self.bulk_in.wait_next_complete(Duration::ZERO) else {
            return Err(io::ErrorKind::WouldBlock.into());
        };
        if let Err(error) = completion.status {
            completion.buffer.clear();
            self.bulk_in.submit(completion.buffer);
            return Err(error.into());
        }
        if completion.actual_len > bytes.len() {
            completion.buffer.clear();
            self.bulk_in.submit(completion.buffer);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bulk IN completion exceeded the protocol read buffer",
            ));
        }
        bytes[..completion.actual_len].copy_from_slice(&completion.buffer[..completion.actual_len]);
        let count = completion.actual_len;
        completion.buffer.clear();
        self.bulk_in.submit(completion.buffer);
        Ok(count)
    }

    fn set_dtr(&mut self, asserted: bool) -> io::Result<()> {
        set_dtr(&self.control, self.control.interface_number(), asserted)
    }

    fn clear_dtr(&mut self) -> io::Result<()> {
        self.set_dtr(false)
    }
}

impl Drop for UsbTransport {
    fn drop(&mut self) {
        let _ = self.clear_dtr();
    }
}

fn find_official_device(stable_id: &str) -> Result<NusbDeviceInfo, Error> {
    let matches = nusb::list_devices()
        .wait()
        .map_err(|error| Error::Io(error.into()))?
        .filter(is_official_device)
        .filter(|device| device.serial_number() == Some(stable_id))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(Error::Disconnected(PRODUCT.to_owned())),
        [only] => Ok(only.clone()),
        _ => Err(Error::InvalidTopology(format!(
            "multiple official bridge devices report the same stable serial ({})",
            matches.len()
        ))),
    }
}

fn is_official_device(info: &NusbDeviceInfo) -> bool {
    is_official_identity(
        info.vendor_id(),
        info.product_id(),
        info.manufacturer_string(),
        info.product_string(),
    )
}

fn is_official_identity(
    vendor_id: u16,
    product_id: u16,
    manufacturer: Option<&str>,
    product: Option<&str>,
) -> bool {
    vendor_id == VENDOR_ID
        && product_id == PRODUCT_ID
        && manufacturer == Some(MANUFACTURER)
        && product == Some(PRODUCT)
}

fn required_serial(info: &NusbDeviceInfo) -> Result<&str, Error> {
    info.serial_number()
        .filter(|serial| !serial.is_empty())
        .ok_or_else(|| Error::InvalidTopology("official bridge has no stable serial".to_owned()))
}

fn prepare_device(info: &NusbDeviceInfo) -> Result<(Device, Topology), Error> {
    let preflight = preflight_topology(info)?;
    verify_xpad_driver(info, preflight.xinput_interface)?;
    let node = usb_device_node(info);
    let fd = rustix::fs::open(&node, OFlags::RDWR | OFlags::CLOEXEC, Mode::empty())
        .map_err(|error| map_io_error(&node, error.into()))?;
    let mask = interface_mask([preflight.control_interface, preflight.data_interface])?;
    retain_interfaces(&fd, mask).map_err(|error| map_io_error(&node, error))?;
    let device = Device::from_fd(fd)
        .wait()
        .map_err(|error| map_nusb_error(error, &node.display().to_string()))?;
    let configuration = device
        .active_configuration()
        .map_err(|error| Error::InvalidTopology(error.to_string()))?;
    let topology = discover_topology(&configuration)?;
    if topology.control_interface != preflight.control_interface
        || topology.data_interface != preflight.data_interface
        || topology.xinput_interface != preflight.xinput_interface
    {
        return Err(Error::InvalidTopology(format!(
            "enumeration and active descriptors disagree: {preflight:?} vs {topology:?}"
        )));
    }
    Ok((device, topology))
}

fn verify_xpad_driver(info: &NusbDeviceInfo, interface: u8) -> Result<(), Error> {
    let configuration_path = info.sysfs_path().join("bConfigurationValue");
    let configuration = fs::read_to_string(&configuration_path).map_err(|error| {
        Error::Io(io::Error::new(
            error.kind(),
            format!("cannot read '{}': {error}", configuration_path.display()),
        ))
    })?;
    let interface_path =
        usb_interface_sysfs_path(info.sysfs_path(), configuration.trim(), interface)?;
    let driver_path = interface_path.join("driver");
    let driver = fs::read_link(&driver_path).map_err(|error| {
        Error::GamepadUnavailable(format!(
            "{XPAD_REQUIREMENT}; no driver is bound at '{}': {error}",
            driver_path.display()
        ))
    })?;
    if !is_supported_gamepad_driver(driver.file_name().and_then(|name| name.to_str())) {
        return Err(Error::GamepadUnavailable(format!(
            "{XPAD_REQUIREMENT}; found {} at '{}'",
            driver.display(),
            driver_path.display(),
        )));
    }
    Ok(())
}

fn is_supported_gamepad_driver(driver: Option<&str>) -> bool {
    matches!(driver, Some("xpad" | "xpad-noone"))
}

fn usb_interface_sysfs_path(
    device_path: &Path,
    configuration: &str,
    interface: u8,
) -> Result<PathBuf, Error> {
    let device_name = device_path
        .file_name()
        .ok_or_else(|| Error::InvalidTopology("USB sysfs path has no device name".to_owned()))?;
    Ok(device_path.join(format!(
        "{}:{configuration}.{interface}",
        device_name.to_string_lossy()
    )))
}

fn preflight_topology(info: &NusbDeviceInfo) -> Result<Topology, Error> {
    let control = only(
        info.interfaces()
            .filter(|interface| {
                interface.class() == USB_CLASS_COMMUNICATIONS
                    && interface.subclass() == CDC_SUBCLASS_ACM
                    && interface.protocol() == 0
            })
            .map(nusb::InterfaceInfo::interface_number)
            .collect(),
        "CDC ACM control interface",
    )?;
    let data = only(
        info.interfaces()
            .filter(|interface| {
                interface.class() == USB_CLASS_CDC_DATA
                    && interface.subclass() == 0
                    && interface.protocol() == 0
            })
            .map(nusb::InterfaceInfo::interface_number)
            .collect(),
        "CDC data interface",
    )?;
    let xinput = only(
        info.interfaces()
            .filter(|interface| {
                interface.class() == USB_CLASS_VENDOR
                    && interface.subclass() == XINPUT_SUBCLASS
                    && interface.protocol() == XINPUT_PROTOCOL
            })
            .map(nusb::InterfaceInfo::interface_number)
            .collect(),
        "Xbox interface",
    )?;
    Ok(Topology {
        control_interface: control,
        data_interface: data,
        xinput_interface: xinput,
        bulk_in: 0,
        bulk_out: 0,
    })
}

fn discover_topology(configuration: &ConfigurationDescriptor<'_>) -> Result<Topology, Error> {
    let interfaces = configuration
        .interface_alt_settings()
        .filter(|descriptor| descriptor.alternate_setting() == 0)
        .collect::<Vec<_>>();
    let (control_interface, data_interface) = discover_cdc_control(&interfaces)?;
    let (bulk_in, bulk_out) = discover_cdc_data(&interfaces, data_interface)?;
    let xinput_interface = discover_xinput(&interfaces)?;
    Ok(Topology {
        control_interface,
        data_interface,
        xinput_interface,
        bulk_in,
        bulk_out,
    })
}

fn discover_cdc_control(
    interfaces: &[nusb::descriptors::InterfaceDescriptor<'_>],
) -> Result<(u8, u8), Error> {
    let control = only(
        interfaces
            .iter()
            .filter(|descriptor| {
                descriptor.class() == USB_CLASS_COMMUNICATIONS
                    && descriptor.subclass() == CDC_SUBCLASS_ACM
                    && descriptor.protocol() == 0
            })
            .collect(),
        "CDC ACM control interface",
    )?;
    let control_number = control.interface_number();
    let union = only(
        control
            .descriptors()
            .filter(|descriptor| {
                descriptor.descriptor_type() == 0x24
                    && descriptor.len() >= 3
                    && descriptor[2] == 0x06
            })
            .collect(),
        "CDC union descriptor",
    )?;
    if union.len() != 5 || union[3] != control_number {
        return invalid_topology(
            "CDC union descriptor is malformed or does not reference its control interface",
        );
    }
    let data_number = union[4];
    let header = only(
        control
            .descriptors()
            .filter(|descriptor| {
                descriptor.descriptor_type() == 0x24
                    && descriptor.len() >= 3
                    && descriptor[2] == 0x00
            })
            .collect(),
        "CDC header descriptor",
    )?;
    if header.len() != 5 || u16::from_le_bytes([header[3], header[4]]) < 0x0110 {
        return invalid_topology("CDC header is malformed or predates CDC 1.10");
    }
    let call_management = only(
        control
            .descriptors()
            .filter(|descriptor| {
                descriptor.descriptor_type() == 0x24
                    && descriptor.len() >= 3
                    && descriptor[2] == 0x01
            })
            .collect(),
        "CDC call-management descriptor",
    )?;
    if call_management.len() != 5 || call_management[4] != data_number {
        return invalid_topology(
            "CDC call-management descriptor does not reference its data interface",
        );
    }
    let acm = only(
        control
            .descriptors()
            .filter(|descriptor| {
                descriptor.descriptor_type() == 0x24
                    && descriptor.len() >= 3
                    && descriptor[2] == 0x02
            })
            .collect(),
        "CDC ACM descriptor",
    )?;
    if acm.len() != 4 || acm[3] & 0x02 == 0 {
        return invalid_topology(
            "CDC ACM descriptor does not support line coding and control-line state",
        );
    }
    let control_notifications = control
        .endpoints()
        .filter(|endpoint| {
            endpoint.transfer_type() == TransferType::Interrupt
                && endpoint.direction() == Direction::In
        })
        .count();
    if control.num_endpoints() != 1 || control_notifications != 1 {
        return invalid_topology("CDC control interface must have one interrupt IN endpoint");
    }
    Ok((control_number, data_number))
}

fn discover_cdc_data(
    interfaces: &[nusb::descriptors::InterfaceDescriptor<'_>],
    data_number: u8,
) -> Result<(u8, u8), Error> {
    let data = only(
        interfaces
            .iter()
            .filter(|descriptor| {
                descriptor.interface_number() == data_number
                    && descriptor.class() == USB_CLASS_CDC_DATA
                    && descriptor.subclass() == 0
                    && descriptor.protocol() == 0
            })
            .collect(),
        "CDC data interface referenced by the union",
    )?;
    let bulk_endpoints = data
        .endpoints()
        .filter(|endpoint| endpoint.transfer_type() == TransferType::Bulk)
        .collect::<Vec<_>>();
    if data.num_endpoints() != 2 || bulk_endpoints.len() != 2 {
        return invalid_topology("CDC data interface must have exactly two bulk endpoints");
    }
    let bulk_in = only(
        bulk_endpoints
            .iter()
            .filter(|endpoint| endpoint.direction() == Direction::In)
            .map(nusb::descriptors::EndpointDescriptor::address)
            .collect(),
        "CDC bulk IN endpoint",
    )?;
    let bulk_out = only(
        bulk_endpoints
            .iter()
            .filter(|endpoint| endpoint.direction() == Direction::Out)
            .map(nusb::descriptors::EndpointDescriptor::address)
            .collect(),
        "CDC bulk OUT endpoint",
    )?;
    Ok((bulk_in, bulk_out))
}

fn discover_xinput(interfaces: &[nusb::descriptors::InterfaceDescriptor<'_>]) -> Result<u8, Error> {
    let xinput = only(
        interfaces
            .iter()
            .filter(|descriptor| {
                descriptor.class() == USB_CLASS_VENDOR
                    && descriptor.subclass() == XINPUT_SUBCLASS
                    && descriptor.protocol() == XINPUT_PROTOCOL
            })
            .collect(),
        "Xbox interface",
    )?;
    let xinput_endpoints = xinput
        .endpoints()
        .filter(|endpoint| endpoint.transfer_type() == TransferType::Interrupt)
        .collect::<Vec<_>>();
    if xinput.num_endpoints() != 2 || xinput_endpoints.len() != 2 {
        return invalid_topology("Xbox interface must have exactly two interrupt endpoints");
    }
    only(
        xinput_endpoints
            .iter()
            .filter(|endpoint| endpoint.direction() == Direction::In)
            .collect(),
        "Xbox interrupt IN endpoint",
    )?;
    only(
        xinput_endpoints
            .iter()
            .filter(|endpoint| endpoint.direction() == Direction::Out)
            .collect(),
        "Xbox interrupt OUT endpoint",
    )?;
    Ok(xinput.interface_number())
}

fn only<T>(mut values: Vec<T>, name: &str) -> Result<T, Error> {
    if values.len() != 1 {
        return invalid_topology(format!(
            "expected exactly one {name}, found {}",
            values.len()
        ));
    }
    Ok(values.remove(0))
}

fn invalid_topology<T>(message: impl Into<String>) -> Result<T, Error> {
    Err(Error::InvalidTopology(message.into()))
}

fn usb_device_node(info: &NusbDeviceInfo) -> PathBuf {
    Locator {
        bus_number: info.busnum(),
        device_address: info.device_address(),
    }
    .device_node()
}

fn map_io_error(path: &Path, error: io::Error) -> Error {
    let label = path.display().to_string();
    match error.kind() {
        io::ErrorKind::PermissionDenied => Error::AccessDenied(label),
        io::ErrorKind::NotFound | io::ErrorKind::NotConnected => Error::Disconnected(label),
        _ if error.raw_os_error() == Some(16) => Error::Busy(label),
        _ => Error::Io(error),
    }
}

fn map_nusb_error(error: nusb::Error, endpoint: &str) -> Error {
    match error.kind() {
        nusb::ErrorKind::PermissionDenied => Error::AccessDenied(endpoint.to_owned()),
        nusb::ErrorKind::Busy => Error::Busy(endpoint.to_owned()),
        nusb::ErrorKind::Disconnected | nusb::ErrorKind::NotFound => {
            Error::Disconnected(endpoint.to_owned())
        }
        _ => Error::Io(error.into()),
    }
}

fn map_transfer_error(error: TransferError) -> io::Error {
    if error == TransferError::Cancelled {
        io::Error::new(io::ErrorKind::TimedOut, error)
    } else {
        error.into()
    }
}

fn interface_mask(interfaces: impl IntoIterator<Item = u8>) -> Result<u32, Error> {
    interfaces.into_iter().try_fold(0_u32, |mask, interface| {
        1_u32.checked_shl(u32::from(interface)).map_or_else(
            || invalid_topology("usbfs privilege masks support interfaces 0-31"),
            |bit| Ok(mask | bit),
        )
    })
}

#[allow(
    unsafe_code,
    reason = "USBDEVFS_DROP_PRIVILEGES is a pointer ioctl with a kernel-defined u32 ABI"
)]
fn retain_interfaces(fd: &OwnedFd, mask: u32) -> io::Result<()> {
    use linux_raw_sys::ioctl::USBDEVFS_DROP_PRIVILEGES;

    // SAFETY: the request number is the kernel USBDEVFS_DROP_PRIVILEGES ABI and
    // its argument is a valid pointer to the required u32 interface mask.
    let request =
        unsafe { rustix::ioctl::Setter::<{ USBDEVFS_DROP_PRIVILEGES as _ }, u32>::new(mask) };
    // SAFETY: `fd` is an owned usbfs device descriptor and `request` carries the
    // only argument type accepted by USBDEVFS_DROP_PRIVILEGES.
    unsafe { rustix::ioctl::ioctl(fd, request) }.map_err(io::Error::from)
}

fn set_line_coding(control: &Interface, interface: u8) -> io::Result<()> {
    let mut line_coding = Vec::with_capacity(7);
    line_coding.extend_from_slice(&115_200_u32.to_le_bytes());
    line_coding.extend_from_slice(&[0, 0, 8]);
    control
        .control_out(
            ControlOut {
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
                request: 0x20,
                value: 0,
                index: u16::from(interface),
                data: &line_coding,
            },
            CONTROL_TIMEOUT,
        )
        .wait()
        .map_err(io::Error::from)
}

fn set_dtr(control: &Interface, interface: u8, asserted: bool) -> io::Result<()> {
    control
        .control_out(
            ControlOut {
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
                request: 0x22,
                value: u16::from(asserted),
                index: u16::from(interface),
                data: &[],
            },
            CONTROL_TIMEOUT,
        )
        .wait()
        .map_err(io::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_identity_matching_is_exact() {
        assert!(is_official_identity(
            VENDOR_ID,
            PRODUCT_ID,
            Some(MANUFACTURER),
            Some(PRODUCT),
        ));
        assert!(!is_official_identity(
            VENDOR_ID,
            PRODUCT_ID,
            Some("Microsoft"),
            Some(PRODUCT),
        ));
        assert!(!is_official_identity(
            VENDOR_ID,
            PRODUCT_ID,
            Some(MANUFACTURER),
            Some("Controller"),
        ));
    }

    #[test]
    fn topology_is_descriptor_driven() {
        let bytes = configuration_descriptor(4, 7, 9);
        let descriptor = ConfigurationDescriptor::new(&bytes).unwrap();
        assert_eq!(
            discover_topology(&descriptor).unwrap(),
            Topology {
                control_interface: 4,
                data_interface: 7,
                xinput_interface: 9,
                bulk_in: 0x82,
                bulk_out: 0x01,
            }
        );
    }

    #[test]
    fn topology_rejects_invalid_union_and_capabilities() {
        for index in [22, 31, 39] {
            let mut bytes = configuration_descriptor(4, 7, 9);
            bytes[index] = 0;
            let descriptor = ConfigurationDescriptor::new(&bytes).unwrap();
            assert!(discover_topology(&descriptor).is_err());
        }
    }

    #[test]
    fn privilege_masks_retain_only_the_cdc_interfaces() {
        assert_eq!(interface_mask([0, 1, 1]).unwrap(), 0b11);
        assert!(interface_mask([32]).is_err());
    }

    #[test]
    fn cancelled_transfers_are_reported_as_timeouts() {
        assert_eq!(
            map_transfer_error(TransferError::Cancelled).kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            map_transfer_error(TransferError::Disconnected).kind(),
            io::ErrorKind::ConnectionAborted
        );
    }

    #[test]
    fn sysfs_interface_path_uses_the_active_configuration() {
        let path = Path::new("/sys/devices/platform/vhci_hcd.0/usb3/3-1");
        assert_eq!(
            usb_interface_sysfs_path(path, "1", 2).unwrap(),
            path.join("3-1:1.2")
        );
    }

    #[test]
    fn supported_gamepad_driver_names_match_xbox_360_implementations() {
        for driver in ["xpad", "xpad-noone"] {
            assert!(is_supported_gamepad_driver(Some(driver)));
        }
        assert!(!is_supported_gamepad_driver(Some("xone")));
        assert!(!is_supported_gamepad_driver(Some("hid_xpadneo")));
        assert!(!is_supported_gamepad_driver(None));
    }

    fn configuration_descriptor(control: u8, data: u8, xinput: u8) -> Vec<u8> {
        vec![
            9, 2, 90, 0, 3, 1, 0, 0x80, 50, 9, 4, control, 0, 1, 2, 2, 0, 4, 5, 0x24, 0, 0x10,
            0x01, 5, 0x24, 1, 0, data, 4, 0x24, 2, 2, 5, 0x24, 6, control, data, 7, 5, 0x81, 3, 8,
            0, 16, 9, 4, data, 0, 2, 0x0a, 0, 0, 0, 7, 5, 0x01, 2, 64, 0, 0, 7, 5, 0x82, 2, 64, 0,
            0, 9, 4, xinput, 0, 2, 0xff, 0x5d, 1, 5, 7, 5, 0x02, 3, 32, 0, 8, 7, 5, 0x83, 3, 32, 0,
            4,
        ]
    }
}
