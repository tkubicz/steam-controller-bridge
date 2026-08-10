//! Signed release discovery and XIAO firmware installation primitives.
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

#[cfg(target_os = "macos")]
pub use application::{
    guided_replacement_supported, installed_macos_version, stage_application, StagedApplication,
};
pub use artifact::{sha256_hex, verify_artifact, ArtifactError};
/// The post-flash USB identity a signed manifest must match, re-exported from
/// the crate that also selects the live bridge by it.
pub use bridge_output::{
    XIAO_USB_MANUFACTURER, XIAO_USB_PRODUCT, XIAO_USB_PRODUCT_ID, XIAO_USB_VENDOR_ID,
};
pub use cache::{CacheError, ReleaseCache};
pub use firmware::{
    discover_bootloader_volumes, discover_firmware_devices, flash_firmware, validate_uf2,
    BootloaderVolume, FirmwareDevice, FirmwareFlashError, FirmwareFlashProgress,
};
pub use manifest::{
    embedded_trusted_keys, verify_signed_manifest, ApplicationRelease, ArtifactDescriptor,
    FirmwareRelease, ManifestError, ManifestSignature, ReleaseManifestV1, ReleaseSignatures,
    TrustedPublicKey,
};
pub use network::{
    download_to_path, ensure_release_artifact, refresh_catalog_if_due, DownloadError,
    LatestReleaseClient, ReleaseSource,
};

/// GitHub repository that owns the only accepted update channel.
pub const UPDATE_REPOSITORY: &str = "tkubicz/steam-controller-bridge";
pub const MANIFEST_ASSET: &str = "steam-controller-bridge-update-manifest.json";
pub const SIGNATURES_ASSET: &str = "steam-controller-bridge-update-signatures.json";
pub const APPLICATION_BUNDLE_ID: &str = "com.lynxware.steam-controller-bridge";
pub const FIRMWARE_TARGET_ID: &str = "seeed-xiao-nrf52840";
pub const FIRMWARE_BOARD_ID: &str = "Seeed_XIAO_nRF52840";
pub const UF2_FAMILY_ID: u32 = 0xADA5_2840;

/// Coarse state shared by the menu label and the Updates tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCatalog {
    Unavailable(String),
    Checking,
    Current(ReleaseManifestV1),
    Available(ReleaseManifestV1),
}
