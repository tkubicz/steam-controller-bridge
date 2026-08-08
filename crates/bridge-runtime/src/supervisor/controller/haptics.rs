#[allow(
    clippy::wildcard_imports,
    reason = "lizard and haptics supervisors share the controller's private dependencies"
)]
use super::*;

#[derive(Debug, Default)]
pub(crate) struct SharedLizardMetrics {
    pub(crate) active: AtomicBool,
    pub(crate) refreshes: AtomicU64,
    pub(crate) failures: AtomicU64,
    pub(crate) last_refresh_millis: AtomicU64,
}

impl SharedLizardMetrics {
    pub(crate) fn record_success(&self, now: Duration) {
        self.active.store(true, Ordering::Release);
        self.refreshes.fetch_add(1, Ordering::Relaxed);
        let millis = u64::try_from(now.as_millis())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        self.last_refresh_millis.store(millis, Ordering::Release);
    }

    pub(crate) fn record_failure(&self) {
        self.active.store(false, Ordering::Release);
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_disconnected(&self) {
        self.active.store(false, Ordering::Release);
        self.last_refresh_millis.store(0, Ordering::Release);
    }

    pub(crate) fn snapshot(&self, now: Duration) -> LizardStatus {
        let last_refresh_millis = self.last_refresh_millis.load(Ordering::Acquire);
        LizardStatus {
            suppressed: self.active.load(Ordering::Acquire),
            refreshes: self.refreshes.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            last_refresh_age: (last_refresh_millis > 0)
                .then(|| now.saturating_sub(Duration::from_millis(last_refresh_millis - 1))),
        }
    }
}

pub(crate) struct LizardSupervisor {
    pub(crate) mode: LizardMode,
    pub(crate) heartbeat: LizardModeHeartbeat,
    pub(crate) metrics: Arc<SharedLizardMetrics>,
}

impl LizardSupervisor {
    pub(crate) fn new(mode: LizardMode, metrics: Arc<SharedLizardMetrics>) -> Self {
        Self {
            mode,
            heartbeat: LizardModeHeartbeat::new(),
            metrics,
        }
    }

    pub(crate) fn connected(
        &mut self,
        now: Duration,
        session: &HidSession,
    ) -> Result<(), DeviceError> {
        self.heartbeat.connected();
        if self.mode == LizardMode::Suppress {
            self.refresh(now, session)?;
        }
        Ok(())
    }

    pub(crate) fn service(
        &mut self,
        now: Duration,
        session: &HidSession,
    ) -> Result<(), DeviceError> {
        if self.mode == LizardMode::Suppress && self.heartbeat.refresh_due(now) {
            self.refresh(now, session)?;
        }
        Ok(())
    }

    pub(crate) fn refresh(
        &mut self,
        now: Duration,
        session: &HidSession,
    ) -> Result<(), DeviceError> {
        if let Err(error) = session.suppress_lizard_mode() {
            self.metrics.record_failure();
            return Err(error);
        }
        self.heartbeat.refreshed(now);
        self.metrics.record_success(now);
        Ok(())
    }

    pub(crate) fn disconnected(&mut self) {
        self.heartbeat.disconnected();
        self.metrics.record_disconnected();
    }
}

#[derive(Debug, Default)]
pub(crate) struct SharedHapticsMetrics {
    pub(crate) active: AtomicBool,
    pub(crate) degraded: AtomicBool,
    pub(crate) pad_degraded: AtomicBool,
    pub(crate) commands_received: AtomicU64,
    pub(crate) writes: AtomicU64,
    pub(crate) refreshes: AtomicU64,
    pub(crate) coalesced_commands: AtomicU64,
    pub(crate) failures: AtomicU64,
    pub(crate) last_command_millis: AtomicU64,
    pub(crate) pad_feedback_ticks: AtomicU64,
    pub(crate) pad_feedback_coalesced: AtomicU64,
    pub(crate) pad_feedback_failures: AtomicU64,
    pub(crate) last_pad_feedback_millis: AtomicU64,
    pub(crate) pad_feedback_last_error: Mutex<Option<String>>,
}

impl SharedHapticsMetrics {
    pub(crate) fn record_command(&self, now: Duration, coalesced: bool) {
        self.commands_received.fetch_add(1, Ordering::Relaxed);
        if coalesced {
            self.coalesced_commands.fetch_add(1, Ordering::Relaxed);
        }
        let millis = u64::try_from(now.as_millis())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        self.last_command_millis.store(millis, Ordering::Release);
    }

    pub(crate) fn record_success(&self, active: bool, refresh: bool) {
        self.active.store(active, Ordering::Release);
        self.degraded.store(false, Ordering::Release);
        self.writes.fetch_add(1, Ordering::Relaxed);
        if refresh {
            self.refreshes.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_failure(&self) {
        self.active.store(false, Ordering::Release);
        self.degraded.store(true, Ordering::Release);
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_disconnected(&self) {
        self.active.store(false, Ordering::Release);
        self.degraded.store(false, Ordering::Release);
        self.pad_degraded.store(false, Ordering::Release);
        self.last_command_millis.store(0, Ordering::Release);
        self.last_pad_feedback_millis.store(0, Ordering::Release);
        *self
            .pad_feedback_last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    pub(crate) fn record_pad_coalesced(&self, count: u64) {
        if count > 0 {
            self.pad_feedback_coalesced
                .fetch_add(count, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_pad_success(&self, now: Duration) {
        self.pad_degraded.store(false, Ordering::Release);
        self.pad_feedback_ticks.fetch_add(1, Ordering::Relaxed);
        let millis = u64::try_from(now.as_millis())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        self.last_pad_feedback_millis
            .store(millis, Ordering::Release);
        *self
            .pad_feedback_last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    pub(crate) fn record_pad_failure(&self, error: &str) {
        self.pad_degraded.store(true, Ordering::Release);
        self.pad_feedback_failures.fetch_add(1, Ordering::Relaxed);
        *self
            .pad_feedback_last_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(bounded_error(error));
    }

    pub(crate) fn snapshot(&self, now: Duration) -> HapticsStatus {
        let last_command_millis = self.last_command_millis.load(Ordering::Acquire);
        let last_pad_feedback_millis = self.last_pad_feedback_millis.load(Ordering::Acquire);
        let state =
            if self.degraded.load(Ordering::Acquire) || self.pad_degraded.load(Ordering::Acquire) {
                HapticsState::Degraded
            } else if self.active.load(Ordering::Acquire) {
                HapticsState::Active
            } else {
                HapticsState::Idle
            };
        HapticsStatus {
            state,
            commands_received: self.commands_received.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            refreshes: self.refreshes.load(Ordering::Relaxed),
            coalesced_commands: self.coalesced_commands.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            last_command_age: (last_command_millis > 0)
                .then(|| now.saturating_sub(Duration::from_millis(last_command_millis - 1))),
            pad_feedback_ticks: self.pad_feedback_ticks.load(Ordering::Relaxed),
            pad_feedback_coalesced: self.pad_feedback_coalesced.load(Ordering::Relaxed),
            pad_feedback_failures: self.pad_feedback_failures.load(Ordering::Relaxed),
            last_pad_feedback_age: (last_pad_feedback_millis > 0)
                .then(|| now.saturating_sub(Duration::from_millis(last_pad_feedback_millis - 1))),
            pad_feedback_last_error: self
                .pad_feedback_last_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RumbleCommand {
    pub(crate) low_frequency: u16,
    pub(crate) high_frequency: u16,
}

impl RumbleCommand {
    pub(crate) const fn is_active(self) -> bool {
        self.low_frequency != 0 || self.high_frequency != 0
    }
}

#[derive(Debug, Default)]
pub(crate) struct LatestRumbleSlot {
    pub(crate) command: Mutex<Option<RumbleCommand>>,
}

impl LatestRumbleSlot {
    pub(crate) fn publish(&self, command: RumbleCommand) -> bool {
        self.command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(command)
            .is_some()
    }

    pub(crate) fn take(&self) -> Option<RumbleCommand> {
        self.command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    pub(crate) fn clear(&self) {
        self.command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PadFeedbackCommand {
    pub(crate) side: PadHapticSide,
    pub(crate) gain: PadHapticGain,
}

#[derive(Debug, Default)]
pub(crate) struct PendingPadFeedbackState {
    pub(crate) left_gain: Option<PadHapticGain>,
    pub(crate) right_gain: Option<PadHapticGain>,
}

#[derive(Debug, Default)]
pub(crate) struct PendingPadFeedback {
    pub(crate) state: Mutex<PendingPadFeedbackState>,
}

impl PendingPadFeedback {
    pub(crate) fn publish(&self, request: PadFeedbackRequest) -> u64 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut coalesced = 0;
        if let Some(strength) = request.left {
            coalesced += u64::from(state.left_gain.replace(strength.haptic_gain()).is_some());
        }
        if let Some(strength) = request.right {
            coalesced += u64::from(state.right_gain.replace(strength.haptic_gain()).is_some());
        }
        coalesced
    }

    pub(crate) fn take(&self) -> Vec<PadFeedbackCommand> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let left = state.left_gain.take();
        let right = state.right_gain.take();
        match (left, right) {
            (Some(left), Some(right)) if left == right => vec![PadFeedbackCommand {
                side: PadHapticSide::Both,
                gain: left,
            }],
            (Some(left), Some(right)) => vec![
                PadFeedbackCommand {
                    side: PadHapticSide::Left,
                    gain: left,
                },
                PadFeedbackCommand {
                    side: PadHapticSide::Right,
                    gain: right,
                },
            ],
            (Some(gain), None) => vec![PadFeedbackCommand {
                side: PadHapticSide::Left,
                gain,
            }],
            (None, Some(gain)) => vec![PadFeedbackCommand {
                side: PadHapticSide::Right,
                gain,
            }],
            (None, None) => Vec::new(),
        }
    }

    pub(crate) fn clear(&self) {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            PendingPadFeedbackState::default();
    }
}

pub(crate) trait RumbleWriter {
    fn write_rumble(&self, low_frequency: u16, high_frequency: u16) -> Result<(), String>;
}

impl RumbleWriter for HidSession {
    fn write_rumble(&self, low_frequency: u16, high_frequency: u16) -> Result<(), String> {
        self.set_rumble(low_frequency, high_frequency)
            .map_err(|error| error.to_string())
    }
}

pub(crate) trait PadFeedbackWriter {
    fn write_pad_feedback(&self, side: PadHapticSide, gain: PadHapticGain) -> Result<(), String>;
}

impl PadFeedbackWriter for HidSession {
    fn write_pad_feedback(&self, side: PadHapticSide, gain: PadHapticGain) -> Result<(), String> {
        self.pad_haptic_tick(side, gain)
            .map_err(|error| error.to_string())
    }
}

pub(crate) struct PadFeedbackSupervisor {
    pub(crate) connected: bool,
    pub(crate) retry_after: Option<Duration>,
    pub(crate) metrics: Arc<SharedHapticsMetrics>,
}

impl PadFeedbackSupervisor {
    pub(crate) fn new(metrics: Arc<SharedHapticsMetrics>) -> Self {
        Self {
            connected: false,
            retry_after: None,
            metrics,
        }
    }

    pub(crate) fn connected(&mut self) {
        self.connected = true;
        self.retry_after = None;
    }

    pub(crate) fn disconnected(&mut self) {
        self.connected = false;
        self.retry_after = None;
    }

    pub(crate) fn service(
        &mut self,
        now: Duration,
        session: &impl PadFeedbackWriter,
        commands: Vec<PadFeedbackCommand>,
    ) {
        if !self.connected
            || self
                .retry_after
                .is_some_and(|retry_after| now < retry_after)
        {
            return;
        }
        for command in commands {
            match session.write_pad_feedback(command.side, command.gain) {
                Ok(()) => self.metrics.record_pad_success(now),
                Err(error) => {
                    self.retry_after = Some(now.saturating_add(PAD_FEEDBACK_RETRY_INTERVAL));
                    self.metrics.record_pad_failure(&error);
                    eprintln!(
                        "level=warn event=pad_feedback_write_failed error={error:?} retry_ms={}",
                        PAD_FEEDBACK_RETRY_INTERVAL.as_millis()
                    );
                    break;
                }
            }
        }
    }
}

pub(crate) struct HapticsSupervisor {
    pub(crate) connected: bool,
    pub(crate) desired: RumbleCommand,
    pub(crate) lease_received: Option<Duration>,
    pub(crate) last_write: Option<Duration>,
    pub(crate) retry_after: Option<Duration>,
    pub(crate) metrics: Arc<SharedHapticsMetrics>,
}

impl HapticsSupervisor {
    pub(crate) fn new(metrics: Arc<SharedHapticsMetrics>) -> Self {
        Self {
            connected: false,
            desired: RumbleCommand::default(),
            lease_received: None,
            last_write: None,
            retry_after: None,
            metrics,
        }
    }

    pub(crate) fn connected(&mut self, now: Duration, session: &impl RumbleWriter) {
        self.connected = true;
        self.desired = RumbleCommand::default();
        self.lease_received = None;
        self.last_write = None;
        self.retry_after = None;
        self.write(now, session, self.desired, false);
    }

    pub(crate) fn command(
        &mut self,
        now: Duration,
        session: &impl RumbleWriter,
        command: RumbleCommand,
    ) {
        let changed = command != self.desired;
        self.desired = command;
        if command.is_active() {
            self.lease_received = Some(now);
            if changed && self.retry_due(now) {
                self.write(now, session, command, false);
            }
        } else {
            self.lease_received = None;
            if changed
                || self.metrics.active.load(Ordering::Acquire)
                || self.metrics.degraded.load(Ordering::Acquire)
            {
                self.write(now, session, command, false);
            }
        }
    }

    pub(crate) fn service(&mut self, now: Duration, session: &impl RumbleWriter) {
        if self.desired.is_active()
            && self
                .lease_received
                .is_some_and(|received| now.saturating_sub(received) >= RUMBLE_LEASE_TIMEOUT)
        {
            self.desired = RumbleCommand::default();
            self.lease_received = None;
            self.write(now, session, self.desired, false);
            return;
        }
        if !self.desired.is_active() {
            return;
        }
        if self.metrics.degraded.load(Ordering::Acquire) {
            if self.retry_due(now) {
                self.write(now, session, self.desired, false);
            }
            return;
        }
        if self
            .last_write
            .is_some_and(|written| now.saturating_sub(written) >= RUMBLE_REFRESH_INTERVAL)
        {
            self.write(now, session, self.desired, true);
        }
    }

    pub(crate) fn shutdown(&mut self, now: Duration, session: &impl RumbleWriter) {
        self.desired = RumbleCommand::default();
        self.lease_received = None;
        if self.connected {
            self.write(now, session, self.desired, false);
            self.connected = false;
        }
    }

    pub(crate) fn disconnected(&mut self) {
        self.connected = false;
        self.desired = RumbleCommand::default();
        self.lease_received = None;
        self.last_write = None;
        self.retry_after = None;
        self.metrics.record_disconnected();
    }

    pub(crate) fn retry_due(&self, now: Duration) -> bool {
        self.retry_after
            .is_none_or(|retry_after| now >= retry_after)
    }

    pub(crate) fn write(
        &mut self,
        now: Duration,
        session: &impl RumbleWriter,
        command: RumbleCommand,
        refresh: bool,
    ) {
        match session.write_rumble(command.low_frequency, command.high_frequency) {
            Ok(()) => {
                self.last_write = Some(now);
                self.retry_after = None;
                self.metrics.record_success(command.is_active(), refresh);
            }
            Err(error) => {
                self.retry_after = Some(now.saturating_add(RUMBLE_RETRY_INTERVAL));
                self.metrics.record_failure();
                eprintln!(
                    "level=warn event=rumble_write_failed error={error:?} retry_ms={}",
                    RUMBLE_RETRY_INTERVAL.as_millis()
                );
            }
        }
    }
}
