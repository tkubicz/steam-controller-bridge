use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;
use thiserror::Error;

use crate::{verify_artifact, ApplicationRelease, ArtifactError, APPLICATION_BUNDLE_ID};

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("application update I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error("{0}")]
    Invalid(String),
    #[error("invalid version {value:?}: {source}")]
    Version {
        value: String,
        #[source]
        source: semver::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedApplication {
    pub bundle_path: PathBuf,
    pub version: Version,
}

/// The caller supplies the running application's package version so bundle
/// validation is tied to the executable being assessed, not this library.
#[must_use]
pub fn guided_replacement_supported(running_version: &str) -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    guided_replacement_supported_at(&executable, running_version)
}

fn guided_replacement_supported_at(executable: &Path, running_version: &str) -> bool {
    let Some(contents) = executable.parent().and_then(Path::parent) else {
        return false;
    };
    let Some(bundle) = contents.parent() else {
        return false;
    };
    if bundle.file_name().and_then(|name| name.to_str()) != Some("Steam Controller Bridge.app") {
        return false;
    }
    let plist = contents.join("Info.plist");
    let Ok(identifier) = plist_value(&plist, "CFBundleIdentifier") else {
        return false;
    };
    let Ok(version) = plist_value(&plist, "CFBundleShortVersionString") else {
        return false;
    };
    identifier == APPLICATION_BUNDLE_ID && version == running_version
}

pub fn installed_macos_version() -> Result<Version, ApplicationError> {
    let output = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()?;
    if !output.status.success() {
        return Err(ApplicationError::Invalid(
            "sw_vers could not determine the macOS version".to_owned(),
        ));
    }
    parse_version(String::from_utf8_lossy(&output.stdout).trim())
}

pub fn stage_application(
    archive: &Path,
    release: &ApplicationRelease,
    staging_root: &Path,
) -> Result<StagedApplication, ApplicationError> {
    verify_artifact(archive, &release.artifact)?;
    if staging_root.exists() {
        fs::remove_dir_all(staging_root)?;
    }
    fs::create_dir_all(staging_root)?;
    let output = Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(archive)
        .arg(staging_root)
        .output()?;
    if !output.status.success() {
        return Err(ApplicationError::Invalid(format!(
            "cannot extract application update: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let mut entries = fs::read_dir(staging_root)?.collect::<Result<Vec<_>, io::Error>>()?;
    if entries.len() != 1 {
        return Err(ApplicationError::Invalid(
            "application archive must contain exactly one top-level item".to_owned(),
        ));
    }
    let bundle_path = entries.remove(0).path();
    if bundle_path.file_name().and_then(|name| name.to_str()) != Some("Steam Controller Bridge.app")
        || !bundle_path.is_dir()
    {
        return Err(ApplicationError::Invalid(
            "application archive does not contain Steam Controller Bridge.app".to_owned(),
        ));
    }
    let plist = bundle_path.join("Contents/Info.plist");
    let bundle_identifier = plist_value(&plist, "CFBundleIdentifier")?;
    let version = parse_version(&plist_value(&plist, "CFBundleShortVersionString")?)?;
    if bundle_identifier != APPLICATION_BUNDLE_ID
        || bundle_identifier != release.bundle_identifier
        || version != release.version
    {
        return Err(ApplicationError::Invalid(
            "staged application identity or version does not match signed metadata".to_owned(),
        ));
    }
    let verification = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&bundle_path)
        .output()?;
    if !verification.status.success() {
        return Err(ApplicationError::Invalid(format!(
            "staged application code signature is invalid: {}",
            String::from_utf8_lossy(&verification.stderr).trim()
        )));
    }
    Ok(StagedApplication {
        bundle_path,
        version,
    })
}

fn plist_value(path: &Path, key: &str) -> Result<String, ApplicationError> {
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", &format!("Print :{key}")])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(ApplicationError::Invalid(format!(
            "application Info.plist has no {key}"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn parse_version(value: &str) -> Result<Version, ApplicationError> {
    let components = value.split('.').count();
    let normalized = match components {
        1 => format!("{value}.0.0"),
        2 => format!("{value}.0"),
        _ => value.to_owned(),
    };
    Version::parse(&normalized).map_err(|source| ApplicationError::Version {
        value: value.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_bundle(version: &str) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let contents = root.path().join("Steam Controller Bridge.app/Contents");
        let executable = contents.join("MacOS/sc-bridge-menu");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{APPLICATION_BUNDLE_ID}</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>"#
            ),
        )
        .unwrap();
        (root, executable)
    }

    #[test]
    fn macos_versions_are_normalized_without_accepting_invalid_versions() {
        assert_eq!(parse_version("13").unwrap(), Version::new(13, 0, 0));
        assert_eq!(parse_version("13.6").unwrap(), Version::new(13, 6, 0));
        assert!(parse_version("thirteen").is_err());
    }

    #[test]
    fn an_unbundled_test_binary_cannot_offer_guided_replacement() {
        assert!(!guided_replacement_supported(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn guided_replacement_uses_the_calling_app_version() {
        let (_root, executable) = fixture_bundle("9.8.7");
        assert!(guided_replacement_supported_at(&executable, "9.8.7"));
        assert!(!guided_replacement_supported_at(&executable, "1.4.0"));
    }
}
