#[allow(
    clippy::wildcard_imports,
    reason = "the worker coordinates its parent module's private mailbox and sink types"
)]
use super::*;

// Owns BindingEngine and the non-Send desktop sink on one dedicated thread.
// Supervisor-facing snapshot publication never waits for desktop injection;
// disconnect and shutdown are acknowledged only after held outputs are released.
pub(crate) struct DesktopBindingsWorker {
    pub(crate) mailbox: Arc<DesktopWorkerMailbox>,
    pub(crate) outputs: Arc<DesktopWorkerOutputs>,
    pub(crate) status: Arc<Mutex<BridgeStatus>>,
    pub(crate) alive: Arc<AtomicBool>,
    pub(crate) started: Instant,
    pub(crate) handle: Option<JoinHandle<()>>,
}

impl DesktopBindingsWorker {
    pub(crate) fn spawn(profile: Option<BindingProfile>, status: Arc<Mutex<BridgeStatus>>) -> Self {
        Self::spawn_with_runtime(status, move || DesktopBindingsRuntime::new(profile))
    }

    pub(crate) fn spawn_with_runtime(
        status: Arc<Mutex<BridgeStatus>>,
        make_runtime: impl FnOnce() -> DesktopBindingsRuntime + Send + 'static,
    ) -> Self {
        let mailbox = Arc::new(DesktopWorkerMailbox::default());
        let worker_mailbox = Arc::clone(&mailbox);
        let outputs = Arc::new(DesktopWorkerOutputs::default());
        let worker_outputs = Arc::clone(&outputs);
        let worker_status = Arc::clone(&status);
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = Arc::clone(&alive);
        let started = Instant::now();
        let handle = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let runtime = make_runtime();
                run_desktop_bindings_worker(
                    runtime,
                    &worker_mailbox,
                    &worker_outputs,
                    &worker_status,
                    started,
                );
            }));
            if result.is_err() {
                publish_desktop_worker_failure(&worker_status, "desktop-input worker panicked");
                eprintln!("level=error event=desktop_input_worker_panicked");
            }
            worker_alive.store(false, Ordering::Release);
            worker_mailbox.close();
        });
        Self {
            mailbox,
            outputs,
            status,
            alive,
            started,
            handle: Some(handle),
        }
    }

    pub(crate) fn observe(&self, snapshot: DesktopInputSnapshot) {
        match self
            .mailbox
            .publish_snapshot(&self.outputs, snapshot, self.started.elapsed())
        {
            DesktopSnapshotPublish::Published | DesktopSnapshotPublish::Overflowed => {}
            DesktopSnapshotPublish::Closed => {
                self.outputs.discard_feedback();
                publish_desktop_worker_failure(&self.status, "desktop-input worker is unavailable");
            }
        }
    }

    pub(crate) fn overflow(&self) {
        if !self.mailbox.publish_overflow(&self.outputs) {
            self.outputs.discard_feedback();
            publish_desktop_worker_failure(&self.status, "desktop-input worker is unavailable");
        }
    }

    pub(crate) fn replace_profile(&self, profile: Option<BindingProfile>, ack: CommandAck) {
        self.enqueue_control(
            DesktopWorkerMessage::ReplaceProfile {
                profile: profile.map(Box::new),
                ack: Some(ack),
            },
            true,
        );
    }

    pub(crate) fn enable(&self, ack: CommandAck) {
        self.enqueue_control(DesktopWorkerMessage::Enable { ack: Some(ack) }, false);
    }

    pub(crate) fn enable_async(&self) {
        self.enqueue_control(DesktopWorkerMessage::Enable { ack: None }, false);
    }

    pub(crate) fn disconnect(&self) -> Result<(), String> {
        let (ack, receiver) = mpsc::channel();
        self.enqueue_control(DesktopWorkerMessage::Disconnect(ack), true);
        receiver
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| "desktop-input worker disconnect timed out".to_owned())?
    }

    pub(crate) fn take_output(&self) -> DesktopWorkerOutput {
        self.outputs.take()
    }

    pub(crate) fn status(&self) -> DesktopBindingsStatus {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .bindings
            .clone()
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), String> {
        self.shutdown_with_timeout(COMMAND_TIMEOUT)
    }

    pub(crate) fn shutdown_with_timeout(&mut self, timeout: Duration) -> Result<(), String> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        let command_result = if self.alive.load(Ordering::Acquire) {
            let (ack, receiver) = mpsc::channel();
            if let Err(message) =
                self.mailbox
                    .push_control(&self.outputs, DesktopWorkerMessage::Shutdown(ack), true)
            {
                // A full mailbox means the worker is not draining. With no
                // Shutdown queued, a join could never be satisfied - detach,
                // exactly as the timeout path below does.
                self.outputs.discard_feedback();
                publish_desktop_worker_failure(&self.status, "desktop-input worker is unavailable");
                (*message).reject("desktop-input worker is unavailable");
                drop(handle);
                return Err("desktop-input worker control queue is full at shutdown".to_owned());
            }
            if let Ok(result) = receiver.recv_timeout(timeout) {
                result
            } else {
                // Rust cannot cancel a thread that is inside a third-party
                // platform call. Detach this final-shutdown worker rather
                // than defeating the timeout with an unconditional join;
                // the queued Shutdown still makes it exit if the call ever
                // returns, and no further work can reach it through `self`.
                drop(handle);
                return Err("desktop-input worker shutdown timed out".to_owned());
            }
        } else {
            Err("desktop-input worker stopped unexpectedly".to_owned())
        };
        let join_result = handle
            .join()
            .map_err(|_| "desktop-input worker panicked".to_owned());
        command_result.and(join_result)
    }

    pub(crate) fn enqueue_control(&self, message: DesktopWorkerMessage, feedback_barrier: bool) {
        if let Err(message) = self
            .mailbox
            .push_control(&self.outputs, message, feedback_barrier)
        {
            self.outputs.discard_feedback();
            publish_desktop_worker_failure(&self.status, "desktop-input worker is unavailable");
            (*message).reject("desktop-input worker is unavailable");
        }
    }
}

impl Drop for DesktopBindingsWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub(crate) fn run_desktop_bindings_worker(
    mut runtime: DesktopBindingsRuntime,
    mailbox: &DesktopWorkerMailbox,
    outputs: &DesktopWorkerOutputs,
    status: &Arc<Mutex<BridgeStatus>>,
    started: Instant,
) {
    if let Some(update) = runtime.take_status_update() {
        publish_desktop_binding_status(status, update);
    }
    let mut shutdown = false;
    let mut applied_generation = mailbox.generation();
    while !shutdown {
        let timeout = runtime.needs_tick().then_some(RUNTIME_POLL_INTERVAL);
        let messages = mailbox.take_batch(timeout);
        for message in messages {
            match message {
                DesktopWorkerMessage::Snapshot(snapshot) => {
                    let current_generation = mailbox.generation();
                    if snapshot.generation == current_generation {
                        let feedback = runtime.observe(snapshot.snapshot, snapshot.now);
                        outputs.publish_feedback(snapshot.feedback_epoch, feedback);
                    }
                    let current_generation = mailbox.generation();
                    apply_desktop_mailbox_overflow(
                        &mut runtime,
                        outputs,
                        &mut applied_generation,
                        current_generation,
                    );
                }
                DesktopWorkerMessage::Overflow => {
                    apply_desktop_mailbox_overflow(
                        &mut runtime,
                        outputs,
                        &mut applied_generation,
                        mailbox.generation(),
                    );
                }
                // Control acknowledgements are the caller's licence to read
                // the shared status, so each arm publishes its status effects
                // before sending the ack. Acking first let a caller - the
                // supervisor's post-disconnect read, or a test using an ack as
                // a barrier - observe the pre-command status and even write it
                // back over the newer one.
                DesktopWorkerMessage::ReplaceProfile { profile, ack } => {
                    let result = runtime.replace_profile(profile.map(|profile| *profile));
                    if let Some(update) = runtime.take_status_update() {
                        publish_desktop_binding_status(status, update);
                    }
                    if let Some(ack) = ack {
                        let _ = ack.send(result);
                    }
                }
                DesktopWorkerMessage::Enable { ack } => {
                    let result = runtime.enable();
                    if let Some(update) = runtime.take_status_update() {
                        publish_desktop_binding_status(status, update);
                    }
                    if let Some(ack) = ack {
                        let _ = ack.send(result);
                    }
                }
                DesktopWorkerMessage::Disconnect(ack) => {
                    let result = runtime.disconnect();
                    if let Some(update) = runtime.take_status_update() {
                        publish_desktop_binding_status(status, update);
                    }
                    let _ = ack.send(result);
                }
                DesktopWorkerMessage::Shutdown(ack) => {
                    let result = runtime.shutdown();
                    if let Some(update) = runtime.take_status_update() {
                        publish_desktop_binding_status(status, update);
                    }
                    let _ = ack.send(result);
                    shutdown = true;
                }
            }
            if runtime.take_discard_pending_feedback() {
                outputs.discard_feedback();
            }
            if let Some(update) = runtime.take_status_update() {
                publish_desktop_binding_status(status, update);
            }
            if shutdown {
                break;
            }
        }
        if !shutdown && runtime.needs_tick() {
            runtime.tick(started.elapsed());
            if runtime.take_discard_pending_feedback() {
                outputs.discard_feedback();
            }
            if let Some(update) = runtime.take_status_update() {
                publish_desktop_binding_status(status, update);
            }
        }
    }
}

pub(crate) fn apply_desktop_mailbox_overflow(
    runtime: &mut DesktopBindingsRuntime,
    outputs: &DesktopWorkerOutputs,
    applied_generation: &mut u64,
    current_generation: u64,
) {
    if *applied_generation == current_generation {
        return;
    }
    runtime.overflow();
    outputs.discard_feedback();
    *applied_generation = current_generation;
    eprintln!(
        "level=warn event=desktop_input_worker_mailbox_overflow action=release_and_rebaseline"
    );
}

pub(crate) fn publish_desktop_binding_status(
    shared: &Arc<Mutex<BridgeStatus>>,
    bindings: DesktopBindingsStatus,
) {
    let mut status = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if status.bindings != bindings {
        status.bindings = bindings;
        status.revision = status.revision.wrapping_add(1);
    }
}

pub(crate) fn publish_desktop_worker_failure(shared: &Arc<Mutex<BridgeStatus>>, error: &str) {
    let mut bindings = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .bindings
        .clone();
    if bindings.state == DesktopBindingsState::Degraded
        && bindings.last_error.as_deref() == Some(error)
    {
        return;
    }
    bindings.state = DesktopBindingsState::Degraded;
    bindings.failures = bindings.failures.saturating_add(1);
    bindings.held_output_count = 0;
    bindings.last_error = Some(bounded_error(error));
    publish_desktop_binding_status(shared, bindings);
}

pub(crate) fn create_desktop_sink() -> Result<Box<dyn DesktopInputSink>, String> {
    let mut factory = desktop_input::current_factory()?;
    let session = factory.detect_session()?;
    factory.create(&session)
}

pub(crate) fn bounded_error(error: &str) -> String {
    const MAX_ERROR_CHARS: usize = 512;
    error.chars().take(MAX_ERROR_CHARS).collect()
}
