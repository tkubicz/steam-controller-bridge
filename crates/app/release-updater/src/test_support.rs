use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use serde_json::json;

use crate::TrustedPublicKey;

pub(crate) fn temporary_directory(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("release-updater-{label}-"))
        .tempdir()
        .unwrap()
}

pub(crate) fn signed_metadata(
    version: &str,
    revision: u16,
) -> (Vec<u8>, Vec<u8>, TrustedPublicKey) {
    let manifest = serde_json::to_vec(&json!({
        "schema_version": 1,
        "release_tag": format!("v{version}"),
        "application_version": version,
        "minimum_macos": "13.0.0",
        "release_notes": "notes",
        "application": {
            "bundle_identifier": "com.lynxware.steam-controller-bridge",
            "version": version,
            "artifact": { "name": "app.zip", "size": 1, "sha256": "11".repeat(32) }
        },
        "firmware": {
            "target": "seeed-xiao-nrf52840",
            "revision": revision,
            "minimum_application_version": version,
            "protocol_version": 1,
            "device_info_format": 1,
            "board_id": "Seeed_XIAO_nRF52840",
            "uf2_family_id": 0xADA5_2840_u32,
            "usb_vendor_id": 0x045e,
            "usb_product_id": 0x028e,
            "usb_manufacturer": "Lynxware",
            "usb_product": "Steam Controller Bridge",
            "artifact": { "name": "firmware.uf2", "size": 1, "sha256": "22".repeat(32) }
        }
    }))
    .unwrap();
    let signing = SigningKey::from_bytes(&[9; 32]);
    let signatures = serde_json::to_vec(&json!({
        "schema_version": 1,
        "signatures": [{
            "key_id": "fixture",
            "signature": BASE64.encode(signing.sign(&manifest).to_bytes())
        }]
    }))
    .unwrap();
    let key = TrustedPublicKey {
        key_id: "fixture".to_owned(),
        bytes: signing.verifying_key().to_bytes(),
    };
    (manifest, signatures, key)
}
