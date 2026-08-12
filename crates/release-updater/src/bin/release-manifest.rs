use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use clap::Parser;
use ed25519_dalek::{Signer as _, SigningKey};
use release_updater::{
    firmware_target, sha256_hex, validate_uf2, verify_signed_manifest, ApplicationRelease,
    ArtifactDescriptor, ArtifactError, FirmwareFlashError, FirmwareTargetCatalogError,
    ManifestError, ManifestSignature, ReleaseManifestV1, ReleaseSignatures, TrustedPublicKey,
    APPLICATION_BUNDLE_ID, MANIFEST_ASSET, SIGNATURES_ASSET,
};
use semver::Version;
use thiserror::Error;

fn main() {
    if let Err(error) = run() {
        eprintln!("release manifest generation failed: {error}");
        std::process::exit(1);
    }
}

#[derive(Debug, Error)]
enum ManifestGenerationError {
    #[error(transparent)]
    Arguments(#[from] clap::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Semver(#[from] semver::Error),
    #[error(transparent)]
    Base64(#[from] base64::DecodeError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Firmware(#[from] FirmwareFlashError),
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(transparent)]
    TargetCatalog(#[from] FirmwareTargetCatalogError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("unsupported firmware target: {0}")]
    UnsupportedTarget(String),
    #[error("{0}")]
    Invalid(String),
}

fn run() -> Result<(), ManifestGenerationError> {
    let arguments = Arguments::try_parse()?;
    let version = validate_release_identity(
        &arguments.release_tag,
        &arguments.version,
        &arguments.key_id,
    )?;
    let firmware_revision = parse_firmware_revision(&arguments.firmware_header)?;
    let target = firmware_target(&arguments.firmware_target)?.ok_or_else(|| {
        ManifestGenerationError::UnsupportedTarget(arguments.firmware_target.clone())
    })?;
    validate_uf2(&arguments.firmware, target.uf2_family_id)?;
    let application = artifact(&arguments.application)?;
    let firmware = artifact(&arguments.firmware)?;
    let firmware = target.firmware_release(firmware_revision, version.clone(), firmware);
    let manifest = ReleaseManifestV1 {
        schema_version: 1,
        release_tag: arguments.release_tag,
        application_version: version.clone(),
        minimum_macos: Version::new(13, 0, 0),
        release_notes: fs::read_to_string(arguments.release_notes)?,
        application: ApplicationRelease {
            bundle_identifier: APPLICATION_BUNDLE_ID.to_owned(),
            version: version.clone(),
            artifact: application,
        },
        firmware,
    };
    fs::create_dir_all(&arguments.output)?;
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    manifest_bytes.push(b'\n');
    let (signatures, signing) = sign_manifest(&manifest_bytes, &arguments.key_id)?;
    let signature_bytes = serde_json::to_vec_pretty(&signatures)?;
    verify_signed_manifest(
        &manifest_bytes,
        &signature_bytes,
        &[TrustedPublicKey {
            key_id: signatures.signatures[0].key_id.clone(),
            bytes: signing.verifying_key().to_bytes(),
        }],
    )
    .map_err(|error| {
        ManifestGenerationError::Invalid(format!("generated manifest did not self-verify: {error}"))
    })?;
    write_public_key(
        arguments.public_key_output.as_deref(),
        &signatures.signatures[0].key_id,
        &signing,
    )?;
    write_release_metadata(&arguments.output, &manifest_bytes, &signature_bytes)
}

fn sign_manifest(
    manifest_bytes: &[u8],
    key_id: &str,
) -> Result<(ReleaseSignatures, SigningKey), ManifestGenerationError> {
    let private = env::var("SC_BRIDGE_UPDATE_PRIVATE_KEY_B64").map_err(|_| {
        ManifestGenerationError::Invalid("SC_BRIDGE_UPDATE_PRIVATE_KEY_B64 is required".to_owned())
    })?;
    let private = BASE64.decode(private)?;
    let private: [u8; 32] = private.try_into().map_err(|_| {
        ManifestGenerationError::Invalid("Ed25519 private key must be exactly 32 bytes".to_owned())
    })?;
    let signing = SigningKey::from_bytes(&private);
    if let Ok(expected_public) = env::var("SC_BRIDGE_UPDATE_PUBLIC_KEY_B64") {
        if BASE64.encode(signing.verifying_key().to_bytes()) != expected_public {
            return Err(ManifestGenerationError::Invalid(
                "private signing key does not match configured public key".to_owned(),
            ));
        }
    }
    let mut signatures = vec![ManifestSignature {
        key_id: key_id.to_owned(),
        signature: BASE64.encode(signing.sign(manifest_bytes).to_bytes()),
    }];
    if let Ok(additional) = env::var("SC_BRIDGE_UPDATE_ADDITIONAL_PRIVATE_KEYS") {
        for entry in additional.split(';').filter(|entry| !entry.is_empty()) {
            let (key_id, encoded) = entry.split_once('=').ok_or_else(|| {
                ManifestGenerationError::Invalid(
                    "additional signing key must be key-id=base64".to_owned(),
                )
            })?;
            if key_id.is_empty() || signatures.iter().any(|item| item.key_id == key_id) {
                return Err(ManifestGenerationError::Invalid(
                    "signing key ids must be non-empty and unique".to_owned(),
                ));
            }
            let bytes: [u8; 32] = BASE64.decode(encoded)?.try_into().map_err(|_| {
                ManifestGenerationError::Invalid(
                    "additional Ed25519 private key must be 32 bytes".to_owned(),
                )
            })?;
            let key = SigningKey::from_bytes(&bytes);
            signatures.push(ManifestSignature {
                key_id: key_id.to_owned(),
                signature: BASE64.encode(key.sign(manifest_bytes).to_bytes()),
            });
        }
    }
    Ok((
        ReleaseSignatures {
            schema_version: 1,
            signatures,
        },
        signing,
    ))
}

fn write_release_metadata(
    output: &Path,
    manifest: &[u8],
    signatures: &[u8],
) -> Result<(), ManifestGenerationError> {
    fs::write(output.join(MANIFEST_ASSET), manifest)?;
    fs::write(output.join(SIGNATURES_ASSET), signatures)?;
    Ok(())
}

fn write_public_key(
    path: Option<&Path>,
    key_id: &str,
    signing: &SigningKey,
) -> Result<(), ManifestGenerationError> {
    let Some(path) = path else {
        return Ok(());
    };
    fs::write(
        path,
        format!(
            "{key_id}={}\n",
            BASE64.encode(signing.verifying_key().to_bytes())
        ),
    )?;
    Ok(())
}

fn validate_release_identity(
    release_tag: &str,
    version: &str,
    key_id: &str,
) -> Result<Version, ManifestGenerationError> {
    if key_id.is_empty() {
        return Err(ManifestGenerationError::Invalid(
            "key id must not be empty".to_owned(),
        ));
    }
    let version = Version::parse(version)?;
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err(ManifestGenerationError::Invalid(
            "release version must be a stable SemVer core version".to_owned(),
        ));
    }
    if release_tag != format!("v{version}") {
        return Err(ManifestGenerationError::Invalid(
            "release tag and version disagree".to_owned(),
        ));
    }
    Ok(version)
}

#[derive(Parser)]
#[command(name = "release-manifest")]
struct Arguments {
    #[arg(long)]
    release_tag: String,
    #[arg(long)]
    version: String,
    #[arg(long)]
    release_notes: PathBuf,
    #[arg(long)]
    application: PathBuf,
    #[arg(long)]
    firmware: PathBuf,
    #[arg(long)]
    firmware_header: PathBuf,
    #[arg(long)]
    firmware_target: String,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    key_id: String,
    /// Optional `key-id=base64` trust-anchor file for a local development build.
    #[arg(long)]
    public_key_output: Option<PathBuf>,
}

fn artifact(path: &Path) -> Result<ArtifactDescriptor, ManifestGenerationError> {
    Ok(ArtifactDescriptor {
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                ManifestGenerationError::Invalid("artifact name is not UTF-8".to_owned())
            })?
            .to_owned(),
        size: path.metadata()?.len(),
        sha256: sha256_hex(path)?,
    })
}

fn parse_firmware_revision(path: &Path) -> Result<u16, ManifestGenerationError> {
    let source = fs::read_to_string(path)?;
    parse_firmware_revision_source(&source)
}

fn parse_firmware_revision_source(source: &str) -> Result<u16, ManifestGenerationError> {
    let marker = "constexpr uint16_t kFirmwareRevision = ";
    let matches = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix(marker))
        .map(|value| {
            value
                .strip_suffix(';')
                .ok_or_else(|| {
                    ManifestGenerationError::Invalid(
                        "firmware revision declaration must end with one semicolon".to_owned(),
                    )
                })?
                .parse::<u16>()
                .map_err(|error| ManifestGenerationError::Invalid(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    match matches.as_slice() {
        [revision] => Ok(*revision),
        _ => Err(ManifestGenerationError::Invalid(
            "firmware header must define kFirmwareRevision exactly once".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_identity_requires_one_stable_version() {
        assert_eq!(
            validate_release_identity("v1.5.0", "1.5.0", "release-2026").expect("valid release"),
            Version::new(1, 5, 0)
        );
        assert!(validate_release_identity("v1.5.1", "1.5.0", "release-2026").is_err());
        assert!(validate_release_identity("v1.5.0-rc.1", "1.5.0-rc.1", "release-2026").is_err());
        assert!(validate_release_identity("v1.5.0", "1.5.0", "").is_err());
    }

    #[test]
    fn firmware_revision_requires_one_exact_declaration() {
        assert_eq!(
            parse_firmware_revision_source("constexpr uint16_t kFirmwareRevision = 7;\n")
                .expect("valid revision"),
            7
        );
        assert!(parse_firmware_revision_source("").is_err());
        assert!(parse_firmware_revision_source(
            "constexpr uint16_t kFirmwareRevision = 7;\nconstexpr uint16_t kFirmwareRevision = 8;\n"
        )
        .is_err());
        assert!(parse_firmware_revision_source(
            "constexpr uint16_t kFirmwareRevision = not_a_number;\n"
        )
        .is_err());
        for invalid in [
            "constexpr uint16_t kFirmwareRevision = 7;;\n",
            "constexpr uint16_t kFirmwareRevision = 7; trailing\n",
            "constexpr uint16_t kFirmwareRevision = 7;\nconstexpr uint16_t kFirmwareRevision = invalid;\n",
        ] {
            assert!(parse_firmware_revision_source(invalid).is_err());
        }
    }

    #[test]
    fn firmware_target_is_a_required_release_input() {
        let common = [
            "release-manifest",
            "--release-tag",
            "v1.7.0",
            "--version",
            "1.7.0",
            "--release-notes",
            "notes.md",
            "--application",
            "app.zip",
            "--firmware",
            "firmware.uf2",
            "--firmware-header",
            "firmware_version.h",
            "--output",
            "release",
            "--key-id",
            "fixture",
        ];
        assert!(Arguments::try_parse_from(common).is_err());
        assert!(Arguments::try_parse_from(
            common
                .into_iter()
                .chain(["--firmware-target", "seeed-xiao-nrf52840"]),
        )
        .is_ok());
    }
}
