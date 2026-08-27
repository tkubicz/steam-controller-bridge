use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use bridge_output::OutputFeedback;

pub(crate) const MAX_EFFECTS: u16 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RumbleParameters {
    pub(crate) strong: u16,
    pub(crate) weak: u16,
    pub(crate) delay: Duration,
    pub(crate) length: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Playback {
    requested_at: Instant,
    count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Effect {
    parameters: RumbleParameters,
    playback: Option<Playback>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RumbleEffectError {
    InvalidId(i16),
    UnknownId(i16),
}

impl std::fmt::Display for RumbleEffectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => write!(formatter, "invalid force-feedback effect ID {id}"),
            Self::UnknownId(id) => write!(formatter, "unknown force-feedback effect ID {id}"),
        }
    }
}

impl std::error::Error for RumbleEffectError {}

#[derive(Debug, Default)]
pub(crate) struct RumbleEffects {
    effects: BTreeMap<i16, Effect>,
    aggregate: (u16, u16),
    pending: Option<OutputFeedback>,
}

impl RumbleEffects {
    pub(crate) fn upload(
        &mut self,
        id: i16,
        parameters: RumbleParameters,
        now: Instant,
    ) -> Result<(), RumbleEffectError> {
        validate_id(id)?;
        let playback = self.effects.get(&id).and_then(|effect| {
            (!effect.is_finished(now))
                .then_some(effect.playback)
                .flatten()
        });
        self.effects.insert(
            id,
            Effect {
                parameters,
                playback,
            },
        );
        Ok(())
    }

    pub(crate) fn erase(&mut self, id: i16) -> Result<(), RumbleEffectError> {
        validate_id(id)?;
        if self.effects.remove(&id).is_none() {
            return Err(RumbleEffectError::UnknownId(id));
        }
        Ok(())
    }

    pub(crate) fn control(
        &mut self,
        id: i16,
        count: u32,
        now: Instant,
    ) -> Result<(), RumbleEffectError> {
        validate_id(id)?;
        let effect = self
            .effects
            .get_mut(&id)
            .ok_or(RumbleEffectError::UnknownId(id))?;
        effect.playback = (count > 0).then_some(Playback {
            requested_at: now,
            count,
        });
        Ok(())
    }

    pub(crate) fn refresh(&mut self, now: Instant) {
        for effect in self.effects.values_mut() {
            if effect.is_finished(now) {
                effect.playback = None;
            }
        }
        let aggregate = self
            .effects
            .values()
            .filter(|effect| effect.is_active(now))
            .fold((0, 0), |(strong, weak), effect| {
                (
                    strong.max(effect.parameters.strong),
                    weak.max(effect.parameters.weak),
                )
            });
        if aggregate != self.aggregate {
            self.aggregate = aggregate;
            self.pending = Some(OutputFeedback::Rumble {
                low_frequency: aggregate.0,
                high_frequency: aggregate.1,
            });
        }
    }

    pub(crate) fn clear(&mut self) {
        self.effects.clear();
        if self.aggregate != (0, 0) {
            self.aggregate = (0, 0);
            self.pending = Some(OutputFeedback::Rumble {
                low_frequency: 0,
                high_frequency: 0,
            });
        }
    }

    pub(crate) fn take_feedback(&mut self) -> Option<OutputFeedback> {
        self.pending.take()
    }
}

impl Effect {
    fn is_active(self, now: Instant) -> bool {
        let Some(playback) = self.playback else {
            return false;
        };
        let Some(start) = playback.requested_at.checked_add(self.parameters.delay) else {
            return false;
        };
        if now < start {
            return false;
        }
        if self.parameters.length.is_zero() {
            return true;
        }
        let Some(total_length) = self.parameters.length.checked_mul(playback.count) else {
            return true;
        };
        start
            .checked_add(total_length)
            .is_none_or(|expiry| now < expiry)
    }

    fn is_finished(self, now: Instant) -> bool {
        let Some(playback) = self.playback else {
            return false;
        };
        if self.parameters.length.is_zero() {
            return false;
        }
        let Some(start) = playback.requested_at.checked_add(self.parameters.delay) else {
            return false;
        };
        let Some(total_length) = self.parameters.length.checked_mul(playback.count) else {
            return false;
        };
        start
            .checked_add(total_length)
            .is_some_and(|expiry| now >= expiry)
    }
}

fn validate_id(id: i16) -> Result<(), RumbleEffectError> {
    if u16::try_from(id).is_ok_and(|id| id < MAX_EFFECTS) {
        Ok(())
    } else {
        Err(RumbleEffectError::InvalidId(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parameters(strong: u16, weak: u16, delay_ms: u64, length_ms: u64) -> RumbleParameters {
        RumbleParameters {
            strong,
            weak,
            delay: Duration::from_millis(delay_ms),
            length: Duration::from_millis(length_ms),
        }
    }

    fn rumble(strong: u16, weak: u16) -> OutputFeedback {
        OutputFeedback::Rumble {
            low_frequency: strong,
            high_frequency: weak,
        }
    }

    fn upload_and_refresh(
        effects: &mut RumbleEffects,
        id: i16,
        parameters: RumbleParameters,
        now: Instant,
    ) {
        effects.upload(id, parameters, now).unwrap();
        effects.refresh(now);
    }

    fn control_and_refresh(effects: &mut RumbleEffects, id: i16, count: u32, now: Instant) {
        effects.control(id, count, now).unwrap();
        effects.refresh(now);
    }

    fn erase_and_refresh(effects: &mut RumbleEffects, id: i16, now: Instant) {
        effects.erase(id).unwrap();
        effects.refresh(now);
    }

    #[test]
    fn upload_update_play_stop_and_erase_follow_effect_ids() {
        let now = Instant::now();
        let mut effects = RumbleEffects::default();
        upload_and_refresh(&mut effects, 3, parameters(10, 20, 0, 100), now);
        assert_eq!(effects.take_feedback(), None);
        control_and_refresh(&mut effects, 3, 1, now);
        assert_eq!(effects.take_feedback(), Some(rumble(10, 20)));
        upload_and_refresh(&mut effects, 3, parameters(30, 40, 0, 100), now);
        assert_eq!(effects.take_feedback(), Some(rumble(30, 40)));
        control_and_refresh(&mut effects, 3, 0, now);
        assert_eq!(effects.take_feedback(), Some(rumble(0, 0)));
        erase_and_refresh(&mut effects, 3, now);
        assert_eq!(effects.take_feedback(), None);
        assert_eq!(effects.erase(3), Err(RumbleEffectError::UnknownId(3)));
    }

    #[test]
    fn delay_expiry_and_replay_count_are_serviced_over_time() {
        let now = Instant::now();
        let mut effects = RumbleEffects::default();
        upload_and_refresh(&mut effects, 0, parameters(100, 200, 20, 30), now);
        control_and_refresh(&mut effects, 0, 2, now);
        effects.refresh(now + Duration::from_millis(19));
        assert_eq!(effects.take_feedback(), None);
        effects.refresh(now + Duration::from_millis(20));
        assert_eq!(effects.take_feedback(), Some(rumble(100, 200)));
        effects.refresh(now + Duration::from_millis(79));
        assert_eq!(effects.take_feedback(), None);
        effects.refresh(now + Duration::from_millis(80));
        assert_eq!(effects.take_feedback(), Some(rumble(0, 0)));
    }

    #[test]
    fn zero_length_effect_continues_until_stopped() {
        let now = Instant::now();
        let mut effects = RumbleEffects::default();
        upload_and_refresh(&mut effects, 0, parameters(1, 2, 0, 0), now);
        control_and_refresh(&mut effects, 0, 1, now);
        assert_eq!(effects.take_feedback(), Some(rumble(1, 2)));
        effects.refresh(now + Duration::from_secs(1));
        assert_eq!(effects.take_feedback(), None);
        control_and_refresh(&mut effects, 0, 0, now + Duration::from_secs(1));
        assert_eq!(effects.take_feedback(), Some(rumble(0, 0)));
    }

    #[test]
    fn overlapping_effects_use_each_channels_maximum() {
        let now = Instant::now();
        let mut effects = RumbleEffects::default();
        upload_and_refresh(&mut effects, 0, parameters(100, 20, 0, 100), now);
        upload_and_refresh(&mut effects, 1, parameters(30, 200, 0, 100), now);
        control_and_refresh(&mut effects, 0, 1, now);
        assert_eq!(effects.take_feedback(), Some(rumble(100, 20)));
        control_and_refresh(&mut effects, 1, 1, now);
        assert_eq!(effects.take_feedback(), Some(rumble(100, 200)));
        erase_and_refresh(&mut effects, 0, now);
        assert_eq!(effects.take_feedback(), Some(rumble(30, 200)));
        erase_and_refresh(&mut effects, 1, now);
        assert_eq!(effects.take_feedback(), Some(rumble(0, 0)));
    }

    #[test]
    fn invalid_and_unknown_ids_are_rejected_without_state_changes() {
        let now = Instant::now();
        let mut effects = RumbleEffects::default();
        assert_eq!(
            effects.upload(-1, parameters(1, 2, 0, 0), now),
            Err(RumbleEffectError::InvalidId(-1))
        );
        assert_eq!(
            effects.control(15, 1, now),
            Err(RumbleEffectError::UnknownId(15))
        );
        assert_eq!(effects.erase(16), Err(RumbleEffectError::InvalidId(16)));
        assert_eq!(effects.take_feedback(), None);
    }

    #[test]
    fn clear_publishes_zero_only_when_rumble_was_active() {
        let now = Instant::now();
        let mut effects = RumbleEffects::default();
        effects.clear();
        assert_eq!(effects.take_feedback(), None);
        upload_and_refresh(&mut effects, 0, parameters(1, 2, 0, 0), now);
        control_and_refresh(&mut effects, 0, 1, now);
        assert_eq!(effects.take_feedback(), Some(rumble(1, 2)));
        effects.clear();
        assert_eq!(effects.take_feedback(), Some(rumble(0, 0)));
    }

    #[test]
    fn updating_an_expired_effect_does_not_restart_it() {
        let now = Instant::now();
        let mut effects = RumbleEffects::default();
        upload_and_refresh(&mut effects, 0, parameters(1, 2, 0, 10), now);
        control_and_refresh(&mut effects, 0, 1, now);
        assert_eq!(effects.take_feedback(), Some(rumble(1, 2)));
        let after_expiry = now + Duration::from_millis(10);
        effects.refresh(after_expiry);
        assert_eq!(effects.take_feedback(), Some(rumble(0, 0)));
        upload_and_refresh(&mut effects, 0, parameters(10, 20, 0, 100), after_expiry);
        assert_eq!(effects.take_feedback(), None);
    }

    #[test]
    fn one_refresh_after_a_batch_publishes_only_the_final_aggregate() {
        let now = Instant::now();
        let mut effects = RumbleEffects::default();
        effects.upload(0, parameters(100, 20, 0, 100), now).unwrap();
        effects.upload(1, parameters(30, 200, 0, 100), now).unwrap();
        effects.control(0, 1, now).unwrap();
        effects.control(1, 1, now).unwrap();
        assert_eq!(effects.take_feedback(), None);
        effects.refresh(now);
        assert_eq!(effects.take_feedback(), Some(rumble(100, 200)));
    }
}
