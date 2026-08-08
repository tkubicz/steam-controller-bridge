use std::time::Duration;

use steam_controller_protocol::{SteamButton, SteamButtons};

/// The control that opens the wheel. Also dismisses it while it is open.
pub const TRIGGER: SteamButton = SteamButton::QuickAccess;
/// Applies the pointed-at profile.
pub const COMMIT: SteamButton = SteamButton::A;
/// Closes the wheel without changing the active profile.
pub const DISMISS: SteamButton = SteamButton::B;
/// Moves to the previous page when the roster outgrows one wheel.
pub const PAGE_PREVIOUS: SteamButton = SteamButton::LeftShoulder;
/// Moves to the next page when the roster outgrows one wheel.
pub const PAGE_NEXT: SteamButton = SteamButton::RightShoulder;

pub const DEFAULT_HOLD: Duration = Duration::from_secs(2);
pub const MIN_HOLD: Duration = Duration::from_millis(500);
pub const MAX_HOLD: Duration = Duration::from_secs(5);

/// Eight sectors is a 45-degree arc each, which an analog stick hits reliably.
pub const DEFAULT_SECTORS_PER_PAGE: usize = 8;
pub const MIN_SECTORS_PER_PAGE: usize = 2;
pub const MAX_SECTORS_PER_PAGE: usize = 12;

/// How far the stick must be pushed before it starts steering the selection.
pub const DEFAULT_ENGAGE_DEAD_ZONE: f32 = 0.55;
/// How far it must fall back before it stops steering, leaving the selection put.
pub const DEFAULT_TRACK_DEAD_ZONE: f32 = 0.35;

/// A wheel needs at least two profiles to be worth opening.
pub const MIN_ROSTER: usize = 2;

/// The most events one [`Picker::update`] can produce: a `Selection` and the
/// `Commit` or `Dismissed` that closed the wheel in the same report. Every
/// other path emits at most one event.
const MAX_EVENTS: usize = 2;

/// Full-scale magnitude of a raw stick axis, matching `controller-mapper`.
pub(super) const AXIS_FULL_SCALE: f32 = 32767.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickerConfig {
    pub hold: Duration,
    pub engage_dead_zone: f32,
    pub track_dead_zone: f32,
    pub sectors_per_page: usize,
}

impl Default for PickerConfig {
    fn default() -> Self {
        Self {
            hold: DEFAULT_HOLD,
            engage_dead_zone: DEFAULT_ENGAGE_DEAD_ZONE,
            track_dead_zone: DEFAULT_TRACK_DEAD_ZONE,
            sectors_per_page: DEFAULT_SECTORS_PER_PAGE,
        }
    }
}

impl PickerConfig {
    /// Clamps every field into a range the state machine can act on.
    ///
    /// Configuration reaches this crate from a settings file, so a nonsensical
    /// value must degrade to a usable wheel rather than wedge the picker.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.hold = self.hold.clamp(MIN_HOLD, MAX_HOLD);
        self.sectors_per_page = self
            .sectors_per_page
            .clamp(MIN_SECTORS_PER_PAGE, MAX_SECTORS_PER_PAGE);
        self.engage_dead_zone = if self.engage_dead_zone.is_finite() {
            self.engage_dead_zone.clamp(0.1, 0.95)
        } else {
            DEFAULT_ENGAGE_DEAD_ZONE
        };
        self.track_dead_zone = if self.track_dead_zone.is_finite() {
            self.track_dead_zone.clamp(0.05, self.engage_dead_zone)
        } else {
            DEFAULT_TRACK_DEAD_ZONE.min(self.engage_dead_zone)
        };
        self
    }
}

/// The profiles the wheel can choose between, and which one is active now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PickerRoster {
    pub len: usize,
    pub active: Option<usize>,
    /// Opaque host generation echoed by selection and commit events so a
    /// frontend can never resolve an old index against a newly reordered list.
    pub revision: u64,
}

impl PickerRoster {
    #[must_use]
    pub const fn new(len: usize, active: Option<usize>) -> Self {
        Self {
            len,
            active,
            revision: 0,
        }
    }

    #[must_use]
    pub const fn with_revision(len: usize, active: Option<usize>, revision: u64) -> Self {
        Self {
            len,
            active,
            revision,
        }
    }

    #[must_use]
    pub const fn is_openable(self) -> bool {
        self.len >= MIN_ROSTER
    }
}

/// One decoded controller report, reduced to what the picker cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PickerInput {
    pub buttons: SteamButtons,
    pub left_stick: (i16, i16),
    pub right_stick: (i16, i16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerEvent {
    /// The hold has gone on long enough that the wheel is probably coming.
    ///
    /// Reported once per hold, halfway through it, so a host that has to build
    /// something expensive to draw the wheel can start now and be ready by the
    /// time [`PickerEvent::Opened`] arrives. An ordinary press is far shorter
    /// than half a hold, so this does not fire for taps.
    Preparing,
    Opened {
        selected: usize,
        page: usize,
        roster_revision: u64,
    },
    Selection {
        selected: usize,
        page: usize,
        roster_revision: u64,
    },
    Commit {
        index: usize,
        roster_revision: u64,
    },
    Dismissed,
    /// The trigger was released before the hold elapsed, so it was an ordinary
    /// press after all and the host should deliver whatever it is bound to.
    TriggerTapped,
}

/// The events from one update, in the order they happened.
///
/// Backed by a fixed array because [`crate::Picker::update`] runs once per controller
/// report, and this crate keeps that path allocation-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PickerEvents {
    events: [Option<PickerEvent>; MAX_EVENTS],
    len: usize,
}

impl PickerEvents {
    pub(super) fn push(&mut self, event: PickerEvent) {
        // The state machine cannot produce more than MAX_EVENTS in one update;
        // the bound is a backstop, not a policy, so a surplus is dropped rather
        // than panicking in the report path.
        debug_assert!(self.len < MAX_EVENTS, "picker event overflow");
        if self.len < MAX_EVENTS {
            self.events[self.len] = Some(event);
            self.len += 1;
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Iterates the events this update produced.
    pub fn iter(&self) -> impl Iterator<Item = PickerEvent> + '_ {
        self.events[..self.len].iter().filter_map(|event| *event)
    }
}

impl IntoIterator for PickerEvents {
    type Item = PickerEvent;
    type IntoIter = std::iter::Flatten<std::array::IntoIter<Option<PickerEvent>, MAX_EVENTS>>;

    fn into_iter(self) -> Self::IntoIter {
        // Slots at `len..` are `None` by construction: `push` is the only
        // writer and appends monotonically.
        self.events.into_iter().flatten()
    }
}
