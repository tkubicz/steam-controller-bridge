//! Talking to the device: the polling worker, the event drain, and the
//! recording writes that hang off it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bridge_output::GamepadOutput;
use bridge_runtime::ActiveControllerFinder;
use controller_mapper::ControllerMapper;
use gamepad_state::GamepadState;
use recording::{RecordingEvent, KIND_DEVICE_CONNECTED, KIND_DEVICE_DISCONNECTED};
use serde_json::json;
use steam_controller_device::{DeviceEvent, HidDeviceInfo, HidSession, RawHidReport};
use steam_controller_protocol::DecodedReport;

use crate::cli::Source;
use crate::mailbox::{InputEvent, InputMailbox};
use crate::{OutputChoice, Visualizer, FRAME_TIME};

/// The worker's end of the connection, plus the flag that stops it.
pub(crate) struct InputChannel {
    pub(crate) mailbox: Arc<InputMailbox>,
    stop: Arc<AtomicBool>,
}

impl Drop for InputChannel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
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
fn open(finder: &mut Option<ActiveControllerFinder>, source: Source) -> Result<Opened, String> {
    match source {
        Source::Collection(index) => HidSession::open_index(index)
            .map(|session| Opened {
                session,
                announce: None,
            })
            .map_err(|error| error.to_string()),
        Source::Discover => {
            let finder = match finder {
                Some(finder) => finder,
                none => {
                    none.insert(ActiveControllerFinder::new().map_err(|error| error.to_string())?)
                }
            };
            finder
                .find()
                .map(|(info, session)| Opened {
                    session,
                    announce: Some(info),
                })
                .map_err(|search| search.to_string())
        }
        // A demo run never reaches the worker.
        Source::Demo(_) => Err("demo runs open no device".to_owned()),
    }
}

pub(crate) fn input_worker(source: Source) -> InputChannel {
    let mailbox = Arc::new(InputMailbox::default());
    let stop = Arc::new(AtomicBool::new(false));
    let worker_mailbox = Arc::clone(&mailbox);
    let worker_stop = Arc::clone(&stop);
    thread::spawn(move || {
        // Built once and reused across scans; it owns the shared HID context.
        let mut finder = None;
        while !worker_stop.load(Ordering::Relaxed) {
            let Opened {
                mut session,
                announce,
            } = match open(&mut finder, source) {
                Ok(opened) => opened,
                Err(error) => {
                    worker_mailbox.publish(InputEvent::Lifecycle(Box::new(Err(error))));
                    // An explicit `--index` is a claim about a specific
                    // collection, so a failure there is final. Discovery is a
                    // standing request, so it keeps trying.
                    if matches!(source, Source::Collection(_)) {
                        return;
                    }
                    thread::sleep(DISCOVERY_INTERVAL);
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
                        worker_mailbox.publish(InputEvent::Report(report));
                    }
                    Ok(Some(DeviceEvent::Disconnected)) => {
                        worker_mailbox.publish(InputEvent::Lifecycle(Box::new(Ok(
                            DeviceEvent::Disconnected,
                        ))));
                        // Drop the session and go back to discovery, so a
                        // replugged controller is picked up on its own.
                        break;
                    }
                    Ok(Some(other)) => {
                        worker_mailbox.publish(InputEvent::Lifecycle(Box::new(Ok(other))));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        worker_mailbox.publish(InputEvent::Lifecycle(Box::new(Err(error))));
                    }
                }
            }
        }
    });
    InputChannel { mailbox, stop }
}

impl Visualizer {
    pub(crate) fn timestamp_us(&self) -> u64 {
        self.recording_started
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    pub(crate) fn record(&mut self, event: &RecordingEvent) {
        if let Some(writer) = &mut self.recording {
            if let Err(error) = writer.write_event(event) {
                self.status = format!("Recording stopped: {error}");
                self.recording = None;
            }
        }
    }

    pub(crate) fn process_events(&mut self) {
        let Some(input) = &self.input else {
            return;
        };
        let (batch, overflowed) = input.mailbox.take_all();
        if overflowed {
            "Input backlog overflowed; recovered from the newest report"
                .clone_into(&mut self.status);
        }
        for queued in batch {
            let event = match queued {
                InputEvent::Report(report) => Ok(DeviceEvent::Report(report)),
                InputEvent::Lifecycle(lifecycle) => *lifecycle,
            };
            match event {
                Err(error) => self.status = error,
                Ok(DeviceEvent::Connected(info)) => {
                    self.connected = true;
                    self.device = info.product.clone().unwrap_or(info.id.clone());
                    self.status = format!("Connected via {}", info.transport);
                    let event = RecordingEvent::new(
                        self.timestamp_us(),
                        KIND_DEVICE_CONNECTED,
                        json!({"id": info.id, "transport": info.transport}),
                    );
                    self.record(&event);
                }
                Ok(DeviceEvent::Disconnected) => {
                    self.connected = false;
                    self.source = None;
                    self.mapped = GamepadState::neutral();
                    self.mapper.reset();
                    "Disconnected; waiting for reconnect".clone_into(&mut self.status);
                    self.record(&RecordingEvent::new(
                        self.timestamp_us(),
                        KIND_DEVICE_DISCONNECTED,
                        json!({}),
                    ));
                }
                Ok(DeviceEvent::Report(report)) => self.handle_report(&report),
            }
        }

        // Once per drain, not per report: this is a one-second average.
        let elapsed = self.rate_started.elapsed();
        if elapsed >= Duration::from_secs(1) {
            self.report_hz = f32::from(self.rate_count) / elapsed.as_secs_f32();
            self.rate_count = 0;
            self.rate_started = Instant::now();
        }
    }

    fn handle_report(&mut self, report: &RawHidReport) {
        self.report_count += 1;
        self.rate_count = self.rate_count.saturating_add(1);
        self.source_report_drops = self
            .source_report_drops
            .saturating_add(report.dropped_reports);
        self.raw.clone_from(&report.data);
        self.raw_report_id = Some(report.report_id);
        let timestamp = self.timestamp_us();
        if let Ok(event) = RecordingEvent::raw_hid_with_metadata(
            timestamp,
            report.report_id,
            &report.data,
            Some(&report.source_device_id),
            Some(&report.transport),
            report.dropped_reports,
        ) {
            self.record(&event);
        }
        match self.decoder.decode(report.report_id, &report.data) {
            Ok(DecodedReport::ControllerState(state)) => {
                self.mapped = self.mapper.map(&state, FRAME_TIME);
                if let Ok(event) = RecordingEvent::decoded_steam_state(timestamp, &state) {
                    self.record(&event);
                }
                if let Ok(event) = RecordingEvent::mapped_gamepad_state(timestamp, &self.mapped) {
                    self.record(&event);
                }
                self.source = Some(state);
                self.publish_mapped();
            }
            Ok(_) => {}
            Err(error) => {
                self.decode_failures += 1;
                self.status = format!("Decode error: {error}");
            }
        }
    }

    /// Hands the freshly mapped state to whichever output backend is selected.
    fn publish_mapped(&mut self) {
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
                        Ok(()) => self.serial_metrics = Some(serial.metrics()),
                        Err(error) => self.status = error.to_string(),
                    }
                }
            }
        }
    }

    pub(crate) fn rebuild_mapper(&mut self) {
        self.config.smoothing_time_constant = self
            .smoothing_enabled
            .then_some(self.smoothing_time_constant);
        match ControllerMapper::new(self.config) {
            Ok(mapper) => self.mapper = mapper,
            Err(error) => self.status = error.to_string(),
        }
    }
}
