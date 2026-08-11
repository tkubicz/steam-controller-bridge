use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use release_updater::{
    ensure_release_artifact, installed_macos_version, refresh_catalog_if_due, stage_application,
    CatalogRefresh, FirmwareFlashProgress, LatestReleaseClient, ReleaseCache, ReleaseManifestV1,
};

use crate::update_check::{running_version, update_context, CHECK_INTERVAL};

fn cache() -> Result<ReleaseCache, String> {
    ReleaseCache::for_current_user().map_err(|error| error.to_string())
}

pub(super) fn fetch_catalog(cancellation: &Arc<AtomicBool>) -> Result<CatalogRefresh, String> {
    let (keys, cache) = update_context()?;
    refresh_catalog_if_due(
        &LatestReleaseClient::cancellable(Arc::clone(cancellation)),
        &cache,
        &keys,
        CHECK_INTERVAL,
        &running_version(),
    )
}

pub(super) fn download_and_stage_application(
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
    let cache = cache()?;
    let artifact = &manifest.application.artifact;
    let path = ensure_release_artifact(
        &LatestReleaseClient::cancellable(Arc::clone(cancellation)),
        &cache,
        &manifest.release_tag,
        artifact,
    )?;
    if cancellation.load(Ordering::Acquire) {
        return Err("application update cancelled".to_owned());
    }
    let staged = stage_application(
        &path,
        &manifest.application,
        &cache.root().join("staged-app"),
    )?;
    Ok(staged.bundle_path)
}

pub(super) fn download_firmware(
    manifest: &ReleaseManifestV1,
    cancellation: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    let cache = cache()?;
    let artifact = &manifest.firmware.artifact;
    ensure_release_artifact(
        &LatestReleaseClient::cancellable(Arc::clone(cancellation)),
        &cache,
        &manifest.release_tag,
        artifact,
    )
}

pub(super) fn progress_text(progress: &FirmwareFlashProgress) -> &'static str {
    match progress {
        FirmwareFlashProgress::LookingForDevice => "Looking for one compatible XIAO…",
        FirmwareFlashProgress::WaitingForBootloader => {
            "Bridge the underside RST and GND pads twice now. Waiting for the XIAO UF2 drive…"
        }
        FirmwareFlashProgress::Writing => "Writing firmware. Do not unplug the board…",
        FirmwareFlashProgress::WaitingForApplication => {
            "Waiting for the flashed device to reconnect…"
        }
        FirmwareFlashProgress::Verifying => "Verifying the reported firmware revision…",
    }
}
