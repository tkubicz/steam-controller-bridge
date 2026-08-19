use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Enumerates roots that may contain removable firmware volumes.
pub trait RemovableVolumeLocator: Send + Sync {
    /// Returns the roots that were discovered and any non-fatal scan warnings.
    ///
    /// # Errors
    ///
    /// Returns [`VolumeScanError`] when the platform's volume directory cannot
    /// be opened at all. Individual entries that disappear or cannot be read
    /// are reported through [`VolumeScan::warnings`] so usable roots remain
    /// available.
    fn enumerate(&self) -> Result<VolumeScan, VolumeScanError>;
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VolumeScan {
    pub roots: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum VolumeScanError {
    #[error("cannot enumerate removable volumes under {}: {source}", root.display())]
    Enumerate {
        root: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("removable-volume discovery is not available on {platform}")]
    UnsupportedPlatform { platform: &'static str },
}

/// The existing macOS removable-volume policy rooted at `/Volumes`.
#[derive(Debug, Clone)]
pub struct MacOsVolumeLocator {
    root: PathBuf,
}

impl Default for MacOsVolumeLocator {
    fn default() -> Self {
        Self {
            root: PathBuf::from("/Volumes"),
        }
    }
}

impl MacOsVolumeLocator {
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl RemovableVolumeLocator for MacOsVolumeLocator {
    fn enumerate(&self) -> Result<VolumeScan, VolumeScanError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(VolumeScan::default());
            }
            Err(source) => {
                return Err(VolumeScanError::Enumerate {
                    root: self.root.clone(),
                    source,
                });
            }
        };
        Ok(collect_scan(
            &self.root,
            entries.map(|entry| entry.map(|entry| entry.path())),
        ))
    }
}

/// Selects the removable-volume provider for the current host.
///
/// # Errors
///
/// Returns [`VolumeScanError::UnsupportedPlatform`] until a provider exists
/// for the current target.
pub fn current_removable_volume_locator() -> Result<Box<dyn RemovableVolumeLocator>, VolumeScanError>
{
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(MacOsVolumeLocator::default()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(VolumeScanError::UnsupportedPlatform {
            platform: std::env::consts::OS,
        })
    }
}

fn collect_scan(
    root: &Path,
    candidates: impl IntoIterator<Item = Result<PathBuf, io::Error>>,
) -> VolumeScan {
    let mut scan = VolumeScan::default();
    for candidate in candidates {
        match candidate {
            Ok(path) => scan.roots.push(path),
            Err(error) => scan.warnings.push(format!(
                "cannot inspect a removable-volume entry under {}: {error}",
                root.display()
            )),
        }
    }
    scan.roots.sort();
    scan.roots.dedup();
    scan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_locator_preserves_the_existing_root_and_sorted_children() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir_all(temporary.path().join("ZED")).unwrap();
        fs::create_dir_all(temporary.path().join("ALPHA")).unwrap();

        let locator = MacOsVolumeLocator::with_root(temporary.path());
        assert_eq!(locator.root(), temporary.path());
        assert_eq!(
            locator.enumerate().unwrap(),
            VolumeScan {
                roots: vec![temporary.path().join("ALPHA"), temporary.path().join("ZED")],
                warnings: Vec::new(),
            }
        );
    }

    #[test]
    fn candidate_scan_deduplicates_roots_and_keeps_partial_failures() {
        let root = Path::new("/Volumes");
        let duplicate = root.join("XIAO");
        let scan = collect_scan(
            root,
            [
                Ok(root.join("OTHER")),
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "fixture denied",
                )),
                Ok(duplicate.clone()),
                Ok(duplicate),
            ],
        );

        assert_eq!(scan.roots, vec![root.join("OTHER"), root.join("XIAO")]);
        assert_eq!(scan.warnings.len(), 1);
        assert!(scan.warnings[0].contains("/Volumes"));
        assert!(scan.warnings[0].contains("fixture denied"));
    }

    #[test]
    fn missing_volume_root_is_an_empty_scan() {
        let temporary = tempfile::tempdir().unwrap();
        let locator = MacOsVolumeLocator::with_root(temporary.path().join("missing"));
        assert_eq!(locator.enumerate().unwrap(), VolumeScan::default());
    }

    #[test]
    fn unreadable_volume_root_is_a_typed_error() {
        let temporary = tempfile::tempdir().unwrap();
        let file = temporary.path().join("not-a-directory");
        fs::write(&file, b"fixture").unwrap();

        let error = MacOsVolumeLocator::with_root(&file)
            .enumerate()
            .unwrap_err();
        assert!(matches!(
            error,
            VolumeScanError::Enumerate { root, .. } if root == file
        ));
    }
}
