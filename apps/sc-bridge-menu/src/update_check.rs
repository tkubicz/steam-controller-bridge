use std::fs;
#[cfg(debug_assertions)]
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
#[cfg(debug_assertions)]
use std::sync::Once;
use std::thread;
use std::time::Duration;

use bridge_runtime::FirmwareInfo;
#[cfg(debug_assertions)]
use release_updater::LocalReleaseClient;
use release_updater::{
    classify_firmware_release, embedded_trusted_keys, firmware_matches_target, firmware_target,
    firmware_targets, refresh_catalog, ArtifactDescriptor, CatalogRefresh, CatalogRefreshError,
    CatalogRefreshPolicy, FirmwareRelease, FirmwareReleaseState, LatestReleaseClient, ReleaseCache,
    ReleaseManifestV1, ReleaseSource, TrustedPublicKey,
};
use semver::Version;

const CHECK_INTERVAL: Duration = Duration::from_hours(24);
#[cfg(debug_assertions)]
const LOCAL_UPDATE_DIRECTORY_ENV: &str = "SC_BRIDGE_LOCAL_UPDATE_DIR";
#[cfg(debug_assertions)]
static LOCAL_UPDATE_NOTICE: Once = Once::new();

#[derive(Clone)]
enum UpdateChannel {
    Production(LatestReleaseClient),
    #[cfg(debug_assertions)]
    Local(LocalReleaseClient),
}

#[derive(Clone)]
pub(crate) struct UpdateContext {
    keys: Vec<TrustedPublicKey>,
    cache: ReleaseCache,
    channel: UpdateChannel,
}

impl UpdateContext {
    pub(crate) fn source(&self) -> &dyn ReleaseSource {
        match &self.channel {
            UpdateChannel::Production(source) => source,
            #[cfg(debug_assertions)]
            UpdateChannel::Local(source) => source,
        }
    }

    pub(crate) fn keys(&self) -> &[TrustedPublicKey] {
        &self.keys
    }

    pub(crate) fn cache(&self) -> &ReleaseCache {
        &self.cache
    }

    pub(crate) fn check_interval(&self) -> Duration {
        match &self.channel {
            UpdateChannel::Production(_) => CHECK_INTERVAL,
            #[cfg(debug_assertions)]
            UpdateChannel::Local(_) => Duration::ZERO,
        }
    }

    pub(crate) fn is_local(&self) -> bool {
        match &self.channel {
            UpdateChannel::Production(_) => false,
            #[cfg(debug_assertions)]
            UpdateChannel::Local(_) => true,
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn local_root(&self) -> Option<&Path> {
        match &self.channel {
            UpdateChannel::Production(_) => None,
            UpdateChannel::Local(source) => Some(source.root()),
        }
    }
}

impl UpdateChannel {
    fn configured() -> Result<Self, String> {
        #[cfg(debug_assertions)]
        if let Some(root) = development_update_source() {
            let source = LocalReleaseClient::new(&root).map_err(|error| {
                format!(
                    "{LOCAL_UPDATE_DIRECTORY_ENV}={} cannot be used: {error}",
                    root.display()
                )
            })?;
            LOCAL_UPDATE_NOTICE.call_once(|| {
                eprintln!(
                    "level=warn event=local_update_source root={}",
                    source.root().display()
                );
            });
            return Ok(Self::Local(source));
        }
        Ok(Self::Production(
            LatestReleaseClient::new().map_err(|error| error.to_string())?,
        ))
    }

    fn cache(&self) -> Result<ReleaseCache, String> {
        match self {
            Self::Production(_) => {
                ReleaseCache::for_current_user().map_err(|error| error.to_string())
            }
            #[cfg(debug_assertions)]
            Self::Local(source) => Ok(ReleaseCache::for_local_source(source.root())),
        }
    }
}

/// The embedded trust anchors and selected release source, or the user-facing
/// reason updates cannot work in this build.
pub(crate) fn update_context() -> Result<UpdateContext, String> {
    let channel = UpdateChannel::configured()?;
    firmware_targets().map_err(|error| error.to_string())?;
    let keys = embedded_trusted_keys().map_err(|error| error.to_string())?;
    if keys.is_empty() {
        let message = match channel {
            #[cfg(debug_assertions)]
            UpdateChannel::Local(_) => {
                "Local updates require a trusted development key in SC_BRIDGE_UPDATE_PUBLIC_KEYS."
            }
            UpdateChannel::Production(_) => {
                "Secure updates are unavailable in this source build: no release public key is embedded."
            }
        };
        return Err(message.to_owned());
    }
    let cache = channel.cache()?;
    Ok(UpdateContext {
        keys,
        cache,
        channel,
    })
}

#[cfg(debug_assertions)]
pub(crate) fn development_update_source() -> Option<PathBuf> {
    std::env::var_os(LOCAL_UPDATE_DIRECTORY_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub struct UpdateChecker {
    manifest: Option<ReleaseManifestV1>,
    result: Option<Receiver<Result<CatalogRefresh, CatalogRefreshError>>>,
    running_version: Version,
}

impl UpdateChecker {
    pub fn new() -> Self {
        let mut checker = Self {
            manifest: None,
            result: None,
            running_version: running_version(),
        };
        let Ok(context) = update_context() else {
            return checker;
        };
        checker.manifest = context.cache().load_manifest(context.keys()).ok();
        let obsolete_application = checker
            .manifest
            .as_ref()
            .filter(|manifest| checker.running_version >= manifest.application_version)
            .map(|manifest| manifest.application.artifact.clone());
        if !context
            .cache()
            .check_due(context.check_interval(), &checker.running_version)
        {
            if let Some(artifact) = obsolete_application {
                let cache = context.cache().clone();
                thread::spawn(move || remove_obsolete_application_cache(&cache, &artifact));
            }
            return checker;
        }
        let running_version = checker.running_version.clone();
        let (sender, result) = mpsc::sync_channel(1);
        thread::spawn(move || {
            if let Some(artifact) = obsolete_application {
                remove_obsolete_application_cache(context.cache(), &artifact);
            }
            let source = context.source();
            let _ = sender.send(refresh_catalog(
                source,
                context.cache(),
                context.keys(),
                CatalogRefreshPolicy::IfDue(context.check_interval()),
                &running_version,
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
            Ok(Ok(CatalogRefresh::Current(manifest))) => {
                self.manifest = Some(manifest);
                self.result = None;
            }
            Ok(Ok(CatalogRefresh::Stale {
                manifest,
                refresh_error,
            })) => {
                eprintln!(
                    "level=warn event=automatic_update_check_stale message={refresh_error:?}"
                );
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

    pub fn available(&self, firmware: Option<FirmwareInfo>) -> bool {
        let Some(manifest) = &self.manifest else {
            return false;
        };
        if self.running_version < manifest.application_version {
            return true;
        }
        firmware_update_available(&self.running_version, &manifest.firmware, firmware)
    }
}

fn firmware_update_available(
    running_version: &Version,
    release: &FirmwareRelease,
    firmware: Option<FirmwareInfo>,
) -> bool {
    let target = firmware_target(&release.target);
    let Some((firmware, target)) = firmware.zip(target) else {
        return false;
    };
    running_version >= &release.minimum_application_version
        && firmware_matches_target(firmware, target)
        && classify_firmware_release(firmware.version, release.revision)
            == FirmwareReleaseState::UpdateAvailable
}

pub(crate) fn remove_obsolete_application_cache(
    cache: &ReleaseCache,
    artifact: &ArtifactDescriptor,
) {
    let _ = fs::remove_dir_all(cache.root().join("staged-app"));
    let _ = fs::remove_file(cache.artifact_path(artifact));
}

pub(crate) fn running_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("package version is semver")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_runtime::{FirmwareTarget, FirmwareTargetId, FirmwareVersion};
    use release_updater::{firmware_targets, ArtifactDescriptor};

    fn release() -> FirmwareRelease {
        let target = &firmware_targets().unwrap()[0];
        target.firmware_release(
            3,
            Version::new(1, 6, 0),
            ArtifactDescriptor {
                name: "firmware.uf2".to_owned(),
                size: 1,
                sha256: "11".repeat(32),
            },
        )
    }

    fn firmware(target: FirmwareTarget, revision: u16) -> FirmwareInfo {
        FirmwareInfo {
            target,
            version: FirmwareVersion::Reported(revision),
            ..FirmwareInfo::default()
        }
    }

    #[test]
    fn firmware_notification_requires_an_exact_catalog_target_match() {
        let release = release();
        let app = Version::new(1, 6, 0);
        let matching = FirmwareTarget::Reported(FirmwareTargetId::new(&release.target).unwrap());
        assert!(firmware_update_available(
            &app,
            &release,
            Some(firmware(matching, 2))
        ));
        assert!(!firmware_update_available(
            &app,
            &release,
            Some(firmware(FirmwareTarget::Unreported, 2))
        ));
        assert!(!firmware_update_available(
            &app,
            &release,
            Some(firmware(FirmwareTarget::Malformed, 2))
        ));
        assert!(!firmware_update_available(
            &app,
            &release,
            Some(firmware(
                FirmwareTarget::Reported(FirmwareTargetId::new("community-nrf52840").unwrap()),
                2,
            ))
        ));
        assert!(!firmware_update_available(
            &app,
            &release,
            Some(firmware(matching, 3))
        ));
    }

    #[test]
    fn obsolete_application_cleanup_removes_staging_and_the_old_artifact() {
        let directory = tempfile::tempdir().unwrap();
        let cache = ReleaseCache::new(directory.path().to_owned());
        let artifact = ArtifactDescriptor {
            name: "application.zip".to_owned(),
            size: 3,
            sha256: "11".repeat(32),
        };
        let artifact_path = cache.artifact_path(&artifact);
        fs::create_dir_all(artifact_path.parent().unwrap()).unwrap();
        fs::write(&artifact_path, b"old").unwrap();
        let staged = cache.root().join("staged-app/Steam Controller Bridge.app");
        fs::create_dir_all(&staged).unwrap();

        remove_obsolete_application_cache(&cache, &artifact);

        assert!(!artifact_path.exists());
        assert!(!cache.root().join("staged-app").exists());
    }
}
