use super::{
    automatic_shutdown_phase, binding_status_for_profile, mpsc, thread, Arc,
    AutomaticShutdownStatus, BindingProfile, BridgeStatus, Duration, JoinHandle, Mutex,
    PickerConfig, PickerEvent, PickerRoster, ProfilePickerStatus, PuckDockAction, RuntimeConfig,
    RuntimeError, RuntimeState, Supervisor, COMMAND_TIMEOUT,
};
#[cfg(target_os = "macos")]
use super::{bounded_error, OutputSelection, PowerEvent, PowerMonitor, SLEEP_TEARDOWN_ACK_TIMEOUT};

pub(crate) type CommandAck = mpsc::Sender<Result<(), String>>;

pub(crate) enum RuntimeCommand {
    Start(CommandAck),
    Stop(CommandAck),
    Shutdown(CommandAck),
    SetIdleShutdown(Option<Duration>, CommandAck),
    SetPuckDockAction(PuckDockAction, CommandAck),
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
            ..BridgeStatus::default()
        }));
        let worker_status = Arc::clone(&status);
        let (command_sender, command_receiver) = mpsc::channel();
        #[cfg(target_os = "macos")]
        let (power_monitor, startup_blocker) = if config.output == OutputSelection::Serial {
            match power_monitor(Arc::clone(&status), command_sender.clone()) {
                Ok(monitor) => (Some(monitor), None),
                Err(error) => {
                    eprintln!("level=error event=system_power_monitor_failed error={error:?}");
                    (None, Some(error))
                }
            }
        } else {
            (None, None)
        };
        #[cfg(not(target_os = "macos"))]
        let startup_blocker = None;
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
            #[cfg(target_os = "macos")]
            power_monitor: Mutex::new(power_monitor),
        }
    }
}

#[cfg(target_os = "macos")]
fn power_monitor(
    status: Arc<Mutex<BridgeStatus>>,
    commands: mpsc::Sender<RuntimeCommand>,
) -> Result<PowerMonitor, String> {
    PowerMonitor::new(move |event| match event {
        PowerEvent::WillSleep => {
            let (ack, receiver) = mpsc::channel();
            let result = commands
                .send(RuntimeCommand::SuspendForSleep(ack))
                .map_err(|_| "bridge runtime stopped before system sleep".to_owned())
                .and_then(|()| {
                    // Bounded: blocking here delays the sleep, and a teardown
                    // wedged in a platform call must not hold sleep, the wake
                    // notification, and app quit hostage forever. The bound
                    // stays under macOS's ~30 s forced-sleep cap, and a late
                    // acknowledgement lands in a dropped receiver harmlessly.
                    receiver
                        .recv_timeout(SLEEP_TEARDOWN_ACK_TIMEOUT)
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
                    let mut current = status
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    current.last_error = Some(bounded_error(&error));
                    current.revision = current.revision.wrapping_add(1);
                }
            }
        }
        PowerEvent::DidWake => {
            let (ack, _receiver) = mpsc::channel();
            if commands.send(RuntimeCommand::ResumeFromWake(ack)).is_err() {
                eprintln!("level=warn event=system_wake_runtime_unavailable");
            }
        }
    })
    .map_err(|error| format!("cannot monitor system sleep safely: {error}"))
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
    #[cfg(target_os = "macos")]
    power_monitor: Mutex<Option<PowerMonitor>>,
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

    /// Queues a binding-profile switch without restarting HID or serial.
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

    /// Parks the controller at neutral, closes the serial port and HID
    /// handles, and returns only once that teardown has completed.
    ///
    /// For the frontend's system-sleep hook. The port must be **closed before
    /// the machine sleeps**: serial I/O left in flight across a sleep/wake
    /// transition has panicked macOS's USB CDC driver while the XIAO
    /// re-enumerated. The bridge stays suspended — regardless of its
    /// start/stop setting — until [`BridgeHandle::request_resume_from_wake`].
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

    /// Lets the bridge look for its hardware again after a system wake.
    ///
    /// Discovery waits `WAKE_SETTLE_DELAY` first, so the USB stack has
    /// time to finish re-enumerating the XIAO before anything reopens it. A
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

    /// Updates the idle-shutdown timeout without restarting HID or serial.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn set_idle_shutdown_timeout(&self, timeout: Option<Duration>) -> Result<(), RuntimeError> {
        self.command(|ack| RuntimeCommand::SetIdleShutdown(timeout, ack))
    }

    /// Updates the immediate Puck-dock action without restarting HID or serial.
    ///
    /// # Errors
    /// Returns an error if the runtime thread has stopped.
    pub fn set_puck_dock_action(&self, action: PuckDockAction) -> Result<(), RuntimeError> {
        self.command(|ack| RuntimeCommand::SetPuckDockAction(action, ack))
    }

    /// Switches the active binding profile without restarting HID or serial.
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
        #[cfg(target_os = "macos")]
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
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn command(
        &self,
        make_command: impl FnOnce(CommandAck) -> RuntimeCommand,
    ) -> Result<(), RuntimeError> {
        let (sender, receiver) = mpsc::channel();
        self.command_sender
            .send(make_command(sender))
            .map_err(|_| RuntimeError("bridge runtime is no longer running".to_owned()))?;
        receiver
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| RuntimeError("bridge runtime command timed out".to_owned()))?
            .map_err(RuntimeError)
    }

    fn request(
        &self,
        make_command: impl FnOnce(CommandAck) -> RuntimeCommand,
    ) -> Result<(), RuntimeError> {
        let (sender, _receiver) = mpsc::channel();
        self.command_sender
            .send(make_command(sender))
            .map_err(|_| RuntimeError("bridge runtime is no longer running".to_owned()))
    }

    #[cfg(target_os = "macos")]
    fn stop_power_monitor(&self) {
        drop(
            self.power_monitor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
    }
}

impl Drop for BridgeHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
