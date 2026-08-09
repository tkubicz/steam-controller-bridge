#[allow(
    clippy::wildcard_imports,
    reason = "the HID worker shares controller mailbox and haptics implementation types"
)]
use super::*;

#[derive(Debug)]
pub(crate) enum HidWorkerEvent {
    Connected(HidDeviceInfo),
    Disconnected,
    StatusReport(RawHidReport),
    ReportReady,
}

pub(crate) enum HidWorkerControl {
    PowerOff(CommandAck),
}

pub(crate) struct PowerOffSequence {
    pub(crate) ack: Option<CommandAck>,
    pub(crate) attempts: u8,
    pub(crate) successes: u8,
    pub(crate) last_error: Option<String>,
    pub(crate) next_write: Duration,
    pub(crate) disconnected_after_success: bool,
}

pub(crate) trait PowerOffWriter {
    fn write_power_off(&self) -> Result<(), String>;
}

impl PowerOffWriter for HidSession {
    fn write_power_off(&self) -> Result<(), String> {
        self.power_off().map_err(|error| error.to_string())
    }
}

impl PowerOffSequence {
    pub(crate) fn new(ack: CommandAck, now: Duration) -> Self {
        Self {
            ack: Some(ack),
            attempts: 0,
            successes: 0,
            last_error: None,
            next_write: now,
            disconnected_after_success: false,
        }
    }

    pub(crate) fn service(
        &mut self,
        now: Duration,
        session: &impl PowerOffWriter,
    ) -> Option<Result<(), String>> {
        if self.disconnected_after_success {
            return Some(Ok(()));
        }
        if now < self.next_write {
            return None;
        }
        self.attempts = self.attempts.saturating_add(1);
        match session.write_power_off() {
            Ok(()) => {
                self.successes = self.successes.saturating_add(1);
                eprintln!(
                    "level=info event=controller_power_off_write attempt={} total={} result=success",
                    self.attempts, POWER_OFF_BURST_WRITES
                );
            }
            Err(error) => {
                self.last_error = Some(error.clone());
                eprintln!(
                    "level=warn event=controller_power_off_write attempt={} total={} result=failure error={error:?}",
                    self.attempts, POWER_OFF_BURST_WRITES
                );
            }
        }
        if self.attempts >= POWER_OFF_BURST_WRITES {
            if self.successes > 0 {
                Some(Ok(()))
            } else {
                Some(Err(self.last_error.clone().unwrap_or_else(|| {
                    "all controller power-off writes failed".to_owned()
                })))
            }
        } else {
            self.next_write = now.saturating_add(POWER_OFF_BURST_INTERVAL);
            None
        }
    }

    pub(crate) fn note_disconnected(&mut self) {
        if self.successes > 0 {
            self.disconnected_after_success = true;
        }
    }

    pub(crate) fn finish(&mut self, result: Result<(), String>) {
        if let Some(ack) = self.ack.take() {
            let _ = ack.send(result);
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct TransitionMailboxState {
    pub(crate) reports: VecDeque<RawHidReport>,
    pub(crate) transition_run: StableTransitionRun,
    pub(crate) notification_pending: bool,
    pub(crate) overflowed: bool,
}

#[derive(Debug, Default)]
pub(crate) struct TransitionReportMailbox {
    pub(crate) state: Mutex<TransitionMailboxState>,
}

#[derive(Debug, Default)]
pub(crate) struct TransitionReportBatch {
    pub(crate) reports: VecDeque<RawHidReport>,
    pub(crate) overflowed: bool,
}

impl TransitionReportMailbox {
    pub(crate) fn publish(&self, report: RawHidReport, dropped: &AtomicU64) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let transition_mask = raw_desktop_transition_mask(&report);
        if state.transition_run.can_replace_latest(transition_mask) {
            let _ = state.reports.pop_back();
            state.reports.push_back(report);
            dropped.fetch_add(1, Ordering::Relaxed);
        } else if state.reports.len() == INPUT_MAILBOX_CAPACITY {
            dropped.fetch_add(state.reports.len() as u64, Ordering::Relaxed);
            state.reports.clear();
            state.reports.push_back(report);
            state.transition_run.reset_with_latest(transition_mask);
            state.overflowed = true;
        } else {
            state.reports.push_back(report);
            state.transition_run.push(transition_mask);
        }
        let needs_notification = !state.notification_pending;
        state.notification_pending = true;
        needs_notification
    }

    pub(crate) fn take_all(&self) -> TransitionReportBatch {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.notification_pending = false;
        state.transition_run.reset();
        TransitionReportBatch {
            reports: std::mem::take(&mut state.reports),
            overflowed: std::mem::take(&mut state.overflowed),
        }
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reports
            .is_empty()
    }

    pub(crate) fn clear(&self, dropped: &AtomicU64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.reports.is_empty() {
            dropped.fetch_add(state.reports.len() as u64, Ordering::Relaxed);
            state.reports.clear();
        }
        state.notification_pending = false;
        state.overflowed = false;
        state.transition_run.reset();
    }
}

pub(crate) fn raw_desktop_transition_mask(report: &RawHidReport) -> Option<u16> {
    let valid_size = match report.report_id {
        INPUT_REPORT_ID => report.data.len() == INPUT_REPORT_SIZE,
        EXTENDED_INPUT_REPORT_ID => report.data.len() == EXTENDED_INPUT_REPORT_SIZE,
        _ => false,
    };
    if !valid_size {
        return None;
    }
    let buttons = steam_controller_protocol::SteamButtons(u32::from_le_bytes([
        report.data[2],
        report.data[3],
        report.data[4],
        report.data[5],
    ]));
    Some(desktop_transition_mask(
        buttons,
        buttons.contains(SteamButton::LeftPadTouch),
        buttons.contains(SteamButton::RightPadTouch),
    ))
}

pub(crate) struct HidWorker {
    pub(crate) receiver: Receiver<HidWorkerEvent>,
    pub(crate) failure_receiver: Receiver<String>,
    pub(crate) report_mailbox: Arc<TransitionReportMailbox>,
    pub(crate) latest_rumble: Arc<LatestRumbleSlot>,
    pub(crate) pending_pad_feedback: Arc<PendingPadFeedback>,
    pub(crate) control_sender: mpsc::Sender<HidWorkerControl>,
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) handle: Option<JoinHandle<()>>,
    pub(crate) started: Instant,
    pub(crate) lizard_metrics: Arc<SharedLizardMetrics>,
    pub(crate) haptics_metrics: Arc<SharedHapticsMetrics>,
    pub(crate) info: HidDeviceInfo,
}

impl HidWorker {
    #[allow(clippy::too_many_lines)] // Keep HID, lizard, and rumble safety ordering linear.
    pub(crate) fn spawn(
        active: ActiveControllerSource,
        lizard_mode: LizardMode,
        dropped: Arc<AtomicU64>,
    ) -> Result<Self, String> {
        let ActiveControllerSource {
            info, mut session, ..
        } = active;
        let (sender, receiver) = mpsc::sync_channel(64);
        let (failure_sender, failure_receiver) = mpsc::channel();
        let report_mailbox = Arc::new(TransitionReportMailbox::default());
        let worker_latest_report = Arc::clone(&report_mailbox);
        let latest_rumble = Arc::new(LatestRumbleSlot::default());
        let worker_latest_rumble = Arc::clone(&latest_rumble);
        let pending_pad_feedback = Arc::new(PendingPadFeedback::default());
        let worker_pending_pad_feedback = Arc::clone(&pending_pad_feedback);
        let (control_sender, control_receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let lizard_metrics = Arc::new(SharedLizardMetrics::default());
        let worker_lizard_metrics = Arc::clone(&lizard_metrics);
        let haptics_metrics = Arc::new(SharedHapticsMetrics::default());
        let worker_haptics_metrics = Arc::clone(&haptics_metrics);
        let worker_started = Instant::now();

        let mut initial_lizard =
            LizardSupervisor::new(lizard_mode, Arc::clone(&worker_lizard_metrics));
        initial_lizard
            .connected(worker_started.elapsed(), &session)
            .map_err(|error| {
                format!(
                    "lizard-mode suppression failed before input was accepted; \
                     XIAO was not activated: {error}"
                )
            })?;

        let worker_info = info.clone();
        let handle = thread::spawn(move || {
            let mut lizard = initial_lizard;
            let mut haptics = HapticsSupervisor::new(worker_haptics_metrics);
            haptics.connected(worker_started.elapsed(), &session);
            let mut pad_feedback = PadFeedbackSupervisor::new(Arc::clone(&haptics.metrics));
            pad_feedback.connected();
            let mut accepting_input = true;
            let mut power_off_sequence: Option<PowerOffSequence> = None;
            while !worker_stop.load(Ordering::Acquire) {
                if power_off_sequence.is_none() {
                    if let Ok(HidWorkerControl::PowerOff(ack)) = control_receiver.try_recv() {
                        accepting_input = false;
                        worker_latest_report.clear(&dropped);
                        worker_latest_rumble.clear();
                        worker_pending_pad_feedback.clear();
                        haptics.shutdown(worker_started.elapsed(), &session);
                        pad_feedback.disconnected();
                        lizard.disconnected();
                        power_off_sequence =
                            Some(PowerOffSequence::new(ack, worker_started.elapsed()));
                    }
                }
                if let Some(sequence) = power_off_sequence.as_mut() {
                    if let Some(result) = sequence.service(worker_started.elapsed(), &session) {
                        let failed = result.is_err();
                        sequence.finish(result);
                        power_off_sequence = None;
                        if failed {
                            accepting_input = true;
                            haptics.connected(worker_started.elapsed(), &session);
                            pad_feedback.connected();
                            if let Err(error) = lizard.connected(worker_started.elapsed(), &session)
                            {
                                worker_latest_report.clear(&dropped);
                                let _ = failure_sender.send(format!(
                                    "lizard-mode suppression could not resume after a failed \
                                     power-off attempt: {error}"
                                ));
                                break;
                            }
                        }
                    }
                }
                if accepting_input {
                    if let Err(error) = lizard.service(worker_started.elapsed(), &session) {
                        worker_latest_report.clear(&dropped);
                        let _ = failure_sender.send(format!(
                            "lizard-mode refresh failed; XIAO was neutralized and input stopped: {error}"
                        ));
                        break;
                    }
                }
                if accepting_input {
                    if let Some(command) = worker_latest_rumble.take() {
                        haptics.command(worker_started.elapsed(), &session, command);
                    }
                }
                if accepting_input {
                    haptics.service(worker_started.elapsed(), &session);
                    pad_feedback.service(
                        worker_started.elapsed(),
                        &session,
                        worker_pending_pad_feedback.take(),
                    );
                }
                match session.poll(RUNTIME_POLL_INTERVAL) {
                    Ok(Some(DeviceEvent::Connected(info))) => {
                        if !accepting_input {
                            continue;
                        }
                        if let Err(error) = lizard.connected(worker_started.elapsed(), &session) {
                            worker_latest_report.clear(&dropped);
                            let _ = failure_sender.send(format!(
                                "lizard-mode suppression failed after reconnect: {error}"
                            ));
                            break;
                        }
                        haptics.connected(worker_started.elapsed(), &session);
                        worker_pending_pad_feedback.clear();
                        pad_feedback.connected();
                        if !send_worker_event(
                            &sender,
                            HidWorkerEvent::Connected(info),
                            &worker_stop,
                        ) {
                            break;
                        }
                    }
                    Ok(Some(DeviceEvent::Disconnected)) => {
                        if let Some(sequence) = power_off_sequence.as_mut() {
                            sequence.note_disconnected();
                            continue;
                        }
                        if !accepting_input {
                            continue;
                        }
                        haptics.disconnected();
                        pad_feedback.disconnected();
                        worker_latest_rumble.clear();
                        worker_pending_pad_feedback.clear();
                        lizard.disconnected();
                        worker_latest_report.clear(&dropped);
                        if !send_worker_event(&sender, HidWorkerEvent::Disconnected, &worker_stop) {
                            break;
                        }
                    }
                    Ok(Some(DeviceEvent::Report(report))) => {
                        if !accepting_input {
                            continue;
                        }
                        let published = if is_latest_state_report(report.report_id) {
                            publish_report(&sender, &worker_latest_report, report, &dropped)
                        } else {
                            send_worker_event(
                                &sender,
                                HidWorkerEvent::StatusReport(report),
                                &worker_stop,
                            )
                        };
                        if !published {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        haptics.disconnected();
                        pad_feedback.disconnected();
                        worker_latest_rumble.clear();
                        worker_pending_pad_feedback.clear();
                        lizard.disconnected();
                        worker_latest_report.clear(&dropped);
                        let _ = failure_sender.send(format!("HID worker failed: {error}"));
                        break;
                    }
                }
            }
            while !worker_stop.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(1));
            }
            worker_latest_report.clear(&dropped);
            worker_latest_rumble.clear();
            worker_pending_pad_feedback.clear();
            pad_feedback.disconnected();
            haptics.shutdown(worker_started.elapsed(), &session);
            lizard.disconnected();
        });
        Ok(Self {
            receiver,
            failure_receiver,
            report_mailbox,
            latest_rumble,
            pending_pad_feedback,
            control_sender,
            stop,
            handle: Some(handle),
            started: worker_started,
            lizard_metrics,
            haptics_metrics,
            info: worker_info,
        })
    }

    pub(crate) fn device_info(&self) -> &HidDeviceInfo {
        &self.info
    }

    pub(crate) fn take_failure(&self) -> Option<String> {
        self.failure_receiver.try_recv().ok()
    }

    pub(crate) fn lizard_diagnostics(&self) -> LizardStatus {
        self.lizard_metrics.snapshot(self.started.elapsed())
    }

    pub(crate) fn haptics_diagnostics(&self) -> HapticsStatus {
        self.haptics_metrics.snapshot(self.started.elapsed())
    }

    pub(crate) fn set_rumble(&self, low_frequency: u16, high_frequency: u16) {
        let command = RumbleCommand {
            low_frequency,
            high_frequency,
        };
        let coalesced = self.latest_rumble.publish(command);
        self.haptics_metrics
            .record_command(self.started.elapsed(), coalesced);
    }

    pub(crate) fn request_pad_feedback(&self, request: PadFeedbackRequest) {
        if request == PadFeedbackRequest::NONE {
            return;
        }
        let coalesced = self.pending_pad_feedback.publish(request);
        self.haptics_metrics.record_pad_coalesced(coalesced);
    }

    pub(crate) fn clear_pad_feedback(&self) {
        self.pending_pad_feedback.clear();
    }

    pub(crate) fn power_off(&self) -> Result<(), String> {
        let (ack_sender, ack_receiver) = mpsc::channel();
        self.control_sender
            .send(HidWorkerControl::PowerOff(ack_sender))
            .map_err(|_| "HID worker stopped before controller power-off could start".to_owned())?;
        ack_receiver
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| "controller power-off sequence timed out".to_owned())?
    }

    pub(crate) fn take_report_batch(&self) -> TransitionReportBatch {
        self.report_mailbox.take_all()
    }

    pub(crate) fn has_pending_report(&self) -> bool {
        self.report_mailbox.has_pending()
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| "HID worker panicked".to_owned())?;
        }
        Ok(())
    }
}

impl Drop for HidWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub(crate) fn send_worker_event(
    sender: &SyncSender<HidWorkerEvent>,
    mut event: HidWorkerEvent,
    stop: &AtomicBool,
) -> bool {
    loop {
        match sender.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                if stop.load(Ordering::Acquire) {
                    return false;
                }
                event = returned;
                thread::sleep(Duration::from_millis(1));
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

pub(crate) fn publish_report(
    sender: &SyncSender<HidWorkerEvent>,
    latest_report: &TransitionReportMailbox,
    report: RawHidReport,
    dropped: &AtomicU64,
) -> bool {
    if !latest_report.publish(report, dropped) {
        return true;
    }
    match sender.try_send(HidWorkerEvent::ReportReady) {
        Ok(()) | Err(TrySendError::Full(_)) => true,
        Err(TrySendError::Disconnected(_)) => false,
    }
}
