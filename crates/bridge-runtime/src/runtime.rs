use super::{
    automatic_shutdown_phase, binding_status_for_profile, bounded_error, mpsc, thread, Arc,
    AutomaticShutdownStatus, BindingProfile, BridgeStatus, Duration, JoinHandle, Mutex,
    OutputSelection, OutputStatus, PickerConfig, PickerEvent, PickerRoster, PowerEvent,
    PowerMonitor, ProfilePickerStatus, PuckDockAction, RuntimeConfig, RuntimeError, RuntimeState,
    Supervisor, COMMAND_TIMEOUT,
};
use std::time::Instant;

const POWER_CALLBACK_COMPLETION_RESERVE: Duration = Duration::from_millis(100);

pub(crate) type CommandAck = mpsc::Sender<Result<(), String>>;

pub(crate) enum RuntimeCommand {
    Start(CommandAck),
    Stop(CommandAck),
    Shutdown(CommandAck),
    SetIdleShutdown(Option<Duration>, CommandAck),
    SetPuckDockAction(PuckDockAction, CommandAck),
    SetOutput(OutputSelection, CommandAck),
    SetBindingProfile(Box<Option<BindingProfile>>, CommandAck),
    EnableDesktopBindings(CommandAck),
    SetPickerConfig(Option<PickerConfig>, CommandAck),
    SetPickerRoster(PickerRoster, CommandAck),
    /// Park the device and close every hardware handle ahead of system sleep.
    SuspendForSleep(CommandAck),
    /// Let discovery run again after a system wake.
    ResumeFromWake(CommandAck),
    /// Park the device for an updater operation without changing user intent.
    SuspendForUpdate(CommandAck),
    /// Release only the updater suspension; system sleep still wins.
    ResumeFromUpdate(CommandAck),
}

/// Where picker events go. Called on the runtime thread, so it must not block.
pub type PickerEventSink = Box<dyn Fn(PickerEvent) + Send>;

pub struct BridgeRuntime;

impl BridgeRuntime {
    #[must_use]
    pub fn spawn(config: RuntimeConfig) -> BridgeHandle {
        Self::spawn_with_picker(config, Box::new(|_| {}))
    }

    /// Spawns the runtime and streams profile-wheel events to `picker_events`.
    ///
    /// The sink runs on the runtime thread between controller reports, so it
    /// must return promptly; hand the event to a channel and wake the UI rather
    /// than doing work inside it. The sink is separate from [`RuntimeConfig`]
    /// because that stays plain, cloneable data.
    #[must_use]
    pub fn spawn_with_picker(
        config: RuntimeConfig,
        picker_events: PickerEventSink,
    ) -> BridgeHandle {
        let status = Arc::new(Mutex::new(BridgeStatus {
            state: RuntimeState::Discovering,
            detail: "Starting bridge runtime".to_owned(),
            automatic_shutdown: AutomaticShutdownStatus {
                configured_timeout: config.idle_shutdown_timeout,
                puck_dock_action: config.puck_dock_action,
                phase: automatic_shutdown_phase(&config),
                ..AutomaticShutdownStatus::default()
            },
            bindings: binding_status_for_profile(config.binding_profile.as_ref()),
            profile_picker: picker_status(&config, false),
            output: OutputStatus::configured(&config.output),
            ..BridgeStatus::default()
        }));
        let worker_status = Arc::clone(&status);
        let (command_sender, command_receiver) = mpsc::channel();
        let (power_error_sender, power_errors) = mpsc::channel();
        let (power_monitor, startup_blocker) =
            match power_monitor(command_sender.clone(), power_error_sender) {
                Ok(monitor) => {
                    if !monitor.is_live() {
                        eprintln!("level=warn event=system_power_monitor_unavailable");
                    }
                    (Some(monitor), None)
                }
                Err(error) => {
                    eprintln!("level=error event=system_power_monitor_failed error={error:?}");
                    (None, Some(error))
                }
            };
        let join = thread::spawn(move || {
            let mut supervisor = Supervisor::new(
                config,
                worker_status,
                command_receiver,
                picker_events,
                startup_blocker,
            );
            supervisor.run();
        });
        BridgeHandle {
            command_sender,
            status,
            join: Mutex::new(Some(join)),
            power_monitor: Mutex::new(power_monitor),
            power_errors: Mutex::new(power_errors),
        }
    }
}

fn power_monitor(
    commands: mpsc::Sender<RuntimeCommand>,
    power_errors: mpsc::Sender<String>,
) -> Result<PowerMonitor, String> {
    PowerMonitor::new(move |event| {
        handle_power_event(&commands, &power_errors, event);
    })
    .map_err(|error| format!("cannot monitor system sleep safely: {error}"))
}

fn handle_power_event(
    commands: &mpsc::Sender<RuntimeCommand>,
    power_errors: &mpsc::Sender<String>,
    event: PowerEvent,
) {
    match event {
        PowerEvent::WillSleep { deadline } => {
            let (ack, receiver) = mpsc::channel();
            let result = commands
                .send(RuntimeCommand::SuspendForSleep(ack))
                .map_err(|_| "bridge runtime stopped before system sleep".to_owned())
                .and_then(|()| {
                    receiver
                        .recv_timeout(power_teardown_timeout(Instant::now(), deadline))
                        .map_err(|_| {
                            "system-sleep hardware teardown did not acknowledge in time".to_owned()
                        })?
                });
            match result {
                Ok(()) => eprintln!("level=info event=system_sleep_hardware_released"),
                Err(error) => {
                    eprintln!(
                        "level=error event=system_sleep_hardware_release_failed error={error:?}"
                    );
                    let _ = power_errors.send(error);
                }
            }
        }
        PowerEvent::DidWake => {
            let (ack, _receiver) = mpsc::channel();
            if commands.send(RuntimeCommand::ResumeFromWake(ack)).is_err() {
                eprintln!("level=warn event=system_wake_runtime_unavailable");
            }
        }
    }
}

fn power_teardown_timeout(now: Instant, deadline: Instant) -> Duration {
    deadline
        .saturating_duration_since(now)
        .saturating_sub(POWER_CALLBACK_COMPLETION_RESERVE)
}

pub(crate) fn picker_status(config: &RuntimeConfig, open: bool) -> ProfilePickerStatus {
    ProfilePickerStatus {
        enabled: config.profile_picker.is_some(),
        open,
        roster_len: config.picker_roster.len,
    }
}

pub struct BridgeHandle {
    command_sender: mpsc::Sender<RuntimeCommand>,
    status: Arc<Mutex<BridgeStatus>>,
    join: Mutex<Option<JoinHandle<()>>>,
    power_monitor: Mutex<Option<PowerMonitor>>,
    power_errors: Mutex<mpsc::Receiver<String>>,
}

/// The result of polling any non-blocking runtime command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandPoll {
    Pending,
    /// The deadline elapsed, but the original command remains queued and may
    /// still acknowledge later. Reported at most once per request.
    TimedOut,
    Complete(Result<(), RuntimeError>),
}

/// Kept as names the frontends already use; both polls answer the same
/// question, so they are one type rather than three copies of one enum.
pub type UpdateResumePoll = CommandPoll;
/// The result of polling a non-blocking output-backend change.
pub type OutputChangePoll = CommandPoll;

/// A queued updater-resume request whose acknowledgement can be polled by a UI.
pub struct PendingUpdateResume {
    command: PendingCommand,
}

/// A safety-ordered output change whose acknowledgement can be polled by a UI.
pub struct PendingOutputChange {
    command: PendingCommand,
}

impl PendingOutputChange {
    fn new(receiver: mpsc::Receiver<Result<(), String>>) -> Self {
        Self {
            command: PendingCommand::new(receiver),
        }
    }

    /// Checks for completion without blocking the caller.
    #[must_use]
    pub fn poll(&mut self) -> OutputChangePoll {
        self.command.poll("output change")
    }
}

impl PendingUpdateResume {
    fn new(receiver: mpsc::Receiver<Result<(), String>>) -> Self {
        Self {
            command: PendingCommand::new(receiver),
        }
    }

    /// Checks for completion without blocking the caller.
    #[must_use]
    pub fn poll(&mut self) -> UpdateResumePoll {
        self.command.poll("recovery")
    }
}

struct PendingCommand {
    receiver: mpsc::Receiver<Result<(), String>>,
    deadline: std::time::Instant,
    timeout_reported: bool,
    completion: Option<Result<(), RuntimeError>>,
}

impl PendingCommand {
    fn new(receiver: mpsc::Receiver<Result<(), String>>) -> Self {
        Self {
            receiver,
            deadline: std::time::Instant::now() + COMMAND_TIMEOUT,
            timeout_reported: false,
            completion: None,
        }
    }

    fn poll(&mut self, operation: &str) -> CommandPoll {
        if let Some(result) = &self.completion {
            return CommandPoll::Complete(result.clone());
        }
        let result = match self.receiver.try_recv() {
            Ok(result) => CommandPoll::Complete(result.map_err(RuntimeError)),
            Err(mpsc::TryRecvError::Empty) if std::time::Instant::now() < self.deadline => {
                CommandPoll::Pending
            }
            Err(mpsc::TryRecvError::Empty) if !self.timeout_reported => {
                self.timeout_reported = true;
                CommandPoll::TimedOut
            }
            Err(mpsc::TryRecvError::Empty) => CommandPoll::Pending,
            Err(mpsc::TryRecvError::Disconnected) => CommandPoll::Complete(Err(RuntimeError(
                format!("bridge runtime stopped before acknowledging {operation}"),
            ))),
        };
        if let CommandPoll::Complete(completion) = &result {
            self.completion = Some(completion.clone());
        }
        result
    }
}

impl BridgeHandle {
    /// Reports whether the runtime worker has terminated or has already been joined.
    ///
    /// Frontends use this to distinguish a recoverable command timeout from a
    /// dead runtime that can no longer own hardware or acknowledge cleanup.
    #[must_use]
    pub fn is_terminated(&self) -> bool {
        self.join
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
    }

    /// Queues an idempotent start without blocking the caller.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_start(&self) -> Result<(), RuntimeError> {
        self.request(RuntimeCommand::Start)
    }

    /// Queues a safety-ordered stop without blocking the caller.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_stop(&self) -> Result<(), RuntimeError> {
        self.request(RuntimeCommand::Stop)
    }

    /// Queues an idle-shutdown timeout update without blocking the caller.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_set_idle_shutdown_timeout(
        &self,
        timeout: Option<Duration>,
    ) -> Result<(), RuntimeError> {
        self.request(|ack| RuntimeCommand::SetIdleShutdown(timeout, ack))
    }

    /// Queues an immediate Puck-dock action update without blocking the caller.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_set_puck_dock_action(&self, action: PuckDockAction) -> Result<(), RuntimeError> {
        self.request(|ack| RuntimeCommand::SetPuckDockAction(action, ack))
    }

    /// Begins a neutralize-release-create output transition without blocking the caller.
    ///
    /// # Errors
    /// Returns an error if the runtime has already stopped.
    pub fn begin_set_output(
        &self,
        selection: OutputSelection,
    ) -> Result<PendingOutputChange, RuntimeError> {
        self.begin_command(|ack| RuntimeCommand::SetOutput(selection, ack))
            .map(PendingOutputChange::new)
    }

    /// Queues a binding-profile switch without restarting HID or bridge output.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_set_binding_profile(
        &self,
        profile: Option<BindingProfile>,
    ) -> Result<(), RuntimeError> {
        self.request(|ack| RuntimeCommand::SetBindingProfile(Box::new(profile), ack))
    }

    /// Explicitly asks macOS to enable desktop bindings, allowing a permission prompt.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_enable_desktop_bindings(&self) -> Result<(), RuntimeError> {
        self.request(RuntimeCommand::EnableDesktopBindings)
    }

    /// Enables, reconfigures, or disables the in-game profile wheel.
    ///
    /// `None` disables it and hands Quick Access back to the desktop bindings.
    /// An open wheel is closed, so the caller should hide the overlay too.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_set_picker_config(
        &self,
        config: Option<PickerConfig>,
    ) -> Result<(), RuntimeError> {
        self.request(|ack| RuntimeCommand::SetPickerConfig(config, ack))
    }

    /// Replaces the wheel roster and waits until the runtime has stopped using
    /// the previous index generation.
    ///
    /// # Errors
    /// Returns an error if the runtime stops or does not acknowledge in time.
    pub fn set_picker_roster(&self, roster: PickerRoster) -> Result<(), RuntimeError> {
        self.command(|ack| RuntimeCommand::SetPickerRoster(roster, ack))
    }

    /// Parks the controller at neutral, closes the bridge endpoint and HID
    /// handles, and returns only once that teardown has completed.
    ///
    /// For the frontend's system-sleep hook. The port must be **closed before
    /// the machine sleeps**: bridge I/O left in flight across a sleep/wake
    /// transition has panicked macOS's USB CDC driver while the bridge device
    /// re-enumerated. The bridge stays suspended - regardless of its
    /// start/stop setting - until [`BridgeHandle::request_resume_from_wake`].
    ///
    /// # Errors
    /// Returns an error if the runtime thread stops or the teardown fails.
    pub fn suspend_for_sleep(&self) -> Result<(), RuntimeError> {
        self.command(RuntimeCommand::SuspendForSleep)
    }

    /// Parks every device for an updater operation while preserving whether the
    /// user wanted the bridge running. The acknowledgement is sent only after
    /// neutralization and hardware release complete.
    ///
    /// # Errors
    /// Returns an error if the runtime thread stops or teardown fails.
    pub fn suspend_for_update(&self) -> Result<(), RuntimeError> {
        self.command(RuntimeCommand::SuspendForUpdate)
    }

    /// Releases an updater suspension. A concurrent system-sleep suspension
    /// remains active, and a bridge the user stopped while updating stays
    /// stopped.
    ///
    /// # Errors
    /// Returns an error if the runtime thread stops.
    pub fn resume_from_update(&self) -> Result<(), RuntimeError> {
        self.command(RuntimeCommand::ResumeFromUpdate)
    }

    /// Queues release of an updater suspension without blocking the caller.
    ///
    /// The returned request enforces the same acknowledgement deadline as the
    /// synchronous command API. [`UpdateResumePoll::TimedOut`] is a visible but
    /// non-terminal delay; frontends must retain the request until it returns
    /// [`UpdateResumePoll::Complete`].
    ///
    /// # Errors
    /// Returns an error if the runtime thread has already stopped.
    pub fn begin_resume_from_update(&self) -> Result<PendingUpdateResume, RuntimeError> {
        self.begin_command(RuntimeCommand::ResumeFromUpdate)
            .map(PendingUpdateResume::new)
    }

    /// Lets the bridge look for its hardware again after a system wake.
    ///
    /// Discovery waits `WAKE_SETTLE_DELAY` first, so the USB stack has
    /// time to finish re-enumerating the bridge device before anything reopens it. A
    /// bridge the user had stopped stays stopped.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn request_resume_from_wake(&self) -> Result<(), RuntimeError> {
        self.request(RuntimeCommand::ResumeFromWake)
    }

    /// Requests an idempotent runtime start and waits until the request is accepted.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn start(&self) -> Result<(), RuntimeError> {
        self.command(RuntimeCommand::Start)
    }

    /// Requests an idempotent stop. The acknowledgement is sent only after
    /// neutralization and HID release have completed.
    ///
    /// # Errors
    /// Returns an error if the runtime thread stops or cleanup fails.
    pub fn stop(&self) -> Result<(), RuntimeError> {
        self.command(RuntimeCommand::Stop)
    }

    /// Updates the idle-shutdown timeout without restarting HID or bridge output.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn set_idle_shutdown_timeout(&self, timeout: Option<Duration>) -> Result<(), RuntimeError> {
        self.command(|ack| RuntimeCommand::SetIdleShutdown(timeout, ack))
    }

    /// Updates the immediate Puck-dock action without restarting HID or bridge output.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn set_puck_dock_action(&self, action: PuckDockAction) -> Result<(), RuntimeError> {
        self.command(|ack| RuntimeCommand::SetPuckDockAction(action, ack))
    }

    /// Switches the active binding profile without restarting HID or bridge output.
    ///
    /// # Errors
    /// Returns an error when profile cleanup fails or the runtime has stopped.
    pub fn set_binding_profile(&self, profile: Option<BindingProfile>) -> Result<(), RuntimeError> {
        self.command(|ack| RuntimeCommand::SetBindingProfile(Box::new(profile), ack))
    }

    /// Explicitly retries desktop-input initialization and may show macOS's prompt.
    ///
    /// # Errors
    /// Returns an error when permission/backend initialization fails.
    pub fn enable_desktop_bindings(&self) -> Result<(), RuntimeError> {
        self.command(RuntimeCommand::EnableDesktopBindings)
    }

    /// Stops safely and terminates the runtime thread.
    ///
    /// # Errors
    /// Returns an error if cleanup or joining fails.
    pub fn shutdown(&self) -> Result<(), RuntimeError> {
        self.stop_power_monitor();
        let result = self.command(RuntimeCommand::Shutdown);
        let join_result = self.join();
        result.and(join_result)
    }

    /// Waits for the runtime thread to terminate.
    ///
    /// # Errors
    /// Returns an error if the runtime thread panicked.
    pub fn join(&self) -> Result<(), RuntimeError> {
        let mut join = self
            .join
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(handle) = join.take() {
            handle
                .join()
                .map_err(|_| RuntimeError("bridge runtime thread panicked".to_owned()))?;
        }
        Ok(())
    }

    #[must_use]
    pub fn status(&self) -> BridgeStatus {
        status_snapshot(&self.status, &self.power_errors)
    }

    fn command(
        &self,
        make_command: impl FnOnce(CommandAck) -> RuntimeCommand,
    ) -> Result<(), RuntimeError> {
        self.begin_command(make_command)?
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| RuntimeError("bridge runtime command timed out".to_owned()))?
            .map_err(RuntimeError)
    }

    fn request(
        &self,
        make_command: impl FnOnce(CommandAck) -> RuntimeCommand,
    ) -> Result<(), RuntimeError> {
        self.begin_command(make_command).map(drop)
    }

    fn begin_command(
        &self,
        make_command: impl FnOnce(CommandAck) -> RuntimeCommand,
    ) -> Result<mpsc::Receiver<Result<(), String>>, RuntimeError> {
        let (sender, receiver) = mpsc::channel();
        self.command_sender
            .send(make_command(sender))
            .map_err(|_| RuntimeError("bridge runtime is no longer running".to_owned()))?;
        Ok(receiver)
    }

    fn stop_power_monitor(&self) {
        let _monitor = self
            .power_monitor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

fn status_snapshot(
    status: &Mutex<BridgeStatus>,
    power_errors: &Mutex<mpsc::Receiver<String>>,
) -> BridgeStatus {
    let power_error = power_errors
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .try_iter()
        .last();
    let mut status = status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(error) = power_error {
        status.last_error = Some(bounded_error(&error));
        status.revision = status.revision.wrapping_add(1);
    }
    status.clone()
}

impl Drop for BridgeHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_handler_waits_for_the_runtime_acknowledgement() {
        let (commands, receiver) = mpsc::channel();
        let (power_errors, error_receiver) = mpsc::channel();
        let handler = thread::spawn(move || {
            handle_power_event(
                &commands,
                &power_errors,
                PowerEvent::WillSleep {
                    deadline: std::time::Instant::now() + Duration::from_secs(1),
                },
            );
        });

        let RuntimeCommand::SuspendForSleep(ack) = receiver.recv().unwrap() else {
            panic!("sleep event queued the wrong runtime command");
        };
        assert!(!handler.is_finished());
        ack.send(Ok(())).unwrap();
        handler.join().unwrap();
        assert!(error_receiver.try_recv().is_err());
    }

    #[test]
    fn sleep_handler_queues_deadline_error_while_status_is_contended() {
        let status = Mutex::new(BridgeStatus::default());
        let status_guard = status.lock().unwrap();
        let (commands, receiver) = mpsc::channel();
        let (power_errors, error_receiver) = mpsc::channel();
        handle_power_event(
            &commands,
            &power_errors,
            PowerEvent::WillSleep {
                deadline: std::time::Instant::now(),
            },
        );

        assert!(matches!(
            receiver.recv().unwrap(),
            RuntimeCommand::SuspendForSleep(_)
        ));
        drop(status_guard);
        let snapshot = status_snapshot(&status, &Mutex::new(error_receiver));
        assert_eq!(
            snapshot.last_error.as_deref(),
            Some("system-sleep hardware teardown did not acknowledge in time")
        );
        assert_eq!(snapshot.revision, 1);
    }

    #[test]
    fn sleep_handler_reserves_time_to_complete_the_provider_callback() {
        let now = Instant::now();
        assert_eq!(
            power_teardown_timeout(now, now + Duration::from_secs(1)),
            Duration::from_millis(900)
        );
        assert_eq!(power_teardown_timeout(now, now), Duration::ZERO);
    }

    #[test]
    fn update_resume_acknowledgements_are_polled_without_waiting() {
        let (sender, receiver) = mpsc::channel();
        let mut request = PendingUpdateResume::new(receiver);

        assert!(matches!(request.poll(), UpdateResumePoll::Pending));
        sender.send(Ok(())).unwrap();
        assert_eq!(request.poll(), UpdateResumePoll::Complete(Ok(())));
        assert_eq!(request.poll(), UpdateResumePoll::Complete(Ok(())));
    }

    #[test]
    fn update_resume_timeout_keeps_waiting_for_the_original_acknowledgement() {
        let (sender, receiver) = mpsc::channel();
        let mut timed_out = PendingUpdateResume::new(receiver);
        timed_out.command.deadline = std::time::Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        assert_eq!(timed_out.poll(), UpdateResumePoll::TimedOut);
        assert_eq!(timed_out.poll(), UpdateResumePoll::Pending);
        sender.send(Ok(())).unwrap();
        assert_eq!(timed_out.poll(), UpdateResumePoll::Complete(Ok(())));
        assert_eq!(timed_out.poll(), UpdateResumePoll::Complete(Ok(())));
    }

    #[test]
    fn update_resume_poll_reports_a_stable_disconnection() {
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let mut disconnected = PendingUpdateResume::new(receiver);
        let expected = UpdateResumePoll::Complete(Err(RuntimeError(
            "bridge runtime stopped before acknowledging recovery".to_owned(),
        )));
        assert_eq!(disconnected.poll(), expected);
        assert_eq!(disconnected.poll(), expected);
    }

    #[test]
    fn output_change_poll_is_nonblocking_and_completion_is_stable() {
        let (sender, receiver) = mpsc::channel();
        let mut request = PendingOutputChange::new(receiver);
        assert_eq!(request.poll(), OutputChangePoll::Pending);
        sender.send(Ok(())).unwrap();
        assert_eq!(request.poll(), OutputChangePoll::Complete(Ok(())));
        assert_eq!(request.poll(), OutputChangePoll::Complete(Ok(())));
    }
}
