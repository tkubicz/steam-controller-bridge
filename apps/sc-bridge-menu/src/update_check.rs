use std::fs;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use bridge_runtime::FirmwareVersion;
use release_updater::{
    embedded_trusted_keys, refresh_catalog_if_due, LatestReleaseClient, ReleaseCache,
    ReleaseManifestV1,
};
use semver::Version;

const CHECK_INTERVAL: Duration = Duration::from_hours(24);

pub struct UpdateChecker {
    manifest: Option<ReleaseManifestV1>,
    result: Option<Receiver<Result<ReleaseManifestV1, String>>>,
    running_version: Version,
}

impl UpdateChecker {
    pub fn new() -> Self {
        let Ok(keys) = embedded_trusted_keys() else {
            return Self {
                manifest: None,
                result: None,
                running_version: running_version(),
            };
        };
        if keys.is_empty() {
            return Self {
                manifest: None,
                result: None,
                running_version: running_version(),
            };
        }
        let Ok(cache) = ReleaseCache::for_current_user() else {
            return Self {
                manifest: None,
                result: None,
                running_version: running_version(),
            };
        };
        let manifest = cache.load_manifest(&keys).ok();
        if manifest.as_ref().is_some_and(|manifest| {
            Version::parse(env!("CARGO_PKG_VERSION"))
                .is_ok_and(|running| running >= manifest.application_version)
        }) {
            let _ = fs::remove_dir_all(cache.root().join("staged-app"));
            if let Some(application) = manifest.as_ref().map(|item| &item.application.artifact) {
                let _ = fs::remove_file(cache.artifact_path(application));
            }
        }
        if !cache.check_due(CHECK_INTERVAL) {
            return Self {
                manifest,
                result: None,
                running_version: running_version(),
            };
        }
        let (sender, result) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let checked =
                refresh_catalog_if_due(&LatestReleaseClient, &cache, &keys, CHECK_INTERVAL);
            let _ = sender.send(checked);
        });
        Self {
            manifest,
            result: Some(result),
            running_version: running_version(),
        }
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
            && match firmware {
                FirmwareVersion::Reported(revision) => revision < manifest.firmware.revision,
                FirmwareVersion::Unreported | FirmwareVersion::Malformed => true,
                FirmwareVersion::Pending | FirmwareVersion::UnsupportedFormat(_) => false,
            }
    }
}

fn running_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is semver")
}
