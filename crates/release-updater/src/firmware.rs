use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bridge_output::{
    available_serial_devices, new_firmware_install_receipt, random_firmware_request_id,
    FirmwareCapabilities, FirmwareInfo, FirmwareInstallReceipt, FirmwareInstallSource,
    FirmwareInstallState, FirmwareVersion, SerialConfig, SerialDeviceInfo, SerialOutput,
};

#[cfg(test)]
use crate::UF2_FAMILY_ID;
use crate::{verify_artifact, FirmwareRelease, FIRMWARE_BOARD_ID};

const SEEED_VENDOR_ID: u16 = 0x2886;
const XIAO_APPLICATION_PRODUCT_IDS: [u16; 2] = [0x8044, 0x8045];
const XIAO_BOOTLOADER_PRODUCT_IDS: [u16; 2] = [0x0044, 0x0045];
const XIAO_SENSE_BOARD_ID: &str = "Seeed_XIAO_nRF52840_Sense";
const AUTOMATIC_BOOTLOADER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const AUTOMATIC_BOOTLOADER_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const MANUAL_BOOTLOADER_WAIT_TIMEOUT: Duration = Duration::from_mins(1);
const APPLICATION_RECONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const UF2_BLOCK_SIZE: usize = 512;
const UF2_MAGIC_START_0: u32 = 0x0A32_4655;
const UF2_MAGIC_START_1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
const UF2_FLAG_FAMILY_ID: u32 = 0x0000_2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareDevice {
    pub path: String,
    pub kind: FirmwareDeviceKind,
    pub serial_number: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareDeviceKind {
    BridgeApplication,
    FactoryApplication,
    Uf2Bootloader,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootloaderVolume {
    pub root: PathBuf,
    pub board_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirmwareFlashProgress {
    LookingForDevice,
    RequestingBootloader,
    WaitingForBootloader,
    ManualRecovery,
    Writing,
    WaitingForApplication,
    RecordingReceipt,
    VerifyingReceipt,
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
    ReceiptExpectedPending(FirmwareInstallState),
    ReceiptRecording(String),
    ReceiptMismatch,
    CancelledInBootloader,
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
            Self::ReceiptExpectedPending(actual) => write!(
                formatter,
                "firmware revision started, but its installation marker is {actual:?}; expected a fresh pending receipt"
            ),
            Self::ReceiptRecording(error) => write!(
                formatter,
                "firmware started successfully, but installation verification is incomplete: {error}"
            ),
            Self::ReceiptMismatch => write!(
                formatter,
                "firmware started successfully, but the committed installation receipt did not match the requested receipt"
            ),
            Self::CancelledInBootloader => write!(
                formatter,
                "firmware update cancelled after entering the UF2 bootloader; unplug and reconnect the board to return it to normal operation"
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
                    kind: FirmwareDeviceKind::BridgeApplication,
                    serial_number: device.serial_number,
                })
            } else {
                factory_xiao_kind(&device).map(|kind| FirmwareDevice {
                    path: device.path,
                    kind,
                    serial_number: device.serial_number,
                })
            }
        })
        .collect();
    Ok(devices)
}

fn factory_xiao_kind(device: &SerialDeviceInfo) -> Option<FirmwareDeviceKind> {
    let callout = !cfg!(target_os = "macos") || device.path.starts_with("/dev/cu.");
    if !callout || device.vendor_id != Some(SEEED_VENDOR_ID) {
        return None;
    }
    match device.product_id {
        Some(product_id) if XIAO_APPLICATION_PRODUCT_IDS.contains(&product_id) => {
            Some(FirmwareDeviceKind::FactoryApplication)
        }
        Some(product_id) if XIAO_BOOTLOADER_PRODUCT_IDS.contains(&product_id) => {
            Some(FirmwareDeviceKind::Uf2Bootloader)
        }
        _ => None,
    }
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
) -> Result<FirmwareInfo, FirmwareFlashError> {
    verify_artifact(artifact_path, &release.artifact)
        .map_err(|error| FirmwareFlashError::Artifact(error.to_string()))?;
    validate_uf2(artifact_path, release.uf2_family_id)?;
    let mut adapter = NativeFlashAdapter {
        started: Instant::now(),
        firmware_session: None,
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
    fn firmware_info(&mut self, path: &str) -> Result<FirmwareInfo, FirmwareFlashError>;
    fn enter_uf2_bootloader(&mut self, path: &str) -> Result<(), FirmwareFlashError>;
    fn release_device(&mut self) {}
    fn new_receipt(
        &mut self,
        source: FirmwareInstallSource,
    ) -> Result<FirmwareInstallReceipt, FirmwareFlashError>;
    fn record_install_receipt(
        &mut self,
        path: &str,
        receipt: FirmwareInstallReceipt,
    ) -> Result<FirmwareInstallReceipt, FirmwareFlashError>;
    fn copy_and_flush(&mut self, source: &Path, destination: &Path) -> Result<(), io::Error>;
    fn elapsed(&self) -> Duration;
    fn wait(&mut self, duration: Duration);
}

struct NativeFlashAdapter {
    started: Instant,
    firmware_session: Option<(String, SerialOutput)>,
}

impl NativeFlashAdapter {
    fn take_firmware_session(&mut self, path: &str) -> Result<SerialOutput, FirmwareFlashError> {
        if self
            .firmware_session
            .as_ref()
            .is_some_and(|(session_path, _)| session_path == path)
        {
            return Ok(self
                .firmware_session
                .take()
                .expect("matching firmware session was present")
                .1);
        }
        self.firmware_session = None;
        let mut output = open_firmware(path)?;
        output
            .wait_for_firmware_info(AUTOMATIC_BOOTLOADER_RESPONSE_TIMEOUT)
            .map_err(|error| FirmwareFlashError::Discovery(error.to_string()))?;
        Ok(output)
    }
}

impl FlashAdapter for NativeFlashAdapter {
    fn devices(&mut self) -> Result<Vec<FirmwareDevice>, FirmwareFlashError> {
        discover_firmware_devices()
    }

    fn volumes(&mut self, root: &Path) -> Result<Vec<BootloaderVolume>, FirmwareFlashError> {
        discover_bootloader_volumes(root)
    }

    fn firmware_info(&mut self, path: &str) -> Result<FirmwareInfo, FirmwareFlashError> {
        self.firmware_session = None;
        let mut output = open_firmware(path)?;
        let info = output
            .wait_for_firmware_info(Duration::from_secs(3))
            .map_err(|error| FirmwareFlashError::Discovery(error.to_string()))?;
        self.firmware_session = Some((path.to_owned(), output));
        Ok(info)
    }

    fn enter_uf2_bootloader(&mut self, path: &str) -> Result<(), FirmwareFlashError> {
        let mut output = self.take_firmware_session(path)?;
        output
            .enter_uf2_bootloader(random_request_id()?, AUTOMATIC_BOOTLOADER_RESPONSE_TIMEOUT)
            .map_err(|error| FirmwareFlashError::Discovery(error.to_string()))
    }

    fn release_device(&mut self) {
        self.firmware_session = None;
    }

    fn new_receipt(
        &mut self,
        source: FirmwareInstallSource,
    ) -> Result<FirmwareInstallReceipt, FirmwareFlashError> {
        new_install_receipt(source)
    }

    fn record_install_receipt(
        &mut self,
        path: &str,
        receipt: FirmwareInstallReceipt,
    ) -> Result<FirmwareInstallReceipt, FirmwareFlashError> {
        let mut output = self
            .take_firmware_session(path)
            .map_err(|error| FirmwareFlashError::ReceiptRecording(error.to_string()))?;
        output
            .record_install_receipt_and_wait(
                random_request_id()?,
                receipt,
                AUTOMATIC_BOOTLOADER_RESPONSE_TIMEOUT,
            )
            .map_err(|error| FirmwareFlashError::ReceiptRecording(error.to_string()))
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
) -> Result<FirmwareInfo, FirmwareFlashError> {
    progress(FirmwareFlashProgress::LookingForDevice);
    if cancelled.load(Ordering::Acquire) {
        return Err(FirmwareFlashError::Cancelled);
    }

    let prepared = prepare_flash_target(adapter, release, volumes_root, cancelled, &mut progress)?;
    if cancelled.load(Ordering::Acquire) {
        return Err(FirmwareFlashError::CancelledInBootloader);
    }
    progress(FirmwareFlashProgress::Writing);
    let destination = prepared.volume.root.join(&release.artifact.name);
    let copy_result = adapter.copy_and_flush(artifact_path, &destination);

    verify_reconnected_firmware(
        adapter,
        release,
        prepared.expected_serial.as_deref(),
        prepared.previous_firmware,
        copy_result,
        &mut progress,
    )
}

struct PreparedFlash {
    volume: BootloaderVolume,
    expected_serial: Option<String>,
    previous_firmware: Option<FirmwareInfo>,
}

fn prepare_flash_target(
    adapter: &mut impl FlashAdapter,
    release: &FirmwareRelease,
    volumes_root: &Path,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(FirmwareFlashProgress),
) -> Result<PreparedFlash, FirmwareFlashError> {
    let mounted = select_supported_volume(adapter.volumes(volumes_root)?)?;
    let devices = adapter.devices()?;
    let expected_serial = devices
        .first()
        .and_then(|device| device.serial_number.clone());
    let mut automatic_entry_may_have_started = false;
    let mut previous_firmware = None;
    let volume = if let Some(volume) = mounted {
        if devices.len() > 1
            || devices
                .iter()
                .any(|device| device.kind != FirmwareDeviceKind::Uf2Bootloader)
        {
            return Err(FirmwareFlashError::Discovery(
                "more than one compatible XIAO is connected; disconnect extras".to_owned(),
            ));
        }
        volume
    } else {
        if devices.len() != 1 {
            return Err(FirmwareFlashError::Discovery(
                "connect exactly one compatible XIAO, or mount exactly one supported UF2 drive"
                    .to_owned(),
            ));
        }
        let device = &devices[0];
        let info = if device.kind == FirmwareDeviceKind::BridgeApplication {
            let info = adapter.firmware_info(&device.path)?;
            validate_version_policy(info.version, release.revision)?;
            previous_firmware = Some(info);
            Some(info)
        } else {
            None
        };

        let automatic_supported = info.is_some_and(|info| {
            info.capabilities
                .contains(FirmwareCapabilities::ENTER_UF2_BOOTLOADER)
        });
        let automatic_volume = if automatic_supported {
            progress(FirmwareFlashProgress::RequestingBootloader);
            if cancelled.load(Ordering::Acquire) {
                return Err(FirmwareFlashError::Cancelled);
            }
            // Once the request is sent, a lost acknowledgement cannot tell us
            // whether the board stayed in application mode or entered UF2.
            automatic_entry_may_have_started = true;
            match adapter.enter_uf2_bootloader(&device.path) {
                Ok(()) => {
                    progress(FirmwareFlashProgress::WaitingForBootloader);
                    match wait_for_volume(
                        adapter,
                        volumes_root,
                        cancelled,
                        AUTOMATIC_BOOTLOADER_WAIT_TIMEOUT,
                        true,
                    ) {
                        Ok(volume) => Some(volume),
                        Err(FirmwareFlashError::Timeout(_)) => None,
                        Err(error) => return Err(error),
                    }
                }
                Err(_) => None,
            }
        } else {
            adapter.release_device();
            None
        };
        if let Some(volume) = automatic_volume {
            volume
        } else {
            progress(FirmwareFlashProgress::ManualRecovery);
            wait_for_volume(
                adapter,
                volumes_root,
                cancelled,
                MANUAL_BOOTLOADER_WAIT_TIMEOUT,
                automatic_entry_may_have_started,
            )?
        }
    };
    Ok(PreparedFlash {
        volume,
        expected_serial,
        previous_firmware,
    })
}

fn verify_reconnected_firmware(
    adapter: &mut impl FlashAdapter,
    release: &FirmwareRelease,
    expected_serial: Option<&str>,
    previous_firmware: Option<FirmwareInfo>,
    copy_result: Result<(), io::Error>,
    progress: &mut impl FnMut(FirmwareFlashProgress),
) -> Result<FirmwareInfo, FirmwareFlashError> {
    let mut copy_error = copy_result.err();
    progress(FirmwareFlashProgress::WaitingForApplication);
    let deadline = adapter.elapsed() + APPLICATION_RECONNECT_TIMEOUT;
    let mut last_revision = None;
    while adapter.elapsed() < deadline {
        adapter.wait(Duration::from_millis(250));
        let Ok(devices) = adapter.devices() else {
            continue;
        };
        if devices.len() > 1 {
            return Err(FirmwareFlashError::Discovery(
                "more than one compatible XIAO reconnected; disconnect extras".to_owned(),
            ));
        }
        let matching: Vec<_> = devices
            .into_iter()
            .filter(|device| {
                device.kind == FirmwareDeviceKind::BridgeApplication
                    && expected_serial
                        .is_none_or(|serial| device.serial_number.as_deref() == Some(serial))
            })
            .collect();
        if matching.len() != 1 {
            continue;
        }
        let path = &matching[0].path;
        match adapter.firmware_info(path) {
            Ok(info) if info.version == FirmwareVersion::Reported(release.revision) => {
                if info.install_state != FirmwareInstallState::Pending {
                    return Err(FirmwareFlashError::ReceiptExpectedPending(
                        info.install_state,
                    ));
                }
                if copy_error.is_some()
                    && !provisional_copy_is_verified(previous_firmware, release.revision)
                {
                    return Err(copy_error
                        .take()
                        .expect("copy error was checked as present")
                        .into());
                }
                let requested = adapter.new_receipt(FirmwareInstallSource::AppCenter)?;
                if previous_firmware
                    .and_then(|firmware| match firmware.install_state {
                        FirmwareInstallState::Recorded(receipt) => Some(receipt),
                        _ => None,
                    })
                    .is_some_and(|previous| {
                        previous.installed_at == requested.installed_at
                            || previous.install_id == requested.install_id
                    })
                {
                    return Err(FirmwareFlashError::ReceiptMismatch);
                }
                progress(FirmwareFlashProgress::RecordingReceipt);
                let acknowledged = adapter.record_install_receipt(path, requested)?;
                if acknowledged != requested {
                    return Err(FirmwareFlashError::ReceiptMismatch);
                }
                progress(FirmwareFlashProgress::VerifyingReceipt);
                let verified = adapter
                    .firmware_info(path)
                    .map_err(|error| FirmwareFlashError::ReceiptRecording(error.to_string()))?;
                if verified.version != FirmwareVersion::Reported(release.revision)
                    || verified.install_state != FirmwareInstallState::Recorded(requested)
                {
                    return Err(FirmwareFlashError::ReceiptMismatch);
                }
                return Ok(verified);
            }
            Ok(FirmwareInfo {
                version: FirmwareVersion::Reported(revision),
                ..
            }) => last_revision = Some(revision),
            Ok(_) | Err(_) => {}
        }
    }
    if let Some(error) = copy_error {
        return Err(error.into());
    }
    Err(FirmwareFlashError::Revision {
        expected: release.revision,
        actual: last_revision,
    })
}

fn provisional_copy_is_verified(previous: Option<FirmwareInfo>, target: u16) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    match previous.version {
        FirmwareVersion::Reported(revision) if revision != target => true,
        FirmwareVersion::Reported(_) => {
            matches!(previous.install_state, FirmwareInstallState::Recorded(_))
        }
        FirmwareVersion::Pending
        | FirmwareVersion::Unreported
        | FirmwareVersion::Malformed
        | FirmwareVersion::UnsupportedFormat(_) => false,
    }
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

fn open_firmware(path: &str) -> Result<SerialOutput, FirmwareFlashError> {
    SerialOutput::open(path, 115_200, SerialConfig::default())
        .map_err(|error| FirmwareFlashError::Discovery(error.to_string()))
}

fn random_request_id() -> Result<u32, FirmwareFlashError> {
    random_firmware_request_id().map_err(|error| FirmwareFlashError::Discovery(error.to_string()))
}

fn new_install_receipt(
    source: FirmwareInstallSource,
) -> Result<FirmwareInstallReceipt, FirmwareFlashError> {
    new_firmware_install_receipt(source)
        .map_err(|error| FirmwareFlashError::ReceiptRecording(error.to_string()))
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
    if !supported_board_id(&volume.board_id) {
        return Err(FirmwareFlashError::WrongBoard(volume.board_id));
    }
    Ok(Some(volume))
}

fn supported_board_id(board_id: &str) -> bool {
    matches!(board_id, FIRMWARE_BOARD_ID | XIAO_SENSE_BOARD_ID)
}

fn wait_for_volume(
    adapter: &mut impl FlashAdapter,
    root: &Path,
    cancelled: &AtomicBool,
    timeout: Duration,
    entered_bootloader: bool,
) -> Result<BootloaderVolume, FirmwareFlashError> {
    let deadline = adapter.elapsed() + timeout;
    while adapter.elapsed() < deadline {
        if cancelled.load(Ordering::Acquire) {
            return Err(if entered_bootloader {
                FirmwareFlashError::CancelledInBootloader
            } else {
                FirmwareFlashError::Cancelled
            });
        }
        if let Some(volume) = select_supported_volume(adapter.volumes(root)?)? {
            return Ok(volume);
        }
        adapter.wait(Duration::from_millis(250));
    }
    Err(FirmwareFlashError::Timeout(
        "waiting for the XIAO UF2 drive; quickly press the tiny reset button beside the USB-C connector twice while manual recovery is waiting",
    ))
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
            kind: if bridge_firmware {
                FirmwareDeviceKind::BridgeApplication
            } else {
                FirmwareDeviceKind::FactoryApplication
            },
            serial_number: Some("fixture-serial".to_owned()),
        }
    }

    fn bootloader_device() -> FirmwareDevice {
        FirmwareDevice {
            path: "/dev/cu.fixture".to_owned(),
            kind: FirmwareDeviceKind::Uf2Bootloader,
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
        infos: VecDeque<FirmwareInfo>,
        default_capabilities: FirmwareCapabilities,
        default_install_state: FirmwareInstallState,
        recorded_receipt: Option<FirmwareInstallReceipt>,
        receipt_ack_override: Option<FirmwareInstallReceipt>,
        receipt_recording_error: bool,
        automatic_entry_error: bool,
        automatic_entry_requests: usize,
        copy_error: bool,
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

        fn firmware_info(&mut self, _path: &str) -> Result<FirmwareInfo, FirmwareFlashError> {
            if let Some(info) = self.infos.pop_front() {
                return Ok(info);
            }
            Ok(FirmwareInfo {
                version: self.versions.pop_front().unwrap_or(self.default_version),
                capabilities: self.default_capabilities,
                install_state: self
                    .recorded_receipt
                    .map_or(self.default_install_state, FirmwareInstallState::Recorded),
            })
        }

        fn enter_uf2_bootloader(&mut self, _path: &str) -> Result<(), FirmwareFlashError> {
            self.automatic_entry_requests += 1;
            if self.automatic_entry_error {
                Err(FirmwareFlashError::Discovery(
                    "automatic entry fixture failure".to_owned(),
                ))
            } else {
                Ok(())
            }
        }

        fn new_receipt(
            &mut self,
            source: FirmwareInstallSource,
        ) -> Result<FirmwareInstallReceipt, FirmwareFlashError> {
            Ok(FirmwareInstallReceipt {
                installed_at: 1_786_456_920,
                install_id: [0x42; 16],
                source,
            })
        }

        fn record_install_receipt(
            &mut self,
            _path: &str,
            receipt: FirmwareInstallReceipt,
        ) -> Result<FirmwareInstallReceipt, FirmwareFlashError> {
            if self.receipt_recording_error {
                return Err(FirmwareFlashError::ReceiptRecording(
                    "fixture write failure".to_owned(),
                ));
            }
            let acknowledged = self.receipt_ack_override.unwrap_or(receipt);
            self.recorded_receipt = Some(receipt);
            Ok(acknowledged)
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
            infos: VecDeque::new(),
            default_capabilities: FirmwareCapabilities::default(),
            default_install_state: FirmwareInstallState::Pending,
            recorded_receipt: None,
            receipt_ack_override: None,
            receipt_recording_error: false,
            automatic_entry_error: false,
            automatic_entry_requests: 0,
            copy_error: false,
        }
    }

    fn info(
        revision: u16,
        capabilities: FirmwareCapabilities,
        install_state: FirmwareInstallState,
    ) -> FirmwareInfo {
        FirmwareInfo {
            version: FirmwareVersion::Reported(revision),
            capabilities,
            install_state,
        }
    }

    #[test]
    fn manual_entry_and_exact_receipt_verification_succeed() {
        let mut adapter = fake();
        adapter.devices.push_back(vec![device(false)]);
        adapter.devices.push_back(vec![device(true)]);
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(vec![volume()]);
        adapter.versions.push_back(FirmwareVersion::Reported(7));
        adapter.default_version = FirmwareVersion::Reported(7);
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
        assert!(progress.contains(&FirmwareFlashProgress::ManualRecovery));
        assert!(progress.contains(&FirmwareFlashProgress::RecordingReceipt));
        assert!(progress.contains(&FirmwareFlashProgress::VerifyingReceipt));
    }

    #[test]
    fn automatic_entry_is_used_and_manual_recovery_is_not_shown() {
        let mut adapter = fake();
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(vec![volume()]);
        adapter.devices.push_back(vec![device(true)]);
        adapter.devices.push_back(vec![device(true)]);
        adapter.infos.push_back(info(
            2,
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER | FirmwareCapabilities::INSTALL_RECEIPT,
            FirmwareInstallState::Recorded(FirmwareInstallReceipt {
                installed_at: 100,
                install_id: [1; 16],
                source: FirmwareInstallSource::FirstObserved,
            }),
        ));
        adapter.default_version = FirmwareVersion::Reported(2);
        adapter.default_capabilities =
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER | FirmwareCapabilities::INSTALL_RECEIPT;
        let mut progress = Vec::new();
        flash_with_adapter(
            &mut adapter,
            Path::new("firmware.uf2"),
            &release(2),
            Path::new("/Volumes"),
            &AtomicBool::new(false),
            |state| progress.push(state),
        )
        .unwrap();
        assert_eq!(adapter.automatic_entry_requests, 1);
        assert!(progress.contains(&FirmwareFlashProgress::RequestingBootloader));
        assert!(progress.contains(&FirmwareFlashProgress::WaitingForBootloader));
        assert!(!progress.contains(&FirmwareFlashProgress::ManualRecovery));
    }

    #[test]
    fn automatic_entry_failure_falls_back_to_bounded_manual_recovery() {
        let mut adapter = fake();
        adapter.automatic_entry_error = true;
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(vec![volume()]);
        adapter.devices.push_back(vec![device(true)]);
        adapter.devices.push_back(vec![device(true)]);
        adapter.infos.push_back(info(
            1,
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER,
            FirmwareInstallState::Unsupported,
        ));
        adapter.default_version = FirmwareVersion::Reported(2);
        let mut progress = Vec::new();
        flash_with_adapter(
            &mut adapter,
            Path::new("firmware.uf2"),
            &release(2),
            Path::new("/Volumes"),
            &AtomicBool::new(false),
            |state| progress.push(state),
        )
        .unwrap();
        assert_eq!(adapter.automatic_entry_requests, 1);
        assert!(progress.contains(&FirmwareFlashProgress::ManualRecovery));
    }

    #[test]
    fn cancelling_after_an_uncertain_automatic_entry_explains_recovery() {
        let mut adapter = fake();
        adapter.automatic_entry_error = true;
        adapter.volumes.push_back(Vec::new());
        adapter.devices.push_back(vec![device(true)]);
        adapter.infos.push_back(info(
            2,
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER,
            FirmwareInstallState::Recorded(FirmwareInstallReceipt {
                installed_at: 1,
                install_id: [1; 16],
                source: FirmwareInstallSource::FirstObserved,
            }),
        ));
        let cancelled = AtomicBool::new(false);
        let result = flash_with_adapter(
            &mut adapter,
            Path::new("firmware.uf2"),
            &release(2),
            Path::new("/Volumes"),
            &cancelled,
            |state| {
                if state == FirmwareFlashProgress::ManualRecovery {
                    cancelled.store(true, Ordering::Release);
                }
            },
        );
        assert!(matches!(
            result,
            Err(FirmwareFlashError::CancelledInBootloader)
        ));
    }

    #[test]
    fn same_version_reinstall_rejects_an_unchanged_committed_receipt() {
        let previous = FirmwareInstallReceipt {
            installed_at: 99,
            install_id: [9; 16],
            source: FirmwareInstallSource::AppCenter,
        };
        let mut adapter = fake();
        adapter.volumes.push_back(vec![volume()]);
        adapter.devices.push_back(vec![bootloader_device()]);
        adapter.devices.push_back(vec![device(true)]);
        adapter.infos.push_back(info(
            2,
            FirmwareCapabilities::INSTALL_RECEIPT,
            FirmwareInstallState::Recorded(previous),
        ));
        assert!(matches!(
            flash_with_adapter(
                &mut adapter,
                Path::new("firmware.uf2"),
                &release(2),
                Path::new("/Volumes"),
                &AtomicBool::new(false),
                |_| {},
            ),
            Err(FirmwareFlashError::ReceiptExpectedPending(
                FirmwareInstallState::Recorded(receipt)
            )) if receipt == previous
        ));
    }

    #[test]
    fn receipt_acknowledgement_mismatch_and_record_failure_are_not_success() {
        for recording_error in [false, true] {
            let mut adapter = fake();
            adapter.volumes.push_back(vec![volume()]);
            adapter.devices.push_back(vec![bootloader_device()]);
            adapter.devices.push_back(vec![device(true)]);
            adapter.default_version = FirmwareVersion::Reported(2);
            adapter.receipt_recording_error = recording_error;
            if !recording_error {
                adapter.receipt_ack_override = Some(FirmwareInstallReceipt {
                    installed_at: 1,
                    install_id: [1; 16],
                    source: FirmwareInstallSource::AppCenter,
                });
            }
            let error = flash_with_adapter(
                &mut adapter,
                Path::new("firmware.uf2"),
                &release(2),
                Path::new("/Volumes"),
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap_err();
            assert!(matches!(
                error,
                FirmwareFlashError::ReceiptMismatch | FirmwareFlashError::ReceiptRecording(_)
            ));
        }
    }

    #[test]
    fn cancelling_after_automatic_entry_explains_how_to_leave_bootloader() {
        let mut adapter = fake();
        adapter.volumes.push_back(Vec::new());
        adapter.devices.push_back(vec![device(true)]);
        adapter.infos.push_back(info(
            2,
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER,
            FirmwareInstallState::Recorded(FirmwareInstallReceipt {
                installed_at: 1,
                install_id: [1; 16],
                source: FirmwareInstallSource::FirstObserved,
            }),
        ));
        let cancelled = AtomicBool::new(false);
        let result = flash_with_adapter(
            &mut adapter,
            Path::new("firmware.uf2"),
            &release(2),
            Path::new("/Volumes"),
            &cancelled,
            |state| {
                if state == FirmwareFlashProgress::WaitingForBootloader {
                    cancelled.store(true, Ordering::Release);
                }
            },
        );
        assert!(matches!(
            result,
            Err(FirmwareFlashError::CancelledInBootloader)
        ));
    }

    #[test]
    fn provisional_flush_error_requires_proof_that_a_fresh_image_started() {
        let previous_receipt = FirmwareInstallReceipt {
            installed_at: 1,
            install_id: [1; 16],
            source: FirmwareInstallSource::FirstObserved,
        };
        for previous in [
            info(
                7,
                FirmwareCapabilities::ENTER_UF2_BOOTLOADER,
                FirmwareInstallState::Recorded(previous_receipt),
            ),
            info(
                8,
                FirmwareCapabilities::ENTER_UF2_BOOTLOADER,
                FirmwareInstallState::Recorded(previous_receipt),
            ),
        ] {
            let mut adapter = fake();
            adapter.volumes.push_back(Vec::new());
            adapter.volumes.push_back(vec![volume()]);
            adapter.devices.push_back(vec![device(true)]);
            adapter.devices.push_back(vec![device(true)]);
            adapter.infos.push_back(previous);
            adapter.infos.push_back(info(
                8,
                FirmwareCapabilities::ENTER_UF2_BOOTLOADER,
                FirmwareInstallState::Pending,
            ));
            adapter.default_version = FirmwareVersion::Reported(8);
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

        let mut pending = fake();
        pending.volumes.push_back(Vec::new());
        pending.volumes.push_back(vec![volume()]);
        pending.devices.push_back(vec![device(true)]);
        pending.devices.push_back(vec![device(true)]);
        pending.infos.push_back(info(
            8,
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER,
            FirmwareInstallState::Pending,
        ));
        pending.infos.push_back(info(
            8,
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER,
            FirmwareInstallState::Pending,
        ));
        pending.copy_error = true;
        assert!(matches!(
            flash_with_adapter(
                &mut pending,
                Path::new("firmware.uf2"),
                &release(8),
                Path::new("/Volumes"),
                &AtomicBool::new(false),
                |_| {},
            ),
            Err(FirmwareFlashError::Io(_))
        ));
    }

    #[test]
    fn exact_device_selection_and_revision_mismatch_fail_closed() {
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
            Err(FirmwareFlashError::Discovery(_))
        ));

        let mut mounted_and_factory = fake();
        mounted_and_factory.volumes.push_back(vec![volume()]);
        mounted_and_factory.devices.push_back(vec![device(false)]);
        assert!(matches!(
            flash_with_adapter(
                &mut mounted_and_factory,
                Path::new("firmware.uf2"),
                &release(9),
                Path::new("/Volumes"),
                &AtomicBool::new(false),
                |_| {},
            ),
            Err(FirmwareFlashError::Discovery(_))
        ));

        let mut mismatch = fake();
        mismatch.volumes.push_back(vec![volume()]);
        mismatch.devices.push_back(vec![bootloader_device()]);
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
    fn standard_and_sense_bootloader_boards_are_supported() {
        for board_id in [FIRMWARE_BOARD_ID, XIAO_SENSE_BOARD_ID] {
            assert!(select_supported_volume(vec![BootloaderVolume {
                root: PathBuf::from("/Volumes/XIAO"),
                board_id: board_id.to_owned(),
            }])
            .unwrap()
            .is_some());
        }
    }

    #[test]
    fn wrong_and_multiple_bootloader_boards_are_rejected() {
        let root = std::env::temp_dir().join(format!(
            "release-updater-board-selection-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("wrong")).unwrap();
        fs::write(
            root.join("wrong/INFO_UF2.TXT"),
            "Board-ID: incompatible_board\n",
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
    fn factory_xiao_detection_accepts_standard_and_sense_usb_ids() {
        for (product_id, expected) in [
            (0x8044, FirmwareDeviceKind::FactoryApplication),
            (0x0044, FirmwareDeviceKind::Uf2Bootloader),
            (0x8045, FirmwareDeviceKind::FactoryApplication),
            (0x0045, FirmwareDeviceKind::Uf2Bootloader),
        ] {
            assert_eq!(
                factory_xiao_kind(&SerialDeviceInfo {
                    path: "/dev/cu.fixture".to_owned(),
                    vendor_id: Some(SEEED_VENDOR_ID),
                    product_id: Some(product_id),
                    serial_number: None,
                    manufacturer: Some("Seeed".to_owned()),
                    product: None,
                }),
                Some(expected)
            );
        }
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
