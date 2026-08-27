use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use semver::Version;
use serde::{Deserialize, Serialize};
#[cfg(feature = "local-update-source")]
use sha2::{Digest as _, Sha256};
use tempfile::Builder as TempFileBuilder;
use thiserror::Error;

#[cfg(feature = "local-update-source")]
use crate::artifact::lower_hex;
use crate::{
    verify_artifact, verify_signed_manifest, ArtifactDescriptor, ArtifactError, ManifestError,
    ReleaseManifestV1, TrustedPublicKey,
};

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("update cache I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error("update cache metadata is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("refusing release metadata rollback from app {cached_application}/firmware {cached_firmware} to app {candidate_application}/firmware {candidate_firmware}")]
    Rollback {
        cached_application: Version,
        candidate_application: Version,
        cached_firmware: u16,
        candidate_firmware: u16,
    },
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
        let paths = app_paths::current()
            .map_err(|error| io::Error::other(format!("cannot locate update cache: {error}")))?;
        Ok(Self::new(paths.cache_dir))
    }

    #[cfg(feature = "local-update-source")]
    #[must_use]
    pub fn for_local_source(root: &Path) -> Self {
        let digest = Sha256::digest(root.as_os_str().as_encoded_bytes());
        let identifier = lower_hex(&digest);
        Self::new(
            std::env::temp_dir()
                .join("steam-controller-bridge-local-updates")
                .join(identifier),
        )
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
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
        let cached: CachedMetadata = serde_json::from_slice(&fs::read(self.metadata_path())?)?;
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
        })?;
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
    pub fn check_due(&self, interval: Duration, running_application: &Version) -> bool {
        let Ok(metadata) = fs::metadata(self.last_check_path()) else {
            return true;
        };
        let Ok(modified) = metadata.modified() else {
            return true;
        };
        let recent = SystemTime::now()
            .duration_since(modified)
            .is_ok_and(|age| age < interval);
        if !recent {
            return true;
        }
        fs::read_to_string(self.last_check_path()).map_or(true, |checked_application| {
            checked_application.trim() != running_application.to_string()
        })
    }

    pub(crate) fn mark_check_success(
        &self,
        running_application: &Version,
    ) -> Result<(), CacheError> {
        atomic_write(
            &self.last_check_path(),
            format!("{running_application}\n").as_bytes(),
        )?;
        Ok(())
    }
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("cache path has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut temporary = TempFileBuilder::new()
        .prefix(".release-updater-cache-")
        .tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{signed_metadata, temporary_directory};

    #[test]
    fn current_user_cache_uses_app_path_policy() {
        let expected = app_paths::current().unwrap().cache_dir;
        assert_eq!(ReleaseCache::for_current_user().unwrap().root(), expected);
    }

    #[test]
    fn due_when_missing_and_not_immediately_after_write() {
        let root = temporary_directory("cache");
        let cache = ReleaseCache::new(root.path().to_owned());
        let current = Version::new(1, 5, 0);
        let upgraded = Version::new(1, 6, 0);
        assert!(cache.check_due(Duration::from_mins(1), &current));
        cache.mark_check_success(&current).unwrap();
        assert!(!cache.check_due(Duration::from_mins(1), &current));
        assert!(cache.check_due(Duration::from_mins(1), &upgraded));
    }

    /// Local update sources only exist in `local-update-source` builds.
    #[cfg(feature = "local-update-source")]
    #[test]
    fn local_sources_use_stable_isolated_temporary_caches() {
        let first = temporary_directory("local-cache-first");
        let second = temporary_directory("local-cache-second");
        let first_cache = ReleaseCache::for_local_source(first.path());
        assert_eq!(
            first_cache.root(),
            ReleaseCache::for_local_source(first.path()).root()
        );
        assert_ne!(
            first_cache.root(),
            ReleaseCache::for_local_source(second.path()).root()
        );
        assert!(first_cache
            .root()
            .starts_with(std::env::temp_dir().join("steam-controller-bridge-local-updates")));
    }

    #[test]
    fn cache_replacement_is_atomic_and_refuses_either_version_rollback() {
        let root = temporary_directory("rollback");
        let cache = ReleaseCache::new(root.path().to_owned());
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
    }
}
