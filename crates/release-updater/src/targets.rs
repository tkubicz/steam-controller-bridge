use bridge_output::{FirmwareInfo, FirmwareTarget, BRIDGE_DEVICE_USB_PRODUCT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsbIdentity {
    pub vendor_id: u16,
    pub product_id: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareInstallerStrategy {
    Uf2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareTargetDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub compact_display_name: &'static str,
    pub minimum_compatible_revision: u16,
    pub application_usb: UsbIdentity,
    pub application_manufacturer: &'static str,
    pub application_product: &'static str,
    pub factory_application_usb: &'static [UsbIdentity],
    pub bootloader_usb: &'static [UsbIdentity],
    pub manifest_board_id: &'static str,
    pub accepted_board_ids: &'static [&'static str],
    pub uf2_family_id: u32,
    pub installer: FirmwareInstallerStrategy,
    pub manual_recovery: &'static str,
}

pub const XIAO_USB_VENDOR_ID: u16 = 0x045e;
pub const XIAO_USB_PRODUCT_ID: u16 = 0x028e;
pub const XIAO_USB_MANUFACTURER: &str = "Lynxware";
pub const XIAO_USB_PRODUCT: &str = BRIDGE_DEVICE_USB_PRODUCT;
pub const FIRMWARE_TARGET_ID: &str = "seeed-xiao-nrf52840";
pub const FIRMWARE_BOARD_ID: &str = "Seeed_XIAO_nRF52840";
pub const XIAO_SENSE_BOARD_ID: &str = "Seeed_XIAO_nRF52840_Sense";
pub const UF2_FAMILY_ID: u32 = 0xADA5_2840;
pub const XIAO_MINIMUM_COMPATIBLE_REVISION: u16 = 2;

const SEEED_VENDOR_ID: u16 = 0x2886;
const XIAO_FACTORY_APPLICATION_USB: [UsbIdentity; 2] = [
    UsbIdentity {
        vendor_id: SEEED_VENDOR_ID,
        product_id: 0x8044,
    },
    UsbIdentity {
        vendor_id: SEEED_VENDOR_ID,
        product_id: 0x8045,
    },
];
const XIAO_BOOTLOADER_USB: [UsbIdentity; 2] = [
    UsbIdentity {
        vendor_id: SEEED_VENDOR_ID,
        product_id: 0x0044,
    },
    UsbIdentity {
        vendor_id: SEEED_VENDOR_ID,
        product_id: 0x0045,
    },
];
const XIAO_BOARD_IDS: [&str; 2] = [FIRMWARE_BOARD_ID, XIAO_SENSE_BOARD_ID];

pub const XIAO_NRF52840_TARGET: FirmwareTargetDescriptor = FirmwareTargetDescriptor {
    id: FIRMWARE_TARGET_ID,
    display_name: "Seeed Studio XIAO nRF52840 / Sense",
    compact_display_name: "XIAO nRF52840",
    minimum_compatible_revision: XIAO_MINIMUM_COMPATIBLE_REVISION,
    application_usb: UsbIdentity {
        vendor_id: XIAO_USB_VENDOR_ID,
        product_id: XIAO_USB_PRODUCT_ID,
    },
    application_manufacturer: XIAO_USB_MANUFACTURER,
    application_product: XIAO_USB_PRODUCT,
    factory_application_usb: &XIAO_FACTORY_APPLICATION_USB,
    bootloader_usb: &XIAO_BOOTLOADER_USB,
    manifest_board_id: FIRMWARE_BOARD_ID,
    accepted_board_ids: &XIAO_BOARD_IDS,
    uf2_family_id: UF2_FAMILY_ID,
    installer: FirmwareInstallerStrategy::Uf2,
    manual_recovery: "quickly press the tiny reset button beside the USB-C connector twice while this recovery window is open",
};

#[must_use]
pub fn firmware_target(identifier: &str) -> Option<&'static FirmwareTargetDescriptor> {
    (identifier == XIAO_NRF52840_TARGET.id).then_some(&XIAO_NRF52840_TARGET)
}

#[must_use]
pub fn firmware_matches_target(firmware: FirmwareInfo, target: &FirmwareTargetDescriptor) -> bool {
    matches!(firmware.target, FirmwareTarget::Reported(identifier) if identifier.as_str() == target.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_output::{FirmwareTargetId, FirmwareVersion};

    #[test]
    fn catalog_resolves_only_the_supported_target() {
        assert_eq!(
            firmware_target(FIRMWARE_TARGET_ID),
            Some(&XIAO_NRF52840_TARGET)
        );
        assert_eq!(firmware_target("example-custom-board"), None);
        assert_eq!(XIAO_NRF52840_TARGET.compact_display_name, "XIAO nRF52840");
        assert!(XIAO_NRF52840_TARGET
            .display_name
            .contains(XIAO_NRF52840_TARGET.compact_display_name));
        let firmware = FirmwareInfo {
            target: FirmwareTarget::Reported(FirmwareTargetId::new(FIRMWARE_TARGET_ID).unwrap()),
            version: FirmwareVersion::Reported(3),
            ..FirmwareInfo::default()
        };
        assert!(firmware_matches_target(firmware, &XIAO_NRF52840_TARGET));
    }
}
