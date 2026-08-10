//! Talking to the device: the polling worker, the event drain, and the
//! recording writes that hang off it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bridge_output::GamepadOutput;
use controller_mapper::ControllerMapper;
use gamepad_state::GamepadState;
use recording::{RecordingEvent, KIND_DEVICE_CONNECTED, KIND_DEVICE_DISCONNECTED};
use serde_json::json;
use steam_controller_device::{enumerate, DeviceEvent, HidDeviceInfo, HidSession, RawHidReport};
use steam_controller_discovery::{ActiveControllerFinder, ControllerSearch};
use steam_controller_protocol::DecodedReport;

use crate::cli::Source;
use crate::mailbox::{InputEvent, InputMailbox};
use crate::{InputState, OutputChoice, Visualizer, FRAME_TIME};

const INPUT_TIMEOUT: Duration = Duration::from_millis(200);
const DECODE_FAILURE_LIMIT: u32 = 3;

/// A stop flag paired with a condition variable, so retry waits are both idle
/// and immediately interruptible during ownership transitions or shutdown.
pub(crate) struct StopToken {
    stopped: AtomicBool,
    lock: Mutex<()>,
    wake: Condvar,
}

impl Default for StopToken {
    fn default() -> Self {
        Self {
            stopped: AtomicBool::new(false),
            lock: Mutex::new(()),
            wake: Condvar::new(),
        }
    }
}

impl StopToken {
    pub(crate) fn load(&self, order: Ordering) -> bool {
        self.stopped.load(order)
    }

    pub(crate) fn store(&self, stopped: bool, order: Ordering) {
        self.stopped.store(stopped, order);
        if stopped {
            self.wake.notify_all();
        }
    }

    pub(crate) fn wait(&self, timeout: Duration) {
        if self.load(Ordering::Acquire) {
            return;
        }
        let guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = self
            .wake
            .wait_timeout_while(guard, timeout, |()| !self.load(Ordering::Acquire))
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
}

/// All GUI-mode HID enumeration and open calls run on this process-lifetime
/// thread. hidapi's macOS backend owns a global `IOHIDManager` tied to the run
/// loop of its first enumeration thread; destroying an ordinary worker and
/// enumerating from its replacement otherwise traps in CoreFoundation.
pub(crate) struct HidBroker {
    client: HidBrokerClient,
    worker: Option<thread::JoinHandle<()>>,
}

#[derive(Clone)]
pub(crate) struct HidBrokerClient {
    commands: mpsc::Sender<BrokerCommand>,
}

pub(crate) struct BrokerOpened {
    #[cfg(target_os = "macos")]
    pub(crate) index: usize,
    pub(crate) info: HidDeviceInfo,
    pub(crate) session: HidSession,
    pub(crate) announce_connection: bool,
}

enum BrokerCommand {
    Open {
        source: Source,
        stop: Arc<StopToken>,
        events: mpsc::Sender<BrokerEvent>,
    },
    #[cfg(test)]
    ReportThread(mpsc::Sender<thread::ThreadId>),
    Shutdown,
}

enum BrokerEvent {
    Status(ControllerSearch),
    Opened(Box<BrokerOpened>),
    Failed(String),
}

impl HidBroker {
    pub(crate) fn new() -> Self {
        let (commands, receiver) = mpsc::channel();
        let worker = thread::spawn(move || broker_worker(&receiver));
        Self {
            client: HidBrokerClient { commands },
            worker: Some(worker),
        }
    }

    pub(crate) fn client(&self) -> HidBrokerClient {
        self.client.clone()
    }
}

impl Drop for HidBroker {
    fn drop(&mut self) {
        let _ = self.client.commands.send(BrokerCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl HidBrokerClient {
    pub(crate) fn open(
        &self,
        source: Source,
        stop: Arc<StopToken>,
        mut status: impl FnMut(ControllerSearch),
    ) -> Result<BrokerOpened, String> {
        let (events, receiver) = mpsc::channel();
        self.commands
            .send(BrokerCommand::Open {
                source,
                stop,
                events,
            })
            .map_err(|_| "visualizer HID broker stopped unexpectedly".to_owned())?;
        loop {
            match receiver.recv() {
                Ok(BrokerEvent::Status(message)) => status(message),
                Ok(BrokerEvent::Opened(opened)) => return Ok(*opened),
                Ok(BrokerEvent::Failed(error)) => return Err(error),
                Err(_) => return Err("visualizer HID broker stopped unexpectedly".to_owned()),
            }
        }
    }
}

fn broker_worker(commands: &mpsc::Receiver<BrokerCommand>) {
    let mut finder = None;
    while let Ok(command) = commands.recv() {
        match command {
            BrokerCommand::Open {
                source,
                stop,
                events,
            } => broker_open(source, &stop, &events, &mut finder),
            #[cfg(test)]
            BrokerCommand::ReportThread(sender) => {
                let _ = sender.send(thread::current().id());
            }
            BrokerCommand::Shutdown => return,
        }
    }
}

fn broker_open(
    source: Source,
    stop: &Arc<StopToken>,
    events: &mpsc::Sender<BrokerEvent>,
    finder: &mut Option<ActiveControllerFinder>,
) {
    if let Source::Collection(index) = source {
        let event = enumerate()
            .map_err(|error| error.to_string())
            .and_then(|devices| {
                devices
                    .get(index)
                    .cloned()
                    .ok_or_else(|| format!("HID device index {index} does not exist"))
            })
            .and_then(|info| {
                HidSession::open_info(&info)
                    .map(|session| (info, session))
                    .map_err(|error| error.to_string())
            })
            .map_or_else(BrokerEvent::Failed, |(info, session)| {
                BrokerEvent::Opened(Box::new(BrokerOpened {
                    #[cfg(target_os = "macos")]
                    index,
                    info,
                    session,
                    announce_connection: false,
                }))
            });
        let _ = events.send(event);
        return;
    }
    if matches!(source, Source::Demo(_)) {
        let _ = events.send(BrokerEvent::Failed("demo runs open no device".to_owned()));
        return;
    }
    let active_finder = match finder {
        Some(finder) => finder,
        slot => match ActiveControllerFinder::new() {
            Ok(finder) => slot.insert(finder),
            Err(error) => {
                let _ = events.send(BrokerEvent::Failed(error.to_string()));
                return;
            }
        },
    };
    let mut last_status = None;
    while !stop.load(Ordering::Acquire) {
        match active_finder.find_with_index() {
            Ok((index, info, session)) => {
                #[cfg(not(target_os = "macos"))]
                let _ = index;
                let _ = events.send(BrokerEvent::Opened(Box::new(BrokerOpened {
                    #[cfg(target_os = "macos")]
                    index,
                    info,
                    session,
                    announce_connection: true,
                })));
                return;
            }
            Err(search) => {
                if last_status.as_ref() != Some(&search) {
                    let _ = events.send(BrokerEvent::Status(search.clone()));
                    last_status = Some(search);
                }
            }
        }
        wait_for_stop(stop, Duration::from_millis(250));
    }
    active_finder.clear_candidates();
    let _ = events.send(BrokerEvent::Failed(
        "controller auto-detection was cancelled".to_owned(),
    ));
}

/// The worker's end of the connection, plus the flag that stops it.
pub(crate) struct InputChannel {
    pub(crate) mailbox: Arc<InputMailbox>,
    stop: Arc<StopToken>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Drop for InputChannel {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl InputChannel {
    /// Stops and joins the worker so its HID ownership is released before a
    /// lossless capture session opens the same controller.
    pub(crate) fn shutdown(&mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .map_err(|_| "visualizer input worker panicked".to_owned())
        })
    }
}

/// How long to wait before looking for a controller again.
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(1);

/// An opened controller, plus the connection the caller still has to announce.
struct Opened {
    session: HidSession,
    /// `Some` when discovery already consumed the session's synthetic open
    /// event and the worker must re-announce it. See [`open`].
    announce: Option<HidDeviceInfo>,
}

/// Opens the controller this run is pointed at.
///
/// Discovery delegates to [`ActiveControllerFinder`], which is the runtime's
/// own `AutoActive` path. That matters twice over: the free `enumerate`
/// function returns *every* HID collection in the system rather than the
/// supported ones, and a connected Puck presents four sibling collections of
/// which only one carries input. Picking a first entry gets both wrong.
///
/// Probing costs the synthetic `Connected` event: identifying the active
/// collection means polling it, and that poll consumes the very event that
/// would otherwise tell the UI it is connected. The finder hands back the
/// [`HidDeviceInfo`] it selected precisely so the caller can re-announce it;
/// `--index` skips probing, so there the event is still queued and re-announcing
/// would double-count it.
fn open(
    broker: &HidBrokerClient,
    mailbox: &InputMailbox,
    stop: Arc<StopToken>,
    source: Source,
) -> Result<Opened, String> {
    broker
        .open(source, stop, |status| {
            mailbox.publish(InputEvent::Search(status));
        })
        .map(|opened| Opened {
            session: opened.session,
            announce: opened.announce_connection.then_some(opened.info),
        })
}

pub(crate) fn input_worker(source: Source, broker: HidBrokerClient) -> InputChannel {
    let mailbox = Arc::new(InputMailbox::default());
    let stop = Arc::new(StopToken::default());
    let worker_mailbox = Arc::clone(&mailbox);
    let worker_stop = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        while !worker_stop.load(Ordering::Relaxed) {
            let Opened {
                mut session,
                announce,
            } = match open(&broker, &worker_mailbox, Arc::clone(&worker_stop), source) {
                Ok(opened) => opened,
                Err(error) => {
                    worker_mailbox.publish(InputEvent::Lifecycle(Box::new(Err(error))));
                    // An explicit `--index` is a claim about a specific
                    // collection, so a failure there is final. Discovery is a
                    // standing request, so it keeps trying.
                    if matches!(source, Source::Collection(_)) {
                        return;
                    }
                    wait_for_stop(&worker_stop, DISCOVERY_INTERVAL);
                    continue;
                }
            };
            if let Some(info) = announce {
                worker_mailbox.publish(InputEvent::Lifecycle(Box::new(Ok(
                    DeviceEvent::Connected(info),
                ))));
            }
            while !worker_stop.load(Ordering::Relaxed) {
                let event = session
                    .poll(Duration::from_millis(10))
                    .map_err(|error| error.to_string());
                match event {
                    // Reports may be coalesced under pressure; everything else
                    // is an ordered barrier the mailbox never discards.
                    Ok(Some(DeviceEvent::Report(report))) => {
                        worker_mailbox.publish(InputEvent::report(report));
                    }
                    Ok(Some(DeviceEvent::Disconnected)) => {
                        worker_mailbox.publish(InputEvent::Lifecycle(Box::new(Ok(
                            DeviceEvent::Disconnected,
                        ))));
                        // HidSession owns identity-preserving reconnection.
                        // Keep polling it so an indexed source is not lost
                        // merely because it was briefly unplugged.
                    }
                    Ok(Some(other)) => {
                        worker_mailbox.publish(InputEvent::Lifecycle(Box::new(Ok(other))));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        worker_mailbox.publish(InputEvent::Lifecycle(Box::new(Err(error))));
                        // A failed HID refresh can return immediately. Back off
                        // while retaining the session instead of spinning or
                        // reopening an explicit index against a changed list.
                        wait_for_stop(&worker_stop, DISCOVERY_INTERVAL);
                    }
                }
            }
        }
    });
    InputChannel {
        mailbox,
        stop,
        worker: Some(worker),
    }
}

fn wait_for_stop(stop: &StopToken, duration: Duration) {
    stop.wait(duration);
}

impl Visualizer {
    pub(crate) fn timestamp_us(&self) -> u64 {
        self.recording_started
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    pub(crate) fn record_lazy(
        &mut self,
        make_event: impl FnOnce() -> Result<RecordingEvent, String>,
    ) {
        if !self
            .recording
            .as_ref()
            .is_some_and(crate::recording_sink::RecordingSession::is_accepting)
        {
            return;
        }
        let event = match make_event() {
            Ok(event) => event,
            Err(error) => {
                self.stop_recording_with_error(&error);
                return;
            }
        };
        if let Some(error) = self
            .recording
            .as_ref()
            .and_then(|recording| recording.record(event).err())
        {
            self.stop_recording_with_error(&error);
        }
    }

    fn stop_recording_with_error(&mut self, error: &str) {
        if let Some(recording) = &mut self.recording {
            recording.request_finish();
        }
        self.recording_stop_error = Some(error.to_owned());
        self.status = format!("Recording stopping: {error}");
    }

    pub(crate) fn service_recording(&mut self) {
        let result = self
            .recording
            .as_ref()
            .and_then(crate::recording_sink::RecordingSession::poll_result);
        if let Some(result) = result {
            self.recording = None;
            self.status = match result {
                Ok(()) => self.recording_stop_error.take().map_or_else(
                    || "Recording stopped".to_owned(),
                    |error| format!("Recording stopped: {error}"),
                ),
                Err(error) => format!("Recording stopped: {error}"),
            };
            self.recording_stop_error = None;
        }
    }

    pub(crate) fn process_events(&mut self) {
        let Some(input) = &self.input else {
            return;
        };
        let published = input.mailbox.counters.published();
        let overflowed = input.mailbox.take_all(&mut self.input_events);
        let mut batch = std::mem::take(&mut self.input_events);
        if overflowed {
            "Input backlog overflowed; recovered from the newest report"
                .clone_into(&mut self.status);
        }
        for queued in batch.drain(..) {
            if let InputEvent::Search(search) = queued {
                self.status = search.to_string();
                self.connection_search = Some(search);
                continue;
            }
            self.connection_search = None;
            let (event, received_at) = match queued {
                InputEvent::Report {
                    report,
                    received_at,
                } => (Ok(DeviceEvent::Report(report)), Some(received_at)),
                InputEvent::Lifecycle(lifecycle) => (*lifecycle, None),
                InputEvent::Search(_) => unreachable!("search events were handled above"),
            };
            match event {
                Err(error) => self.status = error,
                Ok(DeviceEvent::Connected(info)) => {
                    self.connected = true;
                    self.last_report_timestamp = None;
                    self.last_controller_input = None;
                    self.consecutive_decode_failures = 0;
                    self.device = info.product.clone().unwrap_or(info.id.clone());
                    self.status = format!("Connected via {}", info.transport);
                    let timestamp = self.timestamp_us();
                    self.record_lazy(|| {
                        Ok(RecordingEvent::new(
                            timestamp,
                            KIND_DEVICE_CONNECTED,
                            json!({"id": info.id, "transport": info.transport}),
                        ))
                    });
                }
                Ok(DeviceEvent::Disconnected) => {
                    self.connected = false;
                    self.source = None;
                    self.last_report_timestamp = None;
                    self.last_controller_input = None;
                    self.consecutive_decode_failures = 0;
                    self.neutralize_output();
                    if !self.status.starts_with("Output neutralization failed") {
                        "Disconnected; waiting for reconnect".clone_into(&mut self.status);
                    }
                    let timestamp = self.timestamp_us();
                    self.record_lazy(|| {
                        Ok(RecordingEvent::new(
                            timestamp,
                            KIND_DEVICE_DISCONNECTED,
                            json!({}),
                        ))
                    });
                }
                Ok(DeviceEvent::Report(report)) => {
                    self.handle_report(&report, received_at.unwrap_or_else(Instant::now));
                }
            }
        }
        self.input_events = batch;

        // Once per drain, not per report: this is a one-second average.
        let elapsed = self.rate_started.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let reports =
                u16::try_from(published.saturating_sub(self.rate_report_start)).unwrap_or(u16::MAX);
            self.report_hz = f32::from(reports) / elapsed.as_secs_f32();
            self.rate_report_start = published;
            self.rate_started = Instant::now();
        }
    }

    fn handle_report(&mut self, report: &RawHidReport, received_at: Instant) {
        self.report_count += 1;
        self.source_report_drops = self
            .source_report_drops
            .saturating_add(report.dropped_reports);
        self.raw.clone_from(&report.data);
        self.raw_report_id = Some(report.report_id);
        let timestamp = self.timestamp_us();
        self.record_lazy(|| {
            RecordingEvent::raw_hid_with_metadata(
                timestamp,
                report.report_id,
                &report.data,
                Some(&report.source_device_id),
                Some(&report.transport),
                report.dropped_reports,
            )
            .map_err(|error| error.to_string())
        });
        match self.decoder.decode(report.report_id, &report.data) {
            Ok(DecodedReport::ControllerState(state)) => {
                self.consecutive_decode_failures = 0;
                self.last_controller_input = Some(received_at);
                // Only state reports advance the mapper clock. Battery/status
                // reports and malformed traffic do not represent an input
                // sample and must not shorten the next smoothing interval.
                let delta_time = self.report_delta_time(report.timestamp);
                self.mapped = self.mapper.map(&state, delta_time);
                self.input_state = InputState::Active;
                self.record_lazy(|| {
                    RecordingEvent::decoded_steam_state(timestamp, &state)
                        .map_err(|error| error.to_string())
                });
                let mapped = self.mapped;
                self.record_lazy(|| {
                    RecordingEvent::mapped_gamepad_state(timestamp, &mapped)
                        .map_err(|error| error.to_string())
                });
                self.source = Some(state);
                self.publish_mapped();
            }
            Ok(_) => self.consecutive_decode_failures = 0,
            Err(error) => {
                self.decode_failures += 1;
                self.consecutive_decode_failures += 1;
                if self.consecutive_decode_failures >= DECODE_FAILURE_LIMIT {
                    self.neutralize_output();
                    if !self.status.starts_with("Output neutralization failed") {
                        self.status = format!(
                            "Decode error: {error}; output neutralized after {DECODE_FAILURE_LIMIT} failures"
                        );
                    }
                } else {
                    self.status = format!("Decode error: {error}");
                }
            }
        }
    }

    fn report_delta_time(&mut self, timestamp: Duration) -> f32 {
        let delta = self
            .last_report_timestamp
            .and_then(|previous| timestamp.checked_sub(previous))
            .filter(|delta| !delta.is_zero())
            .map_or(FRAME_TIME, |delta| delta.as_secs_f32());
        self.last_report_timestamp = Some(timestamp);
        delta
    }

    pub(crate) fn check_input_timeout(&mut self) {
        if self.input_state == InputState::Active
            && self
                .last_controller_input
                .is_some_and(|last| last.elapsed() >= INPUT_TIMEOUT)
        {
            self.neutralize_output();
            if !self.status.starts_with("Output neutralization failed") {
                "Input timed out; output neutralized".clone_into(&mut self.status);
            }
        }
    }

    /// Hands the freshly mapped state to whichever output backend is selected.
    pub(crate) fn publish_mapped(&mut self) {
        match self.output {
            OutputChoice::Disabled => {}
            OutputChoice::Mock => {
                if self.last_output != Some(self.mapped) {
                    self.packets_sent += 1;
                    self.last_output = Some(self.mapped);
                }
            }
            OutputChoice::Serial => {
                if let Some(serial) = &mut self.serial {
                    match serial.send_state(&self.mapped) {
                        Ok(()) => {
                            self.serial_metrics = Some(serial.metrics());
                        }
                        Err(error) => {
                            self.status = error.to_string();
                            self.serial = None;
                            self.serial_metrics = None;
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn neutralize_output(&mut self) {
        self.mapper.reset();
        self.mapped = GamepadState::neutral();
        self.last_controller_input = None;
        if self.input_state == InputState::Neutralized {
            return;
        }
        match self.output {
            OutputChoice::Disabled => {}
            OutputChoice::Mock => {
                if self.last_output != Some(GamepadState::neutral()) {
                    self.packets_sent += 1;
                    self.last_output = Some(GamepadState::neutral());
                }
            }
            OutputChoice::Serial => {
                if let Some(serial) = &mut self.serial {
                    if let Err(error) = serial.send_neutral() {
                        self.status = format!("Output neutralization failed: {error}");
                        self.serial = None;
                        self.serial_metrics = None;
                    }
                }
            }
        }
        self.input_state = InputState::Neutralized;
    }

    pub(crate) fn select_output(&mut self, choice: OutputChoice) {
        if choice == self.output {
            return;
        }
        if self.output == OutputChoice::Serial {
            self.disconnect_serial();
        }
        self.output = choice;
        self.last_output = None;
        self.serial_metrics = None;
        if choice == OutputChoice::Mock {
            self.publish_mapped();
        }
    }

    pub(crate) fn disconnect_serial(&mut self) {
        if let Some(mut serial) = self.serial.take() {
            if let Err(error) = serial.send_neutral() {
                self.status = format!("Serial closed; neutralization failed: {error}");
            } else {
                "Serial disconnected after neutral output".clone_into(&mut self.status);
            }
        }
        self.serial_metrics = None;
    }

    pub(crate) fn rebuild_mapper(&mut self) {
        self.config.smoothing_time_constant = self
            .smoothing_enabled
            .then_some(self.smoothing_time_constant);
        match ControllerMapper::new(self.config) {
            Ok(mut mapper) => {
                if let Some(source) = &self.source {
                    self.mapped = mapper.map(source, FRAME_TIME);
                    self.input_state = InputState::Active;
                }
                self.mapper = mapper;
                self.last_report_timestamp = None;
                self.publish_mapped();
            }
            Err(error) => self.status = error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BrokerCommand, HidBroker, Visualizer, DECODE_FAILURE_LIMIT, INPUT_TIMEOUT};
    use crate::cli::Source;
    use crate::demo::DemoState;
    use crate::{InputState, OutputChoice};
    use controller_mapper::RightAxisSource;
    use gamepad_state::GamepadState;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use steam_controller_device::RawHidReport;

    fn analog_demo() -> Visualizer {
        Visualizer::new(Source::Demo(DemoState::Analog))
    }

    #[test]
    fn hid_broker_keeps_one_process_lifetime_worker_thread() {
        let broker = HidBroker::new();
        let client = broker.client();
        let worker_id = || {
            let (sender, receiver) = mpsc::channel();
            client
                .commands
                .send(BrokerCommand::ReportThread(sender))
                .expect("broker accepts diagnostics");
            receiver.recv().expect("broker reports its thread")
        };
        assert_eq!(worker_id(), worker_id());
    }

    /// Exercises the native hidapi/IOHIDManager lifecycle that cannot be
    /// modeled by the platform-neutral broker test. Run explicitly on macOS;
    /// it requires only enumeration, not an available controller.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "native macOS HID lifecycle regression"]
    fn dashboard_worker_can_restart_after_join_without_rebinding_hidapi() {
        let broker = HidBroker::new();
        for _ in 0..2 {
            let mut input = super::input_worker(Source::Discover, broker.client());
            // Let the broker enter native enumeration. `shutdown` then waits
            // for that request to acknowledge cancellation before the next
            // worker is created.
            std::thread::sleep(Duration::from_millis(500));
            input.shutdown().expect("worker joins cleanly");
        }
    }

    #[test]
    fn reset_and_input_timeout_publish_neutral_to_an_active_output() {
        let mut visualizer = analog_demo();
        visualizer.select_output(OutputChoice::Mock);
        assert_ne!(visualizer.last_output, Some(GamepadState::neutral()));
        let active_packets = visualizer.packets_sent;

        visualizer.last_controller_input = Some(
            Instant::now()
                .checked_sub(INPUT_TIMEOUT)
                .expect("valid past"),
        );
        visualizer.check_input_timeout();

        assert_eq!(visualizer.mapped, GamepadState::neutral());
        assert_eq!(visualizer.last_output, Some(GamepadState::neutral()));
        assert_eq!(visualizer.packets_sent, active_packets + 1);
        assert_eq!(visualizer.input_state, InputState::Neutralized);
    }

    #[test]
    fn mapper_changes_immediately_remap_a_static_demo() {
        let mut visualizer = analog_demo();
        let stick = (visualizer.mapped.right_x, visualizer.mapped.right_y);
        visualizer.config.right_axis_source = RightAxisSource::RightPad;
        visualizer.rebuild_mapper();
        let pad = (visualizer.mapped.right_x, visualizer.mapped.right_y);
        assert_ne!(pad, stick, "the demo must not wait for a hardware report");
        assert!(
            pad.0 < 0.0 && pad.1 < 0.0,
            "the demo right pad is lower-left"
        );
    }

    #[test]
    fn mapper_timing_uses_report_gaps_and_recovers_from_timestamp_resets() {
        let mut visualizer = analog_demo();
        visualizer.last_report_timestamp = None;
        assert!((visualizer.report_delta_time(Duration::from_millis(4)) - 0.004).abs() < 1e-6);
        assert!((visualizer.report_delta_time(Duration::from_millis(12)) - 0.008).abs() < 1e-6);
        assert!((visualizer.report_delta_time(Duration::from_millis(1)) - 0.004).abs() < 1e-6);
    }

    #[test]
    fn repeated_decode_failures_neutralize_the_selected_output() {
        let mut visualizer = analog_demo();
        visualizer.select_output(OutputChoice::Mock);
        for index in 0..DECODE_FAILURE_LIMIT {
            visualizer.handle_report(
                &RawHidReport {
                    timestamp: Duration::from_millis(u64::from(index + 1) * 4),
                    report_id: 0x45,
                    data: vec![0x45],
                    source_device_id: "test".to_owned(),
                    transport: "test".to_owned(),
                    dropped_reports: 0,
                },
                Instant::now(),
            );
        }
        assert_eq!(visualizer.mapped, GamepadState::neutral());
        assert_eq!(visualizer.last_output, Some(GamepadState::neutral()));
        assert_eq!(visualizer.input_state, InputState::Neutralized);
    }

    #[test]
    fn a_delayed_report_does_not_extend_the_input_watchdog() {
        let mut visualizer = Visualizer::new(Source::Demo(DemoState::Neutral));
        visualizer.select_output(OutputChoice::Mock);
        let mut data = vec![0_u8; steam_controller_protocol::INPUT_REPORT_SIZE];
        data[0] = steam_controller_protocol::INPUT_REPORT_ID;
        data[6..8].copy_from_slice(&12_000_i16.to_le_bytes());
        let received_at = Instant::now()
            .checked_sub(INPUT_TIMEOUT)
            .expect("valid past");
        visualizer.handle_report(
            &RawHidReport {
                timestamp: Duration::from_millis(4),
                report_id: steam_controller_protocol::INPUT_REPORT_ID,
                data,
                source_device_id: "test".to_owned(),
                transport: "test".to_owned(),
                dropped_reports: 0,
            },
            received_at,
        );
        assert_eq!(visualizer.input_state, InputState::Active);

        visualizer.check_input_timeout();

        assert_eq!(visualizer.input_state, InputState::Neutralized);
        assert_eq!(visualizer.last_output, Some(GamepadState::neutral()));
    }

    #[test]
    fn non_state_reports_do_not_advance_the_mapper_clock() {
        let mut visualizer = Visualizer::new(Source::Demo(DemoState::Neutral));
        visualizer.last_report_timestamp = Some(Duration::from_millis(4));
        let mut data = vec![0_u8; 15];
        data[0] = steam_controller_protocol::BATTERY_REPORT_ID;
        visualizer.handle_report(
            &RawHidReport {
                timestamp: Duration::from_millis(8),
                report_id: steam_controller_protocol::BATTERY_REPORT_ID,
                data,
                source_device_id: "test".to_owned(),
                transport: "test".to_owned(),
                dropped_reports: 0,
            },
            Instant::now(),
        );
        assert_eq!(
            visualizer.last_report_timestamp,
            Some(Duration::from_millis(4))
        );
    }
}
