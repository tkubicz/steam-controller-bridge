use super::*;
use bridge_core::BridgeConfig;
use controller_mapper::MapperConfig;

#[test]
fn supervisor_timer_reports_the_slowest_phase_after_a_stall() {
    let started = Instant::now();
    let mut timer = SupervisorIterationTimer::new_at("commands", started);
    timer.enter_at("hid_wait", started + Duration::from_millis(4));
    timer.enter_at("controller_reports", started + Duration::from_millis(62));

    let stall = timer
        .take_stall_at(started + Duration::from_millis(70))
        .unwrap();
    assert_eq!(stall.elapsed, Duration::from_millis(70));
    assert_eq!(stall.phase, "hid_wait");
    assert_eq!(stall.phase_elapsed, Duration::from_millis(58));
    assert!(timer
        .take_stall_at(started + Duration::from_millis(80))
        .is_none());
}

#[test]
fn supervisor_timer_ignores_iterations_below_the_warning_threshold() {
    let started = Instant::now();
    let mut timer = SupervisorIterationTimer::new_at("commands", started);
    timer.enter_at("hid_wait", started + Duration::from_millis(10));
    let below_threshold = SUPERVISOR_STALL_THRESHOLD.saturating_sub(Duration::from_millis(1));

    assert!(timer.take_stall_at(started + below_threshold).is_none());
}

#[test]
fn desktop_worker_mailbox_coalesces_motion_without_dropping_edges() {
    let mailbox = DesktopWorkerMailbox::default();
    let outputs = DesktopWorkerOutputs::default();
    let neutral = DesktopInputSnapshot::buttons_only(SteamButtons::default());
    let touched = |x| DesktopInputSnapshot {
        right_pad: PadSample {
            x,
            touched: true,
            ..PadSample::NEUTRAL
        },
        ..neutral
    };
    let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);

    assert_eq!(
        mailbox.publish_snapshot(&outputs, neutral, Duration::ZERO),
        DesktopSnapshotPublish::Published
    );
    assert_eq!(
        mailbox.publish_snapshot(&outputs, touched(0), Duration::from_millis(1)),
        DesktopSnapshotPublish::Published
    );
    assert_eq!(
        mailbox.publish_snapshot(&outputs, touched(10), Duration::from_millis(2)),
        DesktopSnapshotPublish::Published
    );
    assert_eq!(
        mailbox.publish_snapshot(&outputs, touched(20), Duration::from_millis(3)),
        DesktopSnapshotPublish::Published
    );
    assert_eq!(
        mailbox.publish_snapshot(
            &outputs,
            DesktopInputSnapshot {
                buttons: r4,
                ..touched(20)
            },
            Duration::from_millis(4),
        ),
        DesktopSnapshotPublish::Published
    );
    assert_eq!(
        mailbox.publish_snapshot(&outputs, touched(20), Duration::from_millis(5)),
        DesktopSnapshotPublish::Published
    );

    let messages = mailbox.take_batch(Some(Duration::ZERO));
    let snapshots = messages
        .into_iter()
        .map(|message| match message {
            DesktopWorkerMessage::Snapshot(snapshot) => snapshot,
            _ => panic!("expected only desktop snapshots"),
        })
        .collect::<Vec<_>>();
    assert_eq!(snapshots.len(), 5);
    assert_eq!(snapshots[0].snapshot, neutral);
    assert_eq!(snapshots[1].snapshot, touched(0));
    assert_eq!(snapshots[2].snapshot, touched(20));
    assert_eq!(snapshots[2].now, Duration::from_millis(3));
    assert_eq!(snapshots[3].snapshot.buttons, r4);
    assert_eq!(snapshots[4].snapshot.buttons, SteamButtons::default());
}

#[test]
fn desktop_worker_mailbox_preserves_pad_click_edges_during_motion() {
    let mailbox = DesktopWorkerMailbox::default();
    let outputs = DesktopWorkerOutputs::default();
    let touched = |x, clicked: bool| DesktopInputSnapshot {
        buttons: if clicked {
            SteamButtons(1_u32 << SteamButton::RightPadClick as u8)
        } else {
            SteamButtons::default()
        },
        right_pad: PadSample {
            x,
            touched: true,
            pressed: clicked,
            ..PadSample::NEUTRAL
        },
        ..DesktopInputSnapshot::buttons_only(SteamButtons::default())
    };

    for (index, snapshot) in [
        DesktopInputSnapshot::buttons_only(SteamButtons::default()),
        touched(0, false),
        touched(10, false),
        touched(20, true),
        touched(30, false),
        touched(40, false),
        touched(50, false),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            mailbox.publish_snapshot(&outputs, snapshot, Duration::from_millis(index as u64)),
            DesktopSnapshotPublish::Published
        );
    }

    let snapshots = mailbox
        .take_batch(Some(Duration::ZERO))
        .into_iter()
        .map(|message| match message {
            DesktopWorkerMessage::Snapshot(snapshot) => snapshot.snapshot,
            _ => panic!("expected only desktop snapshots"),
        })
        .collect::<Vec<_>>();
    // The analog-only run still coalesces (x=40 is replaced by x=50), but both
    // the click press and its release edge survive.
    assert_eq!(snapshots.len(), 6);
    assert!(snapshots[3].right_pad.pressed);
    assert_eq!(snapshots[3].right_pad.x, 20);
    assert!(!snapshots[4].right_pad.pressed);
    assert_eq!(snapshots[4].right_pad.x, 30);
    assert_eq!(snapshots[5].right_pad.x, 50);
}

#[test]
fn desktop_worker_mailbox_overflow_keeps_control_barriers_and_latest_state() {
    let mailbox = DesktopWorkerMailbox::default();
    let outputs = DesktopWorkerOutputs::default();
    let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);
    for index in 0..DESKTOP_INPUT_MAILBOX_CAPACITY {
        let buttons = if index % 2 == 0 {
            SteamButtons::default()
        } else {
            r4
        };
        assert_eq!(
            mailbox.publish_snapshot(
                &outputs,
                DesktopInputSnapshot::buttons_only(buttons),
                Duration::from_millis(index as u64),
            ),
            DesktopSnapshotPublish::Published
        );
    }
    assert!(mailbox
        .push_control(
            &outputs,
            DesktopWorkerMessage::ReplaceProfile {
                profile: None,
                ack: None,
            },
            true,
        )
        .is_ok());
    let latest = DesktopInputSnapshot::buttons_only(r4);
    assert_eq!(
        mailbox.publish_snapshot(&outputs, latest, Duration::from_secs(1)),
        DesktopSnapshotPublish::Overflowed
    );

    let mut messages = mailbox.take_batch(Some(Duration::ZERO));
    assert!(matches!(
        messages.pop_front(),
        Some(DesktopWorkerMessage::ReplaceProfile { .. })
    ));
    assert!(matches!(
        messages.pop_front(),
        Some(DesktopWorkerMessage::Overflow)
    ));
    let Some(DesktopWorkerMessage::Snapshot(snapshot)) = messages.pop_front() else {
        panic!("overflow must retain the latest desktop snapshot");
    };
    assert_eq!(snapshot.snapshot, latest);
    assert_eq!(snapshot.generation, 1);
    assert!(messages.is_empty());
}

#[test]
fn desktop_worker_control_mailbox_is_bounded_with_a_reserved_safety_slot() {
    let mailbox = DesktopWorkerMailbox::default();
    let outputs = DesktopWorkerOutputs::default();
    for _ in 0..(DESKTOP_CONTROL_MAILBOX_CAPACITY - 1) {
        assert!(mailbox
            .push_control(&outputs, DesktopWorkerMessage::Enable { ack: None }, false,)
            .is_ok());
    }
    assert!(mailbox
        .push_control(&outputs, DesktopWorkerMessage::Enable { ack: None }, false,)
        .is_err());

    let (disconnect_ack, _disconnect_receiver) = mpsc::channel();
    assert!(mailbox
        .push_control(
            &outputs,
            DesktopWorkerMessage::Disconnect(disconnect_ack),
            true,
        )
        .is_ok());
    let (shutdown_ack, _shutdown_receiver) = mpsc::channel();
    assert!(mailbox
        .push_control(&outputs, DesktopWorkerMessage::Shutdown(shutdown_ack), true,)
        .is_err());
    assert_eq!(
        mailbox.take_batch(Some(Duration::ZERO)).len(),
        DESKTOP_CONTROL_MAILBOX_CAPACITY
    );
}

#[test]
fn desktop_worker_barriers_discard_staged_pad_feedback() {
    let outputs = DesktopWorkerOutputs::default();
    outputs.publish_feedback(
        0,
        PadFeedbackRequest {
            left: Some(desktop_bindings::PadFeedbackStrength::Low),
            right: Some(desktop_bindings::PadFeedbackStrength::High),
        },
    );

    outputs.invalidate_feedback(1);
    outputs.publish_feedback(
        0,
        PadFeedbackRequest {
            left: Some(desktop_bindings::PadFeedbackStrength::Medium),
            right: None,
        },
    );

    let output = outputs.take();
    assert_eq!(output.feedback, PadFeedbackRequest::NONE);
    assert!(output.discard_pending_feedback);
    outputs.publish_feedback(
        1,
        PadFeedbackRequest {
            left: Some(desktop_bindings::PadFeedbackStrength::Medium),
            right: None,
        },
    );
    let recovered = outputs.take();
    assert_eq!(
        recovered.feedback.left,
        Some(desktop_bindings::PadFeedbackStrength::Medium)
    );
    assert!(!recovered.discard_pending_feedback);
    assert_eq!(outputs.take().feedback, PadFeedbackRequest::NONE);
}

#[test]
fn desktop_worker_mailbox_waits_indefinitely_until_work_arrives() {
    let mailbox = Arc::new(DesktopWorkerMailbox::default());
    let worker_mailbox = Arc::clone(&mailbox);
    let outputs = DesktopWorkerOutputs::default();
    let (started, started_receiver) = mpsc::channel();
    let (completed, completed_receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        started.send(()).unwrap();
        let messages = worker_mailbox.take_batch(None);
        completed.send(messages.len()).unwrap();
    });
    started_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap();

    thread::sleep(RUNTIME_POLL_INTERVAL * 3);
    assert!(matches!(
        completed_receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    assert!(mailbox
        .push_control(&outputs, DesktopWorkerMessage::Enable { ack: None }, false)
        .is_ok());
    assert_eq!(
        completed_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        1
    );
    handle.join().unwrap();
}

struct FakeDiscoverySession {
    events: std::collections::VecDeque<Result<Option<DeviceEvent>, String>>,
    timeouts: Arc<Mutex<Vec<Duration>>>,
}

impl FakeDiscoverySession {
    fn idle(timeouts: Arc<Mutex<Vec<Duration>>>) -> Self {
        Self {
            events: std::collections::VecDeque::new(),
            timeouts,
        }
    }

    fn with_report(report: RawHidReport, timeouts: Arc<Mutex<Vec<Duration>>>) -> Self {
        Self {
            events: [Ok(Some(DeviceEvent::Report(report)))].into(),
            timeouts,
        }
    }

    fn with_error(error: &str, timeouts: Arc<Mutex<Vec<Duration>>>) -> Self {
        Self {
            events: [Err(error.to_owned())].into(),
            timeouts,
        }
    }

    fn with_events(
        events: Vec<Result<Option<DeviceEvent>, String>>,
        timeouts: Arc<Mutex<Vec<Duration>>>,
    ) -> Self {
        Self {
            events: events.into(),
            timeouts,
        }
    }
}

impl ControllerProbeSession for FakeDiscoverySession {
    fn poll_for_discovery(&mut self, timeout: Duration) -> Result<Option<DeviceEvent>, String> {
        self.timeouts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(timeout);
        self.events.pop_front().unwrap_or(Ok(None))
    }
}

#[derive(Default)]
struct FakeRumbleWriter {
    fail: AtomicBool,
    writes: Mutex<Vec<(u16, u16)>>,
}

impl RumbleWriter for FakeRumbleWriter {
    fn write_rumble(&self, low_frequency: u16, high_frequency: u16) -> Result<(), String> {
        if self.fail.load(Ordering::Acquire) {
            return Err("injected rumble failure".to_owned());
        }
        self.writes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((low_frequency, high_frequency));
        Ok(())
    }
}

#[derive(Default)]
struct FakePadFeedbackWriter {
    fail: AtomicBool,
    writes: Mutex<Vec<(PadHapticSide, PadHapticGain)>>,
}

impl PadFeedbackWriter for FakePadFeedbackWriter {
    fn write_pad_feedback(&self, side: PadHapticSide, gain: PadHapticGain) -> Result<(), String> {
        if self.fail.load(Ordering::Acquire) {
            return Err("injected pad feedback failure".to_owned());
        }
        self.writes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((side, gain));
        Ok(())
    }
}

struct FakePowerOffWriter {
    results: Mutex<std::collections::VecDeque<Result<(), String>>>,
    writes: AtomicU64,
}

impl FakePowerOffWriter {
    fn new(results: impl IntoIterator<Item = Result<(), String>>) -> Self {
        Self {
            results: Mutex::new(results.into_iter().collect()),
            writes: AtomicU64::new(0),
        }
    }
}

impl PowerOffWriter for FakePowerOffWriter {
    fn write_power_off(&self) -> Result<(), String> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        self.results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or(Ok(()))
    }
}

fn serial_info(path: &str, serial: &str) -> SerialDeviceInfo {
    SerialDeviceInfo {
        path: path.to_owned(),
        vendor_id: Some(bridge_output::XIAO_USB_VENDOR_ID),
        product_id: Some(bridge_output::XIAO_USB_PRODUCT_ID),
        serial_number: Some(serial.to_owned()),
        manufacturer: Some(bridge_output::XIAO_USB_MANUFACTURER.to_owned()),
        product: Some(bridge_output::XIAO_USB_PRODUCT.to_owned()),
    }
}

fn controller_info(product_id: u16, interface_number: i32, transport: &str) -> HidDeviceInfo {
    HidDeviceInfo {
        id: format!("{transport}-{interface_number}"),
        path: format!("{transport}-{interface_number}"),
        vendor_id: steam_controller_device::PROTEUS_VENDOR_ID,
        product_id,
        usage_page: steam_controller_device::STEAM_USAGE_PAGE,
        usage: steam_controller_device::STEAM_CONTROLLER_USAGE,
        interface_number,
        serial_number: Some("redacted".to_owned()),
        manufacturer: Some("Valve Corporation".to_owned()),
        product: Some(if transport == "Bluetooth" {
            "Steam Ctrl (BT)".to_owned()
        } else {
            "Steam Controller Puck".to_owned()
        }),
        transport: transport.to_owned(),
    }
}

fn controller_state_report(source: &str) -> RawHidReport {
    let mut data = vec![0; steam_controller_protocol::INPUT_REPORT_SIZE];
    data[0] = INPUT_REPORT_ID;
    RawHidReport {
        timestamp: Duration::ZERO,
        report_id: INPUT_REPORT_ID,
        data,
        source_device_id: source.to_owned(),
        transport: "USB".to_owned(),
        dropped_reports: 0,
    }
}

/// Exactly one active source, or the runtime refuses to guess. This is what
/// stops a four-collection Puck from being opened on the wrong interface.
#[test]
fn a_unique_active_source_is_required() {
    assert_eq!(choose_unique_active(&[]), Ok(None));
    assert_eq!(choose_unique_active(&[3]), Ok(Some(3)));
    assert_eq!(choose_unique_active(&[1, 2]), Err(vec![1, 2]));
}

#[test]
fn runtime_defaults_to_zero_configuration_serial_bridge() {
    let config = RuntimeConfig::default();
    assert_eq!(config.controller, ControllerSelection::AutoActive);
    assert_eq!(config.serial, SerialSelection::AutoXiao);
    assert_eq!(config.output, OutputSelection::Serial);
    assert_eq!(config.lizard_mode, LizardMode::Suppress);
    assert_eq!(config.idle_shutdown_timeout, Some(Duration::from_mins(15)));
    assert_eq!(config.puck_dock_action, PuckDockAction::LeaveOn);
    assert!(config.binding_profile.is_none());
}

#[test]
fn runtime_timeout_updates_enforce_the_documented_minimum_and_maximum() {
    assert!(validate_idle_shutdown_timeout(None).is_ok());
    assert!(validate_idle_shutdown_timeout(Some(Duration::from_secs(59))).is_err());
    assert!(validate_idle_shutdown_timeout(Some(Duration::from_mins(1))).is_ok());
    assert!(validate_idle_shutdown_timeout(Some(Duration::from_hours(24))).is_ok());
    assert!(validate_idle_shutdown_timeout(Some(
        Duration::from_hours(24) + Duration::from_secs(1)
    ))
    .is_err());
}

#[test]
fn invalid_battery_values_remain_unknown() {
    assert_eq!(valid_battery_percent(0), Some(0));
    assert_eq!(valid_battery_percent(100), Some(100));
    assert_eq!(valid_battery_percent(101), None);
    assert_eq!(valid_battery_percent(u8::MAX), None);
}

#[test]
fn disabled_recording_does_not_construct_events() {
    let mut writer = None;
    let constructed = std::cell::Cell::new(false);
    record_lazy(&mut writer, || {
        constructed.set(true);
        Ok(RecordingEvent::new(0, recording::KIND_RAW_HID, json!({})))
    })
    .unwrap();
    assert!(!constructed.get());
}

#[test]
fn transition_mailbox_coalesces_analog_reports_but_preserves_button_edges() {
    let mailbox = TransitionReportMailbox::default();
    let dropped = AtomicU64::new(0);
    let report = |sequence: u8, buttons: u32| {
        let mut data = vec![0; steam_controller_protocol::INPUT_REPORT_SIZE];
        data[0] = INPUT_REPORT_ID;
        data[1] = sequence;
        data[2..6].copy_from_slice(&buttons.to_le_bytes());
        RawHidReport {
            timestamp: Duration::ZERO,
            report_id: INPUT_REPORT_ID,
            data,
            source_device_id: "mailbox".to_owned(),
            transport: "USB".to_owned(),
            dropped_reports: 0,
        }
    };
    let r4 = 1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8;
    assert!(mailbox.publish(report(1, 0), &dropped));
    assert!(!mailbox.publish(report(2, 0), &dropped));
    assert!(!mailbox.publish(report(6, 0), &dropped));
    assert!(!mailbox.publish(report(3, r4), &dropped));
    assert!(!mailbox.publish(report(4, r4), &dropped));
    assert!(!mailbox.publish(report(7, r4), &dropped));
    assert!(!mailbox.publish(report(5, 0), &dropped));
    let batch = mailbox.take_all();
    assert!(!batch.overflowed);
    assert_eq!(
        batch
            .reports
            .iter()
            .map(|report| report.data[1])
            .collect::<Vec<_>>(),
        vec![1, 6, 3, 7, 5]
    );
    assert_eq!(dropped.load(Ordering::Relaxed), 2);
}

#[test]
fn transition_mailbox_preserves_pad_click_edges_between_analog_reports() {
    let mailbox = TransitionReportMailbox::default();
    let dropped = AtomicU64::new(0);
    let report = |sequence: u8, buttons: u32, x: i16| {
        let mut data = vec![0; INPUT_REPORT_SIZE];
        data[0] = INPUT_REPORT_ID;
        data[1] = sequence;
        data[2..6].copy_from_slice(&buttons.to_le_bytes());
        data[24..26].copy_from_slice(&x.to_le_bytes());
        RawHidReport {
            timestamp: Duration::ZERO,
            report_id: INPUT_REPORT_ID,
            data,
            source_device_id: "mailbox".to_owned(),
            transport: "USB".to_owned(),
            dropped_reports: 0,
        }
    };
    let touch = 1_u32 << SteamButton::RightPadTouch as u8;
    let right_click = 1_u32 << SteamButton::RightPadClick as u8;
    let left_click = 1_u32 << SteamButton::LeftPadClick as u8;

    assert!(mailbox.publish(report(1, touch, 0), &dropped));
    assert!(!mailbox.publish(report(2, touch, 100), &dropped));
    assert!(!mailbox.publish(report(3, touch | right_click, 150), &dropped));
    assert!(!mailbox.publish(report(4, touch, 200), &dropped));
    assert!(!mailbox.publish(report(5, touch, 250), &dropped));
    assert!(!mailbox.publish(report(6, touch, 300), &dropped));
    assert!(!mailbox.publish(report(7, touch | left_click, 300), &dropped));
    assert!(!mailbox.publish(report(8, touch, 300), &dropped));

    let batch = mailbox.take_all();
    assert!(!batch.overflowed);
    // Both click edges survive; only the analog-only report 5 is coalesced.
    assert_eq!(
        batch
            .reports
            .iter()
            .map(|report| report.data[1])
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 6, 7, 8]
    );
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
}

#[test]
fn transition_mailbox_preserves_pad_touch_baseline_and_latest_coordinates() {
    let mailbox = TransitionReportMailbox::default();
    let dropped = AtomicU64::new(0);
    let report = |sequence: u8, touched: bool, x: i16| {
        let mut data = vec![0; INPUT_REPORT_SIZE];
        data[0] = INPUT_REPORT_ID;
        data[1] = sequence;
        let buttons = if touched {
            1_u32 << SteamButton::RightPadTouch as u8
        } else {
            0
        };
        data[2..6].copy_from_slice(&buttons.to_le_bytes());
        data[24..26].copy_from_slice(&x.to_le_bytes());
        RawHidReport {
            timestamp: Duration::ZERO,
            report_id: INPUT_REPORT_ID,
            data,
            source_device_id: "mailbox".to_owned(),
            transport: "USB".to_owned(),
            dropped_reports: 0,
        }
    };
    assert!(mailbox.publish(report(1, false, 0), &dropped));
    assert!(!mailbox.publish(report(2, true, 100), &dropped));
    assert!(!mailbox.publish(report(3, true, 200), &dropped));
    assert!(!mailbox.publish(report(4, true, 300), &dropped));
    assert!(!mailbox.publish(report(5, false, 0), &dropped));
    let batch = mailbox.take_all();
    assert_eq!(
        batch
            .reports
            .iter()
            .map(|report| report.data[1])
            .collect::<Vec<_>>(),
        vec![1, 2, 4, 5]
    );
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
}

#[test]
fn transition_mailbox_overflow_retains_newest_as_recovery_baseline() {
    let mailbox = TransitionReportMailbox::default();
    let dropped = AtomicU64::new(0);
    let capacity = u8::try_from(INPUT_MAILBOX_CAPACITY).unwrap();
    for sequence in 0..=capacity {
        let mut data = vec![0; steam_controller_protocol::INPUT_REPORT_SIZE];
        data[0] = INPUT_REPORT_ID;
        data[1] = sequence;
        let buttons = if sequence % 2 == 0 {
            0
        } else {
            1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8
        };
        data[2..6].copy_from_slice(&buttons.to_le_bytes());
        let _ = mailbox.publish(
            RawHidReport {
                timestamp: Duration::ZERO,
                report_id: INPUT_REPORT_ID,
                data,
                source_device_id: "mailbox".to_owned(),
                transport: "USB".to_owned(),
                dropped_reports: 0,
            },
            &dropped,
        );
    }
    let batch = mailbox.take_all();
    assert!(batch.overflowed);
    assert_eq!(batch.reports.len(), 1);
    assert_eq!(batch.reports[0].data[1], capacity);
    assert_eq!(
        dropped.load(Ordering::Relaxed),
        INPUT_MAILBOX_CAPACITY as u64
    );
}

#[derive(Clone)]
struct SharedDesktopSink(Arc<Mutex<Vec<String>>>);

impl DesktopInputSink for SharedDesktopSink {
    fn key(&mut self, key: desktop_bindings::KeyboardKey, pressed: bool) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .push(format!("key:{key:?}:{pressed}"));
        Ok(())
    }

    fn modifier(
        &mut self,
        modifier: desktop_bindings::Modifier,
        pressed: bool,
    ) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .push(format!("modifier:{modifier:?}:{pressed}"));
        Ok(())
    }

    fn mouse_button(
        &mut self,
        button: desktop_bindings::MouseButton,
        pressed: bool,
    ) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .push(format!("mouse:{button:?}:{pressed}"));
        Ok(())
    }

    fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("move:{x}:{y}"));
        Ok(())
    }

    fn scroll(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("scroll:{x}:{y}"));
        Ok(())
    }
}

struct DropTrackedDesktopSink {
    inner: SharedDesktopSink,
    drops: Arc<AtomicU64>,
}

impl DropTrackedDesktopSink {
    fn new(events: Arc<Mutex<Vec<String>>>, drops: Arc<AtomicU64>) -> Self {
        Self {
            inner: SharedDesktopSink(events),
            drops,
        }
    }
}

impl Drop for DropTrackedDesktopSink {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

impl DesktopInputSink for DropTrackedDesktopSink {
    fn key(&mut self, key: desktop_bindings::KeyboardKey, pressed: bool) -> Result<(), String> {
        self.inner.key(key, pressed)
    }

    fn modifier(
        &mut self,
        modifier: desktop_bindings::Modifier,
        pressed: bool,
    ) -> Result<(), String> {
        self.inner.modifier(modifier, pressed)
    }

    fn mouse_button(
        &mut self,
        button: desktop_bindings::MouseButton,
        pressed: bool,
    ) -> Result<(), String> {
        self.inner.mouse_button(button, pressed)
    }

    fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.inner.mouse_move(x, y)
    }

    fn scroll(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.inner.scroll(x, y)
    }
}

struct BlockingMotionSink {
    inner: SharedDesktopSink,
    entered: Option<mpsc::Sender<()>>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl DesktopInputSink for BlockingMotionSink {
    fn key(&mut self, key: desktop_bindings::KeyboardKey, pressed: bool) -> Result<(), String> {
        self.inner.key(key, pressed)
    }

    fn modifier(
        &mut self,
        modifier: desktop_bindings::Modifier,
        pressed: bool,
    ) -> Result<(), String> {
        self.inner.modifier(modifier, pressed)
    }

    fn mouse_button(
        &mut self,
        button: desktop_bindings::MouseButton,
        pressed: bool,
    ) -> Result<(), String> {
        self.inner.mouse_button(button, pressed)
    }

    fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.inner.mouse_move(x, y)?;
        if let Some(entered) = self.entered.take() {
            let _ = entered.send(());
            let (released, wake) = &*self.gate;
            let mut released = released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while !*released {
                released = wake
                    .wait(released)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
        Ok(())
    }

    fn scroll(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.inner.scroll(x, y)
    }
}

struct LatencyProbeSink {
    starts: Arc<Mutex<VecDeque<Instant>>>,
    samples: SyncSender<Duration>,
}

impl DesktopInputSink for LatencyProbeSink {
    fn key(&mut self, _key: desktop_bindings::KeyboardKey, _pressed: bool) -> Result<(), String> {
        let started = self
            .starts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .ok_or_else(|| "latency probe received an unexpected key event".to_owned())?;
        self.samples
            .send(started.elapsed())
            .map_err(|_| "latency probe receiver disconnected".to_owned())
    }

    fn modifier(
        &mut self,
        _modifier: desktop_bindings::Modifier,
        _pressed: bool,
    ) -> Result<(), String> {
        Ok(())
    }

    fn mouse_button(
        &mut self,
        _button: desktop_bindings::MouseButton,
        _pressed: bool,
    ) -> Result<(), String> {
        Ok(())
    }

    fn mouse_move(&mut self, _x: i32, _y: i32) -> Result<(), String> {
        Ok(())
    }

    fn scroll(&mut self, _x: i32, _y: i32) -> Result<(), String> {
        Ok(())
    }
}

struct FailingMotionSink;

impl DesktopInputSink for FailingMotionSink {
    fn key(&mut self, _key: desktop_bindings::KeyboardKey, _pressed: bool) -> Result<(), String> {
        Ok(())
    }

    fn modifier(
        &mut self,
        _modifier: desktop_bindings::Modifier,
        _pressed: bool,
    ) -> Result<(), String> {
        Ok(())
    }

    fn mouse_button(
        &mut self,
        _button: desktop_bindings::MouseButton,
        _pressed: bool,
    ) -> Result<(), String> {
        Ok(())
    }

    fn mouse_move(&mut self, _x: i32, _y: i32) -> Result<(), String> {
        Err("desktop permission revoked".to_owned())
    }

    fn scroll(&mut self, _x: i32, _y: i32) -> Result<(), String> {
        Err("desktop permission revoked".to_owned())
    }
}

fn desktop_snapshot(buttons: steam_controller_protocol::SteamButtons) -> DesktopInputSnapshot {
    DesktopInputSnapshot::buttons_only(buttons)
}

