use std::fs;
#[cfg(debug_assertions)]
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
#[cfg(debug_assertions)]
use std::sync::Once;
use std::thread;
use std::time::Duration;

use bridge_runtime::FirmwareVersion;
#[cfg(debug_assertions)]
use release_updater::LocalReleaseClient;
use release_updater::{
    classify_firmware_release, embedded_trusted_keys, refresh_catalog_if_due, ArtifactDescriptor,
    CatalogRefresh, FirmwareReleaseState, LatestReleaseClient, ReleaseCache, ReleaseManifestV1,
    ReleaseSource, TrustedPublicKey,
};
use semver::Version;

const CHECK_INTERVAL: Duration = Duration::from_hours(24);
#[cfg(debug_assertions)]
const LOCAL_UPDATE_DIRECTORY_ENV: &str = "SC_BRIDGE_LOCAL_UPDATE_DIR";
#[cfg(debug_assertions)]
static LOCAL_UPDATE_NOTICE: Once = Once::new();

#[derive(Clone)]
enum UpdateChannel {
    Production,
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
    pub(crate) fn source(&self, cancellation: Option<Arc<AtomicBool>>) -> Box<dyn ReleaseSource> {
        match &self.channel {
            UpdateChannel::Production => cancellation.map_or_else(
                || Box::new(LatestReleaseClient::default()) as Box<dyn ReleaseSource>,
                |flag| Box::new(LatestReleaseClient::cancellable(flag)),
            ),
            #[cfg(debug_assertions)]
            UpdateChannel::Local(source) => cancellation.map_or_else(
                || Box::new(source.clone()) as Box<dyn ReleaseSource>,
                |flag| Box::new(source.clone().cancellable(flag)),
            ),
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
            UpdateChannel::Production => CHECK_INTERVAL,
            #[cfg(debug_assertions)]
            UpdateChannel::Local(_) => Duration::ZERO,
        }
    }

    pub(crate) fn is_local(&self) -> bool {
        match &self.channel {
            UpdateChannel::Production => false,
            #[cfg(debug_assertions)]
            UpdateChannel::Local(_) => true,
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn local_root(&self) -> Option<&Path> {
        match &self.channel {
            UpdateChannel::Production => None,
            UpdateChannel::Local(source) => Some(source.root()),
        }
    }
}

impl UpdateChannel {
    #[cfg(debug_assertions)]
    fn configured() -> Result<Self, String> {
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
        Ok(Self::Production)
    }

    fn cache(&self) -> Result<ReleaseCache, String> {
        match self {
            Self::Production => ReleaseCache::for_current_user().map_err(|error| error.to_string()),
            #[cfg(debug_assertions)]
            Self::Local(source) => Ok(ReleaseCache::for_local_source(source.root())),
        }
    }
}

/// The embedded trust anchors and selected release source, or the user-facing
/// reason updates cannot work in this build.
pub(crate) fn update_context() -> Result<UpdateContext, String> {
    #[cfg(debug_assertions)]
    let channel = UpdateChannel::configured()?;
    #[cfg(not(debug_assertions))]
    let channel = UpdateChannel::Production;
    let keys = embedded_trusted_keys().map_err(|error| error.to_string())?;
    if keys.is_empty() {
        let message = match channel {
            #[cfg(debug_assertions)]
            UpdateChannel::Local(_) => {
                "Local updates require a trusted development key in SC_BRIDGE_UPDATE_PUBLIC_KEYS."
            }
            UpdateChannel::Production => {
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
    result: Option<Receiver<Result<CatalogRefresh, String>>>,
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
            let source = context.source(None);
            let _ = sender.send(refresh_catalog_if_due(
                source.as_ref(),
                context.cache(),
                context.keys(),
                context.check_interval(),
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
