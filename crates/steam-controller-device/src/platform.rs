use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use hidapi::{BusType, DeviceInfo, HidApi, HidDevice};
use rustix::fs::{flock, FlockOperation};

use crate::{DeviceError, DeviceEvent, HidDeviceInfo, RawHidReport};

const REPORT_BUFFER_SIZE: usize = 1024;
const RECONNECT_INTERVAL: Duration = Duration::from_millis(500);

/// Enumerates all HID collections using a stable path-based ordering.
///
/// # Errors
///
/// Returns [`DeviceError`] when the native HID context cannot be initialized.
pub fn enumerate() -> Result<Vec<HidDeviceInfo>, DeviceError> {
    let api = HidApi::new().map_err(|error| backend_error(&error))?;
    let mut devices: Vec<_> = api.device_list().map(convert_info).collect();
    devices.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(devices)
}

pub struct HidSession {
    api: HidApi,
    selected: HidDeviceInfo,
    _ownership_lock: File,
    device: Option<HidDevice>,
    started: Instant,
    next_reconnect: Instant,
    pending_connected: bool,
}

impl HidSession {
    /// Opens the enumerated HID collection at `index`.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] if enumeration fails, the index is invalid, or
    /// another project process owns the selected slot, or macOS refuses to
    /// open the selected HID collection.
    pub fn open_index(index: usize) -> Result<Self, DeviceError> {
        let api = HidApi::new().map_err(|error| backend_error(&error))?;
        let mut devices: Vec<_> = api.device_list().cloned().collect();
        devices.sort_by(|left, right| left.path().to_bytes().cmp(right.path().to_bytes()));
        let native = devices
            .get(index)
            .cloned()
            .ok_or(DeviceError::InvalidIndex(index))?;
        let selected = convert_info(&native);
        let ownership_lock = acquire_ownership_lock(&selected)?;
        let device = native.open_device(&api).map_err(|error| {
            DeviceError::Backend(format!(
                "cannot open the selected collection; verify Input Monitoring \
                 permission, fully quit Steam, boot out its ipcserver LaunchAgent, \
                 and then retry: {error}"
            ))
        })?;
        let now = Instant::now();
        Ok(Self {
            api,
            selected,
            _ownership_lock: ownership_lock,
            device: Some(device),
            started: now,
            next_reconnect: now,
            pending_connected: true,
        })
    }

    #[must_use]
    pub fn device_info(&self) -> &HidDeviceInfo {
        &self.selected
    }

    /// Sends the single SDL-compatible Steam Controller 2 lizard-off setting.
    ///
    /// The operation is rejected unless the selected collection is an official
    /// Proteus Puck controller slot. No arbitrary feature-report API is exposed.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] for an unsupported collection, disconnected
    /// session, or native HID write failure.
    pub fn suppress_lizard_mode(&self) -> Result<(), DeviceError> {
        if !self.selected.supports_lizard_mode_suppression() {
            return Err(DeviceError::UnsupportedLizardSuppressionTarget {
                vendor_id: self.selected.vendor_id,
                product_id: self.selected.product_id,
                usage_page: self.selected.usage_page,
                usage: self.selected.usage,
                interface_number: self.selected.interface_number,
            });
        }
        let device = self.device.as_ref().ok_or(DeviceError::NotConnected)?;
        device
            .send_feature_report(&steam_controller_protocol::lizard_mode_off_feature_report())
            .map_err(|error| {
                DeviceError::Backend(format!(
                    "lizard-mode feature write failed; ensure Steam Controller 2 \
                     is awake in Puck mode and the selected slot is producing \
                     0x42/0x45 reports: {error}"
                ))
            })
    }

    /// Waits for the next lifecycle event or input report.
    ///
    /// A read failure emits `Disconnected`; subsequent calls periodically
    /// re-enumerate and reopen the same collection identity.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] if refreshing or opening the HID backend fails.
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<DeviceEvent>, DeviceError> {
        if self.pending_connected {
            self.pending_connected = false;
            return Ok(Some(DeviceEvent::Connected(self.selected.clone())));
        }

        if self.device.is_none() {
            if Instant::now() < self.next_reconnect {
                thread::sleep(
                    timeout.min(
                        self.next_reconnect
                            .saturating_duration_since(Instant::now()),
                    ),
                );
                return Ok(None);
            }
            self.api
                .refresh_devices()
                .map_err(|error| backend_error(&error))?;
            let candidate = self
                .api
                .device_list()
                .find(|candidate| matches_selected(candidate, &self.selected))
                .cloned();
            if let Some(candidate) = candidate {
                if let Ok(device) = candidate.open_device(&self.api) {
                    self.selected = convert_info(&candidate);
                    self.device = Some(device);
                    return Ok(Some(DeviceEvent::Connected(self.selected.clone())));
                }
                self.next_reconnect = Instant::now() + RECONNECT_INTERVAL;
                return Ok(None);
            }
            self.next_reconnect = Instant::now() + RECONNECT_INTERVAL;
            return Ok(None);
        }

        let mut buffer = [0_u8; REPORT_BUFFER_SIZE];
        let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let Some(device) = self.device.as_ref() else {
            return Ok(None);
        };
        let read_result = device.read_timeout(&mut buffer, timeout_ms);
        match read_result {
            Ok(0) => Ok(None),
            Ok(length) => {
                let data = buffer[..length].to_vec();
                Ok(Some(DeviceEvent::Report(RawHidReport {
                    timestamp: self.started.elapsed(),
                    report_id: data.first().copied().unwrap_or(0),
                    data,
                    source_device_id: self.selected.id.clone(),
                    transport: self.selected.transport.clone(),
                    dropped_reports: 0,
                })))
            }
            Err(_) => {
                self.device = None;
                self.next_reconnect = Instant::now() + RECONNECT_INTERVAL;
                Ok(Some(DeviceEvent::Disconnected))
            }
        }
    }
}

fn acquire_ownership_lock(selected: &HidDeviceInfo) -> Result<File, DeviceError> {
    let lock_path = ownership_lock_path(selected);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            DeviceError::Backend(format!(
                "cannot create Puck ownership lock {}: {error}",
                lock_path.display()
            ))
        })?;
    flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|_| {
        DeviceError::OwnershipConflict {
            interface_number: selected.interface_number,
        }
    })?;
    Ok(file)
}

fn ownership_lock_path(selected: &HidDeviceInfo) -> PathBuf {
    let identity = selected
        .serial_number
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(&selected.path);
    let identity_hash = identity
        .as_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    std::env::temp_dir().join(format!(
        "steam-controller-bridge-{:04x}-{:04x}-{identity_hash:016x}-if{}.lock",
        selected.vendor_id, selected.product_id, selected.interface_number
    ))
}

fn matches_selected(candidate: &DeviceInfo, selected: &HidDeviceInfo) -> bool {
    let info = convert_info(candidate);
    info.path == selected.path
        || (info.same_physical_device(selected)
            && info.usage_page == selected.usage_page
            && info.usage == selected.usage
            && info.interface_number == selected.interface_number)
}

fn convert_info(info: &DeviceInfo) -> HidDeviceInfo {
    let path = info.path().to_string_lossy().into_owned();
    HidDeviceInfo {
        id: path.clone(),
        path,
        vendor_id: info.vendor_id(),
        product_id: info.product_id(),
        usage_page: info.usage_page(),
        usage: info.usage(),
        interface_number: info.interface_number(),
        serial_number: info.serial_number().map(str::to_owned),
        manufacturer: info.manufacturer_string().map(str::to_owned),
        product: info.product_string().map(str::to_owned),
        transport: match info.bus_type() {
            BusType::Usb => "USB",
            BusType::Bluetooth => "Bluetooth",
            BusType::I2c => "I2C",
            BusType::Spi => "SPI",
            BusType::Unknown => "Unknown",
        }
        .to_owned(),
    }
}

fn backend_error(error: &hidapi::HidError) -> DeviceError {
    DeviceError::Backend(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_target(identity: &str) -> HidDeviceInfo {
        HidDeviceInfo {
            id: identity.to_owned(),
            path: identity.to_owned(),
            vendor_id: 0x28de,
            product_id: 0x1304,
            usage_page: 0xff00,
            usage: 0x0001,
            interface_number: 2,
            serial_number: Some(identity.to_owned()),
            manufacturer: Some("Valve Software".to_owned()),
            product: Some("Steam Controller Puck".to_owned()),
            transport: "USB".to_owned(),
        }
    }

    #[test]
    fn ownership_lock_rejects_a_second_project_process() {
        let identity = format!(
            "lock-test-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        );
        let target = lock_target(&identity);
        let first = acquire_ownership_lock(&target).expect("first lock");
        assert!(matches!(
            acquire_ownership_lock(&target),
            Err(DeviceError::OwnershipConflict {
                interface_number: 2
            })
        ));
        drop(first);
        let second = acquire_ownership_lock(&target).expect("lock after release");
        drop(second);
        std::fs::remove_file(ownership_lock_path(&target)).expect("remove test lock");
    }
}
