use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

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
pub struct LatestReleaseClient;

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
    pub fn fetch_metadata(&self, directory: &Path) -> Result<(Vec<u8>, Vec<u8>), DownloadError> {
        fs::create_dir_all(directory)?;
        let manifest = directory.join(MANIFEST_ASSET);
        let signatures = directory.join(SIGNATURES_ASSET);
        download_to_path(&latest_asset_url(MANIFEST_ASSET)?, &manifest, 128 * 1024)?;
        download_to_path(&latest_asset_url(SIGNATURES_ASSET)?, &signatures, 16 * 1024)?;
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
        download_to_path(&url, destination, maximum_size)
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
) -> Result<ReleaseManifestV1, String> {
    if !cache.check_due(interval) {
        return cache
            .load_manifest(trusted_keys)
            .map_err(|error| error.to_string());
    }
    cache
        .mark_check_attempt()
        .map_err(|error| error.to_string())?;
    let temporary = cache.root().join("metadata-download");
    match source.fetch_metadata(&temporary) {
        Ok((manifest, signatures)) => cache
            .store_manifest(&manifest, &signatures, trusted_keys)
            .map_err(|error| error.to_string()),
        Err(error) => cache.load_manifest(trusted_keys).map_err(|_| {
            format!("Cannot check releases and no verified cache is available: {error}")
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
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("download destination has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary: PathBuf = parent.join(format!(
        ".{}.{}.download",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("update"),
        std::process::id()
    ));
    let output = Command::new("/usr/bin/curl")
        .args(CURL_DOWNLOAD_ARGS)
        .arg(maximum_size.to_string())
        .arg("--output")
        .arg(&temporary)
        .arg(url)
        .output()?;
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

#[cfg(test)]
mod tests {
    use super::*;

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

    struct FakeSource;

    impl ReleaseSource for FakeSource {
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
    fn fake_network_artifact_is_verified_and_then_reused_offline() {
        let root =
            std::env::temp_dir().join(format!("release-updater-network-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let cache = ReleaseCache::new(root.clone());
        let artifact = ArtifactDescriptor {
            name: "asset.bin".to_owned(),
            size: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned(),
        };
        let path = ensure_release_artifact(&FakeSource, &cache, "v1.0.0", &artifact).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"abc");
        let path = ensure_release_artifact(&FakeSource, &cache, "v1.0.0", &artifact).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"abc");
        let _ = fs::remove_dir_all(root);
    }
}
