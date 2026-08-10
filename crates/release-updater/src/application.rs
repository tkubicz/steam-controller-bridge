use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;

use crate::{verify_artifact, ApplicationRelease, APPLICATION_BUNDLE_ID};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedApplication {
    pub bundle_path: PathBuf,
    pub version: Version,
}

#[must_use]
pub fn guided_replacement_supported() -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
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
    identifier == APPLICATION_BUNDLE_ID && version == env!("CARGO_PKG_VERSION")
}

pub fn installed_macos_version() -> Result<Version, String> {
    let output = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("sw_vers could not determine the macOS version".to_owned());
    }
    parse_version(String::from_utf8_lossy(&output.stdout).trim())
}

pub fn stage_application(
    archive: &Path,
    release: &ApplicationRelease,
    staging_root: &Path,
) -> Result<StagedApplication, String> {
    verify_artifact(archive, &release.artifact).map_err(|error| error.to_string())?;
    if staging_root.exists() {
        fs::remove_dir_all(staging_root).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(staging_root).map_err(|error| error.to_string())?;
    let output = Command::new("/usr/bin/ditto")
        .args(["-x", "-k"])
        .arg(archive)
        .arg(staging_root)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "cannot extract application update: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut entries = fs::read_dir(staging_root)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|error| error.to_string())?;
    if entries.len() != 1 {
        return Err("application archive must contain exactly one top-level item".to_owned());
    }
    let bundle_path = entries.remove(0).path();
    if bundle_path.file_name().and_then(|name| name.to_str()) != Some("Steam Controller Bridge.app")
        || !bundle_path.is_dir()
    {
        return Err("application archive does not contain Steam Controller Bridge.app".to_owned());
    }
    let plist = bundle_path.join("Contents/Info.plist");
    let bundle_identifier = plist_value(&plist, "CFBundleIdentifier")?;
    let version = parse_version(&plist_value(&plist, "CFBundleShortVersionString")?)?;
    if bundle_identifier != APPLICATION_BUNDLE_ID
        || bundle_identifier != release.bundle_identifier
        || version != release.version
    {
        return Err(
            "staged application identity or version does not match signed metadata".to_owned(),
        );
    }
    let verification = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&bundle_path)
        .output()
        .map_err(|error| error.to_string())?;
    if !verification.status.success() {
        return Err(format!(
            "staged application code signature is invalid: {}",
            String::from_utf8_lossy(&verification.stderr).trim()
        ));
    }
    Ok(StagedApplication {
        bundle_path,
        version,
    })
}

fn plist_value(path: &Path, key: &str) -> Result<String, String> {
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", &format!("Print :{key}")])
        .arg(path)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("application Info.plist has no {key}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn parse_version(value: &str) -> Result<Version, String> {
    let components = value.split('.').count();
    let normalized = match components {
        1 => format!("{value}.0.0"),
        2 => format!("{value}.0"),
        _ => value.to_owned(),
    };
    Version::parse(&normalized).map_err(|error| format!("invalid version {value:?}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_versions_are_normalized_without_accepting_invalid_versions() {
        assert_eq!(parse_version("13").unwrap(), Version::new(13, 0, 0));
        assert_eq!(parse_version("13.6").unwrap(), Version::new(13, 6, 0));
        assert!(parse_version("thirteen").is_err());
    }

    #[test]
    fn an_unbundled_test_binary_cannot_offer_guided_replacement() {
        assert!(!guided_replacement_supported());
    }
}
