use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use release_updater::{
    ensure_release_artifact, installed_macos_version, refresh_catalog_if_due, stage_application,
    CatalogRefresh, FirmwareFlashProgress, ReleaseManifestV1,
};

use crate::update_check::{running_version, UpdateContext};

pub(super) fn fetch_catalog(
    context: &UpdateContext,
    cancellation: &Arc<AtomicBool>,
) -> Result<CatalogRefresh, String> {
    let source = context.source(Some(Arc::clone(cancellation)));
    refresh_catalog_if_due(
        source.as_ref(),
        context.cache(),
        context.keys(),
        context.check_interval(),
        &running_version(),
    )
}

pub(super) fn download_and_stage_application(
    context: &UpdateContext,
    manifest: &ReleaseManifestV1,
    cancellation: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    if cancellation.load(Ordering::Acquire) {
        return Err("application update cancelled".to_owned());
    }
    if installed_macos_version()? < manifest.minimum_macos {
        return Err(format!(
            "This release requires macOS {} or newer.",
            manifest.minimum_macos
        ));
    }
    let source = context.source(Some(Arc::clone(cancellation)));
    let artifact = &manifest.application.artifact;
    let path = ensure_release_artifact(
        source.as_ref(),
        context.cache(),
        &manifest.release_tag,
        artifact,
    )?;
    if cancellation.load(Ordering::Acquire) {
        return Err("application update cancelled".to_owned());
    }
    let staged = stage_application(
        &path,
        &manifest.application,
        &context.cache().root().join("staged-app"),
    )?;
    Ok(staged.bundle_path)
}

pub(super) fn download_firmware(
    context: &UpdateContext,
    manifest: &ReleaseManifestV1,
    cancellation: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    let source = context.source(Some(Arc::clone(cancellation)));
    let artifact = &manifest.firmware.artifact;
    ensure_release_artifact(
        source.as_ref(),
        context.cache(),
        &manifest.release_tag,
        artifact,
    )
}

pub(super) fn progress_text(progress: &FirmwareFlashProgress) -> &'static str {
    match progress {
        FirmwareFlashProgress::LookingForDevice => "Looking for one compatible XIAO…",
        FirmwareFlashProgress::RequestingBootloader => {
            "Requesting automatic UF2 bootloader mode…"
        }
        FirmwareFlashProgress::WaitingForBootloader => {
            "Waiting for the automatic XIAO UF2 drive…"
        }
        FirmwareFlashProgress::ManualRecovery => {
            "Automatic entry is unavailable. Bridge the underside RST and GND pads twice while this recovery window is open…"
        }
        FirmwareFlashProgress::Writing => "Writing firmware. Do not unplug the board…",
        FirmwareFlashProgress::WaitingForApplication => {
            "Waiting for the flashed device to reconnect…"
        }
        FirmwareFlashProgress::RecordingReceipt => {
            "Firmware started. Recording the verified installation receipt…"
        }
        FirmwareFlashProgress::VerifyingReceipt => {
            "Reading the committed installation receipt back from the board…"
        }
    }
}
