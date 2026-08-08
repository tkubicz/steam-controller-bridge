#[allow(
    clippy::wildcard_imports,
    reason = "the active session shares the supervisor's safety-critical dependencies"
)]
use super::*;

impl Supervisor {
    #[allow(clippy::too_many_lines)] // Safety ordering is clearest in one linear ownership loop.
    pub(super) fn run_active(
        &mut self,
        active: ActiveControllerSource,
        mut output: OutputSession,
    ) -> Result<(ActiveExit, OutputSession, Result<(), String>), String> {
        self.automatic_shutdown
            .source_selected(&active.info, &self.config);
        let initial_controller_seen = active.controller_seen;
        let mut engine = BridgeEngine::new(self.config.bridge, self.config.mapper)
            .map_err(|error| error.to_string())?;
        let mut recording = self.open_recording()?;
        let started = Instant::now();
        let dropped = Arc::new(AtomicU64::new(0));
        let mut worker =
            match HidWorker::spawn(active, self.config.lizard_mode, Arc::clone(&dropped)) {
                Ok(worker) => worker,
                Err(error) => {
                    let _ = engine.shutdown(&mut *output.output);
                    return Err(error);
                }
            };
        if let Err(error) = output.output.service() {
            let _ = engine.shutdown(&mut *output.output);
            worker.shutdown()?;
            return Err(format!(
                "XIAO service failed before input activation: {error}"
            ));
        }
        while output.output.take_feedback().is_some() {
            // Feedback received before the input worker exists is not a valid
            // post-reconnect lease. A continuing effect will be refreshed by
            // the XIAO within 25 ms.
        }
        let mut last_controller_state = Instant::now();
        let mut controller_state_seen = initial_controller_seen;
        let mut controller_connected = initial_controller_seen;
        let mut last_status = Instant::now()
            .checked_sub(STATUS_INTERVAL)
            .unwrap_or_else(Instant::now);
        let mut idle_activity = IdleActivityTracker::new(self.config.idle_shutdown_timeout);
        let mut latest_charge_state: Option<ControllerChargeState> = None;
        let mut last_charge_report = None;
        let mut pending_automatic_shutdown = None;
        self.desktop_bindings.enable_async();
        let mut picker = PickerRuntime::new(self.config.profile_picker);
        engine.connected();
        self.automatic_shutdown.phase = automatic_shutdown_phase(&self.config);
        self.automatic_shutdown.trigger = None;
        self.transition(RuntimeState::Running, "Bridge running", None);
        let automatic_status = self
            .automatic_shutdown
            .status(&self.config, None, Instant::now());
        let binding_status = self.desktop_bindings.status();
        self.update_status(|status| {
            status.source.connected = true;
            status.source.active = initial_controller_seen;
            status.controller.connected = initial_controller_seen;
            status.controller.last_state_age = initial_controller_seen.then_some(Duration::ZERO);
            status.lizard = worker.lizard_diagnostics();
            status.haptics = worker.haptics_diagnostics();
            status.bindings = binding_status;
            status.profile_picker = picker_status(&self.config, false);
            status.automatic_shutdown = automatic_status;
        });
        eprintln!(
            "level=info event=bridge_running input_transport={:?} input_interface={} \
             input_product={:?} input_serial={} xiao_path={:?} lizard_mode={:?}",
            worker.device_info().controller_transport(),
            worker.device_info().interface_number,
            worker.device_info().product,
            masked_serial(worker.device_info().serial_number.as_deref()),
            output.xiao.as_ref().map(|info| info.path.as_str()),
            self.config.lizard_mode
        );
        record_device_event(
            &mut recording,
            started,
            KIND_DEVICE_CONNECTED,
            Some(worker.device_info()),
        )?;

        let exit = 'active: loop {
            let mut iteration_timer = SupervisorIterationTimer::new("commands");
            let command_exit = self.service_active_commands(
                started.elapsed(),
                &mut idle_activity,
                &mut picker,
                &mut engine,
                &mut output,
                &worker,
            );
            iteration_timer.enter("worker_health");
            if let Some(command_exit) = command_exit {
                break command_exit;
            }
            if let Some(error) = worker.take_failure() {
                self.dismiss_picker(&mut picker);
                let _ = engine.shutdown(&mut *output.output);
                let _ = self.desktop_bindings.disconnect();
                worker.shutdown()?;
                self.clear_controller_status();
                return Err(error);
            }
            let mut direct_report = None;
            iteration_timer.enter("hid_wait");
            match worker.receiver.recv_timeout(RUNTIME_POLL_INTERVAL) {
                Ok(HidWorkerEvent::Connected(info)) => {
                    self.update_source_discovered(&info, false);
                }
                Ok(HidWorkerEvent::Disconnected) => {
                    let _ = engine.disconnected(&mut *output.output);
                    break ActiveExit::SourceLost;
                }
                Ok(HidWorkerEvent::StatusReport(report)) => {
                    direct_report = Some(report);
                }
                Ok(HidWorkerEvent::ReportReady) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break ActiveExit::SourceLost;
                }
            }
            iteration_timer.enter("mailbox");
            let batch = worker.take_report_batch();
            if batch.overflowed {
                self.desktop_bindings.overflow();
                worker.clear_pad_feedback();
                eprintln!(
                    "level=warn event=desktop_binding_mailbox_overflow action=release_and_rebaseline"
                );
            }
            iteration_timer.enter("controller_reports");
            for report in direct_report.into_iter().chain(batch.reports) {
                match process_report(
                    &report,
                    &mut engine,
                    &mut *output.output,
                    &mut recording,
                    started,
                    &mut idle_activity,
                ) {
                    Ok(ReportEffect::ControllerState {
                        meaningful_activity,
                        desktop_input,
                        picker_input,
                    }) => {
                        let now = started.elapsed();
                        let was_open = picker.is_open();
                        let events = picker.observe(now, &picker_input, self.config.picker_roster);
                        let tapped = events
                            .iter()
                            .any(|event| matches!(event, PickerEvent::TriggerTapped));
                        for event in events {
                            self.emit_picker_event(event);
                        }
                        // Refreshed every report, not just on an open/close
                        // edge: the wheel keeps withholding the button that
                        // closed it until the user lets go, so what is
                        // suppressed changes while the wheel is already shut.
                        engine.set_output_suppression(picker.suppression());
                        if picker.is_open() != was_open {
                            let picker_status = picker_status(&self.config, picker.is_open());
                            self.update_status(|status| status.profile_picker = picker_status);
                        }
                        if tapped {
                            // The hold swallowed the press to keep a Quick
                            // Access binding from firing. It turned out to be
                            // an ordinary press, so deliver it now as a tap:
                            // this synthesizes the down edge, and the real
                            // snapshot below is already the matching up edge.
                            self.desktop_bindings.observe(DesktopInputSnapshot {
                                buttons: profile_picker::with_trigger(desktop_input.buttons),
                                ..desktop_input
                            });
                        }
                        let desktop_input = DesktopInputSnapshot {
                            buttons: picker.mask_trigger(desktop_input.buttons),
                            ..desktop_input
                        };
                        self.desktop_bindings.observe(desktop_input);
                        last_controller_state = Instant::now();
                        controller_connected = true;
                        if meaningful_activity {
                            self.automatic_shutdown
                                .activity_after_failed_dock_attempt(Instant::now());
                        }
                        // Only the first state publishes eagerly, so the UI reacts
                        // without waiting for STATUS_INTERVAL. Doing this per report
                        // would clone the whole status and take the shared lock at
                        // controller report rate; the periodic block below keeps
                        // these same fields current afterwards.
                        if !controller_state_seen {
                            controller_state_seen = true;
                            self.update_status(|status| {
                                status.source.active = true;
                                status.controller.connected = true;
                                status.controller.last_state_age = Some(Duration::ZERO);
                            });
                        }
                    }
                    Ok(ReportEffect::Connected) => {
                        controller_connected = true;
                    }
                    Ok(ReportEffect::Battery {
                        percent,
                        charge_state,
                    }) => {
                        let charging_transition = latest_charge_state
                            .is_some_and(|previous| !previous.is_external_power())
                            && charge_state.is_external_power();
                        latest_charge_state = Some(charge_state);
                        last_charge_report = Some(Instant::now());
                        if charging_transition {
                            idle_activity.reset(started.elapsed());
                            eprintln!(
                                "level=info event=idle_shutdown_timer_reset reason=charging_transition"
                            );
                        }
                        let dock_event = self.automatic_shutdown.observe_charge_state(
                            worker.device_info(),
                            charge_state,
                            self.config.puck_dock_action,
                        );
                        self.update_status(|status| {
                            status.battery_percent = percent;
                            status.battery_charge_state = Some(charge_state);
                        });
                        if dock_event && self.automatic_shutdown.retry_due(Instant::now()) {
                            eprintln!(
                                "level=info event=puck_dock_detected charge_state={charge_state:?}"
                            );
                            pending_automatic_shutdown = Some(ShutdownTrigger::PuckDock);
                        }
                    }
                    Ok(ReportEffect::Disconnected) => {
                        let _ = engine.disconnected(&mut *output.output);
                        break 'active ActiveExit::SourceLost;
                    }
                    Ok(ReportEffect::None) => {}
                    Err(error) if is_output_error(&error) => {
                        break 'active ActiveExit::OutputLost(format!(
                            "XIAO output failed; waiting for reconnect: {error}"
                        ));
                    }
                    Err(error) => {
                        eprintln!("level=warn event=report_processing_failed error={error:?}");
                    }
                }
            }
            iteration_timer.enter("desktop_worker_output");
            let desktop_output = self.desktop_bindings.take_output();
            if desktop_output.discard_pending_feedback {
                worker.clear_pad_feedback();
            }
            worker.request_pad_feedback(desktop_output.feedback);
            let lost = dropped.swap(0, Ordering::AcqRel);
            if lost > 0 {
                engine.note_dropped_reports(lost);
            }
            iteration_timer.enter("output_service");
            let service_result = if should_tick_input_timeout(lost, worker.has_pending_report()) {
                engine
                    .tick(started.elapsed(), &mut *output.output)
                    .map(|_| ())
            } else {
                output
                    .output
                    .service()
                    .map_err(bridge_core::BridgeError::Output)
            };
            if let Err(error) = service_result {
                break ActiveExit::OutputLost(format!(
                    "XIAO service failed; waiting for reconnect: {error}"
                ));
            }
            iteration_timer.enter("output_feedback");
            while let Some(feedback) = output.output.take_feedback() {
                match feedback {
                    OutputFeedback::Rumble {
                        low_frequency,
                        high_frequency,
                    } => worker.set_rumble(low_frequency, high_frequency),
                }
            }
            iteration_timer.enter("automatic_shutdown");
            let now = Instant::now();
            let dock_retry_due = self.automatic_shutdown.trigger == Some(ShutdownTrigger::PuckDock)
                && self.automatic_shutdown.phase == AutomaticShutdownPhase::Degraded
                && !self.automatic_shutdown.dock_episode_handled
                && self.automatic_shutdown.retry_due(now)
                && worker.device_info().controller_transport() == Some(ControllerTransport::Puck)
                && latest_charge_state.is_some_and(ControllerChargeState::is_external_power)
                && last_charge_report.is_some_and(|reported| {
                    now.saturating_duration_since(reported) <= BATTERY_STATUS_FRESHNESS
                });
            let idle_shutdown_due = idle_activity.deadline_reached(started.elapsed())
                && idle_activity.is_neutral()
                && self.automatic_shutdown.retry_due(now);
            let automatic_trigger = pending_automatic_shutdown
                .take()
                .or_else(|| dock_retry_due.then_some(ShutdownTrigger::PuckDock))
                .or_else(|| idle_shutdown_due.then_some(ShutdownTrigger::IdleTimeout));
            if let Some(trigger) = automatic_trigger {
                if output.xiao.is_none() {
                    eprintln!(
                        "level=warn event=automatic_shutdown_skipped trigger={trigger:?} reason=xiao_not_ready"
                    );
                } else if trigger != ShutdownTrigger::IdleTimeout || idle_activity.is_neutral() {
                    self.automatic_shutdown.begin(trigger);
                    let powering_off = self.automatic_shutdown.status(
                        &self.config,
                        idle_activity.idle_age(started.elapsed()),
                        now,
                    );
                    self.update_status(|status| status.automatic_shutdown = powering_off);
                    match engine.shutdown(&mut *output.output) {
                        Ok(_) => match worker.power_off() {
                            Ok(()) => {
                                self.automatic_shutdown.succeeded(Instant::now(), trigger);
                                let automatic = self.automatic_shutdown.status(
                                    &self.config,
                                    idle_activity.idle_age(started.elapsed()),
                                    Instant::now(),
                                );
                                self.update_status(|status| {
                                    status.automatic_shutdown = automatic;
                                    status.last_error = None;
                                });
                                break ActiveExit::AutomaticShutdown {
                                    info: worker.device_info().clone(),
                                    trigger,
                                };
                            }
                            Err(error) => {
                                self.automatic_shutdown
                                    .failed(Instant::now(), trigger, &error);
                                let automatic = self.automatic_shutdown.status(
                                    &self.config,
                                    idle_activity.idle_age(started.elapsed()),
                                    Instant::now(),
                                );
                                self.update_status(|status| {
                                    status.automatic_shutdown = automatic;
                                    status.last_error = Some(format!(
                                        "automatic controller shutdown failed; gameplay continues: {error}"
                                    ));
                                });
                            }
                        },
                        Err(error) => {
                            break ActiveExit::OutputLost(format!(
                                "cannot neutralize XIAO before automatic controller shutdown: {error}"
                            ));
                        }
                    }
                }
            }
            if last_controller_state.elapsed() >= ACTIVE_SLOT_TIMEOUT {
                let _ = engine.disconnected(&mut *output.output);
                break ActiveExit::SourceLost;
            }
            iteration_timer.enter("status_update");
            if last_status.elapsed() >= STATUS_INTERVAL {
                let controller_age = last_controller_state.elapsed();
                let automatic = self.automatic_shutdown.status(
                    &self.config,
                    idle_activity.idle_age(started.elapsed()),
                    Instant::now(),
                );
                let binding_status = self.desktop_bindings.status();
                self.update_status(|status| {
                    status.bridge_metrics = engine.metrics();
                    status.output_diagnostics = output.output.diagnostics();
                    status.lizard = worker.lizard_diagnostics();
                    status.haptics = worker.haptics_diagnostics();
                    status.bindings = binding_status;
                    status.profile_picker = picker_status(&self.config, picker.is_open());
                    status.source.active =
                        controller_state_seen && controller_age < ACTIVE_SLOT_TIMEOUT;
                    status.controller.connected =
                        controller_connected && controller_age < ACTIVE_SLOT_TIMEOUT;
                    status.controller.last_state_age =
                        controller_connected.then_some(controller_age);
                    status.automatic_shutdown = automatic;
                });
                last_status = Instant::now();
            }
        };

        self.transition(RuntimeState::Stopping, "Neutralizing output", None);
        self.dismiss_picker(&mut picker);
        let neutral_result = engine.shutdown(&mut *output.output);
        let worker_result = worker.shutdown();
        let desktop_result = self.desktop_bindings.disconnect();
        idle_activity.pause();
        let automatic = self
            .automatic_shutdown
            .status(&self.config, None, Instant::now());
        let binding_status = self.desktop_bindings.status();
        self.update_status(|status| {
            status.bridge_metrics = engine.metrics();
            status.output_diagnostics = output.output.diagnostics();
            status.lizard = worker.lizard_diagnostics();
            status.haptics = worker.haptics_diagnostics();
            status.bindings = binding_status;
            status.automatic_shutdown = automatic;
        });
        self.clear_controller_status();
        let recording_result =
            record_device_event(&mut recording, started, KIND_DEVICE_DISCONNECTED, None);
        let neutral_result = neutral_result
            .map(|_| ())
            .map_err(|error| format!("cannot neutralize XIAO before HID release: {error}"));
        let required_cleanup =
            worker_result
                .and(recording_result)
                .and(if matches!(exit, ActiveExit::OutputLost(_)) {
                    Ok(())
                } else {
                    neutral_result
                });
        if exit.has_acknowledgement() {
            return Ok((exit, output, required_cleanup.and(desktop_result)));
        }
        required_cleanup?;
        if let Err(error) = desktop_result {
            eprintln!("level=warn event=desktop_input_worker_disconnect_failed error={error:?}");
        }
        Ok((exit, output, Ok(())))
    }

    pub(super) fn open_recording(&self) -> Result<Option<RecordingWriter<File>>, String> {
        self.config
            .recording_path
            .as_ref()
            .map(|path| {
                File::create(path)
                    .map(RecordingWriter::new)
                    .map_err(|error| {
                        format!("cannot create recording '{}': {error}", path.display())
                    })
            })
            .transpose()
    }
}
