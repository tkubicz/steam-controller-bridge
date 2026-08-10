use std::collections::HashSet;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{APPLICATION_BUNDLE_ID, FIRMWARE_BOARD_ID, FIRMWARE_TARGET_ID, UF2_FAMILY_ID};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifestV1 {
    pub schema_version: u32,
    pub release_tag: String,
    pub application_version: Version,
    pub minimum_macos: Version,
    pub release_notes: String,
    pub application: ApplicationRelease,
    pub firmware: FirmwareRelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationRelease {
    pub bundle_identifier: String,
    pub version: Version,
    pub artifact: ArtifactDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirmwareRelease {
    pub target: String,
    pub revision: u16,
    pub minimum_application_version: Version,
    pub protocol_version: u8,
    pub device_info_format: u8,
    pub board_id: String,
    pub uf2_family_id: u32,
    pub usb_vendor_id: u16,
    pub usb_product_id: u16,
    pub usb_manufacturer: String,
    pub usb_product: String,
    pub artifact: ArtifactDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDescriptor {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSignatures {
    pub schema_version: u32,
    pub signatures: Vec<ManifestSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSignature {
    pub key_id: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPublicKey {
    pub key_id: String,
    pub bytes: [u8; 32],
}

/// Public keys injected into release builds as
/// `key-id=base64;next-key=base64`. Source builds deliberately have no update
/// authority unless their builder opts in.
pub fn embedded_trusted_keys() -> Result<Vec<TrustedPublicKey>, ManifestError> {
    let Some(encoded) = option_env!("SC_BRIDGE_UPDATE_PUBLIC_KEYS") else {
        return Ok(Vec::new());
    };
    encoded
        .split(';')
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (key_id, value) = entry.split_once('=').ok_or_else(|| {
                ManifestError::InvalidField("embedded update key has no id".to_owned())
            })?;
            let bytes = BASE64.decode(value).map_err(|error| {
                ManifestError::InvalidField(format!("invalid embedded update key: {error}"))
            })?;
            let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
                ManifestError::InvalidField(
                    "embedded Ed25519 public key must be 32 bytes".to_owned(),
                )
            })?;
            Ok(TrustedPublicKey {
                key_id: key_id.to_owned(),
                bytes,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    InvalidSignatureEnvelope(String),
    NoTrustedSignature,
    InvalidJson(String),
    InvalidField(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSignatureEnvelope(error) => {
                write!(formatter, "invalid release signature envelope: {error}")
            }
            Self::NoTrustedSignature => write!(formatter, "release has no trusted signature"),
            Self::InvalidJson(error) => {
                write!(formatter, "invalid signed release manifest: {error}")
            }
            Self::InvalidField(error) => {
                write!(formatter, "invalid release manifest field: {error}")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

/// Verifies at least one signature over the exact manifest bytes before JSON
/// parsing. Unknown key ids are ignored to allow staged key rotation.
pub fn verify_signed_manifest(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    trusted_keys: &[TrustedPublicKey],
) -> Result<ReleaseManifestV1, ManifestError> {
    let envelope: ReleaseSignatures = serde_json::from_slice(signature_bytes)
        .map_err(|error| ManifestError::InvalidSignatureEnvelope(error.to_string()))?;
    if envelope.schema_version != 1 || envelope.signatures.is_empty() {
        return Err(ManifestError::InvalidSignatureEnvelope(
            "schema must be 1 and signatures must not be empty".to_owned(),
        ));
    }
    let mut seen = HashSet::new();
    let mut verified = false;
    for candidate in &envelope.signatures {
        if candidate.key_id.is_empty() || !seen.insert(candidate.key_id.as_str()) {
            return Err(ManifestError::InvalidSignatureEnvelope(
                "signature key ids must be non-empty and unique".to_owned(),
            ));
        }
        let Some(key) = trusted_keys
            .iter()
            .find(|key| key.key_id == candidate.key_id)
        else {
            continue;
        };
        let Ok(signature_bytes) = BASE64.decode(&candidate.signature) else {
            continue;
        };
        let Ok(signature) = Signature::from_slice(&signature_bytes) else {
            continue;
        };
        let Ok(verifying_key) = VerifyingKey::from_bytes(&key.bytes) else {
            continue;
        };
        if verifying_key.verify(manifest_bytes, &signature).is_ok() {
            verified = true;
            break;
        }
    }
    if !verified {
        return Err(ManifestError::NoTrustedSignature);
    }

    let manifest: ReleaseManifestV1 = serde_json::from_slice(manifest_bytes)
        .map_err(|error| ManifestError::InvalidJson(error.to_string()))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_manifest(manifest: &ReleaseManifestV1) -> Result<(), ManifestError> {
    if manifest.schema_version != 1 {
        return Err(ManifestError::InvalidField(
            "schema_version must be 1".to_owned(),
        ));
    }
    if manifest.release_tag != format!("v{}", manifest.application_version) {
        return Err(ManifestError::InvalidField(
            "release_tag must be v followed by application_version".to_owned(),
        ));
    }
    if manifest.application.version != manifest.application_version
        || manifest.application.bundle_identifier != APPLICATION_BUNDLE_ID
    {
        return Err(ManifestError::InvalidField(
            "application identity/version mismatch".to_owned(),
        ));
    }
    if manifest.firmware.target != FIRMWARE_TARGET_ID
        || manifest.firmware.board_id != FIRMWARE_BOARD_ID
        || manifest.firmware.uf2_family_id != UF2_FAMILY_ID
    {
        return Err(ManifestError::InvalidField(
            "unsupported firmware target".to_owned(),
        ));
    }
    if manifest.firmware.protocol_version != 1 || manifest.firmware.device_info_format != 1 {
        return Err(ManifestError::InvalidField(
            "unsupported firmware protocol or device-info format".to_owned(),
        ));
    }
    if manifest.firmware.minimum_application_version > manifest.application_version
        || manifest.firmware.usb_vendor_id != 0x045e
        || manifest.firmware.usb_product_id != 0x028e
        || manifest.firmware.usb_manufacturer != "Lynxware"
        || manifest.firmware.usb_product != "Steam Controller Bridge"
    {
        return Err(ManifestError::InvalidField(
            "incompatible firmware/application or USB identity".to_owned(),
        ));
    }
    validate_artifact(&manifest.application.artifact, 100 * 1024 * 1024)?;
    validate_artifact(&manifest.firmware.artifact, 4 * 1024 * 1024)?;
    if manifest.release_notes.len() > 64 * 1024 {
        return Err(ManifestError::InvalidField(
            "release notes exceed 64 KiB".to_owned(),
        ));
    }
    Ok(())
}

fn validate_artifact(
    artifact: &ArtifactDescriptor,
    maximum_size: u64,
) -> Result<(), ManifestError> {
    if artifact.name.is_empty()
        || artifact.name.contains('/')
        || artifact.name.contains('\\')
        || artifact.size == 0
        || artifact.size > maximum_size
        || artifact.sha256.len() != 64
        || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ManifestError::InvalidField(format!(
            "invalid artifact {}",
            artifact.name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};

    fn manifest() -> ReleaseManifestV1 {
        ReleaseManifestV1 {
            schema_version: 1,
            release_tag: "v1.5.0".to_owned(),
            application_version: Version::new(1, 5, 0),
            minimum_macos: Version::new(13, 0, 0),
            release_notes: "Safe update".to_owned(),
            application: ApplicationRelease {
                bundle_identifier: APPLICATION_BUNDLE_ID.to_owned(),
                version: Version::new(1, 5, 0),
                artifact: ArtifactDescriptor {
                    name: "steam-controller-bridge-macos.zip".to_owned(),
                    size: 12,
                    sha256: "11".repeat(32),
                },
            },
            firmware: FirmwareRelease {
                target: FIRMWARE_TARGET_ID.to_owned(),
                revision: 2,
                minimum_application_version: Version::new(1, 5, 0),
                protocol_version: 1,
                device_info_format: 1,
                board_id: FIRMWARE_BOARD_ID.to_owned(),
                uf2_family_id: UF2_FAMILY_ID,
                usb_vendor_id: 0x045e,
                usb_product_id: 0x028e,
                usb_manufacturer: "Lynxware".to_owned(),
                usb_product: "Steam Controller Bridge".to_owned(),
                artifact: ArtifactDescriptor {
                    name: "steam-controller-bridge-xiao-nrf52840.uf2".to_owned(),
                    size: 24,
                    sha256: "22".repeat(32),
                },
            },
        }
    }

    fn signed(manifest: &ReleaseManifestV1) -> (Vec<u8>, Vec<u8>, TrustedPublicKey) {
        let bytes = serde_json::to_vec(manifest).unwrap();
        let signing = SigningKey::from_bytes(&[7; 32]);
        let envelope = ReleaseSignatures {
            schema_version: 1,
            signatures: vec![ManifestSignature {
                key_id: "fixture".to_owned(),
                signature: BASE64.encode(signing.sign(&bytes).to_bytes()),
            }],
        };
        (
            bytes,
            serde_json::to_vec(&envelope).unwrap(),
            TrustedPublicKey {
                key_id: "fixture".to_owned(),
                bytes: signing.verifying_key().to_bytes(),
            },
        )
    }

    #[test]
    fn verifies_before_parsing_and_rejects_tampering() {
        let expected = manifest();
        let (bytes, signatures, key) = signed(&expected);
        assert_eq!(
            verify_signed_manifest(&bytes, &signatures, std::slice::from_ref(&key)).unwrap(),
            expected
        );
        let mut tampered = bytes;
        tampered[0] ^= 1;
        assert_eq!(
            verify_signed_manifest(&tampered, &signatures, &[key]),
            Err(ManifestError::NoTrustedSignature)
        );
    }

    #[test]
    fn accepts_one_known_signature_during_rotation() {
        let expected = manifest();
        let (bytes, mut signatures, key) = signed(&expected);
        let mut envelope: ReleaseSignatures = serde_json::from_slice(&signatures).unwrap();
        envelope.signatures.insert(
            0,
            ManifestSignature {
                key_id: "future".to_owned(),
                signature: BASE64.encode([0; 64]),
            },
        );
        signatures = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(
            verify_signed_manifest(&bytes, &signatures, &[key]).unwrap(),
            expected
        );
    }

    #[test]
    fn rejects_target_and_path_confusion() {
        let mut invalid = manifest();
        invalid.firmware.board_id = "Seeed_XIAO_nRF52840_Sense".to_owned();
        let (bytes, signatures, key) = signed(&invalid);
        assert!(matches!(
            verify_signed_manifest(&bytes, &signatures, &[key]),
            Err(ManifestError::InvalidField(_))
        ));

        let mut invalid = manifest();
        invalid.application.artifact.name = "../app.zip".to_owned();
        let (bytes, signatures, key) = signed(&invalid);
        assert!(matches!(
            verify_signed_manifest(&bytes, &signatures, &[key]),
            Err(ManifestError::InvalidField(_))
        ));
    }
}
