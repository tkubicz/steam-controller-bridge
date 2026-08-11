use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use clap::Parser;
use ed25519_dalek::{Signer as _, SigningKey};
use release_updater::{
    sha256_hex, validate_uf2, verify_signed_manifest, ApplicationRelease, ArtifactDescriptor,
    FirmwareRelease, ManifestSignature, ReleaseManifestV1, ReleaseSignatures, TrustedPublicKey,
    APPLICATION_BUNDLE_ID, FIRMWARE_BOARD_ID, FIRMWARE_TARGET_ID, MANIFEST_ASSET, SIGNATURES_ASSET,
    UF2_FAMILY_ID, XIAO_USB_MANUFACTURER, XIAO_USB_PRODUCT, XIAO_USB_PRODUCT_ID,
    XIAO_USB_VENDOR_ID,
};
use semver::Version;

fn main() {
    if let Err(error) = run() {
        eprintln!("release manifest generation failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments = Arguments::try_parse().map_err(|error| error.to_string())?;
    let version = validate_release_identity(
        &arguments.release_tag,
        &arguments.version,
        &arguments.key_id,
    )?;
    let firmware_revision = parse_firmware_revision(&arguments.firmware_header)?;
    validate_uf2(&arguments.firmware, UF2_FAMILY_ID).map_err(|error| error.to_string())?;
    let application = artifact(&arguments.application)?;
    let firmware = artifact(&arguments.firmware)?;
    let manifest = ReleaseManifestV1 {
        schema_version: 1,
        release_tag: arguments.release_tag,
        application_version: version.clone(),
        minimum_macos: Version::new(13, 0, 0),
        release_notes: fs::read_to_string(arguments.release_notes)
            .map_err(|error| error.to_string())?,
        application: ApplicationRelease {
            bundle_identifier: APPLICATION_BUNDLE_ID.to_owned(),
            version: version.clone(),
            artifact: application,
        },
        firmware: FirmwareRelease {
            target: FIRMWARE_TARGET_ID.to_owned(),
            revision: firmware_revision,
            minimum_application_version: version,
            protocol_version: 1,
            device_info_format: 1,
            board_id: FIRMWARE_BOARD_ID.to_owned(),
            uf2_family_id: UF2_FAMILY_ID,
            usb_vendor_id: XIAO_USB_VENDOR_ID,
            usb_product_id: XIAO_USB_PRODUCT_ID,
            usb_manufacturer: XIAO_USB_MANUFACTURER.to_owned(),
            usb_product: XIAO_USB_PRODUCT.to_owned(),
            artifact: firmware,
        },
    };
    fs::create_dir_all(&arguments.output).map_err(|error| error.to_string())?;
    let mut manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    manifest_bytes.push(b'\n');
    let private = env::var("SC_BRIDGE_UPDATE_PRIVATE_KEY_B64")
        .map_err(|_| "SC_BRIDGE_UPDATE_PRIVATE_KEY_B64 is required".to_owned())?;
    let private = BASE64.decode(private).map_err(|error| error.to_string())?;
    let private: [u8; 32] = private
        .try_into()
        .map_err(|_| "Ed25519 private key must be exactly 32 bytes".to_owned())?;
    let signing = SigningKey::from_bytes(&private);
    if let Ok(expected_public) = env::var("SC_BRIDGE_UPDATE_PUBLIC_KEY_B64") {
        if BASE64.encode(signing.verifying_key().to_bytes()) != expected_public {
            return Err("private signing key does not match configured public key".to_owned());
        }
    }
    let mut signatures = vec![ManifestSignature {
        key_id: arguments.key_id,
        signature: BASE64.encode(signing.sign(&manifest_bytes).to_bytes()),
    }];
    if let Ok(additional) = env::var("SC_BRIDGE_UPDATE_ADDITIONAL_PRIVATE_KEYS") {
        for entry in additional.split(';').filter(|entry| !entry.is_empty()) {
            let (key_id, encoded) = entry
                .split_once('=')
                .ok_or("additional signing key must be key-id=base64")?;
            if key_id.is_empty() || signatures.iter().any(|item| item.key_id == key_id) {
                return Err("signing key ids must be non-empty and unique".to_owned());
            }
            let bytes: [u8; 32] = BASE64
                .decode(encoded)
                .map_err(|error| error.to_string())?
                .try_into()
                .map_err(|_| "additional Ed25519 private key must be 32 bytes")?;
            let key = SigningKey::from_bytes(&bytes);
            signatures.push(ManifestSignature {
                key_id: key_id.to_owned(),
                signature: BASE64.encode(key.sign(&manifest_bytes).to_bytes()),
            });
        }
    }
    let signatures = ReleaseSignatures {
        schema_version: 1,
        signatures,
    };
    let signature_bytes =
        serde_json::to_vec_pretty(&signatures).map_err(|error| error.to_string())?;
    verify_signed_manifest(
        &manifest_bytes,
        &signature_bytes,
        &[TrustedPublicKey {
            key_id: signatures.signatures[0].key_id.clone(),
            bytes: signing.verifying_key().to_bytes(),
        }],
    )
    .map_err(|error| format!("generated manifest did not self-verify: {error}"))?;
    write_public_key(
        arguments.public_key_output.as_deref(),
        &signatures.signatures[0].key_id,
        &signing,
    )?;
    write_release_metadata(&arguments.output, &manifest_bytes, &signature_bytes)
}

fn write_release_metadata(output: &Path, manifest: &[u8], signatures: &[u8]) -> Result<(), String> {
    fs::write(output.join(MANIFEST_ASSET), manifest).map_err(|error| error.to_string())?;
    fs::write(output.join(SIGNATURES_ASSET), signatures).map_err(|error| error.to_string())
}

fn write_public_key(path: Option<&Path>, key_id: &str, signing: &SigningKey) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };
    fs::write(
        path,
        format!(
            "{key_id}={}\n",
            BASE64.encode(signing.verifying_key().to_bytes())
        ),
    )
    .map_err(|error| error.to_string())
}

fn validate_release_identity(
    release_tag: &str,
    version: &str,
    key_id: &str,
) -> Result<Version, String> {
    if key_id.is_empty() {
        return Err("key id must not be empty".to_owned());
    }
    let version = Version::parse(version).map_err(|error| error.to_string())?;
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err("release version must be a stable SemVer core version".to_owned());
    }
    if release_tag != format!("v{version}") {
        return Err("release tag and version disagree".to_owned());
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
    output: PathBuf,
    #[arg(long)]
    key_id: String,
    /// Optional `key-id=base64` trust-anchor file for a local development build.
    #[arg(long)]
    public_key_output: Option<PathBuf>,
}

fn artifact(path: &Path) -> Result<ArtifactDescriptor, String> {
    Ok(ArtifactDescriptor {
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("artifact name is not UTF-8")?
            .to_owned(),
        size: path.metadata().map_err(|error| error.to_string())?.len(),
        sha256: sha256_hex(path).map_err(|error| error.to_string())?,
    })
}

fn parse_firmware_revision(path: &Path) -> Result<u16, String> {
    let source = fs::read_to_string(path).map_err(|error| error.to_string())?;
    parse_firmware_revision_source(&source)
}

fn parse_firmware_revision_source(source: &str) -> Result<u16, String> {
    let marker = "constexpr uint16_t kFirmwareRevision = ";
    let matches = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix(marker))
        .map(|value| value.trim_end_matches(';').parse::<u16>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    match matches.as_slice() {
        [revision] => Ok(*revision),
        _ => Err("firmware header must define kFirmwareRevision exactly once".to_owned()),
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
    }
}
