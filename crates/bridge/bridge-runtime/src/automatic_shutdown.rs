use super::{
    same_controller_collection, AutomaticShutdownPhase, AutomaticShutdownStatus, BindingProfile,
    ControllerChargeState, ControllerTransport, DesktopBindingsState, DesktopBindingsStatus,
    Duration, HidDeviceInfo, Instant, PuckDockAction, RuntimeConfig, ShutdownTrigger,
    AUTOMATIC_SHUTDOWN_RETRY_INTERVAL, MAX_IDLE_SHUTDOWN_TIMEOUT, MIN_IDLE_SHUTDOWN_TIMEOUT,
};

pub(crate) fn automatic_shutdown_phase(config: &RuntimeConfig) -> AutomaticShutdownPhase {
    if config.idle_shutdown_timeout.is_none() && config.puck_dock_action == PuckDockAction::LeaveOn
    {
        AutomaticShutdownPhase::Disabled
    } else {
        AutomaticShutdownPhase::Monitoring
    }
}

pub(crate) fn validate_idle_shutdown_timeout(timeout: Option<Duration>) -> Result<(), String> {
    if timeout.is_some_and(|value| value < MIN_IDLE_SHUTDOWN_TIMEOUT) {
        return Err("idle-shutdown timeout must be at least one minute".to_owned());
    }
    if timeout.is_some_and(|value| value > MAX_IDLE_SHUTDOWN_TIMEOUT) {
        return Err("idle-shutdown timeout cannot exceed 1440 minutes".to_owned());
    }
    Ok(())
}

pub(crate) struct AutomaticShutdownRuntime {
    pub(crate) phase: AutomaticShutdownPhase,
    pub(crate) trigger: Option<ShutdownTrigger>,
    pub(crate) successful_shutdowns: u64,
    pub(crate) failures: u64,
    pub(crate) last_success: Option<Instant>,
    pub(crate) retry_after: Option<Instant>,
    pub(crate) dock_identity: Option<HidDeviceInfo>,
    pub(crate) dock_episode_handled: bool,
    pub(crate) dock_failure_at: Option<Instant>,
}

impl AutomaticShutdownRuntime {
    pub(crate) fn new(config: &RuntimeConfig) -> Self {
        Self {
            phase: automatic_shutdown_phase(config),
            trigger: None,
            successful_shutdowns: 0,
            failures: 0,
            last_success: None,
            retry_after: None,
            dock_identity: None,
            dock_episode_handled: false,
            dock_failure_at: None,
        }
    }

    pub(crate) fn source_selected(&mut self, info: &HidDeviceInfo, config: &RuntimeConfig) {
        if self
            .dock_identity
            .as_ref()
            .is_some_and(|identity| !same_controller_collection(identity, info))
        {
            self.clear_dock_episode("source_replaced");
        }
        self.dock_identity = Some(info.clone());
        self.phase = automatic_shutdown_phase(config);
        self.trigger = None;
        self.retry_after = None;
        self.dock_failure_at = None;
    }

    pub(crate) fn set_dock_action(&mut self, action: PuckDockAction, config: &RuntimeConfig) {
        if action != config.puck_dock_action {
            self.clear_dock_episode("policy_changed");
        }
    }

    pub(crate) fn clear_dock_episode(&mut self, reason: &str) {
        if self.dock_episode_handled {
            eprintln!("level=info event=puck_dock_episode_cleared reason={reason:?}");
        }
        self.dock_episode_handled = false;
        self.dock_failure_at = None;
    }

    pub(crate) fn observe_charge_state(
        &mut self,
        info: &HidDeviceInfo,
        charge_state: ControllerChargeState,
        action: PuckDockAction,
    ) -> bool {
        if charge_state == ControllerChargeState::Discharging {
            self.clear_dock_episode("discharging");
            return false;
        }
        action == PuckDockAction::PowerOff
            && info.controller_transport() == Some(ControllerTransport::Puck)
            && charge_state.is_external_power()
            && !self.dock_episode_handled
    }

    pub(crate) fn activity_after_failed_dock_attempt(&mut self, now: Instant) {
        if self.trigger == Some(ShutdownTrigger::PuckDock)
            && self.dock_failure_at.is_some_and(|failure| now > failure)
            && !self.dock_episode_handled
        {
            self.dock_episode_handled = true;
            self.retry_after = None;
            eprintln!(
                "level=info event=puck_dock_episode_handled reason=activity_after_failed_shutdown"
            );
        }
    }

    pub(crate) fn begin(&mut self, trigger: ShutdownTrigger) {
        self.phase = AutomaticShutdownPhase::PoweringOff;
        self.trigger = Some(trigger);
        eprintln!("level=info event=automatic_shutdown_started trigger={trigger:?}");
    }

    pub(crate) fn succeeded(&mut self, now: Instant, trigger: ShutdownTrigger) {
        self.phase = AutomaticShutdownPhase::Sleeping;
        self.trigger = Some(trigger);
        self.successful_shutdowns = self.successful_shutdowns.wrapping_add(1);
        self.last_success = Some(now);
        self.retry_after = None;
        self.dock_failure_at = None;
        if trigger == ShutdownTrigger::PuckDock {
            self.dock_episode_handled = true;
            eprintln!("level=info event=puck_dock_episode_handled reason=power_off_succeeded");
        }
        eprintln!("level=info event=automatic_shutdown_succeeded trigger={trigger:?}");
    }

    pub(crate) fn failed(&mut self, now: Instant, trigger: ShutdownTrigger, error: &str) {
        self.phase = AutomaticShutdownPhase::Degraded;
        self.trigger = Some(trigger);
        self.failures = self.failures.wrapping_add(1);
        self.retry_after = Some(now + AUTOMATIC_SHUTDOWN_RETRY_INTERVAL);
        self.dock_failure_at = (trigger == ShutdownTrigger::PuckDock).then_some(now);
        eprintln!(
            "level=warn event=automatic_shutdown_failed trigger={trigger:?} error={error:?} retry_ms={}",
            AUTOMATIC_SHUTDOWN_RETRY_INTERVAL.as_millis()
        );
    }

    pub(crate) fn retry_due(&self, now: Instant) -> bool {
        self.retry_after.is_none_or(|deadline| now >= deadline)
    }

    pub(crate) fn status(
        &self,
        config: &RuntimeConfig,
        idle_age: Option<Duration>,
        now: Instant,
    ) -> AutomaticShutdownStatus {
        AutomaticShutdownStatus {
            configured_timeout: config.idle_shutdown_timeout,
            puck_dock_action: config.puck_dock_action,
            puck_dock_episode_handled: self.dock_episode_handled,
            neutral_idle_age: idle_age,
            phase: self.phase,
            trigger: self.trigger,
            successful_shutdowns: self.successful_shutdowns,
            failures: self.failures,
            last_successful_shutdown_age: self
                .last_success
                .map(|last| now.saturating_duration_since(last)),
            retry_after: self
                .retry_after
                .map(|retry| retry.saturating_duration_since(now)),
        }
    }
}

pub(crate) struct ControllerCooldown {
    pub(crate) info: HidDeviceInfo,
    pub(crate) until: Instant,
}

pub(crate) fn binding_status_for_profile(
    profile: Option<&BindingProfile>,
) -> DesktopBindingsStatus {
    let Some(profile) = profile else {
        return DesktopBindingsStatus::default();
    };
    let configured_count = profile.configured_output_count();
    DesktopBindingsStatus {
        state: if configured_count == 0 {
            DesktopBindingsState::Disabled
        } else {
            DesktopBindingsState::PermissionRequired
        },
        active_profile_id: Some(profile.id.clone()),
        active_profile_name: Some(profile.name.clone()),
        configured_binding_count: configured_count,
        ..DesktopBindingsStatus::default()
    }
}
