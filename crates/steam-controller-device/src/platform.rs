use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use hidapi::{BusType, DeviceInfo, HidApi, HidDevice};

use crate::{
    ControllerSession, ControllerSessionStep, DeviceError, DeviceEvent, HidDeviceInfo,
    PROTEUS_VENDOR_ID,
};

const REPORT_BUFFER_SIZE: usize = 1024;

/// Enumerates all HID collections using a stable path-based ordering.
///
/// # Errors
///
/// Returns [`DeviceError`] when the native HID context cannot be initialized.
pub fn enumerate() -> Result<Vec<HidDeviceInfo>, DeviceError> {
    let api = HidApi::new().map_err(|error| backend_error(&error))?;
    Ok(convert_and_sort(api.device_list()))
}

/// Reusable HID context for discovery.
///
/// Constructing a [`HidApi`] initializes HIDAPI and enumerates every collection
/// in the system, so doing it per scan or per open attempt is what made idle
/// discovery expensive. This type builds one context and reuses it for filtered
/// scans, full-inventory scans, and opening sessions.
pub struct ControllerEnumerator {
    api: HidApi,
    initial_devices: Option<Vec<HidDeviceInfo>>,
}

impl ControllerEnumerator {
    /// Creates a reusable enumerator.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] when the native HID context cannot be initialized.
    pub fn new() -> Result<Self, DeviceError> {
        let api = HidApi::new().map_err(|error| backend_error(&error))?;
        // Constructing the context already enumerated everything, so keep that
        // snapshot rather than immediately scanning again.
        let initial_devices = Some(convert_and_sort(api.device_list()));
        Ok(Self {
            api,
            initial_devices,
        })
    }

    /// Refreshes only the supported Puck and Bluetooth controller identities.
    ///
    /// Asks macOS for Valve's VID instead of rebuilding metadata for every HID
    /// collection. The result still passes through the exact Puck/Bluetooth
    /// classifier before it leaves this type.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] when native HID enumeration fails.
    pub fn enumerate(&mut self) -> Result<Vec<HidDeviceInfo>, DeviceError> {
        if let Some(initial_devices) = self.initial_devices.take() {
            return Ok(supported_controllers(initial_devices));
        }
        self.api
            .reset_devices()
            .map_err(|error| backend_error(&error))?;
        self.api
            .add_devices(PROTEUS_VENDOR_ID, 0)
            .map_err(|error| backend_error(&error))?;
        Ok(supported_controllers(convert_and_sort(
            self.api.device_list(),
        )))
    }

    /// Refreshes the whole system inventory, whose ordering defines the global
    /// indices `sc-probe list` prints and `--index N` selects.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] when native HID enumeration fails.
    pub fn enumerate_all(&mut self) -> Result<Vec<HidDeviceInfo>, DeviceError> {
        if let Some(initial_devices) = self.initial_devices.take() {
            return Ok(initial_devices);
        }
        self.api
            .refresh_devices()
            .map_err(|error| backend_error(&error))?;
        Ok(convert_and_sort(self.api.device_list()))
    }

    /// Opens a previously enumerated collection using this context.
    ///
    /// Retrying an open is the hot path whenever another process owns a
    /// collection, so it must not build a HID context of its own.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] when the collection is no longer listed, is
    /// already owned, or macOS refuses access.
    pub fn open(&self, info: &HidDeviceInfo) -> Result<HidSession, DeviceError> {
        let native = self
            .api
            .device_list()
            .find(|candidate| matches_selected(candidate, info))
            .cloned()
            .ok_or(DeviceError::NotConnected)?;
        HidSession::open_borrowed(&self.api, &native)
    }
}

fn supported_controllers(devices: Vec<HidDeviceInfo>) -> Vec<HidDeviceInfo> {
    devices
        .into_iter()
        .filter(HidDeviceInfo::is_supported_controller_source)
        .collect()
}

pub struct HidSession {
    /// Only reconnection needs a context of its own. A session opened through
    /// [`ControllerEnumerator`] starts without one and builds it lazily, so a
    /// session that never loses its device never pays for a second enumeration.
    api: Option<HidApi>,
    lifecycle: ControllerSession,
    _ownership_lock: File,
    device: Option<HidDevice>,
    started: Instant,
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
        Self::open_owned(api, &native)
    }

    /// Opens a previously enumerated collection by stable identity.
    ///
    /// Prefer [`ControllerEnumerator::open`] when a context already exists;
    /// this constructs one.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] when the collection disappeared, is already
    /// owned, or macOS refuses access.
    pub fn open_info(info: &HidDeviceInfo) -> Result<Self, DeviceError> {
        let api = HidApi::new().map_err(|error| backend_error(&error))?;
        let native = api
            .device_list()
            .find(|candidate| matches_selected(candidate, info))
            .cloned()
            .ok_or(DeviceError::NotConnected)?;
        Self::open_owned(api, &native)
    }

    fn open_owned(api: HidApi, native: &DeviceInfo) -> Result<Self, DeviceError> {
        let (selected, ownership_lock, device) = Self::claim(&api, native)?;
        Ok(Self::new_session(
            Some(api),
            selected,
            ownership_lock,
            device,
        ))
    }

    fn open_borrowed(api: &HidApi, native: &DeviceInfo) -> Result<Self, DeviceError> {
        let (selected, ownership_lock, device) = Self::claim(api, native)?;
        Ok(Self::new_session(None, selected, ownership_lock, device))
    }

    /// Takes the project ownership lock, then opens the native device.
    ///
    /// The lock is attempted first because it is the cheap check and the one
    /// that fails when another project process already owns the collection.
    fn claim(
        api: &HidApi,
        native: &DeviceInfo,
    ) -> Result<(HidDeviceInfo, File, HidDevice), DeviceError> {
        let selected = convert_info(native);
        let ownership_lock = acquire_ownership_lock(&selected)?;
        let device = native.open_device(api).map_err(|error| {
            DeviceError::Backend(format!(
                "cannot open the selected collection; verify Input Monitoring \
                 permission, fully quit Steam, boot out its ipcserver LaunchAgent, \
                 and then retry: {error}"
            ))
        })?;
        Ok((selected, ownership_lock, device))
    }

    fn new_session(
        api: Option<HidApi>,
        selected: HidDeviceInfo,
        ownership_lock: File,
        device: HidDevice,
    ) -> Self {
        let now = Instant::now();
        Self {
            api,
            lifecycle: ControllerSession::new(selected),
            _ownership_lock: ownership_lock,
            device: Some(device),
            started: now,
        }
    }

    #[must_use]
    pub fn device_info(&self) -> &HidDeviceInfo {
        self.lifecycle.device_info()
    }

    /// Sends the single SDL-compatible Steam Controller 2 lizard-off setting.
    ///
    /// The operation is rejected unless the selected collection is one of the
    /// exact supported Puck or direct Bluetooth collections. No arbitrary
    /// feature-report API is exposed.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] for an unsupported collection, disconnected
    /// session, or native HID write failure.
    pub fn suppress_lizard_mode(&self) -> Result<(), DeviceError> {
        let selected = self.device_info();
        if !selected.supports_lizard_mode_suppression() {
            return Err(DeviceError::UnsupportedLizardSuppressionTarget {
                vendor_id: selected.vendor_id,
                product_id: selected.product_id,
                usage_page: selected.usage_page,
                usage: selected.usage,
                interface_number: selected.interface_number,
            });
        }
        let device = self.device.as_ref().ok_or(DeviceError::NotConnected)?;
        device
            .send_feature_report(&steam_controller_protocol::lizard_mode_off_feature_report())
            .map_err(|error| {
                DeviceError::Backend(format!(
                    "lizard-mode feature write failed; ensure Steam Controller 2 \
                     is awake on the selected transport and its vendor collection is producing \
                     0x42/0x45 reports: {error}"
                ))
            })
    }

    /// Sends the SDL-compatible standard dual-rumble output report.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] for an unsupported collection, disconnected
    /// session, or native HID output failure.
    pub fn set_rumble(&self, low_frequency: u16, high_frequency: u16) -> Result<(), DeviceError> {
        let selected = self.device_info();
        if !selected.supports_rumble() {
            return Err(DeviceError::UnsupportedRumbleTarget {
                vendor_id: selected.vendor_id,
                product_id: selected.product_id,
                usage_page: selected.usage_page,
                usage: selected.usage,
                interface_number: selected.interface_number,
            });
        }
        let device = self.device.as_ref().ok_or(DeviceError::NotConnected)?;
        device
            .write(&steam_controller_protocol::rumble_output_report(
                low_frequency,
                high_frequency,
            ))
            .map(|_| ())
            .map_err(|error| {
                DeviceError::Backend(format!(
                    "rumble output write failed; ensure Steam Controller 2 is \
                     awake on the selected transport and its vendor collection is active: {error}"
                ))
            })
    }

    /// Sends one finite SDL Triton trackpad tick.
    ///
    /// The operation is rejected unless the selected collection is an exact
    /// supported Puck or direct Bluetooth collection. No arbitrary output API
    /// is exposed.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] for an unsupported collection, disconnected
    /// session, or native HID output failure.
    pub fn pad_haptic_tick(
        &self,
        side: steam_controller_protocol::PadHapticSide,
        gain: steam_controller_protocol::PadHapticGain,
    ) -> Result<(), DeviceError> {
        let selected = self.device_info();
        if !selected.supports_pad_haptics() {
            return Err(DeviceError::UnsupportedPadHapticsTarget {
                vendor_id: selected.vendor_id,
                product_id: selected.product_id,
                usage_page: selected.usage_page,
                usage: selected.usage,
                interface_number: selected.interface_number,
            });
        }
        let device = self.device.as_ref().ok_or(DeviceError::NotConnected)?;
        device
            .write(&steam_controller_protocol::pad_haptic_tick_output_report(
                side, gain,
            ))
            .map(|_| ())
            .map_err(|error| {
                DeviceError::Backend(format!(
                    "pad-haptic output write failed; ensure Steam Controller 2 is \
                     awake on the selected transport and its vendor collection is active: {error}"
                ))
            })
    }

    /// Sends the fixed Steam Controller 2 power-off feature command.
    ///
    /// The operation is rejected unless the selected collection is an exact
    /// supported Puck or direct Bluetooth collection. No arbitrary feature
    /// write is exposed.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] for an unsupported collection, disconnected
    /// session, or native HID write failure.
    pub fn power_off(&self) -> Result<(), DeviceError> {
        let selected = self.device_info();
        if !selected.supports_power_off() {
            return Err(DeviceError::UnsupportedPowerOffTarget {
                vendor_id: selected.vendor_id,
                product_id: selected.product_id,
                usage_page: selected.usage_page,
                usage: selected.usage,
                interface_number: selected.interface_number,
            });
        }
        let device = self.device.as_ref().ok_or(DeviceError::NotConnected)?;
        device
            .send_feature_report(&steam_controller_protocol::power_off_feature_report())
            .map_err(|error| {
                DeviceError::Backend(format!(
                    "power-off feature write failed; ensure Steam Controller 2 is \
                     awake on the selected transport and its vendor collection is active: {error}"
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
        match self.lifecycle.next_step(self.started.elapsed(), timeout) {
            ControllerSessionStep::Event(event) => Ok(Some(event)),
            ControllerSessionStep::Read { timeout } => Ok(self.read_report(timeout)),
            ControllerSessionStep::Wait { duration } => {
                thread::sleep(duration);
                Ok(None)
            }
            ControllerSessionStep::Retry => self.retry_connect(),
            ControllerSessionStep::Close => {
                self.close_native_handles();
                self.lifecycle.closed();
                Ok(None)
            }
            ControllerSessionStep::Stopped => Ok(None),
        }
    }

    /// Closes the native HID device and reconnect context while the ownership
    /// lock is still held. Calling this more than once is harmless.
    pub fn shutdown(&mut self) {
        self.lifecycle.request_shutdown();
        if self
            .lifecycle
            .next_step(self.started.elapsed(), Duration::ZERO)
            == ControllerSessionStep::Close
        {
            self.close_native_handles();
            self.lifecycle.closed();
        }
    }

    fn read_report(&mut self, timeout: Duration) -> Option<DeviceEvent> {
        let mut buffer = [0_u8; REPORT_BUFFER_SIZE];
        let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let device = self.device.as_ref()?;
        match device.read_timeout(&mut buffer, timeout_ms) {
            Ok(0) => None,
            Ok(length) => Some(self.lifecycle.report(
                self.started.elapsed(),
                buffer[..length].to_vec(),
                0,
            )),
            Err(_) => {
                self.device = None;
                Some(self.lifecycle.disconnected(self.started.elapsed()))
            }
        }
    }

    fn retry_connect(&mut self) -> Result<Option<DeviceEvent>, DeviceError> {
        // Reconnection is the only thing this session needs a context for, so
        // a session opened through ControllerEnumerator builds one here rather
        // than at open time.
        if self.api.is_none() {
            self.api = Some(HidApi::new().map_err(|error| backend_error(&error))?);
        }
        let selected = self.lifecycle.device_info().clone();
        let api = self
            .api
            .as_mut()
            .ok_or_else(|| DeviceError::Backend("HID reconnect context missing".to_owned()))?;
        api.refresh_devices()
            .map_err(|error| backend_error(&error))?;
        let candidate = api
            .device_list()
            .find(|candidate| matches_selected(candidate, &selected))
            .cloned();
        let Some(candidate) = candidate else {
            self.lifecycle.retry_failed(self.started.elapsed());
            return Ok(None);
        };
        let Some(device) = candidate.open_device(api).ok() else {
            self.lifecycle.retry_failed(self.started.elapsed());
            return Ok(None);
        };
        self.device = Some(device);
        Ok(Some(self.lifecycle.reconnected(convert_info(&candidate))))
    }

    fn close_native_handles(&mut self) {
        self.device = None;
        self.api = None;
    }
}

impl Drop for HidSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn acquire_ownership_lock(selected: &HidDeviceInfo) -> Result<File, DeviceError> {
    let paths = app_paths::current().map_err(|error| {
        DeviceError::Backend(format!(
            "cannot locate the Steam Controller input ownership-lock directory: {error}"
        ))
    })?;
    acquire_ownership_lock_in(&paths.runtime_dir, selected)
}

fn acquire_ownership_lock_in(
    runtime_dir: &Path,
    selected: &HidDeviceInfo,
) -> Result<File, DeviceError> {
    let mut directory = fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        directory.mode(0o700);
    }
    directory.create(runtime_dir).map_err(|error| {
        DeviceError::Backend(format!(
            "cannot create Steam Controller input ownership-lock directory {}: {error}",
            runtime_dir.display()
        ))
    })?;

    let lock_path = ownership_lock_path(runtime_dir, selected);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            DeviceError::Backend(format!(
                "cannot create Steam Controller input ownership lock {}: {error}",
                lock_path.display()
            ))
        })?;
    fs4::FileExt::try_lock(&file).map_err(|_| DeviceError::OwnershipConflict {
        interface_number: selected.interface_number,
    })?;
    Ok(file)
}

fn ownership_lock_path(runtime_dir: &Path, selected: &HidDeviceInfo) -> PathBuf {
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
    runtime_dir.join(format!(
        "steam-controller-bridge-{:04x}-{:04x}-{identity_hash:016x}-if{}.lock",
        selected.vendor_id, selected.product_id, selected.interface_number
    ))
}

fn matches_selected(candidate: &DeviceInfo, selected: &HidDeviceInfo) -> bool {
    let info = convert_info(candidate);
    same_collection(&info, selected)
}

fn same_collection(info: &HidDeviceInfo, selected: &HidDeviceInfo) -> bool {
    (info.path == selected.path || info.same_physical_device(selected))
        && info.usage_page == selected.usage_page
        && info.usage == selected.usage
        && info.interface_number == selected.interface_number
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

fn convert_and_sort<'a>(devices: impl Iterator<Item = &'a DeviceInfo>) -> Vec<HidDeviceInfo> {
    let mut devices: Vec<_> = devices.map(convert_info).collect();
    devices.sort_by(|left, right| left.path.cmp(&right.path));
    devices
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
    fn ownership_lock_uses_the_runtime_directory_and_lives_with_its_file_handle() {
        let identity = format!(
            "lock-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_nanos()
        );
        let target = lock_target(&identity);
        let runtime_dir = std::env::temp_dir().join(&identity).join("runtime");
        let lock_path = ownership_lock_path(&runtime_dir, &target);
        assert_eq!(lock_path.parent(), Some(runtime_dir.as_path()));
        assert!(!runtime_dir.exists());

        let first = acquire_ownership_lock_in(&runtime_dir, &target).expect("first lock");
        assert!(lock_path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&runtime_dir)
                    .expect("runtime directory metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert!(matches!(
            acquire_ownership_lock_in(&runtime_dir, &target),
            Err(DeviceError::OwnershipConflict {
                interface_number: 2
            })
        ));
        drop(first);
        let second = acquire_ownership_lock_in(&runtime_dir, &target).expect("lock after release");
        drop(second);
        std::fs::remove_file(lock_path).expect("remove test lock");
        std::fs::remove_dir_all(runtime_dir.parent().expect("test directory"))
            .expect("remove test directory");
    }

    #[test]
    fn macos_app_path_policy_preserves_the_existing_lock_directory() {
        let target = lock_target("stable-macos-lock-path");
        let paths = app_paths::current().expect("macOS app paths");
        assert_eq!(paths.runtime_dir, std::env::temp_dir());
        assert_eq!(
            ownership_lock_path(&paths.runtime_dir, &target).parent(),
            Some(std::env::temp_dir().as_path())
        );
    }

    #[test]
    fn shared_hid_paths_do_not_collapse_sibling_collections() {
        let selected = lock_target("shared-path");
        let mut sibling = selected.clone();
        sibling.usage_page = 0x0001;
        sibling.usage = 0x0002;
        assert!(!same_collection(&sibling, &selected));

        let mut same = selected.clone();
        same.id = "different-enumeration-id".to_owned();
        assert!(same_collection(&same, &selected));
    }
}
