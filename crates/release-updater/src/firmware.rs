use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bridge_output::{
    available_serial_devices, FirmwareVersion, GamepadOutput as _, SerialConfig, SerialDeviceInfo,
    SerialOutput,
};

#[cfg(test)]
use crate::UF2_FAMILY_ID;
use crate::{verify_artifact, FirmwareRelease, FIRMWARE_BOARD_ID};

const SEEED_VENDOR_ID: u16 = 0x2886;
const XIAO_APPLICATION_PRODUCT_ID: u16 = 0x8044;
const XIAO_BOOTLOADER_PRODUCT_ID: u16 = 0x0044;
const UF2_BLOCK_SIZE: usize = 512;
const UF2_MAGIC_START_0: u32 = 0x0A32_4655;
const UF2_MAGIC_START_1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
const UF2_FLAG_FAMILY_ID: u32 = 0x0000_2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareDevice {
    pub path: String,
    pub bridge_firmware: bool,
    pub serial_number: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootloaderVolume {
    pub root: PathBuf,
    pub board_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirmwareFlashProgress {
    LookingForDevice,
    EnteringBootloader,
    WaitingForBootloader,
    Writing,
    WaitingForApplication,
    Verifying,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareReleaseState {
    Pending,
    UpdateAvailable,
    Current,
    Newer,
}

#[must_use]
pub const fn classify_firmware_release(
    version: FirmwareVersion,
    target: u16,
) -> FirmwareReleaseState {
    match version {
        FirmwareVersion::Pending => FirmwareReleaseState::Pending,
        FirmwareVersion::Reported(revision) if revision < target => {
            FirmwareReleaseState::UpdateAvailable
        }
        FirmwareVersion::Reported(revision) if revision == target => FirmwareReleaseState::Current,
        FirmwareVersion::Reported(_) | FirmwareVersion::UnsupportedFormat(_) => {
            FirmwareReleaseState::Newer
        }
        FirmwareVersion::Unreported | FirmwareVersion::Malformed => {
            FirmwareReleaseState::UpdateAvailable
        }
    }
}

#[derive(Debug)]
pub enum FirmwareFlashError {
    Io(io::Error),
    Discovery(String),
    WrongBoard(String),
    InvalidUf2(String),
    Cancelled,
    Timeout(&'static str),
    Revision { expected: u16, actual: Option<u16> },
    Artifact(String),
    NewerFirmware,
    VersionUnavailable,
}

impl std::fmt::Display for FirmwareFlashError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "firmware I/O failed: {error}"),
            Self::Discovery(error) => write!(formatter, "cannot select firmware device: {error}"),
            Self::WrongBoard(board) => write!(formatter, "unsupported bootloader board: {board}"),
            Self::InvalidUf2(error) => write!(formatter, "invalid UF2 firmware: {error}"),
            Self::Cancelled => write!(formatter, "firmware update cancelled"),
            Self::Timeout(stage) => write!(formatter, "timed out while {stage}"),
            Self::Revision { expected, actual } => write!(
                formatter,
                "firmware verification reported {actual:?}; expected revision {expected}"
            ),
            Self::Artifact(error) => {
                write!(formatter, "firmware artifact failed verification: {error}")
            }
            Self::NewerFirmware => write!(formatter, "refusing to downgrade newer firmware"),
            Self::VersionUnavailable => write!(
                formatter,
                "current firmware revision could not be verified; reconnect and retry"
            ),
        }
    }
}

impl std::error::Error for FirmwareFlashError {}

impl From<io::Error> for FirmwareFlashError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn discover_firmware_devices() -> Result<Vec<FirmwareDevice>, FirmwareFlashError> {
    let devices = available_serial_devices()
        .map_err(|error| FirmwareFlashError::Discovery(error.to_string()))?
        .into_iter()
        .filter_map(|device| {
            if device.is_xiao_bridge() {
                Some(FirmwareDevice {
                    path: device.path,
                    bridge_firmware: true,
                    serial_number: device.serial_number,
                })
            } else if is_factory_xiao(&device) {
                Some(FirmwareDevice {
                    path: device.path,
                    bridge_firmware: false,
                    serial_number: device.serial_number,
                })
            } else {
                None
            }
        })
        .collect();
    Ok(devices)
}

fn is_factory_xiao(device: &SerialDeviceInfo) -> bool {
    let callout = !cfg!(target_os = "macos") || device.path.starts_with("/dev/cu.");
    callout
        && device.vendor_id == Some(SEEED_VENDOR_ID)
        && device.product_id.is_some_and(|product_id| {
            [XIAO_APPLICATION_PRODUCT_ID, XIAO_BOOTLOADER_PRODUCT_ID].contains(&product_id)
        })
}

pub fn discover_bootloader_volumes(
    root: &Path,
) -> Result<Vec<BootloaderVolume>, FirmwareFlashError> {
    let mut volumes = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(volumes),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let root = entry?.path();
        let info_path = root.join("INFO_UF2.TXT");
        let Ok(info) = fs::read_to_string(info_path) else {
            continue;
        };
        let Some(board_id) = info
            .lines()
            .find_map(|line| line.strip_prefix("Board-ID:").map(str::trim))
        else {
            continue;
        };
        volumes.push(BootloaderVolume {
            root,
            board_id: board_id.to_owned(),
        });
    }
    volumes.sort_by(|left, right| left.root.cmp(&right.root));
    Ok(volumes)
}

pub fn validate_uf2(path: &Path, expected_family: u32) -> Result<(), FirmwareFlashError> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    if length == 0 || length % UF2_BLOCK_SIZE as u64 != 0 {
        return Err(FirmwareFlashError::InvalidUf2(
            "file is not a non-empty sequence of 512-byte blocks".to_owned(),
        ));
    }
    let mut block = [0_u8; UF2_BLOCK_SIZE];
    for _ in 0..length / UF2_BLOCK_SIZE as u64 {
        file.read_exact(&mut block)?;
        let start0 = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        let start1 = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        let flags = u32::from_le_bytes([block[8], block[9], block[10], block[11]]);
        let family = u32::from_le_bytes([block[28], block[29], block[30], block[31]]);
        let end = u32::from_le_bytes([block[508], block[509], block[510], block[511]]);
        if start0 != UF2_MAGIC_START_0 || start1 != UF2_MAGIC_START_1 || end != UF2_MAGIC_END {
            return Err(FirmwareFlashError::InvalidUf2(
                "block magic does not match UF2".to_owned(),
            ));
        }
        if flags & UF2_FLAG_FAMILY_ID == 0 || family != expected_family {
            return Err(FirmwareFlashError::InvalidUf2(format!(
                "family is {family:#010x}, expected {expected_family:#010x}"
            )));
        }
    }
    Ok(())
}

pub fn flash_firmware(
    artifact_path: &Path,
    release: &FirmwareRelease,
    volumes_root: &Path,
    cancelled: &AtomicBool,
    progress: impl FnMut(FirmwareFlashProgress),
) -> Result<(), FirmwareFlashError> {
    verify_artifact(artifact_path, &release.artifact)
        .map_err(|error| FirmwareFlashError::Artifact(error.to_string()))?;
    validate_uf2(artifact_path, release.uf2_family_id)?;
    let mut adapter = NativeFlashAdapter {
        started: Instant::now(),
    };
    flash_with_adapter(
        &mut adapter,
        artifact_path,
        release,
        volumes_root,
        cancelled,
        progress,
    )
}

trait FlashAdapter {
    fn devices(&mut self) -> Result<Vec<FirmwareDevice>, FirmwareFlashError>;
    fn volumes(&mut self, root: &Path) -> Result<Vec<BootloaderVolume>, FirmwareFlashError>;
    fn firmware_version(&mut self, path: &str) -> Result<FirmwareVersion, FirmwareFlashError>;
    fn touch_1200(&mut self, path: &str) -> Result<(), FirmwareFlashError>;
    fn copy_and_flush(&mut self, source: &Path, destination: &Path) -> Result<(), io::Error>;
    fn elapsed(&self) -> Duration;
    fn wait(&mut self, duration: Duration);
}

struct NativeFlashAdapter {
    started: Instant,
}

impl FlashAdapter for NativeFlashAdapter {
    fn devices(&mut self) -> Result<Vec<FirmwareDevice>, FirmwareFlashError> {
        discover_firmware_devices()
    }

    fn volumes(&mut self, root: &Path) -> Result<Vec<BootloaderVolume>, FirmwareFlashError> {
        discover_bootloader_volumes(root)
    }

    fn firmware_version(&mut self, path: &str) -> Result<FirmwareVersion, FirmwareFlashError> {
        read_firmware_version(path)
    }

    fn touch_1200(&mut self, path: &str) -> Result<(), FirmwareFlashError> {
        touch_1200(path)
    }

    fn copy_and_flush(&mut self, source: &Path, destination: &Path) -> Result<(), io::Error> {
        copy_and_flush(source, destination)
    }

    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn wait(&mut self, duration: Duration) {
        thread::sleep(duration);
    }
}

fn flash_with_adapter(
    adapter: &mut impl FlashAdapter,
    artifact_path: &Path,
    release: &FirmwareRelease,
    volumes_root: &Path,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(FirmwareFlashProgress),
) -> Result<(), FirmwareFlashError> {
    progress(FirmwareFlashProgress::LookingForDevice);
    if cancelled.load(Ordering::Acquire) {
        return Err(FirmwareFlashError::Cancelled);
    }

    let mounted = select_supported_volume(adapter.volumes(volumes_root)?)?;
    let devices = adapter.devices()?;
    let expected_serial = devices
        .first()
        .and_then(|device| device.serial_number.clone());
    let volume = if let Some(volume) = mounted {
        let application_devices = devices
            .iter()
            .filter(|device| device.bridge_firmware)
            .count();
        if application_devices != 0 || devices.len() > 1 {
            return Err(FirmwareFlashError::Discovery(
                "more than one compatible XIAO is connected; disconnect extras".to_owned(),
            ));
        }
        volume
    } else {
        if devices.len() > 1 {
            return Err(FirmwareFlashError::Discovery(
                "more than one compatible XIAO is connected; disconnect extras".to_owned(),
            ));
        }
        if let Some(device) = devices.first() {
            if device.bridge_firmware {
                validate_version_policy(adapter.firmware_version(&device.path)?, release.revision)?;
            }
            progress(FirmwareFlashProgress::EnteringBootloader);
            // A refused or disappearing port is also how a successful touch
            // can present. Continue into the documented double-RESET recovery
            // window instead of failing before the user can intervene.
            let _ = adapter.touch_1200(&device.path);
        }
        progress(FirmwareFlashProgress::WaitingForBootloader);
        wait_for_volume(adapter, volumes_root, cancelled, Duration::from_secs(30))?
    };
    if volume.board_id != FIRMWARE_BOARD_ID {
        return Err(FirmwareFlashError::WrongBoard(volume.board_id));
    }
    if cancelled.load(Ordering::Acquire) {
        return Err(FirmwareFlashError::Cancelled);
    }
    progress(FirmwareFlashProgress::Writing);
    let destination = volume.root.join(&release.artifact.name);
    let copy_result = adapter.copy_and_flush(artifact_path, &destination);

    progress(FirmwareFlashProgress::WaitingForApplication);
    let deadline = adapter.elapsed() + Duration::from_secs(30);
    let mut last_revision = None;
    while adapter.elapsed() < deadline {
        adapter.wait(Duration::from_millis(250));
        let Ok(devices) = adapter.devices() else {
            continue;
        };
        let matching: Vec<_> = devices
            .into_iter()
            .filter(|device| {
                device.bridge_firmware
                    && expected_serial
                        .as_ref()
                        .is_none_or(|serial| device.serial_number.as_ref() == Some(serial))
            })
            .collect();
        if matching.len() != 1 {
            continue;
        }
        progress(FirmwareFlashProgress::Verifying);
        match adapter.firmware_version(&matching[0].path) {
            Ok(FirmwareVersion::Reported(revision)) if revision == release.revision => {
                return Ok(())
            }
            Ok(FirmwareVersion::Reported(revision)) => last_revision = Some(revision),
            Ok(_) | Err(_) => {}
        }
    }
    if let Err(error) = copy_result {
        return Err(error.into());
    }
    Err(FirmwareFlashError::Revision {
        expected: release.revision,
        actual: last_revision,
    })
}

fn validate_version_policy(
    version: FirmwareVersion,
    target: u16,
) -> Result<(), FirmwareFlashError> {
    match classify_firmware_release(version, target) {
        FirmwareReleaseState::Pending => Err(FirmwareFlashError::VersionUnavailable),
        FirmwareReleaseState::Newer => Err(FirmwareFlashError::NewerFirmware),
        FirmwareReleaseState::UpdateAvailable | FirmwareReleaseState::Current => Ok(()),
    }
}

fn read_firmware_version(path: &str) -> Result<FirmwareVersion, FirmwareFlashError> {
    let mut output = SerialOutput::open(path, 115_200, SerialConfig::default())
        .map_err(|error| FirmwareFlashError::Discovery(error.to_string()))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let _ = output.service();
        let version = output.firmware_version().unwrap_or_default();
        if version != FirmwareVersion::Pending {
            return Ok(version);
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(output.firmware_version().unwrap_or_default())
}

#[cfg(test)]
fn supported_volume(root: &Path) -> Result<Option<BootloaderVolume>, FirmwareFlashError> {
    select_supported_volume(discover_bootloader_volumes(root)?)
}

fn select_supported_volume(
    volumes: Vec<BootloaderVolume>,
) -> Result<Option<BootloaderVolume>, FirmwareFlashError> {
    if volumes.len() > 1 {
        return Err(FirmwareFlashError::Discovery(
            "more than one UF2 bootloader is mounted; disconnect extras".to_owned(),
        ));
    }
    let Some(volume) = volumes.into_iter().next() else {
        return Ok(None);
    };
    if volume.board_id != FIRMWARE_BOARD_ID {
        return Err(FirmwareFlashError::WrongBoard(volume.board_id));
    }
    Ok(Some(volume))
}

fn wait_for_volume(
    adapter: &mut impl FlashAdapter,
    root: &Path,
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<BootloaderVolume, FirmwareFlashError> {
    let deadline = adapter.elapsed() + timeout;
    while adapter.elapsed() < deadline {
        if cancelled.load(Ordering::Acquire) {
            return Err(FirmwareFlashError::Cancelled);
        }
        if let Some(volume) = select_supported_volume(adapter.volumes(root)?)? {
            return Ok(volume);
        }
        adapter.wait(Duration::from_millis(250));
    }
    Err(FirmwareFlashError::Timeout(
        "waiting for the XIAO bootloader; double-tap RESET",
    ))
}

fn touch_1200(path: &str) -> Result<(), FirmwareFlashError> {
    let mut port = serialport::new(path, 1_200)
        .timeout(Duration::from_millis(250))
        .open()
        .map_err(|error| FirmwareFlashError::Io(io::Error::other(error.to_string())))?;
    port.write_data_terminal_ready(true)
        .map_err(|error| FirmwareFlashError::Io(io::Error::other(error.to_string())))?;
    port.write_data_terminal_ready(false)
        .map_err(|error| FirmwareFlashError::Io(io::Error::other(error.to_string())))?;
    Ok(())
}

fn copy_and_flush(source: &Path, destination: &Path) -> io::Result<()> {
    let mut source = File::open(source)?;
    let mut destination = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(destination)?;
    io::copy(&mut source, &mut destination)?;
    destination.flush()?;
    destination.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn uf2_block(family: u32) -> [u8; UF2_BLOCK_SIZE] {
        let mut block = [0_u8; UF2_BLOCK_SIZE];
        block[0..4].copy_from_slice(&UF2_MAGIC_START_0.to_le_bytes());
        block[4..8].copy_from_slice(&UF2_MAGIC_START_1.to_le_bytes());
        block[8..12].copy_from_slice(&UF2_FLAG_FAMILY_ID.to_le_bytes());
        block[28..32].copy_from_slice(&family.to_le_bytes());
        block[508..512].copy_from_slice(&UF2_MAGIC_END.to_le_bytes());
        block
    }

    fn release(revision: u16) -> FirmwareRelease {
        FirmwareRelease {
            target: crate::FIRMWARE_TARGET_ID.to_owned(),
            revision,
            minimum_application_version: semver::Version::new(1, 4, 0),
            protocol_version: 1,
            device_info_format: 1,
            board_id: FIRMWARE_BOARD_ID.to_owned(),
            uf2_family_id: UF2_FAMILY_ID,
            usb_vendor_id: crate::XIAO_USB_VENDOR_ID,
            usb_product_id: crate::XIAO_USB_PRODUCT_ID,
            usb_manufacturer: crate::XIAO_USB_MANUFACTURER.to_owned(),
            usb_product: crate::XIAO_USB_PRODUCT.to_owned(),
            artifact: crate::ArtifactDescriptor {
                name: "firmware.uf2".to_owned(),
                size: 1,
                sha256: "11".repeat(32),
            },
        }
    }

    fn device(bridge_firmware: bool) -> FirmwareDevice {
        FirmwareDevice {
            path: "/dev/cu.fixture".to_owned(),
            bridge_firmware,
            serial_number: Some("fixture-serial".to_owned()),
        }
    }

    fn volume() -> BootloaderVolume {
        BootloaderVolume {
            root: PathBuf::from("/Volumes/FIXTURE"),
            board_id: FIRMWARE_BOARD_ID.to_owned(),
        }
    }

    struct FakeAdapter {
        now: Duration,
        devices: VecDeque<Vec<FirmwareDevice>>,
        default_devices: Vec<FirmwareDevice>,
        volumes: VecDeque<Vec<BootloaderVolume>>,
        default_volumes: Vec<BootloaderVolume>,
        versions: VecDeque<FirmwareVersion>,
        default_version: FirmwareVersion,
        copy_error: bool,
        touches: usize,
    }

    impl FlashAdapter for FakeAdapter {
        fn devices(&mut self) -> Result<Vec<FirmwareDevice>, FirmwareFlashError> {
            Ok(self
                .devices
                .pop_front()
                .unwrap_or_else(|| self.default_devices.clone()))
        }

        fn volumes(&mut self, _root: &Path) -> Result<Vec<BootloaderVolume>, FirmwareFlashError> {
            Ok(self
                .volumes
                .pop_front()
                .unwrap_or_else(|| self.default_volumes.clone()))
        }

        fn firmware_version(&mut self, _path: &str) -> Result<FirmwareVersion, FirmwareFlashError> {
            Ok(self.versions.pop_front().unwrap_or(self.default_version))
        }

        fn touch_1200(&mut self, _path: &str) -> Result<(), FirmwareFlashError> {
            self.touches += 1;
            Ok(())
        }

        fn copy_and_flush(&mut self, _source: &Path, _destination: &Path) -> Result<(), io::Error> {
            if self.copy_error {
                Err(io::Error::other("bootloader disappeared"))
            } else {
                Ok(())
            }
        }

        fn elapsed(&self) -> Duration {
            self.now
        }

        fn wait(&mut self, duration: Duration) {
            self.now += duration;
        }
    }

    fn fake() -> FakeAdapter {
        FakeAdapter {
            now: Duration::ZERO,
            devices: VecDeque::new(),
            default_devices: Vec::new(),
            volumes: VecDeque::new(),
            default_volumes: Vec::new(),
            versions: VecDeque::new(),
            default_version: FirmwareVersion::Pending,
            copy_error: false,
            touches: 0,
        }
    }

    #[test]
    fn fake_adapter_covers_manual_entry_and_exact_revision_success() {
        let mut adapter = fake();
        adapter.devices.push_back(vec![device(false)]);
        adapter.devices.push_back(vec![device(true)]);
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(vec![volume()]);
        adapter.versions.push_back(FirmwareVersion::Reported(7));
        let mut progress = Vec::new();
        flash_with_adapter(
            &mut adapter,
            Path::new("firmware.uf2"),
            &release(7),
            Path::new("/Volumes"),
            &AtomicBool::new(false),
            |state| progress.push(state),
        )
        .unwrap();
        assert_eq!(adapter.touches, 1);
        assert!(progress.contains(&FirmwareFlashProgress::WaitingForBootloader));
        assert!(progress.contains(&FirmwareFlashProgress::Verifying));
    }

    #[test]
    fn provisional_flush_error_is_success_only_after_fresh_revision_verification() {
        let mut adapter = fake();
        adapter.volumes.push_back(vec![volume()]);
        adapter.devices.push_back(vec![device(false)]);
        adapter.devices.push_back(vec![device(true)]);
        adapter.versions.push_back(FirmwareVersion::Reported(8));
        adapter.copy_error = true;
        flash_with_adapter(
            &mut adapter,
            Path::new("firmware.uf2"),
            &release(8),
            Path::new("/Volumes"),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
    }

    #[test]
    fn fake_clock_bounds_manual_entry_and_revision_mismatch() {
        let mut entry = fake();
        entry.devices.push_back(Vec::new());
        assert!(matches!(
            flash_with_adapter(
                &mut entry,
                Path::new("firmware.uf2"),
                &release(9),
                Path::new("/Volumes"),
                &AtomicBool::new(false),
                |_| {},
            ),
            Err(FirmwareFlashError::Timeout(_))
        ));

        let mut mismatch = fake();
        mismatch.volumes.push_back(vec![volume()]);
        mismatch.devices.push_back(vec![device(false)]);
        mismatch.default_devices = vec![device(true)];
        mismatch.default_version = FirmwareVersion::Reported(8);
        assert!(matches!(
            flash_with_adapter(
                &mut mismatch,
                Path::new("firmware.uf2"),
                &release(9),
                Path::new("/Volumes"),
                &AtomicBool::new(false),
                |_| {},
            ),
            Err(FirmwareFlashError::Revision {
                expected: 9,
                actual: Some(8)
            })
        ));
    }

    #[test]
    fn cancellation_prevents_bootloader_or_copy_work() {
        let mut adapter = fake();
        assert!(matches!(
            flash_with_adapter(
                &mut adapter,
                Path::new("firmware.uf2"),
                &release(10),
                Path::new("/Volumes"),
                &AtomicBool::new(true),
                |_| {},
            ),
            Err(FirmwareFlashError::Cancelled)
        ));
        assert_eq!(adapter.touches, 0);
    }

    #[test]
    fn validates_every_uf2_block_and_family() {
        let path = std::env::temp_dir().join(format!("release-updater-{}.uf2", std::process::id()));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&uf2_block(UF2_FAMILY_ID));
        bytes.extend_from_slice(&uf2_block(UF2_FAMILY_ID));
        fs::write(&path, &bytes).unwrap();
        validate_uf2(&path, UF2_FAMILY_ID).unwrap();
        bytes[28] ^= 1;
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            validate_uf2(&path, UF2_FAMILY_ID),
            Err(FirmwareFlashError::InvalidUf2(_))
        ));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn bootloader_identity_comes_from_info_file_not_volume_name() {
        let root =
            std::env::temp_dir().join(format!("release-updater-volumes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("anything")).unwrap();
        fs::write(
            root.join("anything/INFO_UF2.TXT"),
            format!("UF2 Bootloader\nBoard-ID: {FIRMWARE_BOARD_ID}\n"),
        )
        .unwrap();
        assert_eq!(
            discover_bootloader_volumes(&root).unwrap(),
            vec![BootloaderVolume {
                root: root.join("anything"),
                board_id: FIRMWARE_BOARD_ID.to_owned(),
            }]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn wrong_and_multiple_bootloader_boards_are_rejected() {
        let root = std::env::temp_dir().join(format!(
            "release-updater-board-selection-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sense")).unwrap();
        fs::write(
            root.join("sense/INFO_UF2.TXT"),
            "Board-ID: Seeed_XIAO_nRF52840_Sense\n",
        )
        .unwrap();
        assert!(matches!(
            supported_volume(&root),
            Err(FirmwareFlashError::WrongBoard(_))
        ));
        fs::create_dir_all(root.join("plain")).unwrap();
        fs::write(
            root.join("plain/INFO_UF2.TXT"),
            format!("Board-ID: {FIRMWARE_BOARD_ID}\n"),
        )
        .unwrap();
        assert!(matches!(
            supported_volume(&root),
            Err(FirmwareFlashError::Discovery(_))
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn downgrade_policy_is_fail_closed_for_newer_and_unknown_formats() {
        assert_eq!(
            classify_firmware_release(FirmwareVersion::Pending, 4),
            FirmwareReleaseState::Pending
        );
        assert_eq!(
            classify_firmware_release(FirmwareVersion::Reported(3), 4),
            FirmwareReleaseState::UpdateAvailable
        );
        assert_eq!(
            classify_firmware_release(FirmwareVersion::Reported(4), 4),
            FirmwareReleaseState::Current
        );
        assert_eq!(
            classify_firmware_release(FirmwareVersion::Reported(5), 4),
            FirmwareReleaseState::Newer
        );
        assert_eq!(
            classify_firmware_release(FirmwareVersion::UnsupportedFormat(2), 4),
            FirmwareReleaseState::Newer
        );
        for version in [FirmwareVersion::Unreported, FirmwareVersion::Malformed] {
            assert_eq!(
                classify_firmware_release(version, 4),
                FirmwareReleaseState::UpdateAvailable
            );
        }
        assert!(validate_version_policy(FirmwareVersion::Reported(5), 4).is_err());
        assert!(validate_version_policy(FirmwareVersion::UnsupportedFormat(2), 4).is_err());
        assert!(validate_version_policy(FirmwareVersion::Pending, 4).is_err());
        assert!(validate_version_policy(FirmwareVersion::Reported(4), 4).is_ok());
        assert!(validate_version_policy(FirmwareVersion::Unreported, 4).is_ok());
    }
}
