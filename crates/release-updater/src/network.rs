#[cfg(debug_assertions)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::redirect::Policy;
use semver::Version;
use tempfile::{Builder as TempFileBuilder, NamedTempFile, TempDir};
use thiserror::Error;

use crate::manifest::valid_release_component;
use crate::{
    ArtifactDescriptor, CacheError, ReleaseCache, ReleaseManifestV1, TrustedPublicKey,
    MANIFEST_ASSET, SIGNATURES_ASSET, UPDATE_REPOSITORY,
};

const MANIFEST_MAX_SIZE: u64 = 128 * 1024;
const SIGNATURES_MAX_SIZE: u64 = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(20);
#[cfg(debug_assertions)]
const LOCAL_COPY_BUFFER_SIZE: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("update download I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("update download failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("update download requires an HTTPS URL")]
    HttpsRequired,
    #[error("invalid update URL: {0}")]
    InvalidUrl(String),
    #[error("download is {actual} bytes; limit is {maximum}")]
    TooLarge { maximum: u64, actual: u64 },
    #[error("invalid release asset name")]
    InvalidAssetName,
    #[error("invalid local update source: {0}")]
    InvalidLocalSource(String),
    #[error("update download cancelled")]
    Cancelled,
    #[error("cannot initialize update download runtime: {0}")]
    Runtime(#[source] io::Error),
}

#[derive(Debug, Error)]
pub enum CatalogRefreshError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("cannot lock update catalog: {0}")]
    Lock(#[source] rustix::io::Errno),
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error(
        "Cannot check releases ({refresh_error}) and no verified cache is available ({cache_error})"
    )]
    NoVerifiedCache {
        #[source]
        refresh_error: Box<CatalogRefreshError>,
        cache_error: CacheError,
    },
}

#[derive(Debug, Error)]
pub enum ArtifactFetchError {
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error(transparent)]
    Cache(#[from] CacheError),
}

#[derive(Debug, Clone)]
pub struct LatestReleaseClient {
    client: reqwest::Client,
}

#[cfg(debug_assertions)]
#[derive(Debug, Clone)]
pub struct LocalReleaseClient {
    root: PathBuf,
}

#[derive(Debug)]
pub enum CatalogRefresh {
    Current(ReleaseManifestV1),
    Stale {
        manifest: ReleaseManifestV1,
        refresh_error: CatalogRefreshError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogRefreshPolicy {
    IfDue(Duration),
    Force,
}

pub trait ReleaseSource: Send + Sync {
    fn fetch_metadata(
        &self,
        cancellation: Option<&AtomicBool>,
    ) -> Result<(Vec<u8>, Vec<u8>), DownloadError>;
    fn download_release_asset(
        &self,
        release_tag: &str,
        name: &str,
        destination: &Path,
        maximum_size: u64,
        cancellation: Option<&AtomicBool>,
    ) -> Result<(), DownloadError>;
}

impl LatestReleaseClient {
    pub fn new() -> Result<Self, DownloadError> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .redirect(Policy::limited(5))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self { client })
    }

    fn run<T>(future: impl Future<Output = Result<T, DownloadError>>) -> Result<T, DownloadError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(DownloadError::Runtime)?
            .block_on(future)
    }

    async fn download_async(
        &self,
        url: &str,
        destination: &Path,
        maximum_size: u64,
        cancellation: Option<&AtomicBool>,
    ) -> Result<(), DownloadError> {
        let url = reqwest::Url::parse(url)
            .map_err(|error| DownloadError::InvalidUrl(error.to_string()))?;
        if url.scheme() != "https" {
            return Err(DownloadError::HttpsRequired);
        }
        let parent = destination
            .parent()
            .ok_or_else(|| io::Error::other("download destination has no parent"))?;
        fs::create_dir_all(parent)?;
        let mut temporary = temporary_file(parent, "download")?;
        download_response(
            &self.client,
            url,
            &mut temporary,
            maximum_size,
            cancellation,
        )
        .await?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(destination)
            .map_err(|error| DownloadError::Io(error.error))?;
        Ok(())
    }

    pub fn fetch_metadata(
        &self,
        cancellation: Option<&AtomicBool>,
    ) -> Result<(Vec<u8>, Vec<u8>), DownloadError> {
        let directory = TempDir::new()?;
        let manifest = directory.path().join(MANIFEST_ASSET);
        let signatures = directory.path().join(SIGNATURES_ASSET);
        let manifest_url = latest_asset_url(MANIFEST_ASSET)?;
        let signatures_url = latest_asset_url(SIGNATURES_ASSET)?;
        Self::run(async {
            self.download_async(&manifest_url, &manifest, MANIFEST_MAX_SIZE, cancellation)
                .await?;
            self.download_async(
                &signatures_url,
                &signatures,
                SIGNATURES_MAX_SIZE,
                cancellation,
            )
            .await
        })?;
        Ok((fs::read(manifest)?, fs::read(signatures)?))
    }

    pub fn download_release_asset(
        &self,
        release_tag: &str,
        name: &str,
        destination: &Path,
        maximum_size: u64,
        cancellation: Option<&AtomicBool>,
    ) -> Result<(), DownloadError> {
        if !valid_release_component(release_tag) || !valid_release_component(name) {
            return Err(DownloadError::InvalidAssetName);
        }
        Self::run(self.download_async(
            &format!(
                "https://github.com/{UPDATE_REPOSITORY}/releases/download/{release_tag}/{name}"
            ),
            destination,
            maximum_size,
            cancellation,
        ))
    }
}

async fn download_response(
    client: &reqwest::Client,
    url: reqwest::Url,
    output: &mut NamedTempFile,
    maximum_size: u64,
    cancellation: Option<&AtomicBool>,
) -> Result<(), DownloadError> {
    let mut response = await_or_cancel(client.get(url).send(), cancellation)
        .await?
        .error_for_status()?;
    if let Some(actual) = response.content_length() {
        if actual > maximum_size {
            return Err(DownloadError::TooLarge {
                maximum: maximum_size,
                actual,
            });
        }
    }
    let mut actual = 0_u64;
    loop {
        let Some(chunk) = await_or_cancel(response.chunk(), cancellation).await? else {
            break;
        };
        actual = actual.saturating_add(chunk.len() as u64);
        if actual > maximum_size {
            return Err(DownloadError::TooLarge {
                maximum: maximum_size,
                actual,
            });
        }
        output.write_all(&chunk)?;
    }
    Ok(())
}

async fn await_or_cancel<T>(
    future: impl Future<Output = Result<T, reqwest::Error>>,
    cancellation: Option<&AtomicBool>,
) -> Result<T, DownloadError> {
    let Some(cancellation) = cancellation else {
        return Ok(future.await?);
    };
    tokio::select! {
        result = future => Ok(result?),
        () = wait_for_cancellation(cancellation) => Err(DownloadError::Cancelled),
    }
}

async fn wait_for_cancellation(cancellation: &AtomicBool) {
    loop {
        if cancellation.load(Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(CANCELLATION_POLL_INTERVAL).await;
    }
}

impl ReleaseSource for LatestReleaseClient {
    fn fetch_metadata(
        &self,
        cancellation: Option<&AtomicBool>,
    ) -> Result<(Vec<u8>, Vec<u8>), DownloadError> {
        Self::fetch_metadata(self, cancellation)
    }

    fn download_release_asset(
        &self,
        release_tag: &str,
        name: &str,
        destination: &Path,
        maximum_size: u64,
        cancellation: Option<&AtomicBool>,
    ) -> Result<(), DownloadError> {
        Self::download_release_asset(
            self,
            release_tag,
            name,
            destination,
            maximum_size,
            cancellation,
        )
    }
}

#[cfg(debug_assertions)]
impl LocalReleaseClient {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, DownloadError> {
        let root = fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(DownloadError::InvalidLocalSource(format!(
                "{} is not a directory",
                root.display()
            )));
        }
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn source_file(&self, name: &str) -> Result<PathBuf, DownloadError> {
        if !valid_release_component(name) {
            return Err(DownloadError::InvalidAssetName);
        }
        let path = fs::canonicalize(self.root.join(name))?;
        if !path.starts_with(&self.root) || !path.is_file() {
            return Err(DownloadError::InvalidLocalSource(format!(
                "{} is not a regular file inside {}",
                path.display(),
                self.root.display()
            )));
        }
        Ok(path)
    }

    fn read_file(
        &self,
        name: &str,
        maximum_size: u64,
        cancellation: Option<&AtomicBool>,
    ) -> Result<Vec<u8>, DownloadError> {
        let mut file = File::open(self.source_file(name)?)?;
        let mut bytes = Vec::new();
        copy_bounded(&mut file, &mut bytes, maximum_size, cancellation)?;
        Ok(bytes)
    }

    fn copy_file(
        &self,
        name: &str,
        destination: &Path,
        maximum_size: u64,
        cancellation: Option<&AtomicBool>,
    ) -> Result<(), DownloadError> {
        let mut input = File::open(self.source_file(name)?)?;
        let parent = destination
            .parent()
            .ok_or_else(|| io::Error::other("download destination has no parent"))?;
        fs::create_dir_all(parent)?;
        let mut output = temporary_file(parent, "local-copy")?;
        copy_bounded(&mut input, &mut output, maximum_size, cancellation)?;
        output.as_file().sync_all()?;
        output
            .persist(destination)
            .map_err(|error| DownloadError::Io(error.error))?;
        Ok(())
    }
}

#[cfg(debug_assertions)]
impl ReleaseSource for LocalReleaseClient {
    fn fetch_metadata(
        &self,
        cancellation: Option<&AtomicBool>,
    ) -> Result<(Vec<u8>, Vec<u8>), DownloadError> {
        Ok((
            self.read_file(MANIFEST_ASSET, MANIFEST_MAX_SIZE, cancellation)?,
            self.read_file(SIGNATURES_ASSET, SIGNATURES_MAX_SIZE, cancellation)?,
        ))
    }

    fn download_release_asset(
        &self,
        release_tag: &str,
        name: &str,
        destination: &Path,
        maximum_size: u64,
        cancellation: Option<&AtomicBool>,
    ) -> Result<(), DownloadError> {
        if !valid_release_component(release_tag) {
            return Err(DownloadError::InvalidAssetName);
        }
        self.copy_file(name, destination, maximum_size, cancellation)
    }
}

pub fn refresh_catalog(
    source: &(impl ReleaseSource + ?Sized),
    cache: &ReleaseCache,
    trusted_keys: &[TrustedPublicKey],
    policy: CatalogRefreshPolicy,
    running_application: &Version,
) -> Result<CatalogRefresh, CatalogRefreshError> {
    refresh_catalog_cancellable(
        source,
        cache,
        trusted_keys,
        policy,
        running_application,
        None,
    )
}

pub fn refresh_catalog_cancellable(
    source: &(impl ReleaseSource + ?Sized),
    cache: &ReleaseCache,
    trusted_keys: &[TrustedPublicKey],
    policy: CatalogRefreshPolicy,
    running_application: &Version,
    cancellation: Option<&AtomicBool>,
) -> Result<CatalogRefresh, CatalogRefreshError> {
    if !refresh_due(cache, policy, running_application) {
        return Ok(CatalogRefresh::Current(cache.load_manifest(trusted_keys)?));
    }
    fs::create_dir_all(cache.root())?;
    let refresh_lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(cache.root().join("catalog-refresh.lock"))?;
    rustix::fs::flock(&refresh_lock, rustix::fs::FlockOperation::LockExclusive)
        .map_err(CatalogRefreshError::Lock)?;
    if !refresh_due(cache, policy, running_application) {
        return Ok(CatalogRefresh::Current(cache.load_manifest(trusted_keys)?));
    }

    let refreshed = source
        .fetch_metadata(cancellation)
        .map_err(CatalogRefreshError::from)
        .and_then(|(manifest, signatures)| {
            cache
                .store_manifest(&manifest, &signatures, trusted_keys)
                .map_err(CatalogRefreshError::from)
        });
    match refreshed {
        Ok(manifest) => {
            // The marker is a throttle only. Verified metadata remains usable
            // if recording freshness fails.
            let _ = cache.mark_check_success(running_application);
            Ok(CatalogRefresh::Current(manifest))
        }
        Err(refresh_error) => match cache.load_manifest(trusted_keys) {
            Ok(manifest) => Ok(CatalogRefresh::Stale {
                manifest,
                refresh_error,
            }),
            Err(cache_error) => Err(CatalogRefreshError::NoVerifiedCache {
                refresh_error: Box::new(refresh_error),
                cache_error,
            }),
        },
    }
}

fn refresh_due(cache: &ReleaseCache, policy: CatalogRefreshPolicy, application: &Version) -> bool {
    match policy {
        CatalogRefreshPolicy::IfDue(interval) => cache.check_due(interval, application),
        CatalogRefreshPolicy::Force => true,
    }
}

pub fn ensure_release_artifact(
    source: &(impl ReleaseSource + ?Sized),
    cache: &ReleaseCache,
    release_tag: &str,
    artifact: &ArtifactDescriptor,
) -> Result<PathBuf, ArtifactFetchError> {
    ensure_release_artifact_cancellable(source, cache, release_tag, artifact, None)
}

pub fn ensure_release_artifact_cancellable(
    source: &(impl ReleaseSource + ?Sized),
    cache: &ReleaseCache,
    release_tag: &str,
    artifact: &ArtifactDescriptor,
    cancellation: Option<&AtomicBool>,
) -> Result<PathBuf, ArtifactFetchError> {
    if let Ok(path) = cache.verify_cached_artifact(artifact) {
        return Ok(path);
    }
    let path = cache.artifact_path(artifact);
    source.download_release_asset(
        release_tag,
        &artifact.name,
        &path,
        artifact.size,
        cancellation,
    )?;
    Ok(cache.verify_cached_artifact(artifact)?)
}

fn latest_asset_url(name: &str) -> Result<String, DownloadError> {
    if !valid_release_component(name) {
        return Err(DownloadError::InvalidAssetName);
    }
    Ok(format!(
        "https://github.com/{UPDATE_REPOSITORY}/releases/latest/download/{name}"
    ))
}

pub fn download_to_path(
    url: &str,
    destination: &Path,
    maximum_size: u64,
) -> Result<(), DownloadError> {
    let client = LatestReleaseClient::new()?;
    LatestReleaseClient::run(client.download_async(url, destination, maximum_size, None))
}

fn temporary_file(directory: &Path, suffix: &str) -> io::Result<NamedTempFile> {
    TempFileBuilder::new()
        .prefix(".release-updater-")
        .suffix(&format!("-{suffix}"))
        .tempfile_in(directory)
}

#[cfg(debug_assertions)]
fn check_cancelled(cancellation: Option<&AtomicBool>) -> Result<(), DownloadError> {
    if cancellation.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        Err(DownloadError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(debug_assertions)]
fn copy_bounded(
    input: &mut impl io::Read,
    output: &mut impl io::Write,
    maximum_size: u64,
    cancellation: Option<&AtomicBool>,
) -> Result<u64, DownloadError> {
    let mut buffer = vec![0_u8; LOCAL_COPY_BUFFER_SIZE].into_boxed_slice();
    let mut actual = 0_u64;
    loop {
        check_cancelled(cancellation)?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return Ok(actual);
        }
        actual = actual.saturating_add(count as u64);
        if actual > maximum_size {
            return Err(DownloadError::TooLarge {
                maximum: maximum_size,
                actual,
            });
        }
        output.write_all(&buffer[..count])?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Barrier, Mutex};

    use crate::test_support::{signed_metadata, temporary_directory};

    #[test]
    fn release_components_cannot_escape_the_repository() {
        assert!(valid_release_component("v1.5.0"));
        assert!(valid_release_component("app.zip"));
        assert!(!valid_release_component("../app.zip"));
        assert!(!valid_release_component("folder/app.zip"));
    }

    #[test]
    fn production_downloads_reject_non_https_urls() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            download_to_path("http://example.test/update", &root.path().join("asset"), 1),
            Err(DownloadError::HttpsRequired)
        ));
    }

    #[test]
    fn local_source_bounds_and_cancels_atomic_copies() {
        let root = tempfile::tempdir().unwrap();
        let destination_root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("firmware.uf2"), b"four").unwrap();
        let destination = destination_root.path().join("firmware.uf2");
        fs::write(&destination, b"old").unwrap();
        let source = LocalReleaseClient::new(root.path()).unwrap();
        assert!(matches!(
            source.download_release_asset("v1.6.0", "firmware.uf2", &destination, 3, None),
            Err(DownloadError::TooLarge {
                maximum: 3,
                actual: 4
            })
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            source.download_release_asset(
                "v1.6.0",
                "firmware.uf2",
                &destination,
                4,
                Some(&cancelled),
            ),
            Err(DownloadError::Cancelled)
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"old");
    }

    #[test]
    fn cancellation_wins_while_an_async_request_is_pending() {
        let cancelled = AtomicBool::new(true);
        let pending = std::future::pending::<Result<(), reqwest::Error>>();
        assert!(matches!(
            LatestReleaseClient::run(await_or_cancel(pending, Some(&cancelled))),
            Err(DownloadError::Cancelled)
        ));
    }

    enum MetadataReply {
        Metadata(Vec<u8>, Vec<u8>),
        Failure,
    }

    struct MetadataSource {
        replies: Mutex<VecDeque<MetadataReply>>,
        calls: Mutex<usize>,
    }

    impl MetadataSource {
        fn new(replies: impl IntoIterator<Item = MetadataReply>) -> Self {
            Self {
                replies: Mutex::new(replies.into_iter().collect()),
                calls: Mutex::new(0),
            }
        }

        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    impl ReleaseSource for MetadataSource {
        fn fetch_metadata(
            &self,
            _cancellation: Option<&AtomicBool>,
        ) -> Result<(Vec<u8>, Vec<u8>), DownloadError> {
            *self.calls.lock().unwrap() += 1;
            match self.replies.lock().unwrap().pop_front().unwrap() {
                MetadataReply::Metadata(manifest, signatures) => Ok((manifest, signatures)),
                MetadataReply::Failure => {
                    Err(DownloadError::Io(io::Error::other("fixture failure")))
                }
            }
        }

        fn download_release_asset(
            &self,
            _release_tag: &str,
            _name: &str,
            _destination: &Path,
            _maximum_size: u64,
            _cancellation: Option<&AtomicBool>,
        ) -> Result<(), DownloadError> {
            unreachable!()
        }
    }

    fn refreshed_manifest(refresh: &CatalogRefresh) -> &ReleaseManifestV1 {
        match refresh {
            CatalogRefresh::Current(manifest) | CatalogRefresh::Stale { manifest, .. } => manifest,
        }
    }

    #[test]
    fn forced_refresh_bypasses_only_the_freshness_throttle() {
        let root = temporary_directory("network-force");
        let cache = ReleaseCache::new(root.path().to_owned());
        let (first_manifest, first_signatures, key) = signed_metadata("1.5.0", 5);
        let (second_manifest, second_signatures, _) = signed_metadata("1.6.0", 6);
        let source = MetadataSource::new([
            MetadataReply::Metadata(first_manifest, first_signatures),
            MetadataReply::Metadata(second_manifest, second_signatures),
        ]);
        let application = Version::new(1, 5, 0);
        refresh_catalog(
            &source,
            &cache,
            std::slice::from_ref(&key),
            CatalogRefreshPolicy::IfDue(Duration::from_hours(24)),
            &application,
        )
        .unwrap();
        let cached = refresh_catalog(
            &source,
            &cache,
            std::slice::from_ref(&key),
            CatalogRefreshPolicy::IfDue(Duration::from_hours(24)),
            &application,
        )
        .unwrap();
        assert_eq!(refreshed_manifest(&cached).application_version, application);
        assert_eq!(source.calls(), 1);
        let forced = refresh_catalog(
            &source,
            &cache,
            &[key],
            CatalogRefreshPolicy::Force,
            &application,
        )
        .unwrap();
        assert_eq!(
            refreshed_manifest(&forced).application_version,
            Version::new(1, 6, 0)
        );
        assert_eq!(source.calls(), 2);
        assert!(!cache.check_due(Duration::from_hours(24), &application));
    }

    #[test]
    fn failed_refresh_falls_back_to_verified_cache() {
        let root = temporary_directory("network-fallback");
        let cache = ReleaseCache::new(root.path().to_owned());
        let (manifest, signatures, key) = signed_metadata("1.5.0", 5);
        let source = MetadataSource::new([
            MetadataReply::Metadata(manifest, signatures),
            MetadataReply::Failure,
        ]);
        refresh_catalog(
            &source,
            &cache,
            std::slice::from_ref(&key),
            CatalogRefreshPolicy::Force,
            &Version::new(1, 5, 0),
        )
        .unwrap();
        let stale = refresh_catalog(
            &source,
            &cache,
            &[key],
            CatalogRefreshPolicy::Force,
            &Version::new(1, 5, 0),
        )
        .unwrap();
        assert!(matches!(stale, CatalogRefresh::Stale { .. }));
    }

    #[test]
    fn concurrent_due_refreshes_are_single_flight_and_clean_temporary_directories() {
        let root = temporary_directory("network-concurrent");
        let cache = Arc::new(ReleaseCache::new(root.path().to_owned()));
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
                refresh_catalog(
                    source.as_ref(),
                    &cache,
                    &[key],
                    CatalogRefreshPolicy::IfDue(Duration::from_hours(24)),
                    &Version::new(1, 5, 0),
                )
                .unwrap()
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(source.calls(), 1);
    }
}
