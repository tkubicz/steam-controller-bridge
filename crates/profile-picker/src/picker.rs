use std::time::Duration;

use controller_mapper::gamepad_button;
use gamepad_state::{GamepadButtons, OutputSuppression};
use steam_controller_protocol::{SteamButton, SteamButtons};

use crate::geometry::{bit, magnitude, normalize, page_count, sector_for, sectors_on_page};
use crate::types::{
    PickerConfig, PickerEvent, PickerEvents, PickerInput, PickerRoster, COMMIT, DISMISS, PAGE_NEXT,
    PAGE_PREVIOUS, TRIGGER,
};

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
    pub(super) const fn is_arming(&self) -> bool {
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
