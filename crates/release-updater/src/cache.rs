use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::temporary::unique_temporary_path;
use crate::{
    verify_artifact, verify_signed_manifest, ArtifactDescriptor, ArtifactError, ManifestError,
    ReleaseManifestV1, TrustedPublicKey, MANIFEST_ASSET, SIGNATURES_ASSET,
};

#[derive(Debug)]
pub enum CacheError {
    Io(io::Error),
    Manifest(ManifestError),
    Artifact(ArtifactError),
    Rollback {
        cached_application: Version,
        candidate_application: Version,
        cached_firmware: u16,
        candidate_firmware: u16,
    },
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "update cache I/O failed: {error}"),
            Self::Manifest(error) => error.fmt(formatter),
            Self::Artifact(error) => error.fmt(formatter),
            Self::Rollback {
                cached_application,
                candidate_application,
                cached_firmware,
                candidate_firmware,
            } => write!(
                formatter,
                "refusing release metadata rollback from app {cached_application}/firmware {cached_firmware} to app {candidate_application}/firmware {candidate_firmware}"
            ),
        }
    }
}

impl std::error::Error for CacheError {}

impl From<io::Error> for CacheError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ManifestError> for CacheError {
    fn from(value: ManifestError) -> Self {
        Self::Manifest(value)
    }
}

impl From<ArtifactError> for CacheError {
    fn from(value: ArtifactError) -> Self {
        Self::Artifact(value)
    }
}

#[derive(Debug, Clone)]
pub struct ReleaseCache {
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct CachedMetadata {
    manifest: Vec<u8>,
    signatures: Vec<u8>,
}

impl ReleaseCache {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn for_current_user() -> Result<Self, CacheError> {
        let home =
            std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is unavailable"))?;
        Ok(Self::new(PathBuf::from(home).join(
            "Library/Application Support/Steam Controller Bridge/Updates",
        )))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_ASSET)
    }

    #[must_use]
    pub fn signatures_path(&self) -> PathBuf {
        self.root.join(SIGNATURES_ASSET)
    }

    fn metadata_path(&self) -> PathBuf {
        self.root.join("verified-release-metadata.json")
    }

    fn last_check_path(&self) -> PathBuf {
        self.root.join("last-check")
    }

    #[must_use]
    pub fn artifact_path(&self, artifact: &ArtifactDescriptor) -> PathBuf {
        self.root.join("artifacts").join(&artifact.name)
    }

    pub fn load_manifest(
        &self,
        trusted_keys: &[TrustedPublicKey],
    ) -> Result<ReleaseManifestV1, CacheError> {
        let cached: CachedMetadata = serde_json::from_slice(&fs::read(self.metadata_path())?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(verify_signed_manifest(
            &cached.manifest,
            &cached.signatures,
            trusted_keys,
        )?)
    }

    pub fn store_manifest(
        &self,
        manifest_bytes: &[u8],
        signature_bytes: &[u8],
        trusted_keys: &[TrustedPublicKey],
    ) -> Result<ReleaseManifestV1, CacheError> {
        let candidate = verify_signed_manifest(manifest_bytes, signature_bytes, trusted_keys)?;
        if let Ok(cached) = self.load_manifest(trusted_keys) {
            if candidate.firmware.revision < cached.firmware.revision
                || candidate.application_version < cached.application_version
            {
                return Err(CacheError::Rollback {
                    cached_application: cached.application_version,
                    candidate_application: candidate.application_version,
                    cached_firmware: cached.firmware.revision,
                    candidate_firmware: candidate.firmware.revision,
                });
            }
        }
        fs::create_dir_all(&self.root)?;
        let cached = serde_json::to_vec(&CachedMetadata {
            manifest: manifest_bytes.to_vec(),
            signatures: signature_bytes.to_vec(),
        })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        atomic_write(&self.metadata_path(), &cached)?;
        Ok(candidate)
    }

    pub fn verify_cached_artifact(
        &self,
        descriptor: &ArtifactDescriptor,
    ) -> Result<PathBuf, CacheError> {
        let path = self.artifact_path(descriptor);
        verify_artifact(&path, descriptor)?;
        Ok(path)
    }

    #[must_use]
    pub fn check_due(&self, interval: Duration) -> bool {
        let Ok(metadata) = fs::metadata(self.last_check_path()) else {
            return true;
        };
        let Ok(modified) = metadata.modified() else {
            return true;
        };
        SystemTime::now()
            .duration_since(modified)
            .map_or(true, |age| age >= interval)
    }

    pub(crate) fn mark_check_success(&self) -> Result<(), CacheError> {
        atomic_write(&self.last_check_path(), b"checked\n")?;
        Ok(())
    }
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("cache path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = unique_temporary_path(
        parent,
        path.file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("update")),
        "tmp",
    );
    let result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{signed_metadata, temporary_directory};

    #[test]
    fn due_when_missing_and_not_immediately_after_write() {
        let root = temporary_directory("cache");
        let _ = fs::remove_dir_all(&root);
        let cache = ReleaseCache::new(root.clone());
        assert!(cache.check_due(Duration::from_mins(1)));
        cache.mark_check_success().unwrap();
        assert!(!cache.check_due(Duration::from_mins(1)));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cache_replacement_is_atomic_and_refuses_either_version_rollback() {
        let root = temporary_directory("rollback");
        let _ = fs::remove_dir_all(&root);
        let cache = ReleaseCache::new(root.clone());
        let (manifest, signatures, key) = signed_metadata("1.5.0", 5);
        cache
            .store_manifest(&manifest, &signatures, std::slice::from_ref(&key))
            .unwrap();

        let (older_app, signatures, _) = signed_metadata("1.4.0", 6);
        assert!(matches!(
            cache.store_manifest(&older_app, &signatures, std::slice::from_ref(&key)),
            Err(CacheError::Rollback { .. })
        ));
        let (older_firmware, signatures, _) = signed_metadata("1.6.0", 4);
        assert!(matches!(
            cache.store_manifest(&older_firmware, &signatures, std::slice::from_ref(&key)),
            Err(CacheError::Rollback { .. })
        ));
        assert_eq!(
            cache.load_manifest(&[key]).unwrap().application_version,
            Version::new(1, 5, 0)
        );
        let _ = fs::remove_dir_all(root);
    }
}
