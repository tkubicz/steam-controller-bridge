use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bridge_output::{
    available_bridge_endpoints, new_firmware_install_receipt, random_firmware_request_id,
    BridgeEndpoint, BridgeOutput, BridgeTransportConfig, FirmwareCapabilities, FirmwareInfo,
    FirmwareInstallReceipt, FirmwareInstallSource, FirmwareInstallState, FirmwareVersion,
};

use crate::{
    current_removable_volume_locator, firmware_matches_target, firmware_target, verify_artifact,
    ArtifactError, FirmwareInstallerStrategy, FirmwareRelease, FirmwareTargetDescriptor,
    MacOsVolumeLocator, RemovableVolumeLocator, VolumeScanError,
};
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
    pub endpoint: BridgeEndpoint,
    pub kind: FirmwareDeviceKind,
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

#[derive(Debug, thiserror::Error)]
pub enum FirmwareFlashError {
    #[error("firmware I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("removable-volume discovery failed: {0}")]
    VolumeScan(#[from] VolumeScanError),
    #[error("cannot select firmware device: {0}")]
    Discovery(String),
    #[error("unsupported bootloader board: {0}")]
    WrongBoard(String),
    #[error("unsupported firmware target: {0}")]
    UnsupportedTarget(String),
    #[error("invalid UF2 firmware: {0}")]
    InvalidUf2(String),
    #[error("firmware update cancelled")]
    Cancelled,
    #[error("timed out while {0}")]
    Timeout(String),
    #[error("firmware verification reported {actual:?}; expected revision {expected}")]
    Revision { expected: u16, actual: Option<u16> },
    #[error("firmware artifact failed verification: {0}")]
    Artifact(#[from] ArtifactError),
    #[error("refusing to downgrade newer firmware")]
    NewerFirmware,
    #[error("current firmware revision could not be verified; reconnect and retry")]
    VersionUnavailable,
    #[error("firmware revision started, but its installation marker is {0:?}; expected a fresh pending receipt")]
    ReceiptExpectedPending(FirmwareInstallState),
    #[error("firmware started successfully, but installation verification is incomplete: {0}")]
    ReceiptRecording(String),
    #[error("firmware started successfully, but the committed installation receipt did not match the requested receipt")]
    ReceiptMismatch,
    #[error("firmware update cancelled after entering the UF2 bootloader; unplug and reconnect the board to return it to normal operation")]
    CancelledInBootloader,
}

pub fn discover_firmware_devices(
    target: &FirmwareTargetDescriptor,
) -> Result<Vec<FirmwareDevice>, FirmwareFlashError> {
    available_bridge_endpoints()
        .map_err(|error| FirmwareFlashError::Discovery(error.to_string()))
        .map(|endpoints| classify_firmware_devices(endpoints, target))
}

fn classify_firmware_devices(
    endpoints: Vec<BridgeEndpoint>,
    target: &FirmwareTargetDescriptor,
) -> Vec<FirmwareDevice> {
    endpoints
        .into_iter()
        .filter_map(|endpoint| {
            if endpoint.is_bridge_device() {
                Some(FirmwareDevice {
                    endpoint,
                    kind: FirmwareDeviceKind::BridgeApplication,
                })
            } else {
                target_device_kind(&endpoint, target).map(|kind| FirmwareDevice { endpoint, kind })
            }
        })
        .collect()
}

fn target_device_kind(
    endpoint: &BridgeEndpoint,
    target: &FirmwareTargetDescriptor,
) -> Option<FirmwareDeviceKind> {
    let endpoint = endpoint.usb_identity()?;
    let identity = crate::UsbIdentity {
        vendor_id: endpoint.vendor_id,
        product_id: endpoint.product_id,
    };

    target_usb_device_kind(identity, target)
}

fn target_usb_device_kind(
    identity: crate::UsbIdentity,
    target: &FirmwareTargetDescriptor,
) -> Option<FirmwareDeviceKind> {
    if target.factory_application_usb.contains(&identity) {
        Some(FirmwareDeviceKind::FactoryApplication)
    } else if target.bootloader_usb.contains(&identity) {
        Some(FirmwareDeviceKind::Uf2Bootloader)
    } else {
        None
    }
}

pub fn discover_bootloader_volumes(
    root: &Path,
) -> Result<Vec<BootloaderVolume>, FirmwareFlashError> {
    discover_bootloader_volumes_with(&MacOsVolumeLocator::with_root(root))
}

fn discover_bootloader_volumes_with(
    locator: &dyn RemovableVolumeLocator,
) -> Result<Vec<BootloaderVolume>, FirmwareFlashError> {
    let mut volumes = Vec::new();
    for root in locator.enumerate()?.roots {
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
    cancelled: &AtomicBool,
    progress: impl FnMut(FirmwareFlashProgress),
) -> Result<FirmwareInfo, FirmwareFlashError> {
    let target = firmware_target(&release.target)
        .ok_or_else(|| FirmwareFlashError::UnsupportedTarget(release.target.clone()))?;
    verify_artifact(artifact_path, &release.artifact)?;
    match target.installer {
        FirmwareInstallerStrategy::Uf2 => validate_uf2(artifact_path, target.uf2_family_id)?,
    }
    let volume_locator = current_removable_volume_locator()?;
    let mut adapter = NativeFlashAdapter {
        started: Instant::now(),
        firmware_session: None,
        target,
        volume_locator,
    };
    flash_with_adapter(&mut adapter, artifact_path, release, cancelled, progress)
}

trait FlashAdapter {
    fn devices(&mut self) -> Result<Vec<FirmwareDevice>, FirmwareFlashError>;
    fn volumes(&mut self) -> Result<Vec<BootloaderVolume>, FirmwareFlashError>;
    fn firmware_info(
        &mut self,
        endpoint: &BridgeEndpoint,
    ) -> Result<FirmwareInfo, FirmwareFlashError>;
    fn enter_uf2_bootloader(&mut self, endpoint: &BridgeEndpoint)
        -> Result<(), FirmwareFlashError>;
    fn release_device(&mut self) {}
    fn new_receipt(
        &mut self,
        source: FirmwareInstallSource,
    ) -> Result<FirmwareInstallReceipt, FirmwareFlashError>;
    fn record_install_receipt(
        &mut self,
        endpoint: &BridgeEndpoint,
        receipt: FirmwareInstallReceipt,
    ) -> Result<FirmwareInstallReceipt, FirmwareFlashError>;
    fn copy_and_flush(&mut self, source: &Path, destination: &Path) -> Result<(), io::Error>;
    fn elapsed(&self) -> Duration;
    fn wait(&mut self, duration: Duration);
}

struct NativeFlashAdapter {
    started: Instant,
    firmware_session: Option<BridgeOutput>,
    target: &'static FirmwareTargetDescriptor,
    volume_locator: Box<dyn RemovableVolumeLocator>,
}

impl NativeFlashAdapter {
    fn take_firmware_session(
        &mut self,
        endpoint: &BridgeEndpoint,
    ) -> Result<BridgeOutput, FirmwareFlashError> {
        if self
            .firmware_session
            .as_ref()
            .is_some_and(|session| session.endpoint() == endpoint)
        {
            return Ok(self
                .firmware_session
                .take()
                .expect("matching firmware session was present"));
        }
        self.firmware_session = None;
        let mut output = open_firmware(endpoint)?;
        output
            .wait_for_firmware_info(AUTOMATIC_BOOTLOADER_RESPONSE_TIMEOUT)
            .map_err(|error| FirmwareFlashError::Discovery(error.to_string()))?;
        Ok(output)
    }
}

impl FlashAdapter for NativeFlashAdapter {
    fn devices(&mut self) -> Result<Vec<FirmwareDevice>, FirmwareFlashError> {
        discover_firmware_devices(self.target)
    }

    fn volumes(&mut self) -> Result<Vec<BootloaderVolume>, FirmwareFlashError> {
        discover_bootloader_volumes_with(self.volume_locator.as_ref())
    }

    fn firmware_info(
        &mut self,
        endpoint: &BridgeEndpoint,
    ) -> Result<FirmwareInfo, FirmwareFlashError> {
        self.firmware_session = None;
        let mut output = open_firmware(endpoint)?;
        let info = output
            .wait_for_firmware_info(Duration::from_secs(3))
            .map_err(|error| FirmwareFlashError::Discovery(error.to_string()))?;
        self.firmware_session = Some(output);
        Ok(info)
    }

    fn enter_uf2_bootloader(
        &mut self,
        endpoint: &BridgeEndpoint,
    ) -> Result<(), FirmwareFlashError> {
        let mut output = self.take_firmware_session(endpoint)?;
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
        endpoint: &BridgeEndpoint,
        receipt: FirmwareInstallReceipt,
    ) -> Result<FirmwareInstallReceipt, FirmwareFlashError> {
        let mut output = self
            .take_firmware_session(endpoint)
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
    cancelled: &AtomicBool,
    mut progress: impl FnMut(FirmwareFlashProgress),
) -> Result<FirmwareInfo, FirmwareFlashError> {
    let target = firmware_target(&release.target)
        .ok_or_else(|| FirmwareFlashError::UnsupportedTarget(release.target.clone()))?;
    progress(FirmwareFlashProgress::LookingForDevice);
    if cancelled.load(Ordering::Acquire) {
        return Err(FirmwareFlashError::Cancelled);
    }

    let prepared = prepare_flash_target(adapter, target, release, cancelled, &mut progress)?;
    if cancelled.load(Ordering::Acquire) {
        return Err(FirmwareFlashError::CancelledInBootloader);
    }
    progress(FirmwareFlashProgress::Writing);
    let destination = prepared.volume.root.join(&release.artifact.name);
    let copy_result = adapter.copy_and_flush(artifact_path, &destination);

    verify_reconnected_firmware(
        adapter,
        target,
        release,
        prepared.expected_stable_id.as_deref(),
        prepared.pre_flash_state,
        copy_result,
        &mut progress,
    )
}

#[derive(Debug, Clone, Copy)]
enum PreFlashState {
    Unknown,
    FactoryApplication,
    Bridge(FirmwareInfo),
}

impl PreFlashState {
    fn previous_firmware(self) -> Option<FirmwareInfo> {
        match self {
            Self::Bridge(info) => Some(info),
            Self::Unknown | Self::FactoryApplication => None,
        }
    }

    fn proves_fresh_image(self, target: &FirmwareTargetDescriptor, revision: u16) -> bool {
        match self {
            Self::FactoryApplication => true,
            Self::Bridge(previous) if !firmware_matches_target(previous, target) => true,
            Self::Bridge(previous) => match previous.version {
                FirmwareVersion::Reported(previous_revision) if previous_revision != revision => {
                    true
                }
                FirmwareVersion::Reported(_) => {
                    matches!(previous.install_state, FirmwareInstallState::Recorded(_))
                }
                FirmwareVersion::Pending
                | FirmwareVersion::Unreported
                | FirmwareVersion::Malformed
                | FirmwareVersion::UnsupportedFormat(_) => false,
            },
            Self::Unknown => false,
        }
    }
}

struct PreparedFlash {
    volume: BootloaderVolume,
    expected_stable_id: Option<String>,
    pre_flash_state: PreFlashState,
}

fn prepare_flash_target(
    adapter: &mut impl FlashAdapter,
    target: &FirmwareTargetDescriptor,
    release: &FirmwareRelease,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(FirmwareFlashProgress),
) -> Result<PreparedFlash, FirmwareFlashError> {
    let mounted = select_supported_volume(adapter.volumes()?, target)?;
    let devices = adapter.devices()?;
    let mut expected_stable_id = None;
    let mut automatic_entry_may_have_started = false;
    let mut pre_flash_state = PreFlashState::Unknown;
    let volume = if let Some(volume) = mounted {
        if devices.len() > 1
            || devices
                .iter()
                .any(|device| device.kind != FirmwareDeviceKind::Uf2Bootloader)
        {
            return Err(FirmwareFlashError::Discovery(format!(
                "more than one compatible {} device is connected; disconnect extras",
                target.display_name
            )));
        }
        expected_stable_id = devices
            .first()
            .and_then(|device| device.endpoint.stable_id().map(str::to_owned));
        volume
    } else {
        if devices.len() != 1 {
            return Err(FirmwareFlashError::Discovery(
                format!(
                    "connect exactly one compatible {} device, or mount exactly one supported UF2 drive",
                    target.display_name
                ),
            ));
        }
        let device = &devices[0];
        let info = if device.kind == FirmwareDeviceKind::BridgeApplication {
            let info = adapter.firmware_info(&device.endpoint)?;
            // Even an unidentified or different target is valuable evidence
            // after manual recovery: a newly matching target or changed
            // revision proves that the image started despite an expected UF2
            // disconnect error. It still must not authorize automatic entry.
            pre_flash_state = PreFlashState::Bridge(info);
            if firmware_matches_target(info, target) {
                validate_version_policy(info.version, release.revision)?;
                expected_stable_id = device.endpoint.stable_id().map(str::to_owned);
                Some(info)
            } else {
                adapter.release_device();
                None
            }
        } else {
            if device.kind == FirmwareDeviceKind::FactoryApplication {
                pre_flash_state = PreFlashState::FactoryApplication;
            }
            expected_stable_id = device.endpoint.stable_id().map(str::to_owned);
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
            match adapter.enter_uf2_bootloader(&device.endpoint) {
                Ok(()) => {
                    progress(FirmwareFlashProgress::WaitingForBootloader);
                    match wait_for_volume(
                        adapter,
                        cancelled,
                        AUTOMATIC_BOOTLOADER_WAIT_TIMEOUT,
                        true,
                        target,
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
                cancelled,
                MANUAL_BOOTLOADER_WAIT_TIMEOUT,
                automatic_entry_may_have_started,
                target,
            )?
        }
    };
    Ok(PreparedFlash {
        volume,
        expected_stable_id,
        pre_flash_state,
    })
}

fn verify_reconnected_firmware(
    adapter: &mut impl FlashAdapter,
    target: &FirmwareTargetDescriptor,
    release: &FirmwareRelease,
    expected_stable_id: Option<&str>,
    pre_flash_state: PreFlashState,
    copy_result: Result<(), io::Error>,
    progress: &mut impl FnMut(FirmwareFlashProgress),
) -> Result<FirmwareInfo, FirmwareFlashError> {
    let mut copy_error = copy_result.err();
    progress(FirmwareFlashProgress::WaitingForApplication);
    let deadline = adapter.elapsed() + APPLICATION_RECONNECT_TIMEOUT;
    let mut last_revision = None;
    let mut last_discovery_error = None;
    while adapter.elapsed() < deadline {
        adapter.wait(Duration::from_millis(250));
        let devices = match adapter.devices() {
            Ok(devices) => {
                last_discovery_error = None;
                devices
            }
            Err(error) => {
                last_discovery_error = Some(error);
                continue;
            }
        };
        if devices.len() > 1 {
            return Err(FirmwareFlashError::Discovery(format!(
                "more than one compatible {} device reconnected; disconnect extras",
                target.display_name
            )));
        }
        let matching: Vec<_> = devices
            .into_iter()
            .filter(|device| {
                device.kind == FirmwareDeviceKind::BridgeApplication
                    && expected_stable_id
                        .is_none_or(|serial| device.endpoint.stable_id() == Some(serial))
            })
            .collect();
        if matching.len() != 1 {
            continue;
        }
        let endpoint = &matching[0].endpoint;
        match adapter.firmware_info(endpoint) {
            Ok(info)
                if firmware_matches_target(info, target)
                    && info.version == FirmwareVersion::Reported(release.revision) =>
            {
                if info.install_state != FirmwareInstallState::Pending {
                    return Err(FirmwareFlashError::ReceiptExpectedPending(
                        info.install_state,
                    ));
                }
                if copy_error.is_some()
                    && !pre_flash_state.proves_fresh_image(target, release.revision)
                {
                    return Err(copy_error
                        .take()
                        .expect("copy error was checked as present")
                        .into());
                }
                let requested = adapter.new_receipt(FirmwareInstallSource::AppCenter)?;
                if pre_flash_state
                    .previous_firmware()
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
                let acknowledged = adapter.record_install_receipt(endpoint, requested)?;
                if acknowledged != requested {
                    return Err(FirmwareFlashError::ReceiptMismatch);
                }
                progress(FirmwareFlashProgress::VerifyingReceipt);
                let verified = adapter
                    .firmware_info(endpoint)
                    .map_err(|error| FirmwareFlashError::ReceiptRecording(error.to_string()))?;
                if !firmware_matches_target(verified, target)
                    || verified.version != FirmwareVersion::Reported(release.revision)
                    || verified.install_state != FirmwareInstallState::Recorded(requested)
                {
                    return Err(FirmwareFlashError::ReceiptMismatch);
                }
                return Ok(verified);
            }
            Ok(info) if firmware_matches_target(info, target) => {
                if let FirmwareVersion::Reported(revision) = info.version {
                    last_revision = Some(revision);
                }
            }
            Ok(_) | Err(_) => {}
        }
    }
    Err(reconnect_timeout_error(
        copy_error,
        last_discovery_error,
        release.revision,
        last_revision,
    ))
}

fn reconnect_timeout_error(
    copy_error: Option<io::Error>,
    discovery_error: Option<FirmwareFlashError>,
    expected_revision: u16,
    actual_revision: Option<u16>,
) -> FirmwareFlashError {
    if let Some(error) = copy_error {
        return error.into();
    }
    if actual_revision.is_none() {
        if let Some(error) = discovery_error {
            return error;
        }
    }
    FirmwareFlashError::Revision {
        expected: expected_revision,
        actual: actual_revision,
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

fn open_firmware(endpoint: &BridgeEndpoint) -> Result<BridgeOutput, FirmwareFlashError> {
    BridgeOutput::open(endpoint.clone(), BridgeTransportConfig::default())
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
    select_supported_volume(discover_bootloader_volumes(root)?, test_target())
}

#[cfg(test)]
fn test_target() -> &'static FirmwareTargetDescriptor {
    &crate::firmware_targets().expect("embedded target catalog")[0]
}

fn select_supported_volume(
    volumes: Vec<BootloaderVolume>,
    target: &FirmwareTargetDescriptor,
) -> Result<Option<BootloaderVolume>, FirmwareFlashError> {
    if volumes.len() > 1 {
        return Err(FirmwareFlashError::Discovery(
            "more than one UF2 bootloader is mounted; disconnect extras".to_owned(),
        ));
    }
    let Some(volume) = volumes.into_iter().next() else {
        return Ok(None);
    };
    if !supported_board_id(&volume.board_id, target) {
        return Err(FirmwareFlashError::WrongBoard(volume.board_id));
    }
    Ok(Some(volume))
}

fn supported_board_id(board_id: &str, target: &FirmwareTargetDescriptor) -> bool {
    target
        .accepted_board_ids
        .iter()
        .any(|accepted| accepted == board_id)
}

fn wait_for_volume(
    adapter: &mut impl FlashAdapter,
    cancelled: &AtomicBool,
    timeout: Duration,
    entered_bootloader: bool,
    target: &FirmwareTargetDescriptor,
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
        if let Some(volume) = select_supported_volume(adapter.volumes()?, target)? {
            return Ok(volume);
        }
        adapter.wait(Duration::from_millis(250));
    }
    Err(FirmwareFlashError::Timeout(
        target.manual_recovery_timeout.clone(),
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
    use bridge_output::{
        FirmwareTarget, FirmwareTargetId, LinuxUsbLocator, SerialDeviceInfo,
        DEFAULT_BRIDGE_BAUD_RATE,
    };
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
        let target = test_target();
        target.firmware_release(
            revision,
            semver::Version::new(1, 4, 0),
            crate::ArtifactDescriptor {
                name: "firmware.uf2".to_owned(),
                size: 1,
                sha256: "11".repeat(32),
            },
        )
    }

    fn device(bridge_firmware: bool) -> FirmwareDevice {
        FirmwareDevice {
            endpoint: fixture_endpoint(),
            kind: if bridge_firmware {
                FirmwareDeviceKind::BridgeApplication
            } else {
                FirmwareDeviceKind::FactoryApplication
            },
        }
    }

    fn bootloader_device() -> FirmwareDevice {
        FirmwareDevice {
            endpoint: fixture_endpoint(),
            kind: FirmwareDeviceKind::Uf2Bootloader,
        }
    }

    fn raw_bridge_device(stable_id: &str, bus_number: u8, device_address: u8) -> FirmwareDevice {
        FirmwareDevice {
            endpoint: BridgeEndpoint::official_linux_usb(
                LinuxUsbLocator {
                    bus_number,
                    device_address,
                    control_interface: 0,
                    data_interface: 1,
                    xinput_interface: 2,
                    bulk_in_endpoint: 0x82,
                    bulk_out_endpoint: 0x01,
                },
                stable_id,
            ),
            kind: FirmwareDeviceKind::BridgeApplication,
        }
    }

    fn fixture_endpoint() -> BridgeEndpoint {
        BridgeEndpoint::from_serial_device(
            SerialDeviceInfo {
                path: fixture_serial_path().to_owned(),
                vendor_id: Some(0x2886),
                product_id: Some(0x8044),
                serial_number: Some("fixture-serial".to_owned()),
                manufacturer: Some("Seeed".to_owned()),
                product: None,
            },
            DEFAULT_BRIDGE_BAUD_RATE,
        )
        .expect("fixture uses a valid callout endpoint")
    }

    #[cfg(target_os = "macos")]
    fn fixture_serial_path() -> &'static str {
        "/dev/cu.fixture"
    }

    #[cfg(target_os = "linux")]
    fn fixture_serial_path() -> &'static str {
        "/dev/ttyACM42"
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn fixture_serial_path() -> &'static str {
        "fixture-serial-port"
    }

    fn volume() -> BootloaderVolume {
        BootloaderVolume {
            root: PathBuf::from("/Volumes/FIXTURE"),
            board_id: test_target().manifest_board_id.clone(),
        }
    }

    struct FakeAdapter {
        now: Duration,
        devices: VecDeque<Vec<FirmwareDevice>>,
        default_devices: Vec<FirmwareDevice>,
        default_device_error: Option<String>,
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
        copy_requests: usize,
    }

    impl FlashAdapter for FakeAdapter {
        fn devices(&mut self) -> Result<Vec<FirmwareDevice>, FirmwareFlashError> {
            if let Some(devices) = self.devices.pop_front() {
                return Ok(devices);
            }
            if let Some(error) = &self.default_device_error {
                return Err(FirmwareFlashError::Discovery(error.clone()));
            }
            Ok(self.default_devices.clone())
        }

        fn volumes(&mut self) -> Result<Vec<BootloaderVolume>, FirmwareFlashError> {
            Ok(self
                .volumes
                .pop_front()
                .unwrap_or_else(|| self.default_volumes.clone()))
        }

        fn firmware_info(
            &mut self,
            _endpoint: &BridgeEndpoint,
        ) -> Result<FirmwareInfo, FirmwareFlashError> {
            if let Some(info) = self.infos.pop_front() {
                return Ok(info);
            }
            Ok(FirmwareInfo {
                version: self.versions.pop_front().unwrap_or(self.default_version),
                capabilities: self.default_capabilities,
                install_state: self
                    .recorded_receipt
                    .map_or(self.default_install_state, FirmwareInstallState::Recorded),
                target: FirmwareTarget::Reported(test_target().id),
            })
        }

        fn enter_uf2_bootloader(
            &mut self,
            _endpoint: &BridgeEndpoint,
        ) -> Result<(), FirmwareFlashError> {
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
            _endpoint: &BridgeEndpoint,
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
            self.copy_requests += 1;
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
            default_device_error: None,
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
            copy_requests: 0,
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
            target: FirmwareTarget::Reported(test_target().id),
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
    fn unidentified_or_different_targets_never_request_automatic_bootloader_entry() {
        for reported_target in [
            FirmwareTarget::Unreported,
            FirmwareTarget::Malformed,
            FirmwareTarget::Reported(FirmwareTargetId::new("community-nrf52840").unwrap()),
        ] {
            let mut adapter = fake();
            adapter.volumes.push_back(Vec::new());
            adapter.volumes.push_back(Vec::new());
            adapter.volumes.push_back(vec![volume()]);
            adapter.devices.push_back(vec![device(true)]);
            adapter.devices.push_back(vec![device(true)]);
            let mut unidentified = info(
                2,
                FirmwareCapabilities::ENTER_UF2_BOOTLOADER | FirmwareCapabilities::INSTALL_RECEIPT,
                FirmwareInstallState::Recorded(FirmwareInstallReceipt {
                    installed_at: 100,
                    install_id: [1; 16],
                    source: FirmwareInstallSource::FirstObserved,
                }),
            );
            unidentified.target = reported_target;
            adapter.infos.push_back(unidentified);
            adapter.default_version = FirmwareVersion::Reported(3);
            adapter.default_capabilities = FirmwareCapabilities::INSTALL_RECEIPT;
            let mut progress = Vec::new();

            flash_with_adapter(
                &mut adapter,
                Path::new("firmware.uf2"),
                &release(3),
                &AtomicBool::new(false),
                |state| progress.push(state),
            )
            .unwrap();

            assert_eq!(adapter.automatic_entry_requests, 0);
            assert!(!progress.contains(&FirmwareFlashProgress::RequestingBootloader));
            assert!(progress.contains(&FirmwareFlashProgress::ManualRecovery));
        }
    }

    #[test]
    fn targetless_legacy_update_accepts_disconnect_after_verified_revision_three_reconnect() {
        let mut adapter = fake();
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(vec![volume()]);
        adapter.devices.push_back(vec![device(true)]);
        adapter.devices.push_back(vec![device(true)]);
        let mut legacy = info(
            2,
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER | FirmwareCapabilities::INSTALL_RECEIPT,
            FirmwareInstallState::Recorded(FirmwareInstallReceipt {
                installed_at: 100,
                install_id: [1; 16],
                source: FirmwareInstallSource::FirstObserved,
            }),
        );
        legacy.target = FirmwareTarget::Unreported;
        adapter.infos.push_back(legacy);
        adapter.default_version = FirmwareVersion::Reported(3);
        adapter.default_capabilities = FirmwareCapabilities::INSTALL_RECEIPT;
        adapter.copy_error = true;
        let mut progress = Vec::new();

        let verified = flash_with_adapter(
            &mut adapter,
            Path::new("firmware.uf2"),
            &release(3),
            &AtomicBool::new(false),
            |state| progress.push(state),
        )
        .unwrap();

        assert_eq!(adapter.automatic_entry_requests, 0);
        assert!(progress.contains(&FirmwareFlashProgress::ManualRecovery));
        assert_eq!(verified.version, FirmwareVersion::Reported(3));
        assert!(matches!(
            verified.install_state,
            FirmwareInstallState::Recorded(FirmwareInstallReceipt {
                source: FirmwareInstallSource::AppCenter,
                ..
            })
        ));
    }

    #[test]
    fn reconnect_verification_rejects_a_different_target_at_the_expected_revision() {
        let mut adapter = fake();
        adapter.volumes.push_back(vec![volume()]);
        adapter.devices.push_back(vec![bootloader_device()]);
        adapter.default_devices = vec![device(true)];
        let mut different = info(
            3,
            FirmwareCapabilities::INSTALL_RECEIPT,
            FirmwareInstallState::Pending,
        );
        different.target =
            FirmwareTarget::Reported(FirmwareTargetId::new("community-nrf52840").unwrap());
        adapter.infos.push_back(different);

        assert!(matches!(
            flash_with_adapter(
                &mut adapter,
                Path::new("firmware.uf2"),
                &release(3),
                &AtomicBool::new(false),
                |_| {},
            ),
            Err(FirmwareFlashError::Revision {
                expected: 3,
                actual: None
            })
        ));
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
                &AtomicBool::new(false),
                |_| {},
            ),
            Err(FirmwareFlashError::Io(_))
        ));
    }

    #[test]
    fn factory_install_accepts_disconnect_during_final_flush_after_verified_reconnect() {
        let mut adapter = fake();
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(vec![volume()]);
        adapter.devices.push_back(vec![device(false)]);
        adapter.devices.push_back(vec![device(true)]);
        adapter.infos.push_back(info(
            2,
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER | FirmwareCapabilities::INSTALL_RECEIPT,
            FirmwareInstallState::Pending,
        ));
        adapter.default_version = FirmwareVersion::Reported(2);
        adapter.default_capabilities =
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER | FirmwareCapabilities::INSTALL_RECEIPT;
        adapter.copy_error = true;

        let verified = flash_with_adapter(
            &mut adapter,
            Path::new("firmware.uf2"),
            &release(2),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();

        assert_eq!(verified.version, FirmwareVersion::Reported(2));
        assert!(matches!(
            verified.install_state,
            FirmwareInstallState::Recorded(FirmwareInstallReceipt {
                source: FirmwareInstallSource::AppCenter,
                ..
            })
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
                &AtomicBool::new(true),
                |_| {},
            ),
            Err(FirmwareFlashError::Cancelled)
        ));
    }

    #[test]
    fn validates_every_uf2_block_and_family() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("firmware.uf2");
        let family = test_target().uf2_family_id;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&uf2_block(family));
        bytes.extend_from_slice(&uf2_block(family));
        fs::write(&path, &bytes).unwrap();
        validate_uf2(&path, family).unwrap();
        bytes[28] ^= 1;
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            validate_uf2(&path, family),
            Err(FirmwareFlashError::InvalidUf2(_))
        ));
    }

    #[test]
    fn bootloader_identity_comes_from_info_file_not_volume_name() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::create_dir_all(root.join("anything")).unwrap();
        fs::write(
            root.join("anything/INFO_UF2.TXT"),
            format!(
                "UF2 Bootloader\nBoard-ID: {}\n",
                test_target().manifest_board_id
            ),
        )
        .unwrap();
        assert_eq!(
            discover_bootloader_volumes(root).unwrap(),
            vec![BootloaderVolume {
                root: root.join("anything"),
                board_id: test_target().manifest_board_id.clone(),
            }]
        );
    }

    #[test]
    fn standard_and_sense_bootloader_boards_are_supported() {
        for board_id in &test_target().accepted_board_ids {
            assert!(select_supported_volume(
                vec![BootloaderVolume {
                    root: PathBuf::from("/Volumes/XIAO"),
                    board_id: board_id.clone(),
                }],
                test_target(),
            )
            .unwrap()
            .is_some());
        }
    }

    #[test]
    fn wrong_and_multiple_bootloader_boards_are_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::create_dir_all(root.join("wrong")).unwrap();
        fs::write(
            root.join("wrong/INFO_UF2.TXT"),
            "Board-ID: incompatible_board\n",
        )
        .unwrap();
        assert!(matches!(
            supported_volume(root),
            Err(FirmwareFlashError::WrongBoard(_))
        ));
        fs::create_dir_all(root.join("plain")).unwrap();
        fs::write(
            root.join("plain/INFO_UF2.TXT"),
            format!("Board-ID: {}\n", test_target().manifest_board_id),
        )
        .unwrap();
        assert!(matches!(
            supported_volume(root),
            Err(FirmwareFlashError::Discovery(_))
        ));
    }

    #[test]
    fn wrong_bootloader_board_is_rejected_before_any_write() {
        let mut adapter = fake();
        adapter.volumes.push_back(vec![BootloaderVolume {
            root: PathBuf::from("/Volumes/WRONG"),
            board_id: "community_board".to_owned(),
        }]);

        assert!(matches!(
            flash_with_adapter(
                &mut adapter,
                Path::new("firmware.uf2"),
                &release(3),
                &AtomicBool::new(false),
                |_| {},
            ),
            Err(FirmwareFlashError::WrongBoard(board)) if board == "community_board"
        ));
        assert_eq!(adapter.copy_requests, 0);
        assert_eq!(adapter.automatic_entry_requests, 0);
    }

    #[test]
    fn target_usb_identity_classification_accepts_standard_and_sense_ids() {
        for (product_id, expected) in [
            (0x8044, FirmwareDeviceKind::FactoryApplication),
            (0x0044, FirmwareDeviceKind::Uf2Bootloader),
            (0x8045, FirmwareDeviceKind::FactoryApplication),
            (0x0045, FirmwareDeviceKind::Uf2Bootloader),
        ] {
            assert_eq!(
                target_usb_device_kind(
                    crate::UsbIdentity {
                        vendor_id: 0x2886,
                        product_id,
                    },
                    test_target(),
                ),
                Some(expected)
            );
        }
    }

    #[test]
    fn updater_classifies_raw_bridge_and_factory_serial_endpoints() {
        let raw = raw_bridge_device("raw-bridge", 3, 4).endpoint;
        let factory = fixture_endpoint();

        let devices = classify_firmware_devices(vec![raw.clone(), factory.clone()], test_target());

        assert_eq!(
            devices,
            vec![
                FirmwareDevice {
                    endpoint: raw,
                    kind: FirmwareDeviceKind::BridgeApplication,
                },
                FirmwareDevice {
                    endpoint: factory,
                    kind: FirmwareDeviceKind::FactoryApplication,
                },
            ]
        );
    }

    #[test]
    fn raw_reconnect_tracks_stable_identity_not_ephemeral_usb_address() {
        let previous = info(
            2,
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER | FirmwareCapabilities::INSTALL_RECEIPT,
            FirmwareInstallState::Recorded(FirmwareInstallReceipt {
                installed_at: 100,
                install_id: [1; 16],
                source: FirmwareInstallSource::FirstObserved,
            }),
        );

        let mut matching = fake();
        matching.volumes.push_back(Vec::new());
        matching.volumes.push_back(vec![volume()]);
        matching
            .devices
            .push_back(vec![raw_bridge_device("stable", 3, 4)]);
        matching
            .devices
            .push_back(vec![raw_bridge_device("stable", 7, 12)]);
        matching.infos.push_back(previous);
        matching.default_version = FirmwareVersion::Reported(3);
        matching.default_capabilities = FirmwareCapabilities::INSTALL_RECEIPT;

        assert!(flash_with_adapter(
            &mut matching,
            Path::new("firmware.uf2"),
            &release(3),
            &AtomicBool::new(false),
            |_| {},
        )
        .is_ok());

        let mut different = fake();
        different.volumes.push_back(Vec::new());
        different.volumes.push_back(vec![volume()]);
        different
            .devices
            .push_back(vec![raw_bridge_device("stable", 3, 4)]);
        different
            .devices
            .push_back(vec![raw_bridge_device("other", 3, 4)]);
        different.infos.push_back(previous);
        different.default_version = FirmwareVersion::Reported(3);
        different.default_capabilities = FirmwareCapabilities::INSTALL_RECEIPT;

        assert!(matches!(
            flash_with_adapter(
                &mut different,
                Path::new("firmware.uf2"),
                &release(3),
                &AtomicBool::new(false),
                |_| {},
            ),
            Err(FirmwareFlashError::Revision {
                expected: 3,
                actual: None
            })
        ));
    }

    #[test]
    fn persistent_post_flash_discovery_failure_keeps_its_diagnostic() {
        let mut adapter = fake();
        adapter.volumes.push_back(Vec::new());
        adapter.volumes.push_back(vec![volume()]);
        adapter
            .devices
            .push_back(vec![raw_bridge_device("stable", 3, 4)]);
        adapter.infos.push_back(info(
            2,
            FirmwareCapabilities::ENTER_UF2_BOOTLOADER,
            FirmwareInstallState::Recorded(FirmwareInstallReceipt {
                installed_at: 100,
                install_id: [1; 16],
                source: FirmwareInstallSource::FirstObserved,
            }),
        ));
        adapter.default_device_error = Some("raw USB access denied".to_owned());

        assert!(matches!(
            flash_with_adapter(
                &mut adapter,
                Path::new("firmware.uf2"),
                &release(3),
                &AtomicBool::new(false),
                |_| {},
            ),
            Err(FirmwareFlashError::Discovery(message)) if message == "raw USB access denied"
        ));
    }

    #[test]
    fn target_device_detection_rejects_an_ineligible_endpoint() {
        let device = SerialDeviceInfo {
            path: String::new(),
            vendor_id: Some(0x2886),
            product_id: Some(0x8044),
            serial_number: None,
            manufacturer: Some("Seeed".to_owned()),
            product: None,
        };

        assert_eq!(
            BridgeEndpoint::from_serial_device(device, DEFAULT_BRIDGE_BAUD_RATE),
            None
        );
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
