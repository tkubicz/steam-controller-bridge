use std::fs;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use bridge_runtime::FirmwareVersion;
use release_updater::{
    classify_firmware_release, embedded_trusted_keys, refresh_catalog_if_due, ArtifactDescriptor,
    FirmwareReleaseState, LatestReleaseClient, ReleaseCache, ReleaseManifestV1, TrustedPublicKey,
};
use semver::Version;

pub const CHECK_INTERVAL: Duration = Duration::from_hours(24);

/// The embedded trust anchors and per-user cache every update path shares, or
/// the user-facing reason updates cannot work in this build.
pub fn update_context() -> Result<(Vec<TrustedPublicKey>, ReleaseCache), String> {
    let keys = embedded_trusted_keys().map_err(|error| error.to_string())?;
    if keys.is_empty() {
        return Err(
            "Secure updates are unavailable in this source build: no release public key is embedded."
                .to_owned(),
        );
    }
    let cache = ReleaseCache::for_current_user().map_err(|error| error.to_string())?;
    Ok((keys, cache))
}

pub struct UpdateChecker {
    manifest: Option<ReleaseManifestV1>,
    result: Option<Receiver<Result<ReleaseManifestV1, String>>>,
    running_version: Version,
}

impl UpdateChecker {
    pub fn new() -> Self {
        let mut checker = Self {
            manifest: None,
            result: None,
            running_version: running_version(),
        };
        let Ok((keys, cache)) = update_context() else {
            return checker;
        };
        checker.manifest = cache.load_manifest(&keys).ok();
        let obsolete_application = checker
            .manifest
            .as_ref()
            .filter(|manifest| checker.running_version >= manifest.application_version)
            .map(|manifest| manifest.application.artifact.clone());
        if !cache.check_due(CHECK_INTERVAL) {
            if let Some(artifact) = obsolete_application {
                thread::spawn(move || remove_obsolete_application_cache(&cache, &artifact));
            }
            return checker;
        }
        let (sender, result) = mpsc::sync_channel(1);
        thread::spawn(move || {
            if let Some(artifact) = obsolete_application {
                remove_obsolete_application_cache(&cache, &artifact);
            }
            let _ = sender.send(refresh_catalog_if_due(
                &LatestReleaseClient,
                &cache,
                &keys,
                CHECK_INTERVAL,
            ));
        });
        checker.result = Some(result);
        checker
    }

    pub fn poll(&mut self) {
        let Some(result) = self.result.as_ref() else {
            return;
        };
        match result.try_recv() {
            Ok(Ok(manifest)) => {
                self.manifest = Some(manifest);
                self.result = None;
            }
            Ok(Err(error)) => {
                eprintln!("level=warn event=automatic_update_check_failed message={error:?}");
                self.result = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => self.result = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    pub fn available(&self, firmware: FirmwareVersion) -> bool {
        let Some(manifest) = &self.manifest else {
            return false;
        };
        if self.running_version < manifest.application_version {
            return true;
        }
        self.running_version >= manifest.firmware.minimum_application_version
            && classify_firmware_release(firmware, manifest.firmware.revision)
                == FirmwareReleaseState::UpdateAvailable
    }
}

fn remove_obsolete_application_cache(cache: &ReleaseCache, artifact: &ArtifactDescriptor) {
    let _ = fs::remove_dir_all(cache.root().join("staged-app"));
    let _ = fs::remove_file(cache.artifact_path(artifact));
}

pub(crate) fn running_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is semver")
}
