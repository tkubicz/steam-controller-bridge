//! Hold-to-open radial profile picker, driven entirely from the controller.
//!
//! The user holds the Quick Access button for [`PickerConfig::hold`], which
//! opens a wheel of binding profiles. Either stick then points at a sector,
//! and the wheel stays open after the trigger is released so the selection can
//! be confirmed at leisure. `A` applies the pointed-at profile, `B` dismisses.
//!
//! This crate is pure: it owns the timing and the geometry, and it decides
//! nothing about how the wheel is drawn or how a profile is applied. The host
//! feeds it decoded controller reports and acts on the returned events.

use std::f32::consts::TAU;
use std::time::Duration;

use controller_mapper::gamepad_button;
use gamepad_state::{GamepadButtons, OutputSuppression};
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
const AXIS_FULL_SCALE: f32 = 32767.0;

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
/// Backed by a fixed array because [`Picker::update`] runs once per controller
/// report, and this crate keeps that path allocation-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PickerEvents {
    events: [Option<PickerEvent>; MAX_EVENTS],
    len: usize,
}

impl PickerEvents {
    fn push(&mut self, event: PickerEvent) {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    /// The trigger is down and the hold is being timed.
    Arming {
        since: Duration,
        /// Whether [`PickerEvent::Preparing`] has already been reported.
        prepared: bool,
    },
    Open {
        selected: usize,
        page: usize,
    },
}

/// Which stick is steering the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stick {
    Left,
    Right,
}

#[derive(Debug)]
pub struct Picker {
    config: PickerConfig,
    phase: Phase,
    previous: Option<SteamButtons>,
    /// The stick currently steering the selection, if any. Recentering it
    /// clears this and leaves the selection where the user left it.
    steering: Option<Stick>,
    /// Controls the wheel consumed that are still physically held after it
    /// closed. Cleared bit by bit as the user lets go. See [`Picker::suppression`].
    latched: SteamButtons,
}

/// The controls the wheel reads, and which therefore must never reach the game
/// as a side effect of operating it.
const CONSUMED: [SteamButton; 5] = [TRIGGER, COMMIT, DISMISS, PAGE_PREVIOUS, PAGE_NEXT];

const fn consumed_mask() -> u32 {
    let mut mask = 0;
    let mut index = 0;
    while index < CONSUMED.len() {
        mask |= bit(CONSUMED[index]);
        index += 1;
    }
    mask
}

impl Picker {
    #[must_use]
    pub fn new(config: PickerConfig) -> Self {
        Self {
            config: config.sanitized(),
            phase: Phase::Idle,
            previous: None,
            steering: None,
            latched: SteamButtons(0),
        }
    }

    #[must_use]
    pub const fn config(&self) -> &PickerConfig {
        &self.config
    }

    /// Replaces the configuration, closing the wheel if it is open.
    ///
    /// No event is returned, so the caller must hide the overlay itself. Unlike
    /// [`Picker::close`], reports keep arriving afterwards: consumed controls
    /// that are still physically held are latched, so the press that was aimed
    /// at the wheel — or the hold that was arming it — cannot reach the game as
    /// a fresh press the moment the wheel ceases to exist. The latch drains as
    /// usual on the following updates, and [`Picker::suppression`] must keep
    /// being applied for that to happen.
    pub fn set_config(&mut self, config: PickerConfig) {
        self.config = config.sanitized();
        if let Some(previous) = self.previous {
            let still_held = match self.phase {
                // The wheel consumed all five controls.
                Phase::Open { .. } => previous.0 & consumed_mask(),
                // Only the trigger was withheld while the hold was timed; the
                // rest of the pad was the game's.
                Phase::Arming { .. } => previous.0 & bit(TRIGGER),
                Phase::Idle => 0,
            };
            self.latched = SteamButtons(self.latched.0 | still_held);
        }
        self.phase = Phase::Idle;
        self.steering = None;
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        matches!(self.phase, Phase::Open { .. })
    }

    /// Test-only introspection; the host distinguishes phases by events.
    #[cfg(test)]
    const fn is_arming(&self) -> bool {
        matches!(self.phase, Phase::Arming { .. })
    }

    /// What the game must not see right now.
    ///
    /// While the wheel is up, everything: the user is operating a menu, not
    /// playing, and [`OutputSuppression::Neutral`] is also the only variant that
    /// is safe for a pinned state.
    ///
    /// Afterwards, the buttons the wheel consumed stay withheld until they are
    /// physically released. The wheel closes on a **press**, so without this the
    /// A that applied a profile, or the B that dismissed it, would still be down
    /// on the very next report and would reach the game the moment suppression
    /// lifted. That is a press the user aimed at the overlay, not at the game.
    #[must_use]
    pub fn suppression(&self) -> Option<OutputSuppression> {
        if self.is_open() {
            return Some(OutputSuppression::Neutral);
        }
        if self.latched.0 == 0 {
            return None;
        }
        let mut buttons = GamepadButtons::default();
        for control in CONSUMED {
            if self.latched.contains(control) {
                if let Some(button) = gamepad_button(control) {
                    buttons.set(button, true);
                }
            }
        }
        Some(OutputSuppression::Buttons(buttons))
    }

    /// Whether the trigger is currently the picker's and not the host's.
    #[must_use]
    pub const fn owns_trigger(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Hides the trigger from a button snapshot while the picker owns it, and
    /// while a press that closed the wheel is still latched.
    ///
    /// Without the first a Quick Access desktop binding would fire the moment
    /// the hold starts; [`PickerEvent::TriggerTapped`] tells the host when to
    /// deliver the binding after all. Without the second, dismissing the wheel
    /// with a second Quick Access press would hand the still-held trigger back
    /// to the bindings engine as a fresh press — firing the very binding the
    /// wheel exists to protect.
    #[must_use]
    pub fn mask_trigger(&self, buttons: SteamButtons) -> SteamButtons {
        if self.owns_trigger() || self.latched.contains(TRIGGER) {
            SteamButtons(buttons.0 & !bit(TRIGGER))
        } else {
            buttons
        }
    }

    /// Forces the wheel closed, for a controller that went away mid-selection.
    ///
    /// Returns whether anything was open, so the caller knows to clear
    /// suppression and hide the overlay. No event is produced: the source of
    /// the events is gone.
    pub fn close(&mut self) -> bool {
        let was_active = self.owns_trigger();
        self.phase = Phase::Idle;
        self.previous = None;
        self.steering = None;
        // Nothing to hold back: the controller this would apply to is gone, and
        // a latch with no reports arriving would never clear.
        self.latched = SteamButtons(0);
        was_active
    }

    /// Advances the picker by one controller report.
    ///
    /// `now` must be monotonic across calls. The first call after construction
    /// or [`Picker::close`] is a non-emitting baseline, so a trigger already
    /// held when the controller connects cannot open the wheel.
    pub fn update(
        &mut self,
        now: Duration,
        input: &PickerInput,
        roster: PickerRoster,
    ) -> PickerEvents {
        let mut events = PickerEvents::default();
        let Some(previous) = self.previous.replace(input.buttons) else {
            return events;
        };
        let pressed =
            |button: SteamButton| input.buttons.contains(button) && !previous.contains(button);
        // A latched control is released the moment the user lets go of it.
        self.latched = SteamButtons(self.latched.0 & input.buttons.0);
        let was_open = self.is_open();

        match self.phase {
            Phase::Idle => {
                if pressed(TRIGGER) && roster.is_openable() {
                    self.phase = Phase::Arming {
                        since: now,
                        prepared: false,
                    };
                }
            }
            Phase::Arming { since, prepared } => {
                if !input.buttons.contains(TRIGGER) {
                    // Released early, so it was an ordinary press.
                    self.phase = Phase::Idle;
                    events.push(PickerEvent::TriggerTapped);
                } else if !roster.is_openable() {
                    // The roster shrank under us mid-hold. The wheel can no
                    // longer open, so hand the press back as a tap.
                    self.phase = Phase::Idle;
                    events.push(PickerEvent::TriggerTapped);
                } else if now.saturating_sub(since) >= self.config.hold {
                    let selected_index = roster.active.unwrap_or(0).min(roster.len - 1);
                    let page = selected_index / self.config.sectors_per_page;
                    let selected = selected_index % self.config.sectors_per_page;
                    self.phase = Phase::Open { selected, page };
                    self.steering = None;
                    events.push(PickerEvent::Opened {
                        selected,
                        page,
                        roster_revision: roster.revision,
                    });
                } else if !prepared && now.saturating_sub(since) >= self.config.hold / 2 {
                    self.phase = Phase::Arming {
                        since,
                        prepared: true,
                    };
                    events.push(PickerEvent::Preparing);
                }
            }
            Phase::Open { selected, page } => {
                self.update_open(input, roster, selected, page, &pressed, &mut events);
            }
        }
        if was_open && !self.is_open() {
            // The wheel closed on a press, so whatever closed it is still down.
            // Hold those controls back until the user lets go, or the press
            // aimed at the overlay reaches the game on the very next report.
            self.latched = SteamButtons(input.buttons.0 & consumed_mask());
        }
        events
    }

    fn update_open(
        &mut self,
        input: &PickerInput,
        roster: PickerRoster,
        was_selected: usize,
        was_page: usize,
        pressed: &impl Fn(SteamButton) -> bool,
        events: &mut PickerEvents,
    ) {
        if !roster.is_openable() {
            self.phase = Phase::Idle;
            self.steering = None;
            events.push(PickerEvent::Dismissed);
            return;
        }
        let per_page = self.config.sectors_per_page;
        let pages = page_count(roster.len, per_page);
        let mut page = was_page.min(pages - 1);
        let mut selected = was_selected.min(sectors_on_page(roster.len, per_page, page) - 1);

        if pages > 1 {
            if pressed(PAGE_PREVIOUS) {
                page = (page + pages - 1) % pages;
                selected = selected.min(sectors_on_page(roster.len, per_page, page) - 1);
            } else if pressed(PAGE_NEXT) {
                page = (page + 1) % pages;
                selected = selected.min(sectors_on_page(roster.len, per_page, page) - 1);
            }
        }

        if let Some(sector) = self.steer(input, sectors_on_page(roster.len, per_page, page)) {
            selected = sector;
        }

        if (selected, page) != (was_selected, was_page) {
            self.phase = Phase::Open { selected, page };
            events.push(PickerEvent::Selection {
                selected,
                page,
                roster_revision: roster.revision,
            });
        }

        if pressed(COMMIT) {
            self.phase = Phase::Idle;
            self.steering = None;
            events.push(PickerEvent::Commit {
                index: (page * per_page + selected).min(roster.len - 1),
                roster_revision: roster.revision,
            });
        } else if pressed(DISMISS) || pressed(TRIGGER) {
            self.phase = Phase::Idle;
            self.steering = None;
            events.push(PickerEvent::Dismissed);
        }
    }

    /// Returns the sector a stick is pointing at, or `None` to hold the
    /// selection where it is.
    ///
    /// Either stick works, but a stick only gains control by crossing
    /// [`PickerConfig::engage_dead_zone`] — whether from rest or by taking over
    /// from the other stick. The steering stick keeps control down to
    /// [`PickerConfig::track_dead_zone`], and a thumb resting on the other
    /// stick inside that hysteresis band can never steal the wheel.
    fn steer(&mut self, input: &PickerInput, sectors: usize) -> Option<usize> {
        let position = |stick: Stick| {
            normalize(match stick {
                Stick::Left => input.left_stick,
                Stick::Right => input.right_stick,
            })
        };
        let deflection = |stick: Stick| magnitude(position(stick));
        let further = if deflection(Stick::Left) >= deflection(Stick::Right) {
            Stick::Left
        } else {
            Stick::Right
        };
        let engaged = deflection(further) >= self.config.engage_dead_zone;

        match self.steering {
            // The current stick keeps steering while it stays above the track
            // dead zone; the other one takes over only by crossing engage.
            Some(current) if deflection(current) >= self.config.track_dead_zone => {
                if engaged && further != current {
                    self.steering = Some(further);
                }
            }
            _ => {
                self.steering = engaged.then_some(further);
            }
        }
        self.steering.map(|stick| {
            let (x, y) = position(stick);
            sector_for(x, y, sectors)
        })
    }
}

const fn bit(button: SteamButton) -> u32 {
    1_u32 << button as u8
}

/// Adds the trigger to a button snapshot.
///
/// The host uses this to synthesize the down edge it withheld while the hold
/// was being timed, once [`PickerEvent::TriggerTapped`] says the press was an
/// ordinary one after all.
#[must_use]
pub const fn with_trigger(buttons: SteamButtons) -> SteamButtons {
    SteamButtons(buttons.0 | bit(TRIGGER))
}

fn normalize((x, y): (i16, i16)) -> (f32, f32) {
    (
        (f32::from(x) / AXIS_FULL_SCALE).clamp(-1.0, 1.0),
        (f32::from(y) / AXIS_FULL_SCALE).clamp(-1.0, 1.0),
    )
}

fn magnitude((x, y): (f32, f32)) -> f32 {
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

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    reason = "tests build angles from small literal sector counts"
)]
#[allow(
    clippy::float_cmp,
    reason = "the sanitizer copies constants verbatim, so equality is exact"
)]
mod tests {
    use super::*;
    use gamepad_state::Button;

    const ROSTER: PickerRoster = PickerRoster {
        len: 4,
        active: Some(0),
        revision: 0,
    };

    fn buttons(pressed: &[SteamButton]) -> SteamButtons {
        SteamButtons(pressed.iter().fold(0, |mask, button| mask | bit(*button)))
    }

    fn input(pressed: &[SteamButton]) -> PickerInput {
        PickerInput {
            buttons: buttons(pressed),
            ..PickerInput::default()
        }
    }

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    /// Drives a picker to the open state and returns it, baseline established.
    fn opened(roster: PickerRoster) -> Picker {
        let mut picker = Picker::new(PickerConfig::default());
        assert!(picker.update(ms(0), &input(&[]), roster).is_empty());
        picker.update(ms(10), &input(&[TRIGGER]), roster);
        let events = picker.update(ms(2_010), &input(&[TRIGGER]), roster);
        assert!(picker.is_open(), "picker should have opened");
        assert_eq!(events.len(), 1);
        picker
    }

    #[test]
    fn the_first_update_is_a_baseline_that_cannot_open_the_wheel() {
        let mut picker = Picker::new(PickerConfig::default());
        // A trigger already held when the controller connects must not arm.
        assert!(picker.update(ms(0), &input(&[TRIGGER]), ROSTER).is_empty());
        assert!(!picker.is_arming());
        assert!(picker
            .update(ms(5_000), &input(&[TRIGGER]), ROSTER)
            .is_empty());
        assert!(!picker.is_open());
    }

    #[test]
    fn holding_the_trigger_past_the_threshold_opens_the_wheel() {
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), ROSTER);
        assert!(picker.update(ms(10), &input(&[TRIGGER]), ROSTER).is_empty());
        assert!(picker.is_arming());
        // One millisecond short of the hold has not opened anything; the only
        // thing reported by then is the halfway warning.
        let events: Vec<_> = picker
            .update(ms(2_009), &input(&[TRIGGER]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(events, vec![PickerEvent::Preparing]);
        assert!(!picker.is_open());

        let events: Vec<_> = picker
            .update(ms(2_010), &input(&[TRIGGER]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Opened {
                selected: 0,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn events_echo_the_roster_revision_that_defined_their_indices() {
        let roster = PickerRoster::with_revision(4, Some(0), 37);
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), roster);
        picker.update(ms(10), &input(&[TRIGGER]), roster);
        assert_eq!(
            picker
                .update(ms(2_010), &input(&[TRIGGER]), roster)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![PickerEvent::Opened {
                selected: 0,
                page: 0,
                roster_revision: 37,
            }]
        );
        assert_eq!(
            picker
                .update(ms(2_020), &input(&[COMMIT]), roster)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![PickerEvent::Commit {
                index: 0,
                roster_revision: 37,
            }]
        );
    }

    #[test]
    fn the_halfway_warning_arrives_once_with_time_left_to_act_on_it() {
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), ROSTER);
        picker.update(ms(0), &input(&[TRIGGER]), ROSTER);

        // Nothing at all for the first half of the hold: an ordinary press is
        // far shorter than this, and must not start the host's overlay.
        assert!(picker
            .update(ms(999), &input(&[TRIGGER]), ROSTER)
            .is_empty());

        let events: Vec<_> = picker
            .update(ms(1_000), &input(&[TRIGGER]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(events, vec![PickerEvent::Preparing]);
        // Reported once, not on every report for the rest of the hold.
        for at in [1_100, 1_500, 1_999] {
            assert!(picker.update(ms(at), &input(&[TRIGGER]), ROSTER).is_empty());
        }
        let events: Vec<_> = picker
            .update(ms(2_000), &input(&[TRIGGER]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Opened {
                selected: 0,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn a_tap_never_warns_and_a_fresh_hold_warns_again() {
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), ROSTER);
        picker.update(ms(0), &input(&[TRIGGER]), ROSTER);
        let events: Vec<_> = picker
            .update(ms(200), &input(&[]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::TriggerTapped],
            "a tap must not ask the host to prepare anything"
        );

        // The warning is per hold, so the next one gets its own.
        picker.update(ms(300), &input(&[TRIGGER]), ROSTER);
        let events: Vec<_> = picker
            .update(ms(1_300), &input(&[TRIGGER]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(events, vec![PickerEvent::Preparing]);
    }

    #[test]
    fn a_hold_abandoned_after_the_warning_still_reports_the_tap() {
        // The host started its overlay on the warning, so it has to be told the
        // wheel is not coming after all or the overlay would be left running.
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), ROSTER);
        picker.update(ms(0), &input(&[TRIGGER]), ROSTER);
        assert_eq!(
            picker.update(ms(1_200), &input(&[TRIGGER]), ROSTER).len(),
            1
        );
        let events: Vec<_> = picker
            .update(ms(1_500), &input(&[]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(events, vec![PickerEvent::TriggerTapped]);
        assert!(!picker.is_open());
    }

    #[test]
    fn releasing_before_the_threshold_reports_a_tap() {
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), ROSTER);
        picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
        let events: Vec<_> = picker
            .update(ms(500), &input(&[]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(events, vec![PickerEvent::TriggerTapped]);
        assert!(!picker.is_open());
        assert!(!picker.owns_trigger());
    }

    #[test]
    fn the_wheel_opens_on_the_active_profile() {
        let roster = PickerRoster::new(20, Some(9));
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), roster);
        picker.update(ms(10), &input(&[TRIGGER]), roster);
        let events: Vec<_> = picker
            .update(ms(2_010), &input(&[TRIGGER]), roster)
            .into_iter()
            .collect();
        // Index 9 with eight sectors per page is page 1, sector 1.
        assert_eq!(
            events,
            vec![PickerEvent::Opened {
                selected: 1,
                page: 1,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn a_roster_too_small_to_choose_from_never_arms() {
        for roster in [PickerRoster::new(0, None), PickerRoster::new(1, Some(0))] {
            let mut picker = Picker::new(PickerConfig::default());
            picker.update(ms(0), &input(&[]), roster);
            picker.update(ms(10), &input(&[TRIGGER]), roster);
            assert!(!picker.is_arming(), "{roster:?} should not arm");
            // The trigger stays the host's, so its binding still works.
            assert!(!picker.owns_trigger());
            assert!(picker
                .update(ms(5_000), &input(&[TRIGGER]), roster)
                .is_empty());
        }
    }

    #[test]
    fn a_roster_that_empties_mid_hold_hands_the_press_back() {
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), ROSTER);
        picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
        let events: Vec<_> = picker
            .update(ms(500), &input(&[TRIGGER]), PickerRoster::new(1, Some(0)))
            .into_iter()
            .collect();
        assert_eq!(events, vec![PickerEvent::TriggerTapped]);
    }

    #[test]
    fn the_wheel_stays_open_after_the_trigger_is_released() {
        let mut picker = opened(ROSTER);
        assert!(picker.update(ms(2_100), &input(&[]), ROSTER).is_empty());
        assert!(picker.is_open());
    }

    #[test]
    fn a_applies_the_pointed_at_profile_and_b_does_not() {
        let mut picker = opened(ROSTER);
        picker.update(ms(2_100), &input(&[]), ROSTER);
        let events: Vec<_> = picker
            .update(ms(2_200), &input(&[COMMIT]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Commit {
                index: 0,
                roster_revision: 0,
            }]
        );
        assert!(!picker.is_open());

        let mut picker = opened(ROSTER);
        picker.update(ms(2_100), &input(&[]), ROSTER);
        let events: Vec<_> = picker
            .update(ms(2_200), &input(&[DISMISS]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(events, vec![PickerEvent::Dismissed]);
        assert!(!picker.is_open());
    }

    #[test]
    fn the_wheel_can_be_opened_again_after_a_commit() {
        let mut picker = opened(ROSTER);
        // Commit while the trigger is still held, then release it.
        picker.update(ms(2_100), &input(&[TRIGGER, COMMIT]), ROSTER);
        assert!(!picker.is_open());
        picker.update(ms(2_200), &input(&[]), ROSTER);

        // A fresh hold must open the wheel a second time.
        picker.update(ms(3_000), &input(&[TRIGGER]), ROSTER);
        assert!(picker.is_arming(), "a fresh press must re-arm");
        let events: Vec<_> = picker
            .update(ms(5_010), &input(&[TRIGGER]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Opened {
                selected: 0,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn the_wheel_can_be_opened_again_after_the_trigger_is_released_first() {
        // The gesture the user actually performs: hold, let go, flick, press A.
        let mut picker = opened(ROSTER);
        picker.update(ms(2_100), &input(&[]), ROSTER);
        picker.update(ms(2_200), &input(&[COMMIT]), ROSTER);
        assert!(!picker.is_open());
        picker.update(ms(2_300), &input(&[]), ROSTER);

        picker.update(ms(3_000), &input(&[TRIGGER]), ROSTER);
        let events: Vec<_> = picker
            .update(ms(5_010), &input(&[TRIGGER]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Opened {
                selected: 0,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn a_second_trigger_press_dismisses_without_reopening() {
        let mut picker = opened(ROSTER);
        picker.update(ms(2_100), &input(&[]), ROSTER);
        let events: Vec<_> = picker
            .update(ms(2_200), &input(&[TRIGGER]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(events, vec![PickerEvent::Dismissed]);
        // Holding that same press must not arm a fresh hold, and releasing it
        // must not be reported as a tap.
        assert!(picker
            .update(ms(5_000), &input(&[TRIGGER]), ROSTER)
            .is_empty());
        assert!(picker.update(ms(5_100), &input(&[]), ROSTER).is_empty());
        assert!(!picker.is_open());
    }

    #[test]
    fn the_trigger_is_hidden_from_the_host_only_while_the_picker_owns_it() {
        let mut picker = Picker::new(PickerConfig::default());
        let held = buttons(&[TRIGGER, SteamButton::X]);
        picker.update(ms(0), &input(&[]), ROSTER);
        assert_eq!(picker.mask_trigger(held), held);

        picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
        assert!(picker.is_arming());
        assert_eq!(picker.mask_trigger(held), buttons(&[SteamButton::X]));

        picker.update(ms(2_010), &input(&[TRIGGER]), ROSTER);
        assert!(picker.is_open());
        assert_eq!(picker.mask_trigger(held), buttons(&[SteamButton::X]));

        picker.update(ms(2_100), &input(&[COMMIT]), ROSTER);
        assert_eq!(picker.mask_trigger(held), held);
    }

    #[test]
    fn the_button_that_applied_a_profile_is_held_back_until_released() {
        // Regression: the wheel closes on the press edge, so A is still down on
        // the next report. Lifting suppression there sent that press straight to
        // the game -- a press the user aimed at the overlay.
        let mut picker = opened(ROSTER);
        picker.update(ms(2_100), &input(&[]), ROSTER);
        picker.update(ms(2_200), &input(&[COMMIT]), ROSTER);
        assert!(!picker.is_open());

        let Some(OutputSuppression::Buttons(buttons)) = picker.suppression() else {
            panic!("a still-held commit must keep being withheld");
        };
        assert!(buttons.contains(Button::South));
        assert!(
            !buttons.contains(Button::North),
            "only what the wheel consumed is withheld; the rest of the pad works"
        );

        // Still down a few reports later.
        picker.update(ms(2_250), &input(&[COMMIT]), ROSTER);
        assert!(matches!(
            picker.suppression(),
            Some(OutputSuppression::Buttons(_))
        ));

        // Released, so the game gets the button back.
        picker.update(ms(2_300), &input(&[]), ROSTER);
        assert_eq!(picker.suppression(), None);

        // And a deliberate later press does reach the game.
        picker.update(ms(2_400), &input(&[COMMIT]), ROSTER);
        assert_eq!(picker.suppression(), None);
    }

    #[test]
    fn the_button_that_dismissed_the_wheel_is_held_back_too() {
        for closing in [DISMISS, TRIGGER] {
            let mut picker = opened(ROSTER);
            picker.update(ms(2_100), &input(&[]), ROSTER);
            picker.update(ms(2_200), &input(&[closing]), ROSTER);
            assert!(!picker.is_open());
            let Some(OutputSuppression::Buttons(buttons)) = picker.suppression() else {
                panic!("{closing:?} must keep being withheld while held");
            };
            assert!(buttons
                .contains(gamepad_button(closing).expect("consumed controls are directly mapped")));
            picker.update(ms(2_300), &input(&[]), ROSTER);
            assert_eq!(picker.suppression(), None, "{closing:?}");
        }
    }

    #[test]
    fn a_control_released_before_the_wheel_closed_is_never_latched() {
        // Committing with A while Quick Access was long since released must not
        // withhold Quick Access from the game.
        let mut picker = opened(ROSTER);
        picker.update(ms(2_100), &input(&[]), ROSTER);
        picker.update(ms(2_200), &input(&[COMMIT]), ROSTER);
        let Some(OutputSuppression::Buttons(buttons)) = picker.suppression() else {
            panic!("the commit is still held");
        };
        assert!(!buttons.contains(Button::Extra3));
    }

    #[test]
    fn a_forced_close_latches_nothing() {
        // The controller is gone, so no report will ever arrive to clear a
        // latch. Holding one would withhold those buttons forever.
        let mut picker = opened(ROSTER);
        assert!(picker.close());
        assert_eq!(picker.suppression(), None);
    }

    #[test]
    fn only_an_open_wheel_takes_the_output_from_the_game() {
        let mut picker = Picker::new(PickerConfig::default());
        assert!(picker.suppression().is_none());

        picker.update(ms(0), &input(&[]), ROSTER);
        picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
        // Arming still passes everything through; only opening takes over.
        assert!(picker.suppression().is_none());
        picker.update(ms(1_000), &input(&[TRIGGER]), ROSTER);
        assert!(
            picker.suppression().is_none(),
            "the halfway warning must not touch the game's input"
        );

        picker.update(ms(2_010), &input(&[TRIGGER]), ROSTER);
        assert!(picker.is_open());
        assert_eq!(picker.suppression(), Some(OutputSuppression::Neutral));

        // Closing hands the pad back, save for the still-held button that closed
        // it; releasing that clears the last of it.
        picker.update(ms(2_100), &input(&[DISMISS]), ROSTER);
        picker.update(ms(2_200), &input(&[]), ROSTER);
        assert!(picker.suppression().is_none());
    }

    #[test]
    fn closing_reports_whether_anything_was_active() {
        let mut picker = opened(ROSTER);
        assert!(picker.close());
        assert!(!picker.is_open());
        assert!(!picker.close());

        // After a forced close the next update is a fresh baseline, so a still
        // held trigger cannot immediately reopen the wheel.
        assert!(picker
            .update(ms(3_000), &input(&[TRIGGER]), ROSTER)
            .is_empty());
        assert!(!picker.is_arming());
    }

    fn stick(x: f32, y: f32) -> (i16, i16) {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "test helper builds in-range axis values"
        )]
        ((x * AXIS_FULL_SCALE) as i16, (y * AXIS_FULL_SCALE) as i16)
    }

    #[test]
    fn every_sector_centre_selects_its_own_sector() {
        for sectors in MIN_SECTORS_PER_PAGE..=MAX_SECTORS_PER_PAGE {
            let arc = TAU / sectors as f32;
            for sector in 0..sectors {
                let angle = arc * sector as f32;
                // Sector 0 is up, running clockwise.
                let (x, y) = (angle.sin(), angle.cos());
                assert_eq!(
                    sector_for(x, y, sectors),
                    sector,
                    "sector {sector} of {sectors}"
                );
            }
        }
    }

    #[test]
    fn sector_boundaries_fall_between_neighbours() {
        let sectors = 8;
        let arc = TAU / sectors as f32;
        for sector in 0..sectors {
            let boundary = arc * (sector as f32 + 0.5);
            let nudge = arc * 0.05;
            let before = boundary - nudge;
            let after = boundary + nudge;
            assert_eq!(sector_for(before.sin(), before.cos(), sectors), sector);
            assert_eq!(
                sector_for(after.sin(), after.cos(), sectors),
                (sector + 1) % sectors
            );
        }
    }

    #[test]
    fn straight_up_is_sector_zero_and_straight_right_is_a_quarter_turn() {
        assert_eq!(sector_for(0.0, 1.0, 8), 0);
        assert_eq!(sector_for(1.0, 0.0, 8), 2);
        assert_eq!(sector_for(0.0, -1.0, 8), 4);
        assert_eq!(sector_for(-1.0, 0.0, 8), 6);
        assert_eq!(sector_for(0.0, 0.0, 1), 0);
    }

    #[test]
    fn the_stick_must_be_pushed_past_the_engage_dead_zone_to_steer() {
        let mut picker = opened(ROSTER);
        let config = *picker.config();
        // Below the engage dead zone nothing moves, even pointing elsewhere.
        let weak = config.engage_dead_zone - 0.05;
        let events = picker.update(
            ms(2_100),
            &PickerInput {
                left_stick: stick(0.0, -weak),
                ..PickerInput::default()
            },
            ROSTER,
        );
        assert!(events.is_empty(), "a light push must not steer");

        let events: Vec<_> = picker
            .update(
                ms(2_200),
                &PickerInput {
                    left_stick: stick(0.0, -1.0),
                    ..PickerInput::default()
                },
                ROSTER,
            )
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Selection {
                selected: 2,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn the_selection_stays_put_when_the_stick_is_released() {
        let mut picker = opened(ROSTER);
        picker.update(
            ms(2_100),
            &PickerInput {
                left_stick: stick(0.0, -1.0),
                ..PickerInput::default()
            },
            ROSTER,
        );
        // Recentering must not snap the selection back to where it started;
        // the user still has to reach for A.
        assert!(picker.update(ms(2_200), &input(&[]), ROSTER).is_empty());
        let events: Vec<_> = picker
            .update(ms(2_300), &input(&[COMMIT]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Commit {
                index: 2,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn a_stick_between_the_dead_zones_keeps_steering_once_engaged() {
        let mut picker = opened(ROSTER);
        let config = *picker.config();
        picker.update(
            ms(2_100),
            &PickerInput {
                left_stick: stick(0.0, -1.0),
                ..PickerInput::default()
            },
            ROSTER,
        );
        // Still engaged, so drifting to a neighbour at reduced deflection works.
        let partial = config.engage_dead_zone.midpoint(config.track_dead_zone);
        let events: Vec<_> = picker
            .update(
                ms(2_200),
                &PickerInput {
                    left_stick: stick(-partial, 0.0),
                    ..PickerInput::default()
                },
                ROSTER,
            )
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Selection {
                selected: 3,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn the_stick_pushed_furthest_wins() {
        let mut picker = opened(ROSTER);
        let events: Vec<_> = picker
            .update(
                ms(2_100),
                &PickerInput {
                    // The left stick rests just past the dead zone pointing up;
                    // the right one is slammed down and must win.
                    left_stick: stick(0.0, 0.6),
                    right_stick: stick(0.0, -1.0),
                    ..PickerInput::default()
                },
                ROSTER,
            )
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Selection {
                selected: 2,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn either_stick_steers() {
        for build in [
            (|s| PickerInput {
                left_stick: s,
                ..PickerInput::default()
            }) as fn((i16, i16)) -> PickerInput,
            |s| PickerInput {
                right_stick: s,
                ..PickerInput::default()
            },
        ] {
            let mut picker = opened(ROSTER);
            let events: Vec<_> = picker
                .update(ms(2_100), &build(stick(1.0, 0.0)), ROSTER)
                .into_iter()
                .collect();
            assert_eq!(
                events,
                vec![PickerEvent::Selection {
                    selected: 1,
                    page: 0,
                    roster_revision: 0,
                }]
            );
        }
    }

    #[test]
    fn pages_cover_the_roster_without_gaps_or_overlap() {
        for len in 0..40_usize {
            for per_page in MIN_SECTORS_PER_PAGE..=MAX_SECTORS_PER_PAGE {
                let pages = page_count(len, per_page);
                assert!(pages >= 1);
                if len == 0 {
                    continue;
                }
                let covered: usize = (0..pages)
                    .map(|page| sectors_on_page(len, per_page, page))
                    .sum();
                assert_eq!(covered, len, "len {len} per_page {per_page}");
            }
        }
    }

    #[test]
    fn the_shoulders_page_and_the_commit_index_accounts_for_the_page() {
        let roster = PickerRoster::new(11, Some(0));
        let mut picker = opened(roster);
        let events: Vec<_> = picker
            .update(ms(2_100), &input(&[PAGE_NEXT]), roster)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Selection {
                selected: 0,
                page: 1,
                roster_revision: 0,
            }]
        );
        // Page 1 is a short page: indices 8..11 spread over three sectors, so
        // the arcs are 120 degrees and sector 2 points down-left.
        let angle = TAU / 3.0 * 2.0;
        picker.update(ms(2_200), &input(&[]), roster);
        picker.update(
            ms(2_300),
            &PickerInput {
                left_stick: stick(angle.sin(), angle.cos()),
                ..PickerInput::default()
            },
            roster,
        );
        let events: Vec<_> = picker
            .update(ms(2_400), &input(&[COMMIT]), roster)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Commit {
                index: 10,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn paging_wraps_in_both_directions() {
        let roster = PickerRoster::new(11, Some(0));
        let mut picker = opened(roster);
        let events: Vec<_> = picker
            .update(ms(2_100), &input(&[PAGE_PREVIOUS]), roster)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Selection {
                selected: 0,
                page: 1,
                roster_revision: 0,
            }]
        );
        picker.update(ms(2_200), &input(&[]), roster);
        let events: Vec<_> = picker
            .update(ms(2_300), &input(&[PAGE_NEXT]), roster)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Selection {
                selected: 0,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn a_single_page_roster_ignores_the_shoulders() {
        let mut picker = opened(ROSTER);
        assert!(picker
            .update(ms(2_100), &input(&[PAGE_NEXT]), ROSTER)
            .is_empty());
        assert!(picker
            .update(ms(2_200), &input(&[PAGE_PREVIOUS]), ROSTER)
            .is_empty());
    }

    #[test]
    fn a_roster_that_shrinks_while_open_clamps_the_selection() {
        let roster = PickerRoster::new(11, Some(9));
        let mut picker = opened(roster);
        assert!(picker.is_open());
        // Opened on page 1, sector 1. Losing the last page must pull the
        // selection back onto a page that still exists.
        let smaller = PickerRoster::new(3, Some(0));
        let events: Vec<_> = picker
            .update(ms(2_100), &input(&[]), smaller)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Selection {
                selected: 1,
                page: 0,
                roster_revision: 0,
            }]
        );
        let events: Vec<_> = picker
            .update(ms(2_200), &input(&[COMMIT]), smaller)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Commit {
                index: 1,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn a_roster_that_drops_below_two_while_open_dismisses_the_wheel() {
        let mut picker = opened(ROSTER);
        let events: Vec<_> = picker
            .update(ms(2_100), &input(&[]), PickerRoster::new(1, Some(0)))
            .into_iter()
            .collect();
        assert_eq!(events, vec![PickerEvent::Dismissed]);
        assert!(!picker.is_open());
        assert!(picker.suppression().is_none());
    }

    #[test]
    fn steering_and_committing_in_one_report_reports_both_in_order() {
        let mut picker = opened(ROSTER);
        let events: Vec<_> = picker
            .update(
                ms(2_100),
                &PickerInput {
                    buttons: buttons(&[COMMIT]),
                    left_stick: stick(0.0, -1.0),
                    ..PickerInput::default()
                },
                ROSTER,
            )
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![
                PickerEvent::Selection {
                    selected: 2,
                    page: 0,
                    roster_revision: 0,
                },
                PickerEvent::Commit {
                    index: 2,
                    roster_revision: 0,
                },
            ]
        );
    }

    #[test]
    fn nonsensical_configuration_is_clamped_into_something_usable() {
        let config = PickerConfig {
            hold: MAX_HOLD + Duration::from_secs(1),
            engage_dead_zone: f32::NAN,
            track_dead_zone: 9.0,
            sectors_per_page: 0,
        }
        .sanitized();
        assert_eq!(config.hold, MAX_HOLD);
        assert_eq!(config.sectors_per_page, MIN_SECTORS_PER_PAGE);
        assert_eq!(config.engage_dead_zone, DEFAULT_ENGAGE_DEAD_ZONE);
        assert!(config.track_dead_zone <= config.engage_dead_zone);
        assert!(config.track_dead_zone > 0.0);

        let config = PickerConfig {
            hold: Duration::from_millis(1),
            ..PickerConfig::default()
        }
        .sanitized();
        assert_eq!(config.hold, MIN_HOLD);
    }

    #[test]
    fn replacing_the_configuration_closes_the_wheel() {
        let mut picker = opened(ROSTER);
        picker.set_config(PickerConfig {
            hold: Duration::from_secs(3),
            ..PickerConfig::default()
        });
        assert!(!picker.is_open());
        assert_eq!(picker.config().hold, Duration::from_secs(3));

        // The trigger was still physically down when the configuration change
        // closed the wheel, so it stays withheld — from the game and from the
        // bindings engine alike — until the user lets go. Reports keep arriving
        // here, unlike a forced close, so the latch can drain normally.
        let Some(OutputSuppression::Buttons(withheld)) = picker.suppression() else {
            panic!("a still-held trigger must stay withheld across a config change");
        };
        assert!(withheld.contains(Button::Extra3));
        assert_eq!(picker.mask_trigger(buttons(&[TRIGGER])), SteamButtons(0));

        picker.update(ms(3_000), &input(&[]), ROSTER);
        assert!(picker.suppression().is_none());
        assert_eq!(
            picker.mask_trigger(buttons(&[TRIGGER])),
            buttons(&[TRIGGER])
        );
    }

    #[test]
    fn a_config_change_mid_hold_swallows_the_withheld_press() {
        // Halfway through a hold the trigger has been masked from the bindings
        // engine the whole time. The configuration change abandons the hold
        // without an event, so the press must stay swallowed until release —
        // unmasking it here would hand the engine a fresh down edge instead.
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), ROSTER);
        picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
        assert!(picker.is_arming());

        picker.set_config(PickerConfig {
            hold: Duration::from_secs(3),
            ..PickerConfig::default()
        });
        assert!(!picker.is_arming());
        assert_eq!(
            picker.mask_trigger(buttons(&[TRIGGER])),
            SteamButtons(0),
            "the still-held press must not become a fresh edge"
        );

        // Releasing drains the latch; the next deliberate press is the host's.
        assert!(picker.update(ms(1_000), &input(&[]), ROSTER).is_empty());
        assert_eq!(
            picker.mask_trigger(buttons(&[TRIGGER])),
            buttons(&[TRIGGER])
        );
    }

    #[test]
    fn dismissing_with_the_trigger_keeps_its_binding_masked_until_release() {
        // Regression: the second Quick Access press closes the wheel on its
        // down edge, which returns the picker to Idle on that same report. The
        // trigger must stay hidden from the bindings engine while latched, or
        // cancelling the wheel fires the user's Quick Access binding.
        let mut picker = opened(ROSTER);
        picker.update(ms(2_100), &input(&[]), ROSTER);
        let events: Vec<_> = picker
            .update(ms(2_200), &input(&[TRIGGER]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(events, vec![PickerEvent::Dismissed]);
        assert!(!picker.owns_trigger());
        assert_eq!(picker.mask_trigger(buttons(&[TRIGGER])), SteamButtons(0));

        // Held for a few more reports: still masked.
        picker.update(ms(2_250), &input(&[TRIGGER]), ROSTER);
        assert_eq!(picker.mask_trigger(buttons(&[TRIGGER])), SteamButtons(0));

        // Released and pressed again deliberately: the binding is back.
        picker.update(ms(2_300), &input(&[]), ROSTER);
        assert_eq!(
            picker.mask_trigger(buttons(&[TRIGGER])),
            buttons(&[TRIGGER])
        );
    }

    #[test]
    fn a_resting_thumb_inside_the_hysteresis_band_cannot_steal_the_wheel() {
        // Regression: with one shared steering flag, a stick that never crossed
        // the engage dead zone could take over the moment it was pushed a hair
        // further than the stick that did — flipping the selection to the
        // opposite side of the wheel because of a resting thumb.
        let mut picker = opened(ROSTER);
        picker.update(
            ms(2_100),
            &PickerInput {
                left_stick: stick(0.0, 1.0),
                ..PickerInput::default()
            },
            ROSTER,
        );

        // The left stick relaxes into the hysteresis band; the right thumb
        // rests slightly further out but never crossed engage. The selection
        // must stay with the left stick.
        assert!(
            picker
                .update(
                    ms(2_200),
                    &PickerInput {
                        left_stick: stick(0.0, 0.40),
                        right_stick: stick(0.0, -0.45),
                        ..PickerInput::default()
                    },
                    ROSTER,
                )
                .is_empty(),
            "a stick that never engaged must not steer"
        );

        // Slammed past engage, the other stick does take over.
        let events: Vec<_> = picker
            .update(
                ms(2_300),
                &PickerInput {
                    left_stick: stick(0.0, 0.40),
                    right_stick: stick(0.0, -1.0),
                    ..PickerInput::default()
                },
                ROSTER,
            )
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Selection {
                selected: 2,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn a_stall_that_jumps_past_the_hold_still_opens_the_wheel() {
        // A host that stalls between reports can present a `now` that is past
        // the full hold without the halfway warning ever having fired. The
        // wheel must open regardless; `Preparing` is an optimization, not a
        // precondition.
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), ROSTER);
        picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
        let events: Vec<_> = picker
            .update(ms(10_000), &input(&[TRIGGER]), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Opened {
                selected: 0,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn a_roster_with_no_active_profile_opens_on_the_first_sector() {
        let roster = PickerRoster::new(4, None);
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), roster);
        picker.update(ms(10), &input(&[TRIGGER]), roster);
        let events: Vec<_> = picker
            .update(ms(2_010), &input(&[TRIGGER]), roster)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Opened {
                selected: 0,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn an_out_of_range_active_profile_clamps_to_the_last_sector() {
        // The roster and the active index come from the host over a channel,
        // so a stale pair must degrade to a sane selection, not a panic.
        let roster = PickerRoster::new(4, Some(99));
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), roster);
        picker.update(ms(10), &input(&[TRIGGER]), roster);
        let events: Vec<_> = picker
            .update(ms(2_010), &input(&[TRIGGER]), roster)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Opened {
                selected: 3,
                page: 0,
                roster_revision: 0,
            }]
        );
    }

    #[test]
    fn a_shoulder_held_across_a_commit_is_withheld_until_released() {
        let roster = PickerRoster::new(11, Some(0));
        let mut picker = opened(roster);
        picker.update(ms(2_100), &input(&[]), roster);
        // Page with L1 still held while committing with A.
        picker.update(ms(2_200), &input(&[PAGE_NEXT]), roster);
        picker.update(ms(2_300), &input(&[PAGE_NEXT, COMMIT]), roster);
        assert!(!picker.is_open());
        let Some(OutputSuppression::Buttons(withheld)) = picker.suppression() else {
            panic!("held consumed controls must stay withheld after the close");
        };
        assert!(withheld.contains(Button::RightShoulder));
        assert!(withheld.contains(Button::South));
    }

    #[test]
    fn latched_controls_are_released_one_at_a_time() {
        // Dismiss with a second trigger press while A is also held: both are
        // consumed controls, and each must come back to the game individually
        // as the user lets go of it.
        let mut picker = opened(ROSTER);
        picker.update(ms(2_100), &input(&[]), ROSTER);
        picker.update(ms(2_200), &input(&[COMMIT, TRIGGER]), ROSTER);
        assert!(!picker.is_open());
        let Some(OutputSuppression::Buttons(withheld)) = picker.suppression() else {
            panic!("both held controls must be latched");
        };
        assert!(withheld.contains(Button::South));
        assert!(withheld.contains(Button::Extra3));

        // Let go of A first: only the trigger stays withheld.
        picker.update(ms(2_300), &input(&[TRIGGER]), ROSTER);
        let Some(OutputSuppression::Buttons(withheld)) = picker.suppression() else {
            panic!("the still-held trigger must stay latched");
        };
        assert!(!withheld.contains(Button::South));
        assert!(withheld.contains(Button::Extra3));

        picker.update(ms(2_400), &input(&[]), ROSTER);
        assert!(picker.suppression().is_none());
    }

    #[test]
    fn inverted_dead_zones_clamp_track_below_engage() {
        let config = PickerConfig {
            engage_dead_zone: 0.2,
            track_dead_zone: 0.5,
            ..PickerConfig::default()
        }
        .sanitized();
        assert_eq!(config.engage_dead_zone, 0.2);
        assert_eq!(config.track_dead_zone, 0.2);
    }

    #[test]
    fn geometry_backstops_hold_for_out_of_range_input() {
        // Callers subtract one from `sectors_on_page`, so a page past the end
        // must report one sector, never zero.
        assert_eq!(sectors_on_page(3, 8, 9), 1);
        assert_eq!(page_count(0, 8), 1);
        assert_eq!(page_count(8, 0), 1);
    }

    #[test]
    fn a_clock_that_does_not_advance_never_opens_the_wheel() {
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), ROSTER);
        picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
        // saturating_sub keeps a non-monotonic clock from wrapping into a hold.
        for _ in 0..10 {
            assert!(picker.update(ms(5), &input(&[TRIGGER]), ROSTER).is_empty());
        }
        assert!(!picker.is_open());
    }
}
