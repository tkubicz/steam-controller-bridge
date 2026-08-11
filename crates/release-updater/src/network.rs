use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use semver::Version;

use crate::temporary::unique_temporary_path;
use crate::{
    ArtifactDescriptor, ReleaseCache, ReleaseManifestV1, TrustedPublicKey, MANIFEST_ASSET,
    SIGNATURES_ASSET, UPDATE_REPOSITORY,
};

const CURL_DOWNLOAD_ARGS: &[&str] = &[
    "--fail",
    "--location",
    "--silent",
    "--show-error",
    "--proto",
    "=https",
    // `--proto` constrains only the first URL; redirects need their own
    // restriction to keep the documented HTTPS-only contract.
    "--proto-redir",
    "=https",
    "--max-redirs",
    "5",
    "--tlsv1.2",
    "--connect-timeout",
    "10",
    "--max-time",
    "60",
    "--max-filesize",
];

#[derive(Debug)]
pub enum DownloadError {
    Io(io::Error),
    Curl(String),
    TooLarge { maximum: u64, actual: u64 },
    InvalidAssetName,
    Cancelled,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "update download I/O failed: {error}"),
            Self::Curl(error) => write!(formatter, "update download failed: {error}"),
            Self::TooLarge { maximum, actual } => {
                write!(formatter, "download is {actual} bytes; limit is {maximum}")
            }
            Self::InvalidAssetName => write!(formatter, "invalid release asset name"),
            Self::Cancelled => write!(formatter, "update download cancelled"),
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<io::Error> for DownloadError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone, Default)]
pub struct LatestReleaseClient {
    cancellation: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogRefresh {
    Current(ReleaseManifestV1),
    Stale {
        manifest: ReleaseManifestV1,
        refresh_error: String,
    },
}

pub trait ReleaseSource {
    fn fetch_metadata(&self, directory: &Path) -> Result<(Vec<u8>, Vec<u8>), DownloadError>;
    fn download_release_asset(
        &self,
        release_tag: &str,
        name: &str,
        destination: &Path,
        maximum_size: u64,
    ) -> Result<(), DownloadError>;
}

impl LatestReleaseClient {
    #[must_use]
    pub fn cancellable(cancellation: Arc<AtomicBool>) -> Self {
        Self {
            cancellation: Some(cancellation),
        }
    }

    pub fn fetch_metadata(&self, directory: &Path) -> Result<(Vec<u8>, Vec<u8>), DownloadError> {
        fs::create_dir_all(directory)?;
        let manifest = directory.join(MANIFEST_ASSET);
        let signatures = directory.join(SIGNATURES_ASSET);
        download_to_path_cancellable(
            &latest_asset_url(MANIFEST_ASSET)?,
            &manifest,
            128 * 1024,
            self.cancellation.as_deref(),
        )?;
        download_to_path_cancellable(
            &latest_asset_url(SIGNATURES_ASSET)?,
            &signatures,
            16 * 1024,
            self.cancellation.as_deref(),
        )?;
        Ok((fs::read(manifest)?, fs::read(signatures)?))
    }

    pub fn download_release_asset(
        &self,
        release_tag: &str,
        name: &str,
        destination: &Path,
        maximum_size: u64,
    ) -> Result<(), DownloadError> {
        if !valid_component(release_tag) || !valid_component(name) {
            return Err(DownloadError::InvalidAssetName);
        }
        let url = format!(
            "https://github.com/{UPDATE_REPOSITORY}/releases/download/{release_tag}/{name}"
        );
        download_to_path_cancellable(
            &url,
            destination,
            maximum_size,
            self.cancellation.as_deref(),
        )
    }
}

impl ReleaseSource for LatestReleaseClient {
    fn fetch_metadata(&self, directory: &Path) -> Result<(Vec<u8>, Vec<u8>), DownloadError> {
        Self::fetch_metadata(self, directory)
    }

    fn download_release_asset(
        &self,
        release_tag: &str,
        name: &str,
        destination: &Path,
        maximum_size: u64,
    ) -> Result<(), DownloadError> {
        Self::download_release_asset(self, release_tag, name, destination, maximum_size)
    }
}

pub fn refresh_catalog_if_due(
    source: &impl ReleaseSource,
    cache: &ReleaseCache,
    trusted_keys: &[TrustedPublicKey],
    interval: std::time::Duration,
    running_application: &Version,
) -> Result<CatalogRefresh, String> {
    if !cache.check_due(interval, running_application) {
        return cache
            .load_manifest(trusted_keys)
            .map(CatalogRefresh::Current)
            .map_err(|error| error.to_string());
    }
    fs::create_dir_all(cache.root()).map_err(|error| error.to_string())?;
    let refresh_lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(cache.root().join("catalog-refresh.lock"))
        .map_err(|error| error.to_string())?;
    rustix::fs::flock(&refresh_lock, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|error| error.to_string())?;
    if !cache.check_due(interval, running_application) {
        return cache
            .load_manifest(trusted_keys)
            .map(CatalogRefresh::Current)
            .map_err(|error| error.to_string());
    }
    let temporary =
        unique_temporary_path(cache.root(), std::ffi::OsStr::new("metadata"), "download");
    let refreshed = source
        .fetch_metadata(&temporary)
        .map_err(|error| error.to_string())
        .and_then(|(manifest, signatures)| {
            cache
                .store_manifest(&manifest, &signatures, trusted_keys)
                .map_err(|error| error.to_string())
        });
    let _ = fs::remove_dir_all(&temporary);
    match refreshed {
        Ok(manifest) => {
            // The marker is only a network-throttling optimization. Verified
            // metadata remains usable if writing the marker itself fails.
            let _ = cache.mark_check_success(running_application);
            Ok(CatalogRefresh::Current(manifest))
        }
        Err(refresh_error) => cache
            .load_manifest(trusted_keys)
            .map(|manifest| CatalogRefresh::Stale {
                manifest,
                refresh_error: refresh_error.clone(),
            })
            .map_err(|_| {
                format!("Cannot check releases and no verified cache is available: {refresh_error}")
            }),
    }
}

pub fn ensure_release_artifact(
    source: &impl ReleaseSource,
    cache: &ReleaseCache,
    release_tag: &str,
    artifact: &ArtifactDescriptor,
) -> Result<PathBuf, String> {
    if let Ok(path) = cache.verify_cached_artifact(artifact) {
        return Ok(path);
    }
    let path = cache.artifact_path(artifact);
    source
        .download_release_asset(release_tag, &artifact.name, &path, artifact.size)
        .map_err(|error| error.to_string())?;
    cache
        .verify_cached_artifact(artifact)
        .map_err(|error| error.to_string())
}

fn latest_asset_url(name: &str) -> Result<String, DownloadError> {
    if !valid_component(name) {
        return Err(DownloadError::InvalidAssetName);
    }
    Ok(format!(
        "https://github.com/{UPDATE_REPOSITORY}/releases/latest/download/{name}"
    ))
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte))
}

pub fn download_to_path(
    url: &str,
    destination: &Path,
    maximum_size: u64,
) -> Result<(), DownloadError> {
    download_to_path_cancellable(url, destination, maximum_size, None)
}

fn download_to_path_cancellable(
    url: &str,
    destination: &Path,
    maximum_size: u64,
    cancellation: Option<&AtomicBool>,
) -> Result<(), DownloadError> {
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("download destination has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = unique_temporary_path(
        parent,
        destination
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("update")),
        "download",
    );
    let mut command = Command::new("/usr/bin/curl");
    command
        .args(CURL_DOWNLOAD_ARGS)
        .arg(maximum_size.to_string())
        .arg("--output")
        .arg(&temporary)
        .arg(url);
    let output = match cancellable_output(&mut command, cancellation) {
        Ok(output) => output,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    if !output.status.success() {
        let _ = fs::remove_file(&temporary);
        return Err(DownloadError::Curl(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let actual = fs::metadata(&temporary)?.len();
    if actual > maximum_size {
        let _ = fs::remove_file(&temporary);
        return Err(DownloadError::TooLarge {
            maximum: maximum_size,
            actual,
        });
    }
    fs::rename(&temporary, destination)?;
    Ok(())
}

fn cancellable_output(
    command: &mut Command,
    cancellation: Option<&AtomicBool>,
) -> Result<Output, DownloadError> {
    command.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    loop {
        if cancellation.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DownloadError::Cancelled);
        }
        if child.try_wait()?.is_some() {
            return child.wait_with_output().map_err(DownloadError::Io);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::Duration;

    use crate::test_support::{signed_metadata, temporary_directory};

    #[test]
    fn release_components_cannot_escape_the_repository() {
        assert!(valid_component("v1.5.0"));
        assert!(valid_component("app.zip"));
        assert!(!valid_component("../app.zip"));
        assert!(!valid_component("folder/app.zip"));
        assert!(!valid_component("https://example.test"));
    }

    #[test]
    fn curl_downloads_and_redirects_are_https_only_and_bounded() {
        assert!(CURL_DOWNLOAD_ARGS
            .windows(2)
            .any(|arguments| arguments == ["--proto", "=https"]));
        assert!(CURL_DOWNLOAD_ARGS
            .windows(2)
            .any(|arguments| arguments == ["--proto-redir", "=https"]));
        assert!(CURL_DOWNLOAD_ARGS
            .windows(2)
            .any(|arguments| arguments == ["--max-redirs", "5"]));
    }

    #[test]
    fn cancellation_reaps_a_spawned_download_process() {
        let cancelled = AtomicBool::new(true);
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 10"]);
        let started = std::time::Instant::now();
        assert!(matches!(
            cancellable_output(&mut command, Some(&cancelled)),
            Err(DownloadError::Cancelled)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn cancellable_processes_preserve_exit_status_and_diagnostics() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf diagnostic >&2"]);
        let output = cancellable_output(&mut command, None).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stderr, b"diagnostic");
    }

    enum MetadataReply {
        Metadata(Vec<u8>, Vec<u8>),
        Failure,
    }

    struct MetadataSource {
        replies: Mutex<VecDeque<MetadataReply>>,
        directories: Mutex<Vec<PathBuf>>,
    }

    impl MetadataSource {
        fn new(replies: impl IntoIterator<Item = MetadataReply>) -> Self {
            Self {
                replies: Mutex::new(replies.into_iter().collect()),
                directories: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            self.directories.lock().unwrap().len()
        }
    }

    impl ReleaseSource for MetadataSource {
        fn fetch_metadata(&self, directory: &Path) -> Result<(Vec<u8>, Vec<u8>), DownloadError> {
            self.directories.lock().unwrap().push(directory.to_owned());
            match self.replies.lock().unwrap().pop_front().unwrap() {
                MetadataReply::Metadata(manifest, signatures) => Ok((manifest, signatures)),
                MetadataReply::Failure => Err(DownloadError::Io(io::Error::other(
                    "fixture metadata failure",
                ))),
            }
        }

        fn download_release_asset(
            &self,
            _release_tag: &str,
            _name: &str,
            _destination: &Path,
            _maximum_size: u64,
        ) -> Result<(), DownloadError> {
            unreachable!()
        }
    }

    struct ArtifactSource;

    fn refreshed_manifest(refresh: &CatalogRefresh) -> &ReleaseManifestV1 {
        match refresh {
            CatalogRefresh::Current(manifest) | CatalogRefresh::Stale { manifest, .. } => manifest,
        }
    }

    impl ReleaseSource for ArtifactSource {
        fn fetch_metadata(&self, _directory: &Path) -> Result<(Vec<u8>, Vec<u8>), DownloadError> {
            unreachable!()
        }

        fn download_release_asset(
            &self,
            _release_tag: &str,
            _name: &str,
            destination: &Path,
            _maximum_size: u64,
        ) -> Result<(), DownloadError> {
            fs::create_dir_all(destination.parent().unwrap())?;
            fs::write(destination, b"abc")?;
            Ok(())
        }
    }

    #[test]
    fn failed_first_check_is_immediately_retryable() {
        let root = temporary_directory("network-retry");
        let cache = ReleaseCache::new(root.clone());
        let (manifest, signatures, key) = signed_metadata("1.5.0", 5);
        let source = MetadataSource::new([
            MetadataReply::Failure,
            MetadataReply::Metadata(manifest, signatures),
        ]);

        assert!(refresh_catalog_if_due(
            &source,
            &cache,
            std::slice::from_ref(&key),
            Duration::from_hours(24),
            &Version::new(1, 5, 0),
        )
        .is_err());
        assert!(cache.check_due(Duration::from_hours(24), &Version::new(1, 5, 0)));
        assert_eq!(
            refreshed_manifest(
                &refresh_catalog_if_due(
                    &source,
                    &cache,
                    std::slice::from_ref(&key),
                    Duration::from_hours(24),
                    &Version::new(1, 5, 0),
                )
                .unwrap()
            )
            .application_version,
            semver::Version::new(1, 5, 0)
        );
        assert_eq!(source.calls(), 2);
        assert!(!cache.check_due(Duration::from_hours(24), &Version::new(1, 5, 0)));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn application_upgrade_bypasses_the_previous_versions_throttle() {
        let root = temporary_directory("network-application-upgrade");
        let cache = ReleaseCache::new(root.clone());
        let (old_manifest, old_signatures, key) = signed_metadata("1.5.0", 5);
        let (current_manifest, current_signatures, _) = signed_metadata("1.6.0", 6);
        let source = MetadataSource::new([
            MetadataReply::Metadata(old_manifest, old_signatures),
            MetadataReply::Metadata(current_manifest, current_signatures),
        ]);

        let old_application = Version::new(1, 5, 0);
        let current_application = Version::new(1, 6, 0);
        let first = refresh_catalog_if_due(
            &source,
            &cache,
            std::slice::from_ref(&key),
            Duration::from_hours(24),
            &old_application,
        )
        .unwrap();
        assert_eq!(
            refreshed_manifest(&first).application_version,
            old_application
        );

        let cached = refresh_catalog_if_due(
            &source,
            &cache,
            std::slice::from_ref(&key),
            Duration::from_hours(24),
            &Version::new(1, 5, 0),
        )
        .unwrap();
        assert_eq!(
            refreshed_manifest(&cached).application_version,
            old_application
        );
        assert_eq!(source.calls(), 1);

        let refreshed = refresh_catalog_if_due(
            &source,
            &cache,
            &[key],
            Duration::from_hours(24),
            &current_application,
        )
        .unwrap();
        assert_eq!(
            refreshed_manifest(&refreshed).application_version,
            current_application
        );
        assert_eq!(source.calls(), 2);
        assert!(!cache.check_due(Duration::from_hours(24), &current_application));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_refresh_falls_back_to_verified_cache_without_replacing_it() {
        let root = temporary_directory("network-fallback");
        let cache = ReleaseCache::new(root.clone());
        let (manifest, signatures, key) = signed_metadata("1.5.0", 5);
        let source = MetadataSource::new([
            MetadataReply::Metadata(manifest, signatures),
            MetadataReply::Metadata(b"{}".to_vec(), b"{}".to_vec()),
        ]);

        refresh_catalog_if_due(
            &source,
            &cache,
            std::slice::from_ref(&key),
            Duration::ZERO,
            &Version::new(1, 5, 0),
        )
        .unwrap();
        let fallback = refresh_catalog_if_due(
            &source,
            &cache,
            &[key],
            Duration::ZERO,
            &Version::new(1, 5, 0),
        )
        .unwrap();
        let CatalogRefresh::Stale {
            manifest,
            refresh_error,
        } = fallback
        else {
            panic!("failed refresh must report stale provenance");
        };
        assert_eq!(manifest.application_version, semver::Version::new(1, 5, 0));
        assert!(
            refresh_error.contains("invalid release signature envelope"),
            "unexpected refresh error: {refresh_error}"
        );
        assert_eq!(source.calls(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_refreshes_are_single_flight_and_clean_their_directory() {
        let root = temporary_directory("network-concurrent");
        let cache = Arc::new(ReleaseCache::new(root.clone()));
        let (manifest, signatures, key) = signed_metadata("1.5.0", 5);
        let source = Arc::new(MetadataSource::new([MetadataReply::Metadata(
            manifest, signatures,
        )]));
        let start = Arc::new(Barrier::new(2));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let cache = Arc::clone(&cache);
            let source = Arc::clone(&source);
            let start = Arc::clone(&start);
            let key = key.clone();
            workers.push(std::thread::spawn(move || {
                start.wait();
                refresh_catalog_if_due(
                    source.as_ref(),
                    &cache,
                    &[key],
                    Duration::from_hours(24),
                    &Version::new(1, 5, 0),
                )
                .unwrap()
            }));
        }
        for worker in workers {
            let refresh = worker.join().unwrap();
            assert_eq!(
                refreshed_manifest(&refresh).application_version,
                semver::Version::new(1, 5, 0)
            );
        }
        let directories = source.directories.lock().unwrap();
        assert_eq!(directories.len(), 1);
        assert!(directories.iter().all(|directory| !directory.exists()));
        drop(directories);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fake_network_artifact_is_verified_and_then_reused_offline() {
        let root = temporary_directory("network-artifact");
        let _ = fs::remove_dir_all(&root);
        let cache = ReleaseCache::new(root.clone());
        let artifact = ArtifactDescriptor {
            name: "asset.bin".to_owned(),
            size: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned(),
        };
        let path = ensure_release_artifact(&ArtifactSource, &cache, "v1.0.0", &artifact).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"abc");
        let path = ensure_release_artifact(&ArtifactSource, &cache, "v1.0.0", &artifact).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"abc");
        let _ = fs::remove_dir_all(root);
    }
}
