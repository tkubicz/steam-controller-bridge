use std::ffi::{c_void, CStr};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2_core_foundation::{
    CFBoolean, CFBundle, CFData, CFDictionary, CFIndex, CFNumber, CFRetained, CFString, CFType,
};
use objc2_io_kit::{
    kIOHIDLocationIDKey, kIOHIDManufacturerKey, kIOHIDPhysicalDeviceUniqueIDKey,
    kIOHIDPrimaryUsageKey, kIOHIDPrimaryUsagePageKey, kIOHIDProductIDKey, kIOHIDProductKey,
    kIOHIDReportDescriptorKey, kIOHIDSerialNumberKey, kIOHIDTransportKey, kIOHIDVendorIDKey,
    kIOReturnUnsupported, IOHIDReportType, IOHIDUserDevice, IOHIDUserDeviceGetReportBlock,
    IOHIDUserDeviceOptions, IOHIDUserDeviceSetReportBlock, IOReturn,
};

use crate::contract::{
    HelperResponse, HidReportType, DEVICE_LOCATION_ID, DEVICE_PHYSICAL_UNIQUE_ID,
    DEVICE_SERIAL_NUMBER, HELPER_PROTOCOL_VERSION,
};
use crate::{
    VirtualHidError, VirtualHidErrorClass, VirtualHidHelperMetadata, GAMEPAD_REPORT_DESCRIPTOR,
    NEUTRAL_INPUT_REPORT,
};

type CancelBlock = RcBlock<dyn Fn() + 'static>;

pub(crate) struct VirtualDevice {
    device: Option<CFRetained<IOHIDUserDevice>>,
    queue: Option<DispatchRetained<DispatchQueue>>,
    set_report: Option<SetReportBlock>,
    get_report: Option<GetReportBlock>,
    cancel: Option<CancelBlock>,
    cancelled: mpsc::Receiver<()>,
    metadata: VirtualHidHelperMetadata,
    cancel_requested: bool,
    cancellation_failed: bool,
}

impl VirtualDevice {
    pub(crate) fn create(
        vendor_id: u16,
        product_id: u16,
        responses: mpsc::SyncSender<HelperResponse>,
    ) -> Result<Self, VirtualHidError> {
        let metadata = signing_metadata(vendor_id, product_id);
        let properties = device_properties(vendor_id, product_id);
        // SAFETY: `properties` contains only retained Core Foundation values of
        // the types documented by IOHIDKeys, and it remains alive for the call.
        let device = unsafe {
            IOHIDUserDevice::with_properties(
                None,
                properties.as_opaque(),
                IOHIDUserDeviceOptions::CreateOnActivate.0,
            )
        }
        .ok_or_else(|| creation_error(&metadata))?;

        let event_sequence = Arc::new(AtomicU64::new(1));
        let set_report = SetReportBlock::new(responses.clone(), Arc::clone(&event_sequence));
        let get_report = GetReportBlock::new(responses, event_sequence);
        let (cancel_sender, cancelled) = mpsc::channel();
        let cancel: CancelBlock = RcBlock::new(move || {
            let _ = cancel_sender.send(());
        });
        let queue = DispatchQueue::new("com.lynxware.steam-controller-bridge.virtual-hid", None);

        // SAFETY: The copied heap blocks and dispatch queue are retained in
        // `VirtualDevice` until cancellation has completed. Each callback
        // copies IOKit-owned bytes before returning.
        unsafe {
            device.register_set_report_block(set_report.as_ptr());
            device.register_get_report_block(get_report.as_ptr());
            device.set_cancel_handler(RcBlock::as_ptr(&cancel));
            device.set_dispatch_queue(&queue);
        }
        device.activate();

        let mut owner = Self {
            device: Some(device),
            queue: Some(queue),
            set_report: Some(set_report),
            get_report: Some(get_report),
            cancel: Some(cancel),
            cancelled,
            metadata,
            cancel_requested: false,
            cancellation_failed: false,
        };
        if let Err(error) = owner.dispatch(&NEUTRAL_INPUT_REPORT) {
            let _ = owner.shutdown();
            return Err(error);
        }
        Ok(owner)
    }

    pub(crate) fn metadata(&self) -> VirtualHidHelperMetadata {
        self.metadata.clone()
    }

    pub(crate) fn dispatch(&mut self, report: &[u8]) -> Result<(), VirtualHidError> {
        let device = self.device.as_ref().ok_or_else(|| {
            VirtualHidError::new(
                VirtualHidErrorClass::HelperExited,
                "virtual HID device is already released",
            )
        })?;
        let mut bytes = report.to_vec();
        let pointer = NonNull::new(bytes.as_mut_ptr()).expect("non-empty input report");
        // SAFETY: `pointer` addresses `bytes`, which remains live and unchanged
        // for the duration of the synchronous IOKit call.
        let result = unsafe {
            device.handle_report_with_time_stamp(
                mach_absolute_time(),
                pointer,
                CFIndex::try_from(bytes.len()).expect("input report length fits CFIndex"),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(VirtualHidError::new(
                VirtualHidErrorClass::DispatchFailed,
                format!("IOHIDUserDevice report dispatch failed with IOReturn {result:#x}"),
            ))
        }
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), VirtualHidError> {
        if self.device.is_none() {
            return Ok(());
        }
        if self.cancellation_failed {
            return Err(cancellation_timeout_error());
        }
        if !self.cancel_requested {
            self.cancel_requested = true;
            self.device
                .as_ref()
                .expect("device presence was checked above")
                .cancel();
        }
        if self.cancelled.recv_timeout(Duration::from_secs(1)).is_err() {
            self.cancellation_failed = true;
            return Err(cancellation_timeout_error());
        }
        self.device.take();
        Ok(())
    }

    fn leak_registered_owners(&mut self) {
        // Cancellation did not prove callback quiescence. Releasing any of
        // these owners could race an IOKit callback, so intentionally preserve
        // them until the helper process exits and the OS reclaims its address
        // space.
        // Forgetting the `Option` suppresses the inner value's destructor.
        std::mem::forget(self.device.take());
        std::mem::forget(self.queue.take());
        std::mem::forget(self.set_report.take());
        std::mem::forget(self.get_report.take());
        std::mem::forget(self.cancel.take());
    }
}

impl Drop for VirtualDevice {
    fn drop(&mut self) {
        if self.shutdown().is_err() {
            self.leak_registered_owners();
        }
    }
}

fn cancellation_timeout_error() -> VirtualHidError {
    VirtualHidError::new(
        VirtualHidErrorClass::CancellationTimeout,
        "IOHIDUserDevice cancellation timed out",
    )
}

fn device_properties(vendor_id: u16, product_id: u16) -> CFRetained<CFDictionary<CFType, CFType>> {
    // Paired rather than held in two positional arrays, so a property cannot
    // be silently bound to its neighbour's key.
    let properties: [(&CStr, CFRetained<CFType>); 11] = [
        (
            kIOHIDReportDescriptorKey,
            CFData::from_static_bytes(GAMEPAD_REPORT_DESCRIPTOR).into(),
        ),
        (
            kIOHIDVendorIDKey,
            CFNumber::new_i32(i32::from(vendor_id)).into(),
        ),
        (
            kIOHIDProductIDKey,
            CFNumber::new_i32(i32::from(product_id)).into(),
        ),
        (
            kIOHIDProductKey,
            CFString::from_str("Steam Controller Bridge Virtual Gamepad").into(),
        ),
        (kIOHIDManufacturerKey, CFString::from_str("Lynxware").into()),
        (
            kIOHIDSerialNumberKey,
            CFString::from_str(DEVICE_SERIAL_NUMBER).into(),
        ),
        (
            kIOHIDPhysicalDeviceUniqueIDKey,
            CFString::from_str(DEVICE_PHYSICAL_UNIQUE_ID).into(),
        ),
        (
            kIOHIDLocationIDKey,
            CFNumber::new_i32(DEVICE_LOCATION_ID).into(),
        ),
        (kIOHIDTransportKey, CFString::from_str("USB").into()),
        (kIOHIDPrimaryUsagePageKey, CFNumber::new_i32(0x01).into()),
        (kIOHIDPrimaryUsageKey, CFNumber::new_i32(0x05).into()),
    ];
    let (keys, values): (Vec<_>, Vec<_>) = properties
        .into_iter()
        .map(|(name, value)| (key(name), value))
        .unzip();
    CFDictionary::from_slices(
        &keys.iter().map(AsRef::as_ref).collect::<Vec<_>>(),
        &values.iter().map(AsRef::as_ref).collect::<Vec<_>>(),
    )
}

fn key(value: &CStr) -> CFRetained<CFType> {
    CFString::from_str(value.to_str().expect("IOHID property keys are UTF-8")).into()
}

fn copy_report(pointer: NonNull<u8>, length: CFIndex) -> Option<Vec<u8>> {
    let length = usize::try_from(length).ok()?;
    if length > crate::contract::MAX_RAW_REPORT_LEN {
        return None;
    }
    // SAFETY: IOKit promises `pointer` is valid for `length` bytes for the
    // callback duration. The result is copied before the callback returns.
    Some(unsafe { std::slice::from_raw_parts(pointer.as_ptr(), length) }.to_vec())
}

// `block2` currently implements Rust closures with at most three arguments,
// while IOHIDUserDevice's report callbacks have four. These two small owners
// construct the standard Clang Blocks ABI directly and keep the copied heap
// blocks alive for the entire registration lifetime.
const BLOCK_HAS_COPY_DISPOSE: i32 = 1 << 25;

#[repr(C)]
struct BlockDescriptor<T> {
    reserved: usize,
    size: usize,
    copy: unsafe extern "C" fn(*mut T, *const T),
    dispose: unsafe extern "C" fn(*mut T),
}

struct ReportContext {
    sender: mpsc::SyncSender<HelperResponse>,
    sequence: Arc<AtomicU64>,
}

#[repr(C)]
struct SetReportLiteral {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(*mut Self, IOHIDReportType, u32, NonNull<u8>, CFIndex) -> IOReturn,
    descriptor: *const BlockDescriptor<Self>,
    context: Arc<ReportContext>,
}

struct SetReportBlock(NonNull<SetReportLiteral>);

impl SetReportBlock {
    fn new(sender: mpsc::SyncSender<HelperResponse>, sequence: Arc<AtomicU64>) -> Self {
        let literal = SetReportLiteral {
            // SAFETY: Taking the address of this Blocks runtime class does not
            // read or mutate it; the runtime replaces it in the heap copy.
            isa: std::ptr::addr_of!(_NSConcreteStackBlock).cast(),
            flags: BLOCK_HAS_COPY_DISPOSE,
            reserved: 0,
            invoke: invoke_set_report,
            descriptor: &raw const SET_REPORT_DESCRIPTOR,
            context: Arc::new(ReportContext { sender, sequence }),
        };
        // SAFETY: `literal` has the Clang Blocks ABI layout and its descriptor
        // supplies correct copy/dispose helpers for the captured Arc.
        let copied = unsafe { _Block_copy(std::ptr::addr_of!(literal).cast()) };
        Self(NonNull::new(copied.cast_mut().cast()).expect("_Block_copy returned null"))
    }

    fn as_ptr(&self) -> IOHIDUserDeviceSetReportBlock {
        self.0.as_ptr().cast()
    }
}

impl Drop for SetReportBlock {
    fn drop(&mut self) {
        // SAFETY: This owner holds exactly one reference returned by
        // `_Block_copy` and releases it once.
        unsafe { _Block_release(self.0.as_ptr().cast()) };
    }
}

#[repr(C)]
struct GetReportLiteral {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: unsafe extern "C" fn(
        *mut Self,
        IOHIDReportType,
        u32,
        NonNull<u8>,
        NonNull<CFIndex>,
    ) -> IOReturn,
    descriptor: *const BlockDescriptor<Self>,
    context: Arc<ReportContext>,
}

struct GetReportBlock(NonNull<GetReportLiteral>);

impl GetReportBlock {
    fn new(sender: mpsc::SyncSender<HelperResponse>, sequence: Arc<AtomicU64>) -> Self {
        let literal = GetReportLiteral {
            // SAFETY: See `SetReportBlock::new`.
            isa: std::ptr::addr_of!(_NSConcreteStackBlock).cast(),
            flags: BLOCK_HAS_COPY_DISPOSE,
            reserved: 0,
            invoke: invoke_get_report,
            descriptor: &raw const GET_REPORT_DESCRIPTOR,
            context: Arc::new(ReportContext { sender, sequence }),
        };
        // SAFETY: See `SetReportBlock::new`.
        let copied = unsafe { _Block_copy(std::ptr::addr_of!(literal).cast()) };
        Self(NonNull::new(copied.cast_mut().cast()).expect("_Block_copy returned null"))
    }

    fn as_ptr(&self) -> IOHIDUserDeviceGetReportBlock {
        self.0.as_ptr().cast()
    }
}

impl Drop for GetReportBlock {
    fn drop(&mut self) {
        // SAFETY: See `SetReportBlock::drop`.
        unsafe { _Block_release(self.0.as_ptr().cast()) };
    }
}

unsafe extern "C" fn copy_set_report(dst: *mut SetReportLiteral, src: *const SetReportLiteral) {
    // SAFETY: The Blocks runtime provides distinct, valid literals. Writing a
    // cloned Arc establishes independent ownership in the destination.
    unsafe { std::ptr::write(&raw mut (*dst).context, Arc::clone(&(*src).context)) };
}

unsafe extern "C" fn dispose_set_report(block: *mut SetReportLiteral) {
    // SAFETY: The copy helper initialized this captured Arc exactly once.
    unsafe { std::ptr::drop_in_place(&raw mut (*block).context) };
}

unsafe extern "C" fn invoke_set_report(
    block: *mut SetReportLiteral,
    kind: IOHIDReportType,
    report_id: u32,
    report: NonNull<u8>,
    length: CFIndex,
) -> IOReturn {
    // SAFETY: The Blocks runtime passes the registered literal pointer.
    let context = unsafe { &(*block).context };
    let Some(bytes) = copy_report(report, length) else {
        let _ = context.sender.try_send(HelperResponse::Fatal {
            protocol: HELPER_PROTOCOL_VERSION,
            class: VirtualHidErrorClass::ProtocolViolation,
            message: "IOHIDUserDevice delivered an invalid or oversized set report".to_owned(),
        });
        return kIOReturnUnsupported.cast_signed();
    };
    let _ = context.sender.try_send(HelperResponse::SetReport {
        protocol: HELPER_PROTOCOL_VERSION,
        event_sequence: context.sequence.fetch_add(1, Ordering::Relaxed),
        report_type: report_type(kind),
        report_id,
        report: bytes,
    });
    0
}

unsafe extern "C" fn copy_get_report(dst: *mut GetReportLiteral, src: *const GetReportLiteral) {
    // SAFETY: See `copy_set_report`.
    unsafe { std::ptr::write(&raw mut (*dst).context, Arc::clone(&(*src).context)) };
}

unsafe extern "C" fn dispose_get_report(block: *mut GetReportLiteral) {
    // SAFETY: See `dispose_set_report`.
    unsafe { std::ptr::drop_in_place(&raw mut (*block).context) };
}

unsafe extern "C" fn invoke_get_report(
    block: *mut GetReportLiteral,
    kind: IOHIDReportType,
    report_id: u32,
    _report: NonNull<u8>,
    length: NonNull<CFIndex>,
) -> IOReturn {
    // SAFETY: The Blocks runtime passes the registered literal pointer and
    // IOKit supplies a readable length pointer for the callback duration.
    let (context, max_size) = unsafe {
        (
            &(*block).context,
            usize::try_from(*length.as_ptr()).unwrap_or(0),
        )
    };
    let _ = context.sender.try_send(HelperResponse::GetReport {
        protocol: HELPER_PROTOCOL_VERSION,
        event_sequence: context.sequence.fetch_add(1, Ordering::Relaxed),
        report_type: report_type(kind),
        report_id,
        max_size,
    });
    kIOReturnUnsupported.cast_signed()
}

static SET_REPORT_DESCRIPTOR: BlockDescriptor<SetReportLiteral> = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<SetReportLiteral>(),
    copy: copy_set_report,
    dispose: dispose_set_report,
};

static GET_REPORT_DESCRIPTOR: BlockDescriptor<GetReportLiteral> = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<GetReportLiteral>(),
    copy: copy_get_report,
    dispose: dispose_get_report,
};

const fn report_type(kind: IOHIDReportType) -> HidReportType {
    if kind.0 == IOHIDReportType::Input.0 {
        HidReportType::Input
    } else if kind.0 == IOHIDReportType::Output.0 {
        HidReportType::Output
    } else if kind.0 == IOHIDReportType::Feature.0 {
        HidReportType::Feature
    } else {
        HidReportType::Unknown
    }
}

fn creation_error(metadata: &VirtualHidHelperMetadata) -> VirtualHidError {
    match metadata.entitlement_present {
        Some(false) | None => VirtualHidError::new(
            VirtualHidErrorClass::EntitlementMissing,
            "virtual HID device creation requires com.apple.developer.hid.virtual.device",
        ),
        Some(true) => VirtualHidError::new(
            VirtualHidErrorClass::EntitlementRejected,
            "macOS rejected virtual HID device creation despite the embedded entitlement",
        ),
    }
}

fn signing_metadata(vendor_id: u16, product_id: u16) -> VirtualHidHelperMetadata {
    let bundle_identifier = CFBundle::main_bundle()
        .and_then(|bundle| bundle.identifier())
        .map(|identifier| identifier.to_string());
    let (signing_identifier, entitlement_present) = inspect_current_task();
    VirtualHidHelperMetadata {
        protocol_version: HELPER_PROTOCOL_VERSION,
        vendor_id,
        product_id,
        bundle_identifier,
        signing_identifier,
        entitlement_present,
        dry_run: false,
    }
}

fn inspect_current_task() -> (Option<String>, Option<bool>) {
    // SAFETY: SecTask follows Core Foundation create/copy ownership. Every
    // non-null copied value is wrapped in CFRetained and the opaque task is
    // released once after both queries finish.
    unsafe {
        let task = SecTaskCreateFromSelf(std::ptr::null());
        if task.is_null() {
            return (None, None);
        }
        let mut error = std::ptr::null_mut();
        let signing = SecTaskCopySigningIdentifier(task, &raw mut error);
        release_error(error);
        let signing_identifier =
            NonNull::new(signing).map(|value| CFRetained::<CFString>::from_raw(value).to_string());

        error = std::ptr::null_mut();
        let entitlement_key = CFString::from_str("com.apple.developer.hid.virtual.device");
        let entitlement = SecTaskCopyValueForEntitlement(
            task,
            CFRetained::as_ptr(&entitlement_key).as_ptr(),
            &raw mut error,
        );
        release_error(error);
        let entitlement_present = NonNull::new(entitlement).map(|value| {
            let value = CFRetained::<CFType>::from_raw(value);
            let true_value: CFRetained<CFType> = CFBoolean::new(true).into();
            value == true_value
        });
        CFRelease(task.cast_const());
        (signing_identifier, entitlement_present)
    }
}

unsafe fn release_error(error: *mut CFType) {
    if !error.is_null() {
        // SAFETY: Security returned this optional CFError at +1 retain count.
        unsafe { CFRelease(error.cast::<c_void>().cast_const()) };
    }
}

#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SecTaskCreateFromSelf(allocator: *const c_void) -> *mut c_void;
    fn SecTaskCopySigningIdentifier(task: *mut c_void, error: *mut *mut CFType) -> *mut CFString;
    fn SecTaskCopyValueForEntitlement(
        task: *mut c_void,
        entitlement: *const CFString,
        error: *mut *mut CFType,
    ) -> *mut CFType;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: *const c_void);
    fn mach_absolute_time() -> u64;
}

extern "C" {
    static _NSConcreteStackBlock: [*const c_void; 32];
    fn _Block_copy(block: *const c_void) -> *const c_void;
    fn _Block_release(block: *const c_void);
}
