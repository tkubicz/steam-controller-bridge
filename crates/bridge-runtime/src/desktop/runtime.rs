#[allow(
    clippy::wildcard_imports,
    reason = "desktop sink lifecycle uses its parent worker's private dependencies"
)]
use super::*;

pub(crate) struct DesktopBindingsRuntime {
    pub(crate) engine: Option<BindingEngine>,
    // Once authorized, the sink belongs to the active runtime session. Profile
    // changes may release outputs or remove the engine, but must not destroy
    // the sink: Enigo's macOS `Drop` can sleep for seconds after pad traffic.
    pub(crate) sink: Option<Box<dyn DesktopInputSink>>,
    // Status-only profile changes are allowed before the frontend has completed
    // the ordered macOS permission flow. Only Enable latches runtime activation.
    pub(crate) activation_requested: bool,
    pub(crate) last_snapshot: Option<DesktopInputSnapshot>,
    pub(crate) discard_pending_feedback: bool,
    pub(crate) status: DesktopBindingsStatus,
    pub(crate) status_dirty: bool,
}

impl DesktopBindingsRuntime {
    pub(crate) fn new(profile: Option<BindingProfile>) -> Self {
        let status = binding_status_for_profile(profile.as_ref());
        Self {
            engine: profile.map(BindingEngine::new),
            sink: None,
            activation_requested: false,
            last_snapshot: None,
            discard_pending_feedback: false,
            status,
            status_dirty: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_sink(profile: BindingProfile, sink: Box<dyn DesktopInputSink>) -> Self {
        let mut status = binding_status_for_profile(Some(&profile));
        status.state = DesktopBindingsState::Ready;
        Self {
            engine: Some(BindingEngine::new(profile)),
            sink: Some(sink),
            activation_requested: true,
            last_snapshot: None,
            discard_pending_feedback: false,
            status,
            status_dirty: true,
        }
    }

    pub(crate) fn status(&self) -> DesktopBindingsStatus {
        let mut status = self.status.clone();
        status.held_output_count = self
            .engine
            .as_ref()
            .map_or(0, BindingEngine::held_output_count);
        status
    }

    pub(crate) fn take_status_update(&mut self) -> Option<DesktopBindingsStatus> {
        if !std::mem::take(&mut self.status_dirty) {
            return None;
        }
        Some(self.status())
    }

    pub(crate) fn held_output_count(&self) -> usize {
        self.engine
            .as_ref()
            .map_or(0, BindingEngine::held_output_count)
    }

    pub(crate) fn observe(
        &mut self,
        snapshot: DesktopInputSnapshot,
        now: Duration,
    ) -> PadFeedbackRequest {
        self.last_snapshot = Some(snapshot);
        let held_before = self.held_output_count();
        let (Some(engine), Some(sink)) = (self.engine.as_mut(), self.sink.as_mut()) else {
            return PadFeedbackRequest::NONE;
        };
        let feedback = match engine.observe_snapshot(snapshot, now, sink.as_mut()) {
            Ok(feedback) => {
                if self.status.state == DesktopBindingsState::Degraded {
                    self.status.state = DesktopBindingsState::Ready;
                    self.status.last_error = None;
                    self.status_dirty = true;
                }
                feedback
            }
            Err(error) => {
                self.fail(&error);
                PadFeedbackRequest::NONE
            }
        };
        if self.held_output_count() != held_before {
            self.status_dirty = true;
        }
        feedback
    }

    pub(crate) fn tick(&mut self, now: Duration) {
        let (Some(engine), Some(sink)) = (self.engine.as_mut(), self.sink.as_mut()) else {
            return;
        };
        if let Err(error) = engine.tick(now, sink.as_mut()) {
            self.fail(&error);
        }
    }

    pub(crate) fn needs_tick(&self) -> bool {
        self.sink.is_some() && self.engine.as_ref().is_some_and(BindingEngine::needs_tick)
    }

    pub(crate) fn take_discard_pending_feedback(&mut self) -> bool {
        std::mem::take(&mut self.discard_pending_feedback)
    }

    pub(crate) fn drop_sink(&mut self, reason: &'static str) {
        let Some(sink) = self.sink.take() else {
            return;
        };
        let started = Instant::now();
        drop(sink);
        let elapsed = started.elapsed();
        if elapsed >= SUPERVISOR_STALL_THRESHOLD {
            eprintln!(
                "level=warn event=desktop_sink_drop_stall reason={reason} elapsed_ms={}",
                elapsed.as_millis()
            );
        }
    }

    pub(crate) fn replace_profile(
        &mut self,
        profile: Option<BindingProfile>,
    ) -> Result<(), String> {
        let status = binding_status_for_profile(profile.as_ref());
        let result = if let Some(profile) = profile {
            if let (Some(engine), Some(sink)) = (self.engine.as_mut(), self.sink.as_mut()) {
                engine.replace_profile(profile, sink.as_mut())
            } else {
                self.engine = Some(BindingEngine::new(profile));
                Ok(())
            }
        } else {
            let result =
                if let (Some(engine), Some(sink)) = (self.engine.as_mut(), self.sink.as_mut()) {
                    engine.disconnect(sink.as_mut())
                } else {
                    Ok(())
                };
            self.engine = None;
            result
        };
        if self.status != status {
            self.status = status;
            self.status_dirty = true;
        }
        if let Err(error) = result {
            self.fail(&error);
            return Err(error);
        }
        if self.status.configured_binding_count == 0 {
            return Ok(());
        }
        if self.sink.is_none() && self.activation_requested {
            self.initialize_sink();
        } else if self.sink.is_some() {
            self.status.state = DesktopBindingsState::Ready;
            self.status.last_error = None;
        }
        if let (Some(snapshot), Some(engine), Some(sink)) =
            (self.last_snapshot, self.engine.as_mut(), self.sink.as_mut())
        {
            if let Err(error) = engine.observe_snapshot(snapshot, Duration::ZERO, sink.as_mut()) {
                self.fail(&error);
                return Err(error);
            }
        }
        Ok(())
    }

    pub(crate) fn enable(&mut self) -> Result<(), String> {
        self.activation_requested = true;
        if self.status.configured_binding_count == 0 {
            return Ok(());
        }
        if self.sink.is_none() {
            self.initialize_sink();
        }
        if self.sink.is_none() {
            return Err(self
                .status
                .last_error
                .clone()
                .unwrap_or_else(|| "desktop bindings are unavailable".to_owned()));
        }
        if let (Some(snapshot), Some(engine), Some(sink)) =
            (self.last_snapshot, self.engine.as_mut(), self.sink.as_mut())
        {
            if let Err(error) = engine.observe_snapshot(snapshot, Duration::ZERO, sink.as_mut()) {
                self.fail(&error);
                return Err(error);
            }
        }
        Ok(())
    }

    pub(crate) fn disconnect(&mut self) -> Result<(), String> {
        let held_before = self.held_output_count();
        let result = if let (Some(engine), Some(sink)) = (self.engine.as_mut(), self.sink.as_mut())
        {
            engine.disconnect(sink.as_mut())
        } else {
            Ok(())
        };
        if let Err(error) = &result {
            self.fail(error);
        }
        self.last_snapshot = None;
        self.discard_pending_feedback = true;
        if self.held_output_count() != held_before {
            self.status_dirty = true;
        }
        result
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), String> {
        let result = self.disconnect();
        // Drop can be slow on macOS, so do it on this worker before the
        // shutdown acknowledgement. The owner can then bound its wait without
        // ever blocking the supervisor or joining a permanently wedged drop.
        self.drop_sink("desktop_worker_shutdown");
        result
    }

    pub(crate) fn overflow(&mut self) {
        let _ = self.disconnect();
        self.status.state = DesktopBindingsState::Degraded;
        self.status.failures = self.status.failures.saturating_add(1);
        self.status.last_error = Some(
            "input transition mailbox overflowed; held inputs released and state rebaselined"
                .to_owned(),
        );
        self.status_dirty = true;
    }

    pub(crate) fn initialize_sink(&mut self) {
        match create_desktop_sink() {
            Ok(sink) => {
                self.sink = Some(sink);
                self.status.state = DesktopBindingsState::Ready;
                self.status.last_error = None;
                self.status_dirty = true;
            }
            Err(error) => {
                self.sink = None;
                self.status.state =
                    if error.contains("permission") || error.contains("Accessibility") {
                        DesktopBindingsState::PermissionRequired
                    } else {
                        DesktopBindingsState::Degraded
                    };
                self.status.last_error = Some(bounded_error(&error));
                self.status_dirty = true;
            }
        }
    }

    pub(crate) fn fail(&mut self, error: &str) {
        self.status.state = DesktopBindingsState::Degraded;
        self.status.failures = self.status.failures.saturating_add(1);
        self.status.last_error = Some(bounded_error(error));
        self.status_dirty = true;
        self.drop_sink("backend_failure");
        self.discard_pending_feedback = true;
    }
}

impl Drop for DesktopBindingsRuntime {
    fn drop(&mut self) {
        self.drop_sink("desktop_worker_exit");
    }
}
