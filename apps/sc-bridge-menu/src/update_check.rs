use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
#[cfg(feature = "local-update-source")]
use std::sync::Once;
use std::thread;
use std::time::Duration;

use bridge_runtime::FirmwareInfo;
#[cfg(feature = "local-update-source")]
use release_updater::LocalReleaseClient;
use release_updater::{
    classify_firmware_release, embedded_trusted_keys, firmware_matches_target, firmware_target,
    firmware_targets, refresh_catalog, ArtifactDescriptor, CatalogRefresh, CatalogRefreshError,
    CatalogRefreshPolicy, FirmwareRelease, FirmwareReleaseState, LatestReleaseClient, ReleaseCache,
    ReleaseManifestV1, ReleaseSource, TrustedPublicKey,
};
use semver::Version;

const CHECK_INTERVAL: Duration = Duration::from_hours(24);
#[cfg(feature = "local-update-source")]
const LOCAL_UPDATE_DIRECTORY_ENV: &str = "SC_BRIDGE_LOCAL_UPDATE_DIR";
#[cfg(feature = "local-update-source")]
static LOCAL_UPDATE_NOTICE: Once = Once::new();

#[derive(Clone)]
enum UpdateChannel {
    Production(LatestReleaseClient),
    #[cfg(feature = "local-update-source")]
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
            #[cfg(feature = "local-update-source")]
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
            #[cfg(feature = "local-update-source")]
            UpdateChannel::Local(_) => Duration::ZERO,
        }
    }

    pub(crate) fn is_local(&self) -> bool {
        self.channel.is_local()
    }

    pub(crate) fn local_root(&self) -> Option<&Path> {
        match &self.channel {
            UpdateChannel::Production(_) => None,
            #[cfg(feature = "local-update-source")]
            UpdateChannel::Local(source) => Some(source.root()),
        }
    }
}

impl UpdateChannel {
    fn configured() -> Result<Self, String> {
        let local_root = development_update_source();
        Self::select(local_root.as_deref())
    }

    /// Selects the release channel for a candidate local root. Builds without
    /// `local-update-source` ignore the candidate and stay on GitHub releases.
    fn select(local_root: Option<&Path>) -> Result<Self, String> {
        #[cfg(feature = "local-update-source")]
        if let Some(root) = local_root {
            let source = LocalReleaseClient::new(root).map_err(|error| {
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

        #[cfg(not(feature = "local-update-source"))]
        let _ = local_root;

        Ok(Self::Production(
            LatestReleaseClient::new().map_err(|error| error.to_string())?,
        ))
    }

    fn is_local(&self) -> bool {
        match self {
            Self::Production(_) => false,
            #[cfg(feature = "local-update-source")]
            Self::Local(_) => true,
        }
    }

    fn cache(&self) -> Result<ReleaseCache, String> {
        match self {
            Self::Production(_) => {
                ReleaseCache::for_current_user().map_err(|error| error.to_string())
            }
            #[cfg(feature = "local-update-source")]
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
            #[cfg(feature = "local-update-source")]
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

pub(crate) fn development_update_source() -> Option<PathBuf> {
    #[cfg(feature = "local-update-source")]
    {
        std::env::var_os(LOCAL_UPDATE_DIRECTORY_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(not(feature = "local-update-source"))]
    {
        None
    }
}

pub struct UpdateChecker {
    manifest: Option<ReleaseManifestV1>,
    result: Option<Receiver<Result<CatalogRefresh, CatalogRefreshError>>>,
    running_version: Version,
    started: bool,
}

impl UpdateChecker {
    pub fn new() -> Self {
        Self {
            manifest: None,
            result: None,
            running_version: running_version(),
            started: false,
        }
    }

    fn start_with(&mut self, context: impl FnOnce() -> Result<UpdateContext, String>) {
        if self.started {
            return;
        }
        self.started = true;
        let Ok(context) = context() else { return };
        self.manifest = context.cache().load_manifest(context.keys()).ok();
        let obsolete_application = self
            .manifest
            .as_ref()
            .filter(|manifest| self.running_version >= manifest.application_version)
            .map(|manifest| manifest.application.artifact.clone());
        if !context
            .cache()
            .check_due(context.check_interval(), &self.running_version)
        {
            if let Some(artifact) = obsolete_application {
                let cache = context.cache().clone();
                thread::spawn(move || remove_obsolete_application_cache(&cache, &artifact));
            }
            return;
        }
        let running_version = self.running_version.clone();
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
        self.result = Some(result);
    }

    pub fn poll(&mut self) {
        self.start_with(update_context);
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

impl Default for UpdateChecker {
    fn default() -> Self {
        Self::new()
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
    fn a_local_root_is_honoured_only_by_a_local_update_source_build() {
        let directory = tempfile::tempdir().unwrap();
        let channel = UpdateChannel::select(Some(directory.path()))
            .expect("a valid source selects an update channel");
        assert_eq!(channel.is_local(), cfg!(feature = "local-update-source"));
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

    #[test]
    fn automatic_update_start_is_one_shot_even_when_setup_is_unavailable() {
        let mut checker = UpdateChecker::new();
        checker.start_with(|| Err("unavailable".to_owned()));
        checker.start_with(|| panic!("a second start must not replace in-flight state"));

        assert!(checker.started);
    }
}
