use std::f32::consts::TAU;

use steam_controller_protocol::{SteamButton, SteamButtons};

use crate::types::{AXIS_FULL_SCALE, TRIGGER};

pub(super) const fn bit(button: SteamButton) -> u32 {
    1_u32 << button as u8
}

/// Adds the trigger to a button snapshot.
///
/// The host uses this to synthesize the down edge it withheld while the hold
/// was being timed, once [`crate::PickerEvent::TriggerTapped`] says the press was an
/// ordinary one after all.
#[must_use]
pub const fn with_trigger(buttons: SteamButtons) -> SteamButtons {
    SteamButtons(buttons.0 | bit(TRIGGER))
}

pub(super) fn normalize((x, y): (i16, i16)) -> (f32, f32) {
    (
        (f32::from(x) / AXIS_FULL_SCALE).clamp(-1.0, 1.0),
        (f32::from(y) / AXIS_FULL_SCALE).clamp(-1.0, 1.0),
    )
}

pub(super) fn magnitude((x, y): (f32, f32)) -> f32 {
    x.hypot(y)
}

/// Maps a stick direction onto a wheel sector.
///
/// Sector 0 sits at twelve o'clock and they run clockwise, matching how the
/// overlay draws them. `y` is positive upwards, the same convention
/// `controller-mapper` passes straight through to the gamepad output.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "sector counts are at most MAX_SECTORS_PER_PAGE, and the quotient is non-negative"
)]
pub fn sector_for(x: f32, y: f32, sectors: usize) -> usize {
    if sectors <= 1 {
        return 0;
    }
    let arc = TAU / sectors as f32;
    let mut angle = x.atan2(y);
    if angle < 0.0 {
        angle += TAU;
    }
    // Sectors are centred on their direction, so half an arc of rounding puts
    // the boundary halfway between two neighbours.
    ((angle / arc + 0.5).floor() as usize) % sectors
}

/// How many wheels the roster needs. Always at least one.
#[must_use]
pub const fn page_count(len: usize, per_page: usize) -> usize {
    if per_page == 0 || len == 0 {
        return 1;
    }
    len.div_ceil(per_page)
}

/// How many sectors a given page shows. The last page may be short.
#[must_use]
pub const fn sectors_on_page(len: usize, per_page: usize, page: usize) -> usize {
    let start = page * per_page;
    if start >= len {
        return 1;
    }
    let remaining = len - start;
    if remaining < per_page {
        remaining
    } else {
        per_page
    }
}
