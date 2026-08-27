use std::time::Duration;

use gamepad_state::GamepadState;
use steam_controller_protocol::SteamControllerState;

/// Allocation-free meaningful-input tracking for automatic idle shutdown.
#[derive(Debug, Clone)]
pub(crate) struct IdleActivityTracker {
    timeout: Option<Duration>,
    neutral_since: Option<Duration>,
    latest_neutral: Option<bool>,
}

impl IdleActivityTracker {
    pub(crate) const fn new(timeout: Option<Duration>) -> Self {
        Self {
            timeout,
            neutral_since: None,
            latest_neutral: None,
        }
    }

    pub(crate) fn set_timeout(&mut self, timeout: Option<Duration>, now: Duration) {
        self.timeout = timeout;
        self.reset(now);
    }

    pub(crate) fn reset(&mut self, now: Duration) {
        self.neutral_since = self
            .latest_neutral
            .is_some_and(|neutral| neutral)
            .then_some(now);
    }

    pub(crate) fn pause(&mut self) {
        self.neutral_since = None;
        self.latest_neutral = None;
    }

    pub(crate) fn observe(
        &mut self,
        now: Duration,
        source: &SteamControllerState,
        mapped: &GamepadState,
    ) -> bool {
        let active = meaningful_activity(source, mapped);
        let neutral = !active;
        if active {
            self.neutral_since = None;
        } else if self.latest_neutral != Some(true) {
            self.neutral_since = Some(now);
        }
        self.latest_neutral = Some(neutral);
        active
    }

    pub(crate) fn is_neutral(&self) -> bool {
        self.latest_neutral == Some(true)
    }

    pub(crate) fn idle_age(&self, now: Duration) -> Option<Duration> {
        self.neutral_since.map(|since| now.saturating_sub(since))
    }

    pub(crate) fn deadline_reached(&self, now: Duration) -> bool {
        self.timeout.is_some_and(|timeout| {
            self.idle_age(now)
                .is_some_and(|idle_age| idle_age >= timeout)
        })
    }
}

fn meaningful_activity(source: &SteamControllerState, mapped: &GamepadState) -> bool {
    source.buttons.0 != 0
        || mapped.left_x != 0.0
        || mapped.left_y != 0.0
        || mapped.right_x != 0.0
        || mapped.right_y != 0.0
        || mapped.left_trigger != 0.0
        || mapped.right_trigger != 0.0
        || source.left_pad_touched
        || source.left_pad_pressed
        || source.right_pad_touched
        || source.right_pad_pressed
        || source.left_grip_touched
        || source.right_grip_touched
}

#[cfg(test)]
mod tests {
    use super::*;
    use steam_controller_protocol::{SteamButton, SteamButtons};

    fn source() -> SteamControllerState {
        SteamControllerState {
            report_id: 0x45,
            sequence: 0,
            buttons: SteamButtons::default(),
            left_trigger: 0,
            right_trigger: 0,
            left_stick_x: 0,
            left_stick_y: 0,
            right_stick_x: 0,
            right_stick_y: 0,
            left_pad_x: 0,
            left_pad_y: 0,
            left_pad_pressure: 0,
            left_pad_touched: false,
            left_pad_pressed: false,
            right_pad_x: 0,
            right_pad_y: 0,
            right_pad_pressure: 0,
            right_pad_touched: false,
            right_pad_pressed: false,
            left_grip_touched: false,
            right_grip_touched: false,
            imu_timestamp: 0,
            gyro: None,
            acceleration: None,
            raw_report: Vec::new(),
        }
    }

    #[test]
    fn neutral_time_reaches_deadline_and_reports_do_not_reset_it() {
        let mut tracker = IdleActivityTracker::new(Some(Duration::from_secs(5)));
        let source = source();
        let mapped = GamepadState::neutral();
        assert!(!tracker.observe(Duration::ZERO, &source, &mapped));
        assert!(!tracker.observe(Duration::from_secs(4), &source, &mapped));
        assert!(!tracker.deadline_reached(Duration::from_millis(4_999)));
        assert!(tracker.deadline_reached(Duration::from_secs(5)));
    }

    #[test]
    fn mapped_analog_and_unmapped_touch_activity_reset_the_interval() {
        let mut tracker = IdleActivityTracker::new(Some(Duration::from_secs(5)));
        let mut source = source();
        let mut mapped = GamepadState::neutral();
        tracker.observe(Duration::ZERO, &source, &mapped);
        mapped.left_x = 0.5;
        assert!(tracker.observe(Duration::from_secs(4), &source, &mapped));
        assert!(!tracker.deadline_reached(Duration::from_secs(20)));
        mapped = GamepadState::neutral();
        tracker.observe(Duration::from_secs(5), &source, &mapped);
        assert!(!tracker.deadline_reached(Duration::from_secs(9)));
        assert!(tracker.deadline_reached(Duration::from_secs(10)));

        source.buttons = SteamButtons(1 << SteamButton::LeftPadTouch as u8);
        source.left_pad_touched = true;
        assert!(tracker.observe(Duration::from_secs(11), &source, &mapped));

        source = self::source();
        mapped.left_trigger = 0.25;
        assert!(tracker.observe(Duration::from_secs(12), &source, &mapped));

        mapped = GamepadState::neutral();
        source.buttons = SteamButtons(1 << SteamButton::A as u8);
        assert!(tracker.observe(Duration::from_secs(13), &source, &mapped));
    }

    #[test]
    fn timeout_change_starts_a_fresh_interval_and_never_disables_deadline() {
        let mut tracker = IdleActivityTracker::new(Some(Duration::from_secs(30)));
        tracker.observe(Duration::ZERO, &source(), &GamepadState::neutral());
        tracker.set_timeout(Some(Duration::from_secs(5)), Duration::from_secs(20));
        assert!(!tracker.deadline_reached(Duration::from_secs(24)));
        assert!(tracker.deadline_reached(Duration::from_secs(25)));
        tracker.set_timeout(None, Duration::from_secs(30));
        assert!(!tracker.deadline_reached(Duration::from_mins(5)));
    }
}
