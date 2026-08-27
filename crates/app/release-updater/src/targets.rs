use std::collections::HashSet;
use std::sync::OnceLock;

use bridge_output::{FirmwareInfo, FirmwareTarget, FirmwareTargetId, BRIDGE_DEVICE_USB_PRODUCT};
use semver::Version;
use serde::Deserialize;
use thiserror::Error;

use crate::{ArtifactDescriptor, FirmwareRelease};

const EMBEDDED_CATALOG: &str = include_str!("../firmware-targets.json");
const SUPPORTED_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsbIdentity {
    pub vendor_id: u16,
    pub product_id: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareInstallerStrategy {
    Uf2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareTargetDescriptor {
    pub id: FirmwareTargetId,
    pub display_name: String,
    pub compact_display_name: String,
    pub minimum_compatible_revision: u16,
    pub application_usb: UsbIdentity,
    pub application_manufacturer: String,
    pub application_product: String,
    pub factory_application_usb: Vec<UsbIdentity>,
    pub bootloader_usb: Vec<UsbIdentity>,
    pub manifest_board_id: String,
    pub accepted_board_ids: Vec<String>,
    pub uf2_family_id: u32,
    pub installer: FirmwareInstallerStrategy,
    /// Complete imperative sentence shown while the manual recovery window is open.
    pub manual_recovery: String,
    /// Continues "timed out while ..." when no bootloader volume ever mounts.
    pub manual_recovery_timeout: String,
}

impl FirmwareTargetDescriptor {
    /// Builds the signed-manifest firmware entry from validated target policy.
    #[must_use]
    pub fn firmware_release(
        &self,
        revision: u16,
        minimum_application_version: Version,
        artifact: ArtifactDescriptor,
    ) -> FirmwareRelease {
        FirmwareRelease {
            target: self.id.to_string(),
            revision,
            minimum_application_version,
            protocol_version: 1,
            device_info_format: 1,
            board_id: self.manifest_board_id.clone(),
            uf2_family_id: self.uf2_family_id,
            usb_vendor_id: self.application_usb.vendor_id,
            usb_product_id: self.application_usb.product_id,
            usb_manufacturer: self.application_manufacturer.clone(),
            usb_product: self.application_product.clone(),
            artifact,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FirmwareTargetCatalogError {
    #[error("firmware target catalog is not valid JSON: {0}")]
    InvalidJson(String),
    #[error("unsupported firmware target catalog schema version {0}")]
    UnsupportedSchema(u32),
    #[error("firmware target catalog must contain at least one target")]
    EmptyCatalog,
    #[error("firmware target catalog contains invalid target ID `{0}`")]
    InvalidTargetId(String),
    #[error("firmware target catalog contains duplicate target ID `{0}`")]
    DuplicateTargetId(String),
    #[error("firmware target `{target}` has an empty `{field}` value")]
    EmptyText { target: String, field: &'static str },
    #[error("firmware target `{target}` has invalid hexadecimal `{field}` value `{value}`")]
    InvalidHex {
        target: String,
        field: &'static str,
        value: String,
    },
    #[error("firmware target `{target}` contains duplicate USB identity {identity}")]
    DuplicateUsbIdentity { target: String, identity: String },
    #[error("firmware target `{target}` contains duplicate board ID `{board_id}`")]
    DuplicateBoardId { target: String, board_id: String },
    #[error("firmware target `{target}` primary board ID is not accepted")]
    PrimaryBoardNotAccepted { target: String },
    #[error("firmware target `{target}` has minimum compatible revision zero")]
    InvalidMinimumRevision { target: String },
    #[error("firmware target `{target}` has unsupported installer `{installer}`")]
    UnsupportedInstaller { target: String, installer: String },
    #[error("firmware target `{target}` must use application product marker `{BRIDGE_DEVICE_USB_PRODUCT}`")]
    InvalidApplicationProduct { target: String },
    #[error("firmware target `{target}` has an empty `{field}` list")]
    EmptyList { target: String, field: &'static str },
    #[error("firmware target `{target}` has UF2 family zero")]
    InvalidUf2Family { target: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogDocument {
    schema_version: u32,
    targets: Vec<TargetDocument>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetDocument {
    id: String,
    display_name: String,
    compact_display_name: String,
    minimum_compatible_revision: u16,
    application_usb: UsbIdentityDocument,
    application_manufacturer: String,
    application_product: String,
    factory_application_usb: Vec<UsbIdentityDocument>,
    bootloader_usb: Vec<UsbIdentityDocument>,
    primary_board_id: String,
    accepted_board_ids: Vec<String>,
    uf2_family_id: String,
    installer: String,
    manual_recovery: String,
    manual_recovery_timeout: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UsbIdentityDocument {
    vendor_id: String,
    product_id: String,
}

static CATALOG: OnceLock<Result<Vec<FirmwareTargetDescriptor>, FirmwareTargetCatalogError>> =
    OnceLock::new();

pub fn firmware_targets() -> Result<&'static [FirmwareTargetDescriptor], FirmwareTargetCatalogError>
{
    match CATALOG.get_or_init(|| parse_catalog(EMBEDDED_CATALOG)) {
        Ok(targets) => Ok(targets),
        Err(error) => Err(error.clone()),
    }
}

/// Resolves one usable target. A catalog that failed to parse exposes no
/// targets at all, so callers that only need the descriptor treat both cases
/// alike; [`firmware_targets`] reports why the catalog is unusable.
#[must_use]
pub fn firmware_target(identifier: &str) -> Option<&'static FirmwareTargetDescriptor> {
    firmware_targets()
        .ok()?
        .iter()
        .find(|target| target.id.as_str() == identifier)
}

#[must_use]
pub fn firmware_matches_target(firmware: FirmwareInfo, target: &FirmwareTargetDescriptor) -> bool {
    matches!(firmware.target, FirmwareTarget::Reported(identifier) if identifier == target.id)
}

fn parse_catalog(json: &str) -> Result<Vec<FirmwareTargetDescriptor>, FirmwareTargetCatalogError> {
    let document: CatalogDocument = serde_json::from_str(json)
        .map_err(|error| FirmwareTargetCatalogError::InvalidJson(error.to_string()))?;
    if document.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(FirmwareTargetCatalogError::UnsupportedSchema(
            document.schema_version,
        ));
    }
    if document.targets.is_empty() {
        return Err(FirmwareTargetCatalogError::EmptyCatalog);
    }

    let mut target_ids = HashSet::new();
    document
        .targets
        .into_iter()
        .map(|target| parse_target(target, &mut target_ids))
        .collect()
}

fn parse_target(
    target: TargetDocument,
    target_ids: &mut HashSet<FirmwareTargetId>,
) -> Result<FirmwareTargetDescriptor, FirmwareTargetCatalogError> {
    let target_id = validate_target_identity(&target, target_ids)?;
    validate_target_text(&target)?;
    let application_usb = parse_usb_identity(&target.id, &target.application_usb)?;
    let factory_application_usb = parse_usb_identities(
        &target.id,
        "factory_application_usb",
        &target.factory_application_usb,
    )?;
    let bootloader_usb =
        parse_usb_identities(&target.id, "bootloader_usb", &target.bootloader_usb)?;
    validate_unique_identities(
        &target.id,
        application_usb,
        &factory_application_usb,
        &bootloader_usb,
    )?;
    validate_board_ids(&target)?;
    let uf2_family_id = parse_hex_u32(&target.id, "uf2_family_id", &target.uf2_family_id)?;
    if uf2_family_id == 0 {
        return Err(FirmwareTargetCatalogError::InvalidUf2Family { target: target.id });
    }
    let installer = match target.installer.as_str() {
        "uf2" => FirmwareInstallerStrategy::Uf2,
        _ => {
            return Err(FirmwareTargetCatalogError::UnsupportedInstaller {
                target: target.id,
                installer: target.installer,
            });
        }
    };
    Ok(FirmwareTargetDescriptor {
        id: target_id,
        display_name: target.display_name,
        compact_display_name: target.compact_display_name,
        minimum_compatible_revision: target.minimum_compatible_revision,
        application_usb,
        application_manufacturer: target.application_manufacturer,
        application_product: target.application_product,
        factory_application_usb,
        bootloader_usb,
        manifest_board_id: target.primary_board_id,
        accepted_board_ids: target.accepted_board_ids,
        uf2_family_id,
        installer,
        manual_recovery: target.manual_recovery,
        manual_recovery_timeout: target.manual_recovery_timeout,
    })
}

fn validate_target_identity(
    target: &TargetDocument,
    target_ids: &mut HashSet<FirmwareTargetId>,
) -> Result<FirmwareTargetId, FirmwareTargetCatalogError> {
    let target_id = FirmwareTargetId::new(&target.id)
        .map_err(|_| FirmwareTargetCatalogError::InvalidTargetId(target.id.clone()))?;
    if !target_ids.insert(target_id) {
        return Err(FirmwareTargetCatalogError::DuplicateTargetId(
            target.id.clone(),
        ));
    }
    Ok(target_id)
}

fn validate_target_text(target: &TargetDocument) -> Result<(), FirmwareTargetCatalogError> {
    for (field, value) in [
        ("display_name", target.display_name.as_str()),
        ("compact_display_name", target.compact_display_name.as_str()),
        (
            "application_manufacturer",
            target.application_manufacturer.as_str(),
        ),
        ("application_product", target.application_product.as_str()),
        ("primary_board_id", target.primary_board_id.as_str()),
        ("manual_recovery", target.manual_recovery.as_str()),
        (
            "manual_recovery_timeout",
            target.manual_recovery_timeout.as_str(),
        ),
    ] {
        validate_nonempty(&target.id, field, value)?;
    }
    if target.application_product != BRIDGE_DEVICE_USB_PRODUCT {
        return Err(FirmwareTargetCatalogError::InvalidApplicationProduct {
            target: target.id.clone(),
        });
    }
    if target.minimum_compatible_revision == 0 {
        return Err(FirmwareTargetCatalogError::InvalidMinimumRevision {
            target: target.id.clone(),
        });
    }
    Ok(())
}

fn parse_usb_identities(
    target: &str,
    field: &'static str,
    identities: &[UsbIdentityDocument],
) -> Result<Vec<UsbIdentity>, FirmwareTargetCatalogError> {
    if identities.is_empty() {
        return Err(FirmwareTargetCatalogError::EmptyList {
            target: target.to_owned(),
            field,
        });
    }
    identities
        .iter()
        .map(|identity| parse_usb_identity(target, identity))
        .collect()
}

fn validate_unique_identities(
    target: &str,
    application: UsbIdentity,
    factory: &[UsbIdentity],
    bootloader: &[UsbIdentity],
) -> Result<(), FirmwareTargetCatalogError> {
    let mut identities = HashSet::new();
    for identity in std::iter::once(application)
        .chain(factory.iter().copied())
        .chain(bootloader.iter().copied())
    {
        if !identities.insert(identity) {
            return Err(FirmwareTargetCatalogError::DuplicateUsbIdentity {
                target: target.to_owned(),
                identity: format!("0x{:04x}:0x{:04x}", identity.vendor_id, identity.product_id),
            });
        }
    }
    Ok(())
}

fn validate_board_ids(target: &TargetDocument) -> Result<(), FirmwareTargetCatalogError> {
    if target.accepted_board_ids.is_empty() {
        return Err(FirmwareTargetCatalogError::EmptyList {
            target: target.id.clone(),
            field: "accepted_board_ids",
        });
    }
    let mut board_ids = HashSet::new();
    for board_id in &target.accepted_board_ids {
        validate_nonempty(&target.id, "accepted_board_ids", board_id)?;
        if !board_ids.insert(board_id) {
            return Err(FirmwareTargetCatalogError::DuplicateBoardId {
                target: target.id.clone(),
                board_id: board_id.clone(),
            });
        }
    }
    if !board_ids.contains(&target.primary_board_id) {
        return Err(FirmwareTargetCatalogError::PrimaryBoardNotAccepted {
            target: target.id.clone(),
        });
    }
    Ok(())
}

fn validate_nonempty(
    target: &str,
    field: &'static str,
    value: &str,
) -> Result<(), FirmwareTargetCatalogError> {
    if value.trim().is_empty() {
        return Err(FirmwareTargetCatalogError::EmptyText {
            target: target.to_owned(),
            field,
        });
    }
    Ok(())
}

fn parse_usb_identity(
    target: &str,
    identity: &UsbIdentityDocument,
) -> Result<UsbIdentity, FirmwareTargetCatalogError> {
    Ok(UsbIdentity {
        vendor_id: parse_hex_u16(target, "vendor_id", &identity.vendor_id)?,
        product_id: parse_hex_u16(target, "product_id", &identity.product_id)?,
    })
}

fn parse_hex_u16(
    target: &str,
    field: &'static str,
    value: &str,
) -> Result<u16, FirmwareTargetCatalogError> {
    parse_hex(target, field, value, 4).and_then(|digits| {
        u16::from_str_radix(digits, 16).map_err(|_| invalid_hex(target, field, value))
    })
}

fn parse_hex_u32(
    target: &str,
    field: &'static str,
    value: &str,
) -> Result<u32, FirmwareTargetCatalogError> {
    parse_hex(target, field, value, 8).and_then(|digits| {
        u32::from_str_radix(digits, 16).map_err(|_| invalid_hex(target, field, value))
    })
}

fn parse_hex<'a>(
    target: &str,
    field: &'static str,
    value: &'a str,
    digits: usize,
) -> Result<&'a str, FirmwareTargetCatalogError> {
    let Some(hex) = value.strip_prefix("0x") else {
        return Err(invalid_hex(target, field, value));
    };
    if hex.len() != digits
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_hex(target, field, value));
    }
    Ok(hex)
}

fn invalid_hex(target: &str, field: &'static str, value: &str) -> FirmwareTargetCatalogError {
    FirmwareTargetCatalogError::InvalidHex {
        target: target.to_owned(),
        field,
        value: value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_output::BRIDGE_DEVICE_USB_PRODUCT;
    use bridge_output::{FirmwareTargetId, FirmwareVersion};

    const TARGET_ID: &str = "seeed-xiao-nrf52840";

    #[test]
    fn embedded_catalog_resolves_supported_target() {
        let target = firmware_target(TARGET_ID).unwrap();
        assert_eq!(target.compact_display_name, "XIAO nRF52840");
        assert!(target.display_name.contains(&target.compact_display_name));
        assert_eq!(target.application_product, BRIDGE_DEVICE_USB_PRODUCT);
        assert!(firmware_target("example-custom-board").is_none());
        let firmware = FirmwareInfo {
            target: FirmwareTarget::Reported(FirmwareTargetId::new(TARGET_ID).unwrap()),
            version: FirmwareVersion::Reported(3),
            ..FirmwareInfo::default()
        };
        assert!(firmware_matches_target(firmware, target));
    }

    #[test]
    fn catalog_validation_rejects_unknown_fields_and_bad_hex() {
        let unknown = EMBEDDED_CATALOG.replace(
            "\"schema_version\": 1,",
            "\"schema_version\": 1, \"unexpected\": true,",
        );
        assert!(matches!(
            parse_catalog(&unknown),
            Err(FirmwareTargetCatalogError::InvalidJson(_))
        ));

        let bad_hex = EMBEDDED_CATALOG.replace("0xada52840", "0xADA52840");
        assert!(matches!(
            parse_catalog(&bad_hex),
            Err(FirmwareTargetCatalogError::InvalidHex { .. })
        ));
    }

    #[test]
    fn catalog_validation_rejects_duplicates_and_inconsistent_primary_board() {
        let mut duplicate_target: serde_json::Value =
            serde_json::from_str(EMBEDDED_CATALOG).unwrap();
        let target = duplicate_target["targets"][0].clone();
        duplicate_target["targets"]
            .as_array_mut()
            .unwrap()
            .push(target);
        assert!(matches!(
            parse_catalog(&serde_json::to_string(&duplicate_target).unwrap()),
            Err(FirmwareTargetCatalogError::DuplicateTargetId(_))
        ));

        let duplicate_usb = EMBEDDED_CATALOG.replace("0x8045", "0x8044");
        assert!(matches!(
            parse_catalog(&duplicate_usb),
            Err(FirmwareTargetCatalogError::DuplicateUsbIdentity { .. })
        ));

        let missing_primary = EMBEDDED_CATALOG.replace(
            "\"Seeed_XIAO_nRF52840\",\n        \"Seeed_XIAO_nRF52840_Sense\"",
            "\"Another_Board\",\n        \"Seeed_XIAO_nRF52840_Sense\"",
        );
        assert!(matches!(
            parse_catalog(&missing_primary),
            Err(FirmwareTargetCatalogError::PrimaryBoardNotAccepted { .. })
        ));
    }

    #[test]
    fn catalog_validation_rejects_unsupported_and_empty_policy() {
        let cases = [
            (
                EMBEDDED_CATALOG.replace("\"schema_version\": 1", "\"schema_version\": 2"),
                "unsupported schema",
            ),
            (
                EMBEDDED_CATALOG.replace("\"installer\": \"uf2\"", "\"installer\": \"dfu\""),
                "unsupported installer",
            ),
            (
                EMBEDDED_CATALOG.replace(
                    "\"id\": \"seeed-xiao-nrf52840\"",
                    "\"id\": \"Invalid Target\"",
                ),
                "invalid target id",
            ),
            (
                EMBEDDED_CATALOG.replace(
                    "\"display_name\": \"Seeed Studio XIAO nRF52840 / Sense\"",
                    "\"display_name\": \" \"",
                ),
                "empty text",
            ),
            (
                EMBEDDED_CATALOG.replace(
                    "\"minimum_compatible_revision\": 2",
                    "\"minimum_compatible_revision\": 0",
                ),
                "zero revision",
            ),
            (
                EMBEDDED_CATALOG.replace(
                    "\"application_product\": \"Steam Controller Bridge\"",
                    "\"application_product\": \"Another Product\"",
                ),
                "wrong discovery marker",
            ),
            (
                EMBEDDED_CATALOG.replace("\"uf2_family_id\": \"0xada52840\"", "\"uf2_family_id\": \"0x00000000\""),
                "zero UF2 family",
            ),
            (
                EMBEDDED_CATALOG.replace(
                    "\"factory_application_usb\": [\n        { \"vendor_id\": \"0x2886\", \"product_id\": \"0x8044\" },\n        { \"vendor_id\": \"0x2886\", \"product_id\": \"0x8045\" }\n      ]",
                    "\"factory_application_usb\": []",
                ),
                "empty identity list",
            ),
        ];
        for (json, reason) in cases {
            assert!(parse_catalog(&json).is_err(), "accepted {reason}");
        }

        let duplicate_board =
            EMBEDDED_CATALOG.replace("\"Seeed_XIAO_nRF52840_Sense\"", "\"Seeed_XIAO_nRF52840\"");
        assert!(matches!(
            parse_catalog(&duplicate_board),
            Err(FirmwareTargetCatalogError::DuplicateBoardId { .. })
        ));
    }
}
