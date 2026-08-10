#![allow(clippy::cast_precision_loss)] // Display pixel dimensions become f64 scale metadata.

#[cfg(any(target_os = "macos", test))]
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use crate::device::HidBrokerClient;

#[cfg(any(target_os = "macos", test))]
use recording::RecordingEvent;

#[cfg(target_os = "macos")]
const QUEUE_CAPACITY: usize = 8_192;
#[cfg(any(target_os = "macos", test))]
const REORDER_WINDOW_US: u64 = 10_000;

#[derive(Debug, Clone)]
#[cfg_attr(
    not(target_os = "macos"),
    allow(
        dead_code,
        reason = "portable GUI code creates commands whose fields are consumed only by macOS capture"
    )
)]
pub(crate) enum GuiCaptureCommand {
    Start { trial_id: String, attempt: u32 },
    End { trial_id: String, attempt: u32 },
    Accept { trial_id: String, attempt: u32 },
    Discard { trial_id: String, attempt: u32 },
    Finish,
    Cancel,
}

#[derive(Debug, Clone)]
pub(crate) struct CapturePreflight {
    pub(crate) controller_index: usize,
    pub(crate) controller: String,
    pub(crate) transport: String,
    pub(crate) state_reports: usize,
    pub(crate) lizard_reports: usize,
    pub(crate) event_tap_ready: bool,
    pub(crate) displays: Vec<String>,
    pub(crate) invalid_reason: Option<String>,
}

#[derive(Debug, Clone)]
#[cfg_attr(
    not(target_os = "macos"),
    allow(
        dead_code,
        reason = "capture progress events are produced only by the macOS measurement worker"
    )
)]
pub(crate) enum GuiCaptureEvent {
    Preflight(CapturePreflight),
    TrialPreview {
        trial_id: String,
        points: Vec<(i64, i64)>,
    },
    Finished(Result<(), String>),
}

pub(crate) struct GuiCapture {
    commands: mpsc::Sender<GuiCaptureCommand>,
    events: mpsc::Receiver<GuiCaptureEvent>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl GuiCapture {
    pub(crate) fn start(
        index: Option<usize>,
        output: PathBuf,
        hid_broker: HidBrokerClient,
    ) -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let result = run_gui(
                index,
                &output,
                &worker_stop,
                command_receiver,
                event_sender.clone(),
                &hid_broker,
            );
            let _ = event_sender.send(GuiCaptureEvent::Finished(result));
        });
        Self {
            commands: command_sender,
            events: event_receiver,
            stop,
            worker: Some(worker),
        }
    }

    pub(crate) fn send(&self, command: GuiCaptureCommand) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "capture worker stopped unexpectedly".to_owned())
    }

    /// The queued command lets a live guided worker record the specific
    /// cancel reason; the stop flag reaches a worker still blocked inside
    /// controller discovery, where no command loop is running yet.
    pub(crate) fn cancel(&self) {
        let _ = self.commands.send(GuiCaptureCommand::Cancel);
        self.stop.store(true, Ordering::Release);
    }

    pub(crate) fn try_event(&self) -> Option<GuiCaptureEvent> {
        self.events.try_recv().ok()
    }

    pub(crate) fn join(&mut self) -> Result<(), String> {
        self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .map_err(|_| "capture worker panicked".to_owned())
        })
    }
}

impl Drop for GuiCapture {
    fn drop(&mut self) {
        self.cancel();
        let _ = self.join();
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug)]
struct QueuedEvent {
    sequence: u64,
    event: RecordingEvent,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Default)]
struct EventMerger {
    buffered: VecDeque<QueuedEvent>,
    last_emitted_us: Option<u64>,
}

#[cfg(any(target_os = "macos", test))]
impl EventMerger {
    fn push(&mut self, event: QueuedEvent) -> Result<(), QueuedEvent> {
        if self
            .last_emitted_us
            .is_some_and(|timestamp| event.event.timestamp_us < timestamp)
        {
            return Err(event);
        }
        let position = self.buffered.partition_point(|current| {
            (current.event.timestamp_us, current.sequence)
                <= (event.event.timestamp_us, event.sequence)
        });
        self.buffered.insert(position, event);
        Ok(())
    }

    fn drain_ready(&mut self, now_us: u64) -> Vec<RecordingEvent> {
        let cutoff = now_us.saturating_sub(REORDER_WINDOW_US);
        let ready = self
            .buffered
            .partition_point(|item| item.event.timestamp_us <= cutoff);
        let events: Vec<_> = self
            .buffered
            .drain(..ready)
            .map(|item| item.event)
            .collect();
        if let Some(event) = events.last() {
            self.last_emitted_us = Some(event.timestamp_us);
        }
        events
    }

    fn drain_all(&mut self) -> Vec<RecordingEvent> {
        let events: Vec<_> = self.buffered.drain(..).map(|item| item.event).collect();
        if let Some(event) = events.last() {
            self.last_emitted_us = Some(event.timestamp_us);
        }
        events
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::collections::BTreeSet;
    use std::fs::File;
    use std::io::{self, BufRead as _, BufWriter, Write as _};
    use std::path::Path;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use core_foundation::runloop::CFRunLoop;
    use core_graphics::display::CGDisplay;
    use core_graphics::event::{
        CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventType, CallbackResult, EventField,
    };
    use recording::{
        CaptureMetadata, DisplayMetadata, HostPointerEvent, HostPointerEventKind, RecordingEvent,
        RecordingWriter, KIND_DEVICE_CONNECTED, KIND_DEVICE_DISCONNECTED, KIND_MARKER,
        KIND_WARNING,
    };
    use serde_json::json;
    use steam_controller_device::{enumerate, DeviceEvent, HidDeviceInfo, HidSession};
    use steam_controller_discovery::{same_controller_collection, ActiveControllerFinder};
    use steam_controller_protocol::{DecodedReport, SteamControllerDecoder};

    use crate::lizard::protocol::{guided_trials, GUIDED_PROTOCOL};
    use crate::{cli::Source, device::HidBrokerClient};

    use super::{
        CapturePreflight, EventMerger, GuiCaptureCommand, GuiCaptureEvent, QueuedEvent,
        QUEUE_CAPACITY,
    };

    enum CaptureControl {
        Terminal {
            guided: bool,
            duration_secs: Option<u64>,
        },
        Gui {
            commands: Receiver<GuiCaptureCommand>,
            events: mpsc::Sender<GuiCaptureEvent>,
        },
    }

    enum Message {
        Event(QueuedEvent),
        ProducerDone,
    }

    struct OpenedController {
        index: usize,
        info: HidDeviceInfo,
        session: HidSession,
        announce_connection: bool,
    }

    #[derive(Clone)]
    struct Shared {
        sender: SyncSender<Message>,
        sequence: Arc<AtomicU64>,
        stop: Arc<AtomicBool>,
        invalid_reason: Arc<Mutex<Option<String>>>,
        preview: Arc<Mutex<TrialPreviewBuffer>>,
        completed: Arc<AtomicBool>,
        started: Instant,
    }

    #[derive(Default)]
    struct TrialPreviewBuffer {
        active: bool,
        points: Vec<(i64, i64)>,
        current: (i64, i64),
    }

    impl Shared {
        fn elapsed_us(&self) -> u64 {
            u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX)
        }

        fn event(&self, event: RecordingEvent) {
            if self
                .invalid_reason
                .lock()
                .is_ok_and(|reason| reason.is_some())
            {
                return;
            }
            let queued = QueuedEvent {
                sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
                event,
            };
            match self.sender.try_send(Message::Event(queued)) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    self.invalidate(format!(
                        "bounded capture queue exceeded {QUEUE_CAPACITY} events"
                    ));
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.invalidate("capture writer stopped unexpectedly".to_owned());
                }
            }
        }

        fn invalidate(&self, reason: String) {
            if let Ok(mut slot) = self.invalid_reason.lock() {
                if slot.is_none() {
                    *slot = Some(reason);
                }
            }
            self.stop.store(true, Ordering::Release);
        }

        fn done(&self) {
            let _ = self.sender.send(Message::ProducerDone);
        }

        fn begin_preview(&self) {
            if let Ok(mut preview) = self.preview.lock() {
                preview.active = true;
                preview.current = (0, 0);
                preview.points.clear();
                preview.points.push((0, 0));
            }
        }

        fn push_reference_motion(&self, x: i8, y: i8) {
            if let Ok(mut preview) = self.preview.lock() {
                if preview.active && (x != 0 || y != 0) {
                    preview.current.0 += i64::from(x);
                    preview.current.1 += i64::from(y);
                    let current = preview.current;
                    preview.points.push(current);
                }
            }
        }

        fn end_preview(&self) -> Vec<(i64, i64)> {
            self.preview.lock().map_or_else(
                |_| Vec::new(),
                |mut preview| {
                    preview.active = false;
                    std::mem::take(&mut preview.points)
                },
            )
        }
    }

    pub(super) fn run(
        requested_index: Option<usize>,
        output: &Path,
        guided: bool,
        duration_secs: Option<u64>,
    ) -> Result<(), String> {
        let stop = Arc::new(AtomicBool::new(false));
        install_ctrl_c(Arc::clone(&stop))?;
        run_capture(
            requested_index,
            output,
            &stop,
            None,
            CaptureControl::Terminal {
                guided,
                duration_secs,
            },
        )
    }

    pub(super) fn run_gui(
        requested_index: Option<usize>,
        output: &Path,
        stop: &Arc<AtomicBool>,
        commands: Receiver<GuiCaptureCommand>,
        events: mpsc::Sender<GuiCaptureEvent>,
        hid_broker: &HidBrokerClient,
    ) -> Result<(), String> {
        run_capture(
            requested_index,
            output,
            stop,
            Some(hid_broker),
            CaptureControl::Gui { commands, events },
        )
    }

    #[allow(
        clippy::too_many_lines,
        reason = "capture setup and teardown stay linear so every producer is finalized in order"
    )]
    fn run_capture(
        requested_index: Option<usize>,
        output: &Path,
        stop: &Arc<AtomicBool>,
        hid_broker: Option<&HidBrokerClient>,
        control: CaptureControl,
    ) -> Result<(), String> {
        let guided = matches!(
            &control,
            CaptureControl::Terminal { guided: true, .. } | CaptureControl::Gui { .. }
        );
        let gui_mode = matches!(&control, CaptureControl::Gui { .. });
        let terminal_free = matches!(
            &control,
            CaptureControl::Terminal {
                guided: false,
                duration_secs: None
            }
        );
        let file = File::create(output)
            .map_err(|error| format!("cannot create capture '{}': {error}", output.display()))?;
        let mut writer = RecordingWriter::new(BufWriter::with_capacity(64 * 1024, file));
        let mut initial_metadata = metadata(requested_index.unwrap_or(0), None, guided);
        writer
            .write_event_buffered(
                &RecordingEvent::capture_metadata(0, &initial_metadata)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        let OpenedController {
            index,
            info,
            session,
            announce_connection,
        } = match open_controller(requested_index, stop, hid_broker) {
            Ok(opened) => opened,
            Err(error) => {
                initial_metadata.valid = Some(false);
                initial_metadata.invalid_reason = Some(error.clone());
                writer
                    .write_event(
                        &RecordingEvent::capture_metadata(0, &initial_metadata)
                            .map_err(|write_error| write_error.to_string())?,
                    )
                    .map_err(|write_error| write_error.to_string())?;
                return Err(format!(
                    "{error}; invalid capture metadata remains in '{}'",
                    output.display()
                ));
            }
        };
        initial_metadata.controller_index = index;
        initial_metadata.source_device_id.clone_from(&info.id);
        initial_metadata.transport.clone_from(&info.transport);
        writer
            .write_event_buffered(
                &RecordingEvent::capture_metadata(0, &initial_metadata)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        let started = Instant::now();

        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let invalid_reason = Arc::new(Mutex::new(None));
        let shared = Shared {
            sender,
            sequence: Arc::new(AtomicU64::new(0)),
            stop: Arc::clone(stop),
            invalid_reason: Arc::clone(&invalid_reason),
            preview: Arc::new(Mutex::new(TrialPreviewBuffer::default())),
            completed: Arc::new(AtomicBool::new(false)),
            started,
        };
        let state_count = Arc::new(AtomicUsize::new(0));
        let lizard_count = Arc::new(AtomicUsize::new(0));
        let tap_ready = Arc::new(AtomicBool::new(false));

        if announce_connection {
            shared.event(RecordingEvent::new(
                shared.elapsed_us(),
                KIND_DEVICE_CONNECTED,
                device_json(&info),
            ));
        }

        let hid = spawn_hid(
            session,
            shared.clone(),
            Arc::clone(&state_count),
            Arc::clone(&lizard_count),
        );
        let tap = spawn_event_tap(shared.clone(), Arc::clone(&tap_ready));
        eprintln!(
            "Capturing collection {index} ({}) to '{}'; lizard mode remains enabled.",
            info.transport,
            output.display()
        );
        eprintln!("Preflight: waiting up to 3 seconds for state and 0x40 reports...");
        preflight(&shared, &state_count, &lizard_count, &tap_ready);
        if let CaptureControl::Gui { events, .. } = &control {
            let _ = events.send(GuiCaptureEvent::Preflight(CapturePreflight {
                controller_index: index,
                controller: info.product.clone().unwrap_or_else(|| info.id.clone()),
                transport: info.transport.clone(),
                state_reports: state_count.load(Ordering::Acquire),
                lizard_reports: lizard_count.load(Ordering::Acquire),
                event_tap_ready: tap_ready.load(Ordering::Acquire),
                displays: initial_metadata
                    .displays
                    .iter()
                    .map(|display| {
                        format!(
                            "display {}: {:.0}×{:.0} at ({:.0}, {:.0}), {:.2}× scale",
                            display.id,
                            display.width,
                            display.height,
                            display.x,
                            display.y,
                            display.scale
                        )
                    })
                    .collect(),
                invalid_reason: invalid_reason.lock().ok().and_then(|reason| reason.clone()),
            }));
        }
        let guide = if stop.load(Ordering::Acquire) {
            None
        } else {
            match control {
                CaptureControl::Terminal { guided: true, .. } => Some(spawn_guide(shared.clone())),
                CaptureControl::Terminal {
                    guided: false,
                    duration_secs,
                } => duration_secs.map(|seconds| spawn_timer(Arc::clone(stop), seconds)),
                CaptureControl::Gui { commands, events } => {
                    Some(spawn_gui_guide(shared.clone(), commands, events))
                }
            }
        };
        if terminal_free {
            eprintln!("Press Ctrl+C to finish.");
        }
        // A write failure must not skip the stop/join teardown or the final
        // validity trailer: without `valid: false`, a truncated file would
        // later read as "legacy" instead of invalid.
        let merge_result = merge_until_done(&receiver, &shared, &mut writer);
        stop.store(true, Ordering::Release);
        hid.join()
            .map_err(|_| "HID capture thread panicked".to_owned())?;
        tap.join()
            .map_err(|_| "event-tap thread panicked".to_owned())?;
        if let Some(handle) = guide {
            handle
                .join()
                .map_err(|_| "capture control thread panicked".to_owned())?;
        }
        if let Err(error) = merge_result {
            if let Ok(mut slot) = invalid_reason.lock() {
                slot.get_or_insert(format!("capture write failed: {error}"));
            }
        }

        let states = state_count.load(Ordering::Acquire);
        let lizard = lizard_count.load(Ordering::Acquire);
        if gui_mode
            && !shared.completed.load(Ordering::Acquire)
            && invalid_reason.lock().is_ok_and(|reason| reason.is_none())
        {
            if let Ok(mut reason) = invalid_reason.lock() {
                *reason = Some("guided capture ended before every trial was accepted".to_owned());
            }
        }
        let reason = invalid_reason
            .lock()
            .map_err(|_| "capture validity lock poisoned".to_owned())?
            .clone();
        let mut final_metadata = initial_metadata;
        final_metadata.valid = Some(reason.is_none());
        final_metadata.invalid_reason.clone_from(&reason);
        writer
            .write_event(
                &RecordingEvent::capture_metadata(shared.elapsed_us(), &final_metadata)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
        println!("Capture contains {states} controller states and {lizard} lizard mouse reports.");
        if let Some(reason) = reason {
            Err(format!(
                "capture was explicitly marked invalid: {reason}; partial data remains in '{}'",
                output.display()
            ))
        } else {
            Ok(())
        }
    }

    fn spawn_gui_guide(
        shared: Shared,
        commands: Receiver<GuiCaptureCommand>,
        events: mpsc::Sender<GuiCaptureEvent>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let required: BTreeSet<_> = guided_trials().into_iter().map(|trial| trial.id).collect();
            let mut accepted = BTreeSet::new();
            while !shared.stop.load(Ordering::Acquire) {
                let command = match commands.recv_timeout(Duration::from_millis(25)) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => GuiCaptureCommand::Cancel,
                };
                match command {
                    GuiCaptureCommand::Start { trial_id, attempt } => {
                        shared.begin_preview();
                        shared.event(guided_marker(&shared, &trial_id, attempt, "start"));
                    }
                    GuiCaptureCommand::End { trial_id, attempt } => {
                        shared.event(guided_marker(&shared, &trial_id, attempt, "end"));
                        let _ = events.send(GuiCaptureEvent::TrialPreview {
                            trial_id,
                            points: shared.end_preview(),
                        });
                    }
                    GuiCaptureCommand::Accept { trial_id, attempt } => {
                        shared.event(guided_marker(&shared, &trial_id, attempt, "accepted"));
                        accepted.insert(trial_id);
                    }
                    GuiCaptureCommand::Discard { trial_id, attempt } => {
                        shared.event(guided_marker(&shared, &trial_id, attempt, "discarded"));
                    }
                    GuiCaptureCommand::Finish => {
                        if accepted == required {
                            shared.completed.store(true, Ordering::Release);
                            shared.stop.store(true, Ordering::Release);
                        } else {
                            shared.invalidate(format!(
                                "guided capture finished with {} of {} required trials accepted",
                                accepted.len(),
                                required.len()
                            ));
                        }
                    }
                    GuiCaptureCommand::Cancel => {
                        shared.invalidate("guided capture cancelled before completion".to_owned());
                    }
                }
            }
        })
    }

    fn guided_marker(shared: &Shared, trial_id: &str, attempt: u32, phase: &str) -> RecordingEvent {
        RecordingEvent::new(
            shared.elapsed_us(),
            KIND_MARKER,
            json!({
                "name": trial_id,
                "phase": phase,
                "protocol": GUIDED_PROTOCOL,
                "trial_id": trial_id,
                "attempt": attempt,
            }),
        )
    }

    fn open_controller(
        requested_index: Option<usize>,
        stop: &Arc<AtomicBool>,
        hid_broker: Option<&HidBrokerClient>,
    ) -> Result<OpenedController, String> {
        if let Some(broker) = hid_broker {
            let source = requested_index.map_or(Source::Discover, Source::Collection);
            let opened = broker.open(source, Arc::clone(stop), |status| {
                eprintln!("{status}");
            })?;
            if !opened.info.is_supported_controller_source() {
                return Err(format!(
                    "index {} is not a supported Steam Controller input collection",
                    opened.index
                ));
            }
            return Ok(OpenedController {
                index: opened.index,
                info: opened.info,
                session: opened.session,
                announce_connection: opened.announce_connection,
            });
        }
        if let Some(index) = requested_index {
            let devices = enumerate().map_err(|error| error.to_string())?;
            let info = devices
                .get(index)
                .cloned()
                .ok_or_else(|| format!("HID device index {index} does not exist"))?;
            if !info.is_supported_controller_source() {
                return Err(format!(
                    "index {index} is not a supported Steam Controller input collection"
                ));
            }
            let session = HidSession::open_info(&info).map_err(|error| error.to_string())?;
            return Ok(OpenedController {
                index,
                info,
                session,
                announce_connection: false,
            });
        }

        eprintln!(
            "Auto-detecting the active Steam Controller collection; touch a pad or press a controller button if needed."
        );
        let mut finder = ActiveControllerFinder::new().map_err(|error| error.to_string())?;
        let mut last_status = None;
        while !stop.load(Ordering::Acquire) {
            match finder.find() {
                Ok((info, session)) => {
                    let devices = enumerate().map_err(|error| error.to_string())?;
                    let index = devices
                        .iter()
                        .position(|candidate| same_controller_collection(candidate, &info))
                        .ok_or_else(|| {
                            "the auto-detected controller disappeared while resolving its global HID index"
                                .to_owned()
                        })?;
                    eprintln!("Auto-selected collection {index} ({}).", info.transport);
                    return Ok(OpenedController {
                        index,
                        info,
                        session,
                        announce_connection: true,
                    });
                }
                Err(search) => {
                    let status = search.to_string();
                    if last_status.as_deref() != Some(status.as_str()) {
                        eprintln!("{status}");
                        last_status = Some(status);
                    }
                }
            }
            thread::sleep(Duration::from_millis(250));
        }
        Err("controller auto-detection was cancelled".to_owned())
    }

    fn preflight(
        shared: &Shared,
        state_count: &AtomicUsize,
        lizard_count: &AtomicUsize,
        tap_ready: &AtomicBool,
    ) {
        let preflight_deadline = Instant::now() + Duration::from_secs(3);
        while !shared.stop.load(Ordering::Acquire)
            && Instant::now() < preflight_deadline
            && (state_count.load(Ordering::Acquire) == 0
                || lizard_count.load(Ordering::Acquire) == 0
                || !tap_ready.load(Ordering::Acquire))
        {
            thread::sleep(Duration::from_millis(25));
        }
        if state_count.load(Ordering::Acquire) == 0 {
            shared.invalidate(
                "no controller state reports were observed during preflight".to_owned(),
            );
        }
        if lizard_count.load(Ordering::Acquire) == 0 {
            shared.invalidate(
                "no 0x40 lizard mouse reports were observed during preflight; Steam or another lizard-mode heartbeat may have disabled lizard mode".to_owned(),
            );
        }
        if !tap_ready.load(Ordering::Acquire) {
            shared.invalidate(
                "passive HID event tap did not become ready during preflight".to_owned(),
            );
        }
    }

    fn install_ctrl_c(stop: Arc<AtomicBool>) -> Result<(), String> {
        ctrlc::set_handler(move || stop.store(true, Ordering::Release))
            .map_err(|error| format!("cannot install Ctrl+C handler: {error}"))
    }

    fn spawn_hid(
        mut session: HidSession,
        shared: Shared,
        state_count: Arc<AtomicUsize>,
        lizard_count: Arc<AtomicUsize>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut decoder = SteamControllerDecoder::new();
            while !shared.stop.load(Ordering::Acquire) {
                match session.poll(Duration::from_millis(50)) {
                    Ok(Some(DeviceEvent::Connected(info))) => shared.event(RecordingEvent::new(
                        shared.elapsed_us(),
                        KIND_DEVICE_CONNECTED,
                        device_json(&info),
                    )),
                    Ok(Some(DeviceEvent::Disconnected)) => {
                        shared.event(RecordingEvent::new(
                            shared.elapsed_us(),
                            KIND_DEVICE_DISCONNECTED,
                            json!({}),
                        ));
                        shared.invalidate("controller disconnected during capture".to_owned());
                    }
                    Ok(Some(DeviceEvent::Report(report))) => {
                        let timestamp_us = shared.elapsed_us();
                        match RecordingEvent::raw_hid_with_metadata(
                            timestamp_us,
                            report.report_id,
                            &report.data,
                            Some(&report.source_device_id),
                            Some(&report.transport),
                            report.dropped_reports,
                        ) {
                            Ok(event) => shared.event(event),
                            Err(error) => shared.invalidate(error.to_string()),
                        }
                        if report.dropped_reports > 0 {
                            shared.invalidate(format!(
                                "HID source reported {} dropped reports",
                                report.dropped_reports
                            ));
                        }
                        match decoder.decode(report.report_id, &report.data) {
                            Ok(DecodedReport::ControllerState(state)) => {
                                state_count.fetch_add(1, Ordering::Relaxed);
                                match RecordingEvent::decoded_steam_state(timestamp_us, &state) {
                                    Ok(event) => shared.event(event),
                                    Err(error) => shared.invalidate(error.to_string()),
                                }
                            }
                            Ok(DecodedReport::LizardMouse(mouse)) => {
                                lizard_count.fetch_add(1, Ordering::Relaxed);
                                shared.push_reference_motion(mouse.x, mouse.y);
                                match RecordingEvent::decoded_lizard_mouse(timestamp_us, &mouse) {
                                    Ok(event) => shared.event(event),
                                    Err(error) => shared.invalidate(error.to_string()),
                                }
                            }
                            Ok(_) => {}
                            Err(error) => shared.event(RecordingEvent::new(
                                timestamp_us,
                                KIND_WARNING,
                                json!({"message": error.to_string()}),
                            )),
                        }
                    }
                    Ok(None) => {}
                    Err(error) => shared.invalidate(format!("HID polling failed: {error}")),
                }
            }
            shared.done();
        })
    }

    fn spawn_event_tap(shared: Shared, ready: Arc<AtomicBool>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let callback_shared = shared.clone();
            let result = CGEventTap::with_enabled(
                CGEventTapLocation::HID,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                vec![
                    CGEventType::MouseMoved,
                    CGEventType::LeftMouseDragged,
                    CGEventType::RightMouseDragged,
                    CGEventType::OtherMouseDragged,
                    CGEventType::LeftMouseDown,
                    CGEventType::LeftMouseUp,
                    CGEventType::RightMouseDown,
                    CGEventType::RightMouseUp,
                    CGEventType::OtherMouseDown,
                    CGEventType::OtherMouseUp,
                    CGEventType::ScrollWheel,
                ],
                move |_proxy, event_type, event| {
                    if matches!(
                        event_type,
                        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
                    ) {
                        callback_shared.invalidate(format!(
                            "passive HID event tap was disabled ({event_type:?})"
                        ));
                    } else if let Some(pointer) = pointer_event(event_type, event) {
                        match RecordingEvent::host_pointer(callback_shared.elapsed_us(), &pointer) {
                            Ok(event) => callback_shared.event(event),
                            Err(error) => callback_shared.invalidate(error.to_string()),
                        }
                    }
                    CallbackResult::Keep
                },
                || {
                    ready.store(true, Ordering::Release);
                    let run_loop = CFRunLoop::get_current();
                    let stopper_loop = run_loop.clone();
                    let stopper_shared = shared.clone();
                    let stopper = thread::spawn(move || {
                        while !stopper_shared.stop.load(Ordering::Acquire) {
                            thread::sleep(Duration::from_millis(25));
                        }
                        stopper_loop.stop();
                    });
                    CFRunLoop::run_current();
                    let _ = stopper.join();
                },
            );
            if result.is_err() {
                shared.invalidate(
                    "cannot install passive HID event tap; grant Input Monitoring to the terminal or app running sc-visualizer, then relaunch it".to_owned(),
                );
            }
            shared.done();
        })
    }

    fn pointer_event(event_type: CGEventType, event: &CGEvent) -> Option<HostPointerEvent> {
        let event_kind = match event_type {
            CGEventType::MouseMoved => HostPointerEventKind::Moved,
            CGEventType::LeftMouseDragged => HostPointerEventKind::LeftDragged,
            CGEventType::RightMouseDragged => HostPointerEventKind::RightDragged,
            CGEventType::OtherMouseDragged => HostPointerEventKind::OtherDragged,
            CGEventType::LeftMouseDown => HostPointerEventKind::LeftDown,
            CGEventType::LeftMouseUp => HostPointerEventKind::LeftUp,
            CGEventType::RightMouseDown => HostPointerEventKind::RightDown,
            CGEventType::RightMouseUp => HostPointerEventKind::RightUp,
            CGEventType::OtherMouseDown => HostPointerEventKind::OtherDown,
            CGEventType::OtherMouseUp => HostPointerEventKind::OtherUp,
            CGEventType::ScrollWheel => HostPointerEventKind::Scroll,
            _ => return None,
        };
        let location = event.location();
        Some(HostPointerEvent {
            event_kind,
            delta_x: event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X),
            delta_y: event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y),
            location_x: location.x,
            location_y: location.y,
            scroll_x: event
                .get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2),
            scroll_y: event
                .get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1),
        })
    }

    fn spawn_timer(stop: Arc<AtomicBool>, seconds: u64) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(seconds);
            while !stop.load(Ordering::Acquire) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }
            stop.store(true, Ordering::Release);
        })
    }

    fn spawn_guide(shared: Shared) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let trials = guided_trials();
            eprintln!(
                "Guided capture: do not touch any other mouse or trackpad during a measured interval. Press Enter to start each trial."
            );
            let (input_sender, input_receiver) = mpsc::channel();
            thread::spawn(move || {
                let stdin = io::stdin();
                for line in stdin.lock().lines() {
                    let input = line.map_err(|error| format!("cannot read guided input: {error}"));
                    if input_sender.send(input).is_err() {
                        break;
                    }
                }
            });
            let mut completed = 0;
            for (trial_index, trial) in trials.iter().enumerate() {
                if shared.stop.load(Ordering::Acquire) {
                    break;
                }
                let mut attempt = 1_u32;
                loop {
                    eprint!(
                        "\nTrial {}/{} — {}\n{} Press Enter when ready... ",
                        trial_index + 1,
                        trials.len(),
                        trial.title,
                        trial.instruction
                    );
                    let _ = io::stderr().flush();
                    if wait_for_guided_input(&input_receiver, &shared).is_none() {
                        break;
                    }
                    eprintln!("Starting in one second...");
                    let countdown = Instant::now() + Duration::from_secs(1);
                    while !shared.stop.load(Ordering::Acquire) && Instant::now() < countdown {
                        thread::sleep(Duration::from_millis(25));
                    }
                    let marker = |phase: &str| {
                        RecordingEvent::new(
                            shared.elapsed_us(),
                            KIND_MARKER,
                            json!({
                                "name": trial.id,
                                "phase": phase,
                                "protocol": GUIDED_PROTOCOL,
                                "trial_id": trial.id,
                                "attempt": attempt,
                            }),
                        )
                    };
                    shared.event(marker("start"));
                    let deadline = Instant::now() + trial.duration;
                    while !shared.stop.load(Ordering::Acquire) && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(25));
                    }
                    if shared.stop.load(Ordering::Acquire) {
                        break;
                    }
                    shared.event(marker("end"));
                    eprint!("Accept trial with Enter, or type r then Enter to retry... ");
                    let _ = io::stderr().flush();
                    let Some(decision) = wait_for_guided_input(&input_receiver, &shared) else {
                        break;
                    };
                    if decision.trim().eq_ignore_ascii_case("r") {
                        shared.event(marker("discarded"));
                        attempt = attempt.saturating_add(1);
                    } else {
                        shared.event(marker("accepted"));
                        completed += 1;
                        break;
                    }
                }
            }
            if completed == trials.len() {
                shared.stop.store(true, Ordering::Release);
            } else if shared
                .invalid_reason
                .lock()
                .is_ok_and(|reason| reason.is_none())
            {
                shared
                    .invalidate("guided capture ended before every trial was accepted".to_owned());
            }
        })
    }

    fn wait_for_guided_input(
        receiver: &Receiver<Result<String, String>>,
        shared: &Shared,
    ) -> Option<String> {
        while !shared.stop.load(Ordering::Acquire) {
            match receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(Ok(input)) => return Some(input),
                Ok(Err(error)) => {
                    shared.invalidate(error);
                    return None;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    shared.invalidate("guided input ended before all stages".to_owned());
                    return None;
                }
            }
        }
        None
    }

    fn merge_until_done(
        receiver: &Receiver<Message>,
        shared: &Shared,
        writer: &mut RecordingWriter<BufWriter<File>>,
    ) -> Result<(), String> {
        let mut merger = EventMerger::default();
        let mut done = 0;
        while done < 2 {
            match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(Message::Event(event)) => push_or_invalidate(&mut merger, event, shared),
                Ok(Message::ProducerDone) => done += 1,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            for event in merger.drain_ready(shared.elapsed_us()) {
                writer
                    .write_event_buffered(&event)
                    .map_err(|error| error.to_string())?;
            }
        }
        while let Ok(message) = receiver.try_recv() {
            if let Message::Event(event) = message {
                push_or_invalidate(&mut merger, event, shared);
            }
        }
        for event in merger.drain_all() {
            writer
                .write_event_buffered(&event)
                .map_err(|error| error.to_string())?;
        }
        writer.flush().map_err(|error| error.to_string())
    }

    fn push_or_invalidate(merger: &mut EventMerger, event: QueuedEvent, shared: &Shared) {
        if let Err(event) = merger.push(event) {
            shared.invalidate(format!(
                "capture source delivered timestamp {} after the reorder window emitted a later event",
                event.event.timestamp_us
            ));
        }
    }

    fn metadata(index: usize, info: Option<&HidDeviceInfo>, guided: bool) -> CaptureMetadata {
        CaptureMetadata {
            tool_version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: "macos".to_owned(),
            os_version: command_output("sw_vers", &["-productVersion"]),
            os_build: command_output("sw_vers", &["-buildVersion"]),
            controller_index: index,
            source_device_id: info.map_or_else(|| "pending".to_owned(), |info| info.id.clone()),
            transport: info.map_or_else(|| "pending".to_owned(), |info| info.transport.clone()),
            capture_mode: if guided { "guided" } else { "free" }.to_owned(),
            displays: display_metadata(),
            mouse_scaling: command_output("defaults", &["read", "-g", "com.apple.mouse.scaling"])
                .parse()
                .ok(),
            valid: None,
            invalid_reason: None,
        }
    }

    fn command_output(program: &str, arguments: &[&str]) -> String {
        Command::new(program)
            .args(arguments)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default()
    }

    fn display_metadata() -> Vec<DisplayMetadata> {
        CGDisplay::active_displays()
            .unwrap_or_default()
            .into_iter()
            .map(|id| {
                let display = CGDisplay::new(id);
                let bounds = display.bounds();
                let scale = if bounds.size.width > 0.0 {
                    display.pixels_wide() as f64 / bounds.size.width
                } else {
                    1.0
                };
                DisplayMetadata {
                    id,
                    x: bounds.origin.x,
                    y: bounds.origin.y,
                    width: bounds.size.width,
                    height: bounds.size.height,
                    scale,
                }
            })
            .collect()
    }

    fn device_json(info: &HidDeviceInfo) -> serde_json::Value {
        json!({
            "id": info.id,
            "vendor_id": info.vendor_id,
            "product_id": info.product_id,
            "usage_page": info.usage_page,
            "usage": info.usage,
            "interface_number": info.interface_number,
            "transport": info.transport,
            "product": info.product,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn shared_with_capacity(capacity: usize) -> (Shared, Receiver<Message>) {
            let (sender, receiver) = mpsc::sync_channel(capacity);
            (
                Shared {
                    sender,
                    sequence: Arc::new(AtomicU64::new(0)),
                    stop: Arc::new(AtomicBool::new(false)),
                    invalid_reason: Arc::new(Mutex::new(None)),
                    preview: Arc::new(Mutex::new(TrialPreviewBuffer::default())),
                    completed: Arc::new(AtomicBool::new(false)),
                    started: Instant::now(),
                },
                receiver,
            )
        }

        #[test]
        fn queue_overflow_stops_and_invalidates_capture() {
            let (shared, _receiver) = shared_with_capacity(1);
            shared.event(RecordingEvent::new(0, "first", json!({})));
            shared.event(RecordingEvent::new(1, "overflow", json!({})));
            assert!(shared.stop.load(Ordering::Acquire));
            assert!(shared
                .invalid_reason
                .lock()
                .unwrap()
                .as_deref()
                .is_some_and(|reason| reason.contains("exceeded")));
        }

        #[test]
        fn first_fatal_capture_condition_remains_authoritative() {
            let (shared, _receiver) = shared_with_capacity(1);
            shared.invalidate("event tap disabled by timeout".to_owned());
            shared.invalidate("controller disconnected".to_owned());
            assert_eq!(
                shared.invalid_reason.lock().unwrap().as_deref(),
                Some("event tap disabled by timeout")
            );
        }

        #[test]
        fn guided_input_wait_honors_capture_cancellation() {
            let (shared, _events) = shared_with_capacity(1);
            let (_sender, receiver) = mpsc::channel::<Result<String, String>>();
            shared.stop.store(true, Ordering::Release);

            assert!(wait_for_guided_input(&receiver, &shared).is_none());
            assert!(shared.invalid_reason.lock().unwrap().is_none());
        }

        #[test]
        fn guided_input_disconnect_invalidates_capture() {
            let (shared, _events) = shared_with_capacity(1);
            let (sender, receiver) = mpsc::channel::<Result<String, String>>();
            drop(sender);

            assert!(wait_for_guided_input(&receiver, &shared).is_none());
            assert_eq!(
                shared.invalid_reason.lock().unwrap().as_deref(),
                Some("guided input ended before all stages")
            );
        }
    }
}

pub(crate) fn run(
    index: Option<usize>,
    output: &Path,
    guided: bool,
    duration_secs: Option<u64>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::run(index, output, guided, duration_secs)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (index, output, guided, duration_secs);
        Err("capture is implemented only on macOS; analyze, compare, and dump replay remain portable"
            .to_owned())
    }
}

fn run_gui(
    index: Option<usize>,
    output: &Path,
    stop: &Arc<AtomicBool>,
    commands: mpsc::Receiver<GuiCaptureCommand>,
    events: mpsc::Sender<GuiCaptureEvent>,
    hid_broker: &HidBrokerClient,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::run_gui(index, output, stop, commands, events, hid_broker)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (index, output, stop, commands, events, hid_broker);
        Err(
            "capture is implemented only on macOS; existing captures can still be analyzed"
                .to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn queued(timestamp_us: u64, sequence: u64, name: &str) -> QueuedEvent {
        QueuedEvent {
            sequence,
            event: RecordingEvent::new(timestamp_us, "test", json!({"name": name})),
        }
    }

    #[test]
    fn merger_reorders_sources_and_preserves_equal_timestamp_sequence() {
        let mut merger = EventMerger::default();
        merger.push(queued(30, 2, "late")).unwrap();
        merger.push(queued(10, 1, "early")).unwrap();
        merger.push(queued(30, 0, "equal-first")).unwrap();
        let events = merger.drain_all();
        let names: Vec<_> = events
            .iter()
            .map(|event| event.payload["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["early", "equal-first", "late"]);
    }

    #[test]
    fn reorder_window_keeps_recent_events_and_drains_old_ones() {
        let mut merger = EventMerger::default();
        merger.push(queued(1_000, 0, "old")).unwrap();
        merger.push(queued(15_000, 1, "recent")).unwrap();
        assert_eq!(merger.drain_ready(20_000).len(), 1);
        assert_eq!(merger.drain_all().len(), 1);
    }

    #[test]
    fn event_later_than_reorder_window_is_rejected_explicitly() {
        let mut merger = EventMerger::default();
        merger.push(queued(20_000, 0, "newer")).unwrap();
        assert_eq!(merger.drain_ready(30_000).len(), 1);
        assert!(merger.push(queued(19_999, 1, "too-late")).is_err());
    }
}
