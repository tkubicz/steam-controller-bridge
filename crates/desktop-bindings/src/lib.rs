//! Configurable Steam Controller 2 desktop-input bindings.

mod engine;
mod model;
mod platform;
mod sink;
mod store;

pub use engine::{bindable_mask, BindingEngine};
pub use model::{
    BindableControl, BindingAction, BindingProfile, ControlBindings, DesktopInputSnapshot,
    KeyboardKey, Modifier, MouseButton, PadBindings, PadFeedbackConfig, PadFeedbackRequest,
    PadFeedbackStrength, PadFunctionConfig, PadSample, ScrollPadConfig, BINDINGS_VERSION,
    DEFAULT_PROFILE_ID, DEFAULT_PROFILE_NAME, DEFAULT_SCROLL_SPEED_PERCENT, MAX_PROFILES,
    MAX_PROFILE_NAME_CHARS, MAX_SCROLL_SPEED_PERCENT, MIN_SCROLL_SPEED_PERCENT,
};
#[cfg(target_os = "macos")]
pub use platform::{
    input_monitoring_access, preflight_accessibility_access, preflight_post_event_access,
    request_accessibility_access, request_input_monitoring_access, request_post_event_access,
    MacOsDesktopInput, PermissionState,
};
pub use sink::DesktopInputSink;
pub use store::{
    default_store_path, load_or_create_store, load_store, parse_store, save_store, BindingStore,
};

#[cfg(test)]
mod tests;
