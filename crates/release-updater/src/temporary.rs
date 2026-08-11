use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn unique_temporary_path(parent: &Path, stem: &OsStr, purpose: &str) -> PathBuf {
    parent.join(format!(
        ".{}.{}.{}.{}",
        stem.to_string_lossy(),
        std::process::id(),
        TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        purpose
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_unique_and_stay_below_the_requested_parent() {
        let parent = Path::new("/tmp/updater-test");
        let first = unique_temporary_path(parent, OsStr::new("catalog"), "download");
        let second = unique_temporary_path(parent, OsStr::new("catalog"), "download");
        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(parent));
        assert!(first
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with('.'));
    }
}
