use super::{
    Duration, OutputSuppression, Picker, PickerConfig, PickerEvents, PickerInput, PickerRoster,
    SteamButtons,
};

/// The profile wheel as the active loop sees it.
///
/// Wraps the pure state machine so the loop can ask one object every question
/// it has: what to hide from the game, what to hide from the desktop bindings,
/// and what the user just chose. A `None` picker is the feature switched off,
/// and every method then behaves as if the wheel does not exist.
pub(crate) struct PickerRuntime {
    /// Kept after the feature is switched off so a just-closed wheel's
    /// still-held controls keep draining; [`PickerRuntime::observe`] hands a
    /// disabled picker an empty roster, which can never arm.
    pub(crate) picker: Option<Picker>,
    pub(crate) enabled: bool,
}

impl PickerRuntime {
    pub(crate) fn new(config: Option<PickerConfig>) -> Self {
        Self {
            enabled: config.is_some(),
            picker: config.map(Picker::new),
        }
    }

    /// Replaces the configuration. Returns whether a wheel — open, or a hold
    /// partway toward one — was cancelled, which the caller must answer by
    /// dismissing the overlay.
    ///
    /// The picker itself latches whatever consumed controls are still held, so
    /// the press that was aimed at the wheel cannot leak into the game or the
    /// bindings engine; the caller keeps applying [`PickerRuntime::suppression`]
    /// as usual and the latch drains on release.
    pub(crate) fn set_config(&mut self, config: Option<PickerConfig>) -> bool {
        let was_active = self.picker.as_ref().is_some_and(Picker::owns_trigger);
        self.enabled = config.is_some();
        match (self.picker.as_mut(), config) {
            (Some(picker), Some(config)) => picker.set_config(config),
            (Some(picker), None) => {
                // Re-applying the current configuration closes the wheel and
                // latches the held controls without discarding the drain state.
                let config = *picker.config();
                picker.set_config(config);
            }
            (None, Some(config)) => self.picker = Some(Picker::new(config)),
            (None, None) => {}
        }
        was_active
    }

    pub(crate) fn is_open(&self) -> bool {
        self.picker.as_ref().is_some_and(Picker::is_open)
    }

    pub(crate) fn suppression(&self) -> Option<OutputSuppression> {
        self.picker.as_ref().and_then(Picker::suppression)
    }

    pub(crate) fn mask_trigger(&self, buttons: SteamButtons) -> SteamButtons {
        self.picker
            .as_ref()
            .map_or(buttons, |picker| picker.mask_trigger(buttons))
    }

    pub(crate) fn observe(
        &mut self,
        now: Duration,
        input: &PickerInput,
        roster: PickerRoster,
    ) -> PickerEvents {
        // A disabled picker still sees reports so its latch can drain, but an
        // empty roster keeps it from ever arming again.
        let roster = if self.enabled {
            roster
        } else {
            PickerRoster::default()
        };
        self.picker
            .as_mut()
            .map_or_else(PickerEvents::default, |picker| {
                picker.update(now, input, roster)
            })
    }

    /// Forces the wheel shut. Returns whether it had anything to close.
    pub(crate) fn close(&mut self) -> bool {
        self.picker.as_mut().is_some_and(Picker::close)
    }
}
