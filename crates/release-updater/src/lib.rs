//! Signed release discovery and catalog-driven firmware installation primitives.
//!
//! The menu application owns presentation and bridge lifecycle. This crate
//! owns the security boundary: a release is not trusted until its raw manifest
//! has a valid Ed25519 signature, and an artifact is not usable until its exact
//! size and SHA-256 match that signed manifest.

#![allow(
    clippy::missing_errors_doc,
    reason = "workspace-only APIs are documented by the updater contract"
)]

#[cfg(target_os = "macos")]
mod application;
mod artifact;
mod cache;
mod firmware;
mod manifest;
mod network;
mod targets;
mod temporary;
#[cfg(test)]
mod test_support;

#[cfg(target_os = "macos")]
pub use application::{
    guided_replacement_supported, installed_macos_version, stage_application, StagedApplication,
};
pub use artifact::{sha256_hex, verify_artifact, ArtifactError};
pub use cache::{CacheError, ReleaseCache};
pub use firmware::{
    classify_firmware_release, discover_bootloader_volumes, discover_firmware_devices,
    flash_firmware, validate_uf2, BootloaderVolume, FirmwareDevice, FirmwareDeviceKind,
    FirmwareFlashError, FirmwareFlashProgress, FirmwareReleaseState,
};
pub use manifest::{
    embedded_trusted_keys, verify_signed_manifest, ApplicationRelease, ArtifactDescriptor,
    FirmwareRelease, ManifestError, ManifestSignature, ReleaseManifestV1, ReleaseSignatures,
    TrustedPublicKey,
};
#[cfg(debug_assertions)]
pub use network::LocalReleaseClient;
pub use network::{
    download_to_path, ensure_release_artifact, refresh_catalog_if_due, CatalogRefresh,
    DownloadError, LatestReleaseClient, ReleaseSource,
};
pub use targets::{
    firmware_matches_target, firmware_target, FirmwareInstallerStrategy, FirmwareTargetDescriptor,
    UsbIdentity, FIRMWARE_BOARD_ID, FIRMWARE_TARGET_ID, UF2_FAMILY_ID,
    XIAO_MINIMUM_COMPATIBLE_REVISION, XIAO_NRF52840_TARGET, XIAO_SENSE_BOARD_ID,
    XIAO_USB_MANUFACTURER, XIAO_USB_PRODUCT, XIAO_USB_PRODUCT_ID, XIAO_USB_VENDOR_ID,
};

/// GitHub repository that owns the only accepted production update channel.
pub const UPDATE_REPOSITORY: &str = "tkubicz/steam-controller-bridge";
pub const MANIFEST_ASSET: &str = "steam-controller-bridge-update-manifest.json";
pub const SIGNATURES_ASSET: &str = "steam-controller-bridge-update-signatures.json";
pub const APPLICATION_BUNDLE_ID: &str = "com.lynxware.steam-controller-bridge";
