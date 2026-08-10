use std::path::PathBuf;

use release_updater::{
    ensure_release_artifact, installed_macos_version, refresh_catalog_if_due, stage_application,
    FirmwareFlashProgress, LatestReleaseClient, ReleaseCache, ReleaseManifestV1,
};

use crate::update_check::{update_context, CHECK_INTERVAL};

fn cache() -> Result<ReleaseCache, String> {
    ReleaseCache::for_current_user().map_err(|error| error.to_string())
}

pub(super) fn fetch_catalog() -> Result<ReleaseManifestV1, String> {
    let (keys, cache) = update_context()?;
    refresh_catalog_if_due(&LatestReleaseClient, &cache, &keys, CHECK_INTERVAL)
}

pub(super) fn download_and_stage_application(
    manifest: &ReleaseManifestV1,
) -> Result<PathBuf, String> {
    if installed_macos_version()? < manifest.minimum_macos {
        return Err(format!(
            "This release requires macOS {} or newer.",
            manifest.minimum_macos
        ));
    }
    let cache = cache()?;
    let artifact = &manifest.application.artifact;
    let path = ensure_release_artifact(
        &LatestReleaseClient,
        &cache,
        &manifest.release_tag,
        artifact,
    )?;
    let staged = stage_application(
        &path,
        &manifest.application,
        &cache.root().join("staged-app"),
    )?;
    Ok(staged.bundle_path)
}

pub(super) fn download_firmware(manifest: &ReleaseManifestV1) -> Result<PathBuf, String> {
    let cache = cache()?;
    let artifact = &manifest.firmware.artifact;
    ensure_release_artifact(
        &LatestReleaseClient,
        &cache,
        &manifest.release_tag,
        artifact,
    )
}

pub(super) fn progress_text(progress: &FirmwareFlashProgress) -> &'static str {
    match progress {
        FirmwareFlashProgress::LookingForDevice => "Looking for one compatible XIAO…",
        FirmwareFlashProgress::EnteringBootloader => "Entering the UF2 bootloader…",
        FirmwareFlashProgress::WaitingForBootloader => {
            "Waiting for bootloader. Double-tap RESET if needed…"
        }
        FirmwareFlashProgress::Writing => "Writing firmware. Do not unplug the board…",
        FirmwareFlashProgress::WaitingForApplication => {
            "Waiting for the flashed device to reconnect…"
        }
        FirmwareFlashProgress::Verifying => "Verifying the reported firmware revision…",
    }
}
