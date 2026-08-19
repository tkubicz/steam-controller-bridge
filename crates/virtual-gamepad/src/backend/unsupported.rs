use std::sync::mpsc;

use crate::contract::HelperResponse;
use crate::{VirtualHidError, VirtualHidErrorClass, VirtualHidHelperMetadata};

/// Uninhabited because creation always fails away from macOS. Keeping the
/// method surface identical lets the helper protocol remain portable without
/// pretending a non-macOS device can exist.
pub(crate) enum VirtualDevice {}

impl VirtualDevice {
    pub(crate) fn create(
        _vendor_id: u16,
        _product_id: u16,
        _responses: mpsc::SyncSender<HelperResponse>,
        _fatal_responses: mpsc::Sender<HelperResponse>,
    ) -> Result<Self, VirtualHidError> {
        Err(VirtualHidError::new(
            VirtualHidErrorClass::UnsupportedPlatform,
            "live virtual HID output is supported only on macOS",
        ))
    }

    pub(crate) fn metadata(&self) -> VirtualHidHelperMetadata {
        match *self {}
    }

    pub(crate) fn dispatch(
        &mut self,
        _report: &[u8; crate::INPUT_REPORT_LEN],
    ) -> Result<(), VirtualHidError> {
        match *self {}
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), VirtualHidError> {
        match *self {}
    }
}
