#[allow(
    clippy::wildcard_imports,
    reason = "command and status methods operate on the supervisor's private state"
)]
use super::*;

impl Supervisor {
    pub(super) fn service_idle_commands(&mut self) {
        while let Ok(command) = self.commands.try_recv() {
            self.apply_idle_command(command);
        }
    }

    pub(super) fn wait_for_command(&mut self) {
        if let Ok(command) = self.commands.recv_timeout(DISCOVERY_INTERVAL) {
            self.apply_idle_command(command);
        }
    }

    pub(super) fn wait_or_command(&mut self, duration: Duration) {
        if let Ok(command) = self.commands.recv_timeout(duration) {
            self.apply_idle_command(command);
        }
    }

    pub(super) fn apply_idle_command(&mut self, command: RuntimeCommand) {
        match command {
            RuntimeCommand::Start(ack) => {
                if let Some(error) = &self.startup_blocker {
                    let _ = ack.send(Err(error.clone()));
                } else {
                    self.desired_running = true;
                    let _ = ack.send(Ok(()));
                }
            }
            RuntimeCommand::Stop(ack) => {
                self.desired_running = false;
                self.transition(RuntimeState::Stopping, "Stopping bridge", None);
                self.clear_hardware_status();
                self.pending_stop_acks.push(ack);
            }
            RuntimeCommand::SuspendForSleep(ack) => {
                self.suspended = true;
                self.wake_settle = None;
                self.transition(RuntimeState::Stopping, "Suspending for system sleep", None);
                self.clear_hardware_status();
                // Acknowledged with the stop acks, after every handle is gone.
                self.pending_stop_acks.push(ack);
            }
            RuntimeCommand::ResumeFromWake(ack) => {
                if self.suspended {
                    self.suspended = false;
                    self.wake_settle = Some(Instant::now() + WAKE_SETTLE_DELAY);
                }
                let _ = ack.send(Ok(()));
            }
            RuntimeCommand::Shutdown(ack) => {
                self.desired_running = false;
                self.shutdown_requested = true;
                self.transition(RuntimeState::Stopping, "Stopping bridge", None);
                self.clear_hardware_status();
                self.pending_shutdown_acks.push(ack);
            }
            RuntimeCommand::SetIdleShutdown(timeout, ack) => {
                let result = validate_idle_shutdown_timeout(timeout);
                if result.is_ok() {
                    self.config.idle_shutdown_timeout = timeout;
                    self.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
                    self.update_status(|status| {
                        status.automatic_shutdown.configured_timeout = timeout;
                        status.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
                        status.automatic_shutdown.neutral_idle_age = None;
                    });
                }
                let _ = ack.send(result);
            }
            RuntimeCommand::SetPuckDockAction(action, ack) => {
                self.automatic_shutdown
                    .set_dock_action(action, &self.config);
                self.config.puck_dock_action = action;
                self.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
                let automatic = self
                    .automatic_shutdown
                    .status(&self.config, None, Instant::now());
                self.update_status(|status| status.automatic_shutdown = automatic);
                let _ = ack.send(Ok(()));
            }
            RuntimeCommand::SetBindingProfile(profile, ack) => {
                let profile = *profile;
                self.config.binding_profile.clone_from(&profile);
                self.desktop_bindings.replace_profile(profile, ack);
            }
            RuntimeCommand::EnableDesktopBindings(ack) => {
                self.desktop_bindings.enable(ack);
            }
            RuntimeCommand::SetPickerConfig(config, ack) => {
                self.config.profile_picker = config.map(PickerConfig::sanitized);
                let picker_status = picker_status(&self.config, false);
                self.update_status(|status| status.profile_picker = picker_status);
                let _ = ack.send(Ok(()));
            }
            RuntimeCommand::SetPickerRoster(roster, ack) => {
                self.config.picker_roster = roster;
                let picker_status = picker_status(&self.config, false);
                self.update_status(|status| status.profile_picker = picker_status);
                let _ = ack.send(Ok(()));
            }
        }
    }

    // Commands act on the whole active session, and bundling its parts into a
    // struct only to unpack them again here would hide which ones each command
    // actually touches.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn service_active_commands(
        &mut self,
        now: Duration,
        idle_activity: &mut IdleActivityTracker,
        picker: &mut PickerRuntime,
        engine: &mut BridgeEngine,
        output: &mut OutputSession,
        worker: &HidWorker,
    ) -> Option<ActiveExit> {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                RuntimeCommand::Start(ack) => {
                    if let Some(error) = &self.startup_blocker {
                        let _ = ack.send(Err(error.clone()));
                    } else {
                        let _ = ack.send(Ok(()));
                    }
                }
                RuntimeCommand::Stop(ack) => {
                    self.desired_running = false;
                    // The active loop acknowledges after its neutral-before-release cleanup.
                    return Some(ActiveExit::StoppedWithAck(ack));
                }
                RuntimeCommand::SuspendForSleep(ack) => {
                    self.suspended = true;
                    self.wake_settle = None;
                    // Same cleanup as a stop: the device is parked at neutral
                    // and every handle is closed before the ack lets the
                    // caller's sleep handler return.
                    return Some(ActiveExit::SuspendedWithAck(ack));
                }
                RuntimeCommand::ResumeFromWake(ack) => {
                    // Already awake and running; nothing to resume.
                    let _ = ack.send(Ok(()));
                }
                RuntimeCommand::Shutdown(ack) => {
                    self.desired_running = false;
                    self.shutdown_requested = true;
                    return Some(ActiveExit::ShutdownWithAck(ack));
                }
                RuntimeCommand::SetIdleShutdown(timeout, ack) => {
                    let result = validate_idle_shutdown_timeout(timeout);
                    if result.is_ok() {
                        self.config.idle_shutdown_timeout = timeout;
                        idle_activity.set_timeout(timeout, now);
                        self.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
                        eprintln!(
                            "level=info event=idle_shutdown_setting_changed timeout_secs={:?}",
                            timeout.map(|value| value.as_secs())
                        );
                    }
                    let _ = ack.send(result);
                }
                RuntimeCommand::SetPuckDockAction(action, ack) => {
                    self.automatic_shutdown
                        .set_dock_action(action, &self.config);
                    self.config.puck_dock_action = action;
                    self.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
                    eprintln!("level=info event=puck_dock_action_changed action={action:?}");
                    let _ = ack.send(Ok(()));
                }
                RuntimeCommand::SetBindingProfile(profile, ack) => {
                    neutralize_before_desktop_work(engine, output);
                    worker.clear_pad_feedback();
                    let profile = *profile;
                    self.config.binding_profile.clone_from(&profile);
                    self.desktop_bindings.replace_profile(profile, ack);
                }
                RuntimeCommand::EnableDesktopBindings(ack) => {
                    neutralize_before_desktop_work(engine, output);
                    self.desktop_bindings.enable(ack);
                }
                RuntimeCommand::SetPickerConfig(config, ack) => {
                    self.config.profile_picker = config.map(PickerConfig::sanitized);
                    // A reconfigured wheel is a closed wheel — and a cancelled
                    // hold counts too, or the overlay spawned at `Preparing`
                    // would outlive the hold it was spawned for. The picker
                    // latches whatever is still held, so the suppression it
                    // reports (not a blanket clear) is what hands the game its
                    // controls back without leaking the in-flight press.
                    if picker.set_config(self.config.profile_picker) {
                        engine.set_output_suppression(picker.suppression());
                        self.emit_picker_event(PickerEvent::Dismissed);
                    }
                    let picker_status = picker_status(&self.config, false);
                    self.update_status(|status| status.profile_picker = picker_status);
                    eprintln!(
                        "level=info event=profile_picker_configured enabled={}",
                        self.config.profile_picker.is_some()
                    );
                    let _ = ack.send(Ok(()));
                }
                RuntimeCommand::SetPickerRoster(roster, ack) => {
                    self.config.picker_roster = roster;
                    let picker_status = picker_status(&self.config, picker.is_open());
                    self.update_status(|status| status.profile_picker = picker_status);
                    let _ = ack.send(Ok(()));
                }
            }
        }
        None
    }

    pub(super) fn emit_picker_event(&self, event: PickerEvent) {
        (self.picker_events)(event);
    }

    /// Closes the wheel and tells the frontend, for a controller that went away
    /// or a session that is ending. Suppression dies with the engine.
    pub(super) fn dismiss_picker(&self, picker: &mut PickerRuntime) {
        if picker.close() {
            self.emit_picker_event(PickerEvent::Dismissed);
            let picker_status = picker_status(&self.config, false);
            self.update_status(|status| status.profile_picker = picker_status);
        }
    }

    pub(super) fn transition(&self, state: RuntimeState, detail: &str, error: Option<&str>) {
        self.update_status(|status| {
            status.state = state;
            detail.clone_into(&mut status.detail);
            if let Some(error) = error {
                status.last_error = Some(error.to_owned());
            } else if matches!(state, RuntimeState::Running | RuntimeState::Stopped) {
                status.last_error = None;
            }
        });
    }

    pub(super) fn current_state(&self) -> RuntimeState {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state
    }

    pub(super) fn update_source_discovered(&self, info: &HidDeviceInfo, active: bool) {
        self.update_status(|status| {
            status.source = ControllerSourceStatus {
                identity: Some(info.clone()),
                transport: info.controller_transport(),
                connected: true,
                active,
            };
        });
    }

    pub(super) fn clear_controller_status(&self) {
        self.update_status(|status| {
            status.source = ControllerSourceStatus::default();
            status.controller = ControllerStatus::default();
            status.battery_percent = None;
            status.battery_charge_state = None;
            status.lizard = LizardStatus::default();
        });
    }

    pub(super) fn clear_hardware_status(&self) {
        self.update_status(|status| {
            status.source = ControllerSourceStatus::default();
            status.controller = ControllerStatus::default();
            status.xiao = XiaoStatus::default();
            status.battery_percent = None;
            status.battery_charge_state = None;
            status.lizard = LizardStatus::default();
        });
    }

    pub(super) fn update_status(&self, update: impl FnOnce(&mut BridgeStatus)) -> bool {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = status.clone();
        update(&mut status);
        let changed = status.state != previous.state
            || status.detail != previous.detail
            || status.source != previous.source
            || status.controller != previous.controller
            || status.xiao != previous.xiao
            || status.battery_percent != previous.battery_percent
            || status.battery_charge_state != previous.battery_charge_state
            || status.lizard != previous.lizard
            || status.haptics != previous.haptics
            || status.bindings != previous.bindings
            || status.profile_picker != previous.profile_picker
            || status.automatic_shutdown != previous.automatic_shutdown
            || status.bridge_metrics != previous.bridge_metrics
            || status.output_diagnostics != previous.output_diagnostics
            || status.last_error != previous.last_error;
        if changed {
            status.revision = status.revision.wrapping_add(1);
        }
        changed
    }
}
