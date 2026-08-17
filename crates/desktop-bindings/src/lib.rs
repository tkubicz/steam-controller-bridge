//! Configurable Steam Controller 2 desktop-input bindings.

mod engine;
mod legacy;
mod model;
mod platform;
mod region;
mod sink;
mod store;

pub use engine::{bindable_mask, BindingEngine};
pub use model::{
    BindableControl, BindingAction, BindingProfile, ControlBindings, DesktopInputSnapshot,
    KeyboardKey, Modifier, MouseButton, PadBindings, PadConfig, PadFeedbackConfig,
    PadFeedbackRequest, PadFeedbackStrength, PadMotionMode, PadRegion, PadRegionShape, PadSample,
    PadSide, PadTrigger, BINDINGS_VERSION, DEFAULT_CENTER_PERCENT, DEFAULT_PAD_SPEED_PERCENT,
    DEFAULT_PROFILE_ID, DEFAULT_PROFILE_NAME, MAX_PAD_REGIONS, MAX_PAD_SPEED_PERCENT, MAX_PROFILES,
    MAX_PROFILE_NAME_CHARS, MAX_REGION_NAME_CHARS, MIN_PAD_SPEED_PERCENT,
};
#[cfg(target_os = "macos")]
pub use platform::{
    input_monitoring_access, preflight_accessibility_access, preflight_post_event_access,
    request_accessibility_access, request_input_monitoring_access, request_post_event_access,
    MacOsDesktopInput, PermissionState,
};
pub use sink::DesktopInputSink;
pub use store::{
    default_store_path, load_or_create_store, load_store, parse_store, reset_store, save_store,
    BindingStore,
};

#[cfg(test)]
mod tests;
