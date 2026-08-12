use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use release_updater::{
    ensure_release_artifact_cancellable, installed_macos_version, refresh_catalog_cancellable,
    stage_application, CatalogRefresh, CatalogRefreshPolicy, FirmwareFlashProgress,
    FirmwareTargetDescriptor, ReleaseManifestV1,
};

use crate::update_check::{running_version, UpdateContext};

pub(super) fn fetch_catalog(
    context: &UpdateContext,
    cancellation: &Arc<AtomicBool>,
    policy: CatalogRefreshPolicy,
) -> Result<CatalogRefresh, String> {
    refresh_catalog_cancellable(
        context.source(),
        context.cache(),
        context.keys(),
        policy,
        &running_version(),
        Some(cancellation),
    )
    .map_err(|error| error.to_string())
}

pub(super) fn download_and_stage_application(
    context: &UpdateContext,
    manifest: &ReleaseManifestV1,
    cancellation: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    if cancellation.load(Ordering::Acquire) {
        return Err("application update cancelled".to_owned());
    }
    if installed_macos_version().map_err(|error| error.to_string())? < manifest.minimum_macos {
        return Err(format!(
            "This release requires macOS {} or newer.",
            manifest.minimum_macos
        ));
    }
    let artifact = &manifest.application.artifact;
    let path = ensure_release_artifact_cancellable(
        context.source(),
        context.cache(),
        &manifest.release_tag,
        artifact,
        Some(cancellation),
    )
    .map_err(|error| error.to_string())?;
    if cancellation.load(Ordering::Acquire) {
        return Err("application update cancelled".to_owned());
    }
    let staged = stage_application(
        &path,
        &manifest.application,
        &context.cache().root().join("staged-app"),
    )
    .map_err(|error| error.to_string())?;
    Ok(staged.bundle_path)
}

pub(super) fn download_firmware(
    context: &UpdateContext,
    manifest: &ReleaseManifestV1,
    cancellation: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    let artifact = &manifest.firmware.artifact;
    ensure_release_artifact_cancellable(
        context.source(),
        context.cache(),
        &manifest.release_tag,
        artifact,
        Some(cancellation),
    )
    .map_err(|error| error.to_string())
}

pub(super) fn progress_text(
    progress: &FirmwareFlashProgress,
    target: Option<&FirmwareTargetDescriptor>,
) -> String {
    let target_name = target.map_or("supported firmware target", |target| {
        target.display_name.as_str()
    });
    match progress {
        FirmwareFlashProgress::LookingForDevice => {
            format!("Looking for one compatible {target_name} device…")
        }
        FirmwareFlashProgress::RequestingBootloader => {
            "Requesting automatic UF2 bootloader mode…".to_owned()
        }
        FirmwareFlashProgress::WaitingForBootloader => {
            format!("Waiting for the automatic {target_name} UF2 drive…")
        }
        FirmwareFlashProgress::ManualRecovery => target.map_or_else(
            || "Automatic entry is unavailable. Follow the supported target's manual bootloader recovery instructions while this recovery window is open…".to_owned(),
            |target| format!("Automatic entry is unavailable. {}…", target.manual_recovery),
        ),
        FirmwareFlashProgress::Writing => {
            "Writing firmware. Do not unplug the board…".to_owned()
        }
        FirmwareFlashProgress::WaitingForApplication => {
            "Waiting for the flashed device to reconnect…".to_owned()
        }
        FirmwareFlashProgress::RecordingReceipt => {
            "Firmware started. Recording the verified installation receipt…".to_owned()
        }
        FirmwareFlashProgress::VerifyingReceipt => {
            "Reading the committed installation receipt back from the board…".to_owned()
        }
    }
}
