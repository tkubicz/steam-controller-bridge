//! Hold-to-open radial profile picker, driven entirely from the controller.
//!
//! This crate owns timing, selection geometry, and output suppression. Hosts
//! feed it decoded controller reports and render or apply the returned events.

mod geometry;
mod picker;
mod types;

pub use geometry::{page_count, sector_for, sectors_on_page, with_trigger};
pub use picker::Picker;
pub use types::{
    PickerConfig, PickerEvent, PickerEvents, PickerInput, PickerRoster, COMMIT,
    DEFAULT_ENGAGE_DEAD_ZONE, DEFAULT_HOLD, DEFAULT_SECTORS_PER_PAGE, DEFAULT_TRACK_DEAD_ZONE,
    DISMISS, MAX_HOLD, MAX_SECTORS_PER_PAGE, MIN_HOLD, MIN_ROSTER, MIN_SECTORS_PER_PAGE, PAGE_NEXT,
    PAGE_PREVIOUS, TRIGGER,
};

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    reason = "tests build angles from small literal sector counts"
)]
#[allow(
    clippy::float_cmp,
    reason = "the sanitizer copies constants verbatim, so equality is exact"
)]
mod tests;
