#[test]
fn blocked_desktop_sink_does_not_block_the_supervisor_facing_publisher() {
    let mut profile = BindingProfile::default();
    profile.pads.right_mouse.enabled = true;
    let status = Arc::new(Mutex::new(BridgeStatus::default()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let worker_events = Arc::clone(&events);
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_gate = Arc::clone(&gate);
    let (entered, entered_receiver) = mpsc::channel();
    let worker_profile = profile.clone();
    let mut worker = DesktopBindingsWorker::spawn_with_runtime(Arc::clone(&status), move || {
        DesktopBindingsRuntime::with_sink(
            worker_profile,
            Box::new(BlockingMotionSink {
                inner: SharedDesktopSink(worker_events),
                entered: Some(entered),
                gate: worker_gate,
            }),
        )
    });
    let right_pad = |x| DesktopInputSnapshot {
        right_pad: PadSample {
            x,
            touched: true,
            ..PadSample::NEUTRAL
        },
        ..DesktopInputSnapshot::buttons_only(SteamButtons::default())
    };
    worker.observe(DesktopInputSnapshot::buttons_only(SteamButtons::default()));
    worker.observe(right_pad(0));
    worker.observe(right_pad(3_200));
    entered_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap();

    let (ack, ack_receiver) = mpsc::channel();
    let publish_started = Instant::now();
    for x in 3_201..=3_456 {
        worker.observe(right_pad(x));
    }
    worker.replace_profile(Some(profile), ack);
    let publish_elapsed = publish_started.elapsed();
    assert!(matches!(
        ack_receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    let (released, wake) = &*gate;
    *released
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    wake.notify_all();
    let command_result = ack_receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    let shutdown_result = worker.shutdown();

    assert!(
        publish_elapsed < SUPERVISOR_STALL_THRESHOLD,
        "desktop publisher took {publish_elapsed:?} while the sink was blocked"
    );
    command_result.unwrap();
    shutdown_result.unwrap();
    assert!(!events.lock().unwrap().is_empty());
}

#[test]
fn blocked_desktop_sink_cannot_defeat_the_shutdown_timeout() {
    let mut profile = BindingProfile::default();
    profile.pads.right_mouse.enabled = true;
    let status = Arc::new(Mutex::new(BridgeStatus::default()));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_gate = Arc::clone(&gate);
    let (entered, entered_receiver) = mpsc::channel();
    let mut worker = DesktopBindingsWorker::spawn_with_runtime(Arc::clone(&status), move || {
        DesktopBindingsRuntime::with_sink(
            profile,
            Box::new(BlockingMotionSink {
                inner: SharedDesktopSink(Arc::new(Mutex::new(Vec::new()))),
                entered: Some(entered),
                gate: worker_gate,
            }),
        )
    });
    let alive = Arc::clone(&worker.alive);
    let right_pad = |x| DesktopInputSnapshot {
        right_pad: PadSample {
            x,
            touched: true,
            ..PadSample::NEUTRAL
        },
        ..DesktopInputSnapshot::buttons_only(SteamButtons::default())
    };
    worker.observe(DesktopInputSnapshot::buttons_only(SteamButtons::default()));
    worker.observe(right_pad(0));
    worker.observe(right_pad(3_200));
    entered_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap();

    let started = Instant::now();
    assert_eq!(
        worker.shutdown_with_timeout(Duration::from_millis(20)),
        Err("desktop-input worker shutdown timed out".to_owned())
    );
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "the timeout was followed by a blocking join"
    );

    let (released, wake) = &*gate;
    *released
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    wake.notify_all();
    let deadline = Instant::now() + Duration::from_secs(1);
    while alive.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::yield_now();
    }
    assert!(!alive.load(Ordering::Acquire));
}

#[test]
fn desktop_worker_input_latency_stays_within_budget() {
    const SAMPLE_COUNT: usize = 64;

    fn latency_profile() -> BindingProfile {
        let mut profile = BindingProfile::default();
        profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
            key: desktop_bindings::KeyboardKey::F5,
            modifiers: std::collections::BTreeSet::new(),
        });
        profile
    }

    fn percentile(samples: &mut [Duration], percentile: usize) -> Duration {
        samples.sort_unstable();
        samples[(samples.len() - 1) * percentile / 100]
    }

    fn push_start(starts: &Mutex<VecDeque<Instant>>) {
        starts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(Instant::now());
    }

    let direct_starts = Arc::new(Mutex::new(VecDeque::new()));
    let (direct_sender, direct_receiver) = mpsc::sync_channel(1);
    let mut direct = DesktopBindingsRuntime::with_sink(
        latency_profile(),
        Box::new(LatencyProbeSink {
            starts: Arc::clone(&direct_starts),
            samples: direct_sender,
        }),
    );
    let neutral = SteamButtons::default();
    let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);
    let _ = direct.observe(desktop_snapshot(neutral), Duration::ZERO);
    let mut direct_samples = Vec::with_capacity(SAMPLE_COUNT);
    for index in 0..SAMPLE_COUNT {
        push_start(&direct_starts);
        let buttons = if index % 2 == 0 { r4 } else { neutral };
        let _ = direct.observe(
            desktop_snapshot(buttons),
            Duration::from_micros(u64::try_from(index + 1).unwrap()),
        );
        direct_samples.push(direct_receiver.recv().unwrap());
    }

    let status = Arc::new(Mutex::new(BridgeStatus::default()));
    let worker_starts = Arc::new(Mutex::new(VecDeque::new()));
    let worker_sink_starts = Arc::clone(&worker_starts);
    let (worker_sender, worker_receiver) = mpsc::sync_channel(1);
    let mut worker = DesktopBindingsWorker::spawn_with_runtime(Arc::clone(&status), move || {
        DesktopBindingsRuntime::with_sink(
            latency_profile(),
            Box::new(LatencyProbeSink {
                starts: worker_sink_starts,
                samples: worker_sender,
            }),
        )
    });
    worker.observe(desktop_snapshot(neutral));
    let (barrier, barrier_receiver) = mpsc::channel();
    worker.replace_profile(Some(latency_profile()), barrier);
    barrier_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();

    let mut worker_samples = Vec::with_capacity(SAMPLE_COUNT);
    for index in 0..SAMPLE_COUNT {
        // Give the worker time to return to its Condvar wait so this covers
        // the wake-up path instead of measuring only an already-running loop.
        thread::sleep(Duration::from_millis(1));
        push_start(&worker_starts);
        let buttons = if index % 2 == 0 { r4 } else { neutral };
        worker.observe(desktop_snapshot(buttons));
        worker_samples.push(
            worker_receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
        );
    }
    worker.shutdown().unwrap();

    let direct_p50 = percentile(&mut direct_samples, 50);
    let worker_p50 = percentile(&mut worker_samples, 50);
    let worker_p95 = percentile(&mut worker_samples, 95);
    let worker_max = *worker_samples.last().unwrap();
    eprintln!(
            "desktop worker input latency: direct_p50_us={} worker_p50_us={} worker_p95_us={} worker_max_us={}",
            direct_p50.as_micros(),
            worker_p50.as_micros(),
            worker_p95.as_micros(),
            worker_max.as_micros()
        );

    assert!(
            worker_p95 < RUNTIME_POLL_INTERVAL,
            "desktop worker p95 input latency {worker_p95:?} exceeded the {RUNTIME_POLL_INTERVAL:?} runtime tick; direct p50 was {direct_p50:?} and worker p50 was {worker_p50:?}"
        );
    assert!(
            worker_max < SUPERVISOR_STALL_THRESHOLD,
            "desktop worker max input latency {worker_max:?} exceeded the {SUPERVISOR_STALL_THRESHOLD:?} stall threshold"
        );
}

#[test]
fn desktop_worker_disconnect_acknowledges_held_output_release() {
    let mut profile = BindingProfile::default();
    profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F5,
        modifiers: std::collections::BTreeSet::new(),
    });
    let status = Arc::new(Mutex::new(BridgeStatus::default()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let worker_events = Arc::clone(&events);
    let worker_profile = profile.clone();
    let mut worker = DesktopBindingsWorker::spawn_with_runtime(Arc::clone(&status), move || {
        DesktopBindingsRuntime::with_sink(
            worker_profile,
            Box::new(SharedDesktopSink(worker_events)),
        )
    });
    let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);
    worker.observe(desktop_snapshot(SteamButtons::default()));
    worker.observe(desktop_snapshot(r4));

    // An identical replacement is an ordered no-op and therefore a useful
    // barrier proving that both snapshots were processed first.
    let (ack, receiver) = mpsc::channel::<Result<(), String>>();
    worker.replace_profile(Some(profile), ack);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(*events.lock().unwrap(), ["key:F5:true".to_owned()]);
    assert_eq!(worker.status().held_output_count, 1);

    worker.disconnect().unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
    );
    assert_eq!(worker.status().held_output_count, 0);
    worker.shutdown().unwrap();
}

#[test]
fn desktop_worker_overflow_releases_and_rebaselines_before_recovery() {
    let mut profile = BindingProfile::default();
    profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F5,
        modifiers: std::collections::BTreeSet::new(),
    });
    let status = Arc::new(Mutex::new(BridgeStatus::default()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let worker_events = Arc::clone(&events);
    let worker_profile = profile.clone();
    let mut worker = DesktopBindingsWorker::spawn_with_runtime(Arc::clone(&status), move || {
        DesktopBindingsRuntime::with_sink(
            worker_profile,
            Box::new(SharedDesktopSink(worker_events)),
        )
    });
    let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);
    worker.observe(desktop_snapshot(SteamButtons::default()));
    worker.observe(desktop_snapshot(r4));
    let (first_ack, first_receiver) = mpsc::channel();
    worker.replace_profile(Some(profile.clone()), first_ack);
    first_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();

    worker.overflow();
    worker.observe(desktop_snapshot(r4));
    worker.observe(desktop_snapshot(SteamButtons::default()));
    worker.observe(desktop_snapshot(r4));
    let (second_ack, second_receiver) = mpsc::channel();
    worker.replace_profile(Some(profile), second_ack);
    second_receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();

    assert_eq!(
        *events.lock().unwrap(),
        [
            "key:F5:true".to_owned(),
            "key:F5:false".to_owned(),
            "key:F5:true".to_owned(),
        ]
    );
    assert_eq!(worker.status().held_output_count, 1);
    worker.shutdown().unwrap();
    assert_eq!(events.lock().unwrap().last().unwrap(), "key:F5:false");
}

/// Replays the glue `run_active` puts between a report and the profile
/// wheel, so these tests cover the wiring and not just the state machine.
struct PickerHarness {
    picker: PickerRuntime,
    bindings: DesktopBindingsRuntime,
    engine: BridgeEngine,
    output: MockOutput,
    idle: IdleActivityTracker,
    started: Instant,
    events: Vec<PickerEvent>,
}

impl PickerHarness {
    fn new(profile: BindingProfile, sink: Box<dyn DesktopInputSink>) -> Self {
        let mut engine =
            BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
        engine.connected();
        Self {
            picker: PickerRuntime::new(Some(PickerConfig::default())),
            bindings: DesktopBindingsRuntime::with_sink(profile, sink),
            engine,
            output: MockOutput::default(),
            idle: IdleActivityTracker::new(None),
            started: Instant::now(),
            events: Vec::new(),
        }
    }

    fn feed(&mut self, now: Duration, report: &RawHidReport, roster: PickerRoster) {
        let effect = process_report(
            report,
            &mut self.engine,
            &mut self.output,
            &mut None,
            self.started,
            &mut self.idle,
        )
        .unwrap();
        let ReportEffect::ControllerState {
            desktop_input,
            picker_input,
            ..
        } = effect
        else {
            return;
        };
        let events = self.picker.observe(now, &picker_input, roster);
        let tapped = events
            .iter()
            .any(|event| matches!(event, PickerEvent::TriggerTapped));
        self.events.extend(events);
        // Every report, matching `run_active`: what is suppressed keeps
        // changing after the wheel has already closed, while the button that
        // closed it is still held.
        self.engine
            .set_output_suppression(self.picker.suppression());
        if tapped {
            let _ = self.bindings.observe(
                DesktopInputSnapshot {
                    buttons: profile_picker::with_trigger(desktop_input.buttons),
                    ..desktop_input
                },
                now,
            );
        }
        let _ = self.bindings.observe(
            DesktopInputSnapshot {
                buttons: self.picker.mask_trigger(desktop_input.buttons),
                ..desktop_input
            },
            now,
        );
    }
}

fn quick_access_profile() -> BindingProfile {
    let mut profile = BindingProfile::default();
    profile.bindings.quick_access = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F5,
        modifiers: std::collections::BTreeSet::new(),
    });
    profile
}

fn picker_report(sequence: u8, buttons: &[SteamButton], right_stick: (i16, i16)) -> RawHidReport {
    let mut data = vec![0; INPUT_REPORT_SIZE];
    data[0] = INPUT_REPORT_ID;
    data[1] = sequence;
    let mask = buttons
        .iter()
        .fold(0_u32, |mask, button| mask | 1 << *button as u8);
    data[2..6].copy_from_slice(&mask.to_le_bytes());
    data[14..16].copy_from_slice(&right_stick.0.to_le_bytes());
    data[16..18].copy_from_slice(&right_stick.1.to_le_bytes());
    RawHidReport {
        timestamp: Duration::ZERO,
        report_id: INPUT_REPORT_ID,
        data,
        source_device_id: "picker-test".to_owned(),
        transport: "USB".to_owned(),
        dropped_reports: 0,
    }
}

const TEST_ROSTER: PickerRoster = PickerRoster {
    len: 4,
    active: Some(0),
    revision: 0,
};

#[derive(Clone, Default)]
struct SharedOutput(Arc<Mutex<Vec<gamepad_state::GamepadState>>>);

impl GamepadOutput for SharedOutput {
    fn send_state(
        &mut self,
        state: &gamepad_state::GamepadState,
    ) -> Result<(), bridge_output::OutputError> {
        self.0.lock().unwrap().push(*state);
        Ok(())
    }
}

struct DropOrderOutput(Arc<Mutex<Vec<&'static str>>>);

impl Drop for DropOrderOutput {
    fn drop(&mut self) {
        self.0.lock().unwrap().push("output");
    }
}

impl GamepadOutput for DropOrderOutput {
    fn send_state(
        &mut self,
        _state: &gamepad_state::GamepadState,
    ) -> Result<(), bridge_output::OutputError> {
        Ok(())
    }
}

struct PendingReceiptOutput {
    firmware: FirmwareInfo,
    recorded: Arc<Mutex<Vec<(u32, FirmwareInstallReceipt)>>>,
    pending_response: Option<(u32, FirmwareInstallReceipt)>,
}

impl GamepadOutput for PendingReceiptOutput {
    fn send_state(
        &mut self,
        _state: &gamepad_state::GamepadState,
    ) -> Result<(), bridge_output::OutputError> {
        Ok(())
    }

    fn firmware_info(&self) -> Option<FirmwareInfo> {
        Some(self.firmware)
    }

    fn request_firmware_install_receipt(
        &mut self,
        request_id: u32,
        receipt: FirmwareInstallReceipt,
    ) -> Result<(), bridge_output::OutputError> {
        self.recorded.lock().unwrap().push((request_id, receipt));
        self.pending_response = Some((request_id, receipt));
        Ok(())
    }

    fn poll_firmware_install_receipt(
        &mut self,
        request_id: u32,
        receipt: FirmwareInstallReceipt,
    ) -> Option<Result<FirmwareInstallReceipt, bridge_output::OutputError>> {
        let (actual_id, actual_receipt) = self.pending_response.take()?;
        assert_eq!(actual_id, request_id);
        assert_eq!(actual_receipt, receipt);
        self.firmware.install_state = FirmwareInstallState::Recorded(actual_receipt);
        Some(Ok(actual_receipt))
    }
}

#[test]
fn pending_manual_firmware_gets_one_first_observed_receipt() {
    let status = Arc::new(Mutex::new(BridgeStatus::default()));
    let (_, commands) = mpsc::channel();
    let supervisor = Supervisor::new(
        RuntimeConfig::default(),
        Arc::clone(&status),
        commands,
        Box::new(|_| {}),
        None,
    );
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let mut output = OutputSession {
        output: Box::new(PendingReceiptOutput {
            firmware: FirmwareInfo {
                version: FirmwareVersion::Reported(2),
                capabilities: FirmwareCapabilities::ENTER_UF2_BOOTLOADER
                    | FirmwareCapabilities::INSTALL_RECEIPT,
                install_state: FirmwareInstallState::Pending,
                ..FirmwareInfo::default()
            },
            recorded: Arc::clone(&recorded),
            pending_response: None,
        }),
        serial_device: Some(serial_info("/dev/cu.test", "TESTSERIAL")),
        capabilities: OutputCapabilities::for_selection(&OutputSelection::Serial),
        first_observed_receipt: FirstObservedReceiptState::Idle,
    };

    supervisor.refresh_output_firmware(&mut output);
    supervisor.refresh_output_firmware(&mut output);

    let receipts = recorded.lock().unwrap();
    assert_eq!(receipts.len(), 1);
    let (_, receipt) = receipts[0];
    assert_eq!(receipt.source, FirmwareInstallSource::FirstObserved);
    assert!(receipt.installed_at > 0);
    assert_ne!(receipt.install_id, [0; 16]);
    assert_eq!(
        status
            .lock()
            .unwrap()
            .output
            .firmware
            .unwrap()
            .install_state,
        FirmwareInstallState::Recorded(receipt)
    );
}

struct DroppedReceiptAckOutput {
    attempts: Arc<Mutex<Vec<(u32, FirmwareInstallReceipt)>>>,
}

impl GamepadOutput for DroppedReceiptAckOutput {
    fn send_state(
        &mut self,
        _state: &gamepad_state::GamepadState,
    ) -> Result<(), bridge_output::OutputError> {
        Ok(())
    }

    fn firmware_info(&self) -> Option<FirmwareInfo> {
        Some(FirmwareInfo {
            version: FirmwareVersion::Reported(2),
            capabilities: FirmwareCapabilities::INSTALL_RECEIPT,
            install_state: FirmwareInstallState::Pending,
            ..FirmwareInfo::default()
        })
    }

    fn request_firmware_install_receipt(
        &mut self,
        request_id: u32,
        receipt: FirmwareInstallReceipt,
    ) -> Result<(), bridge_output::OutputError> {
        self.attempts.lock().unwrap().push((request_id, receipt));
        Ok(())
    }

    fn poll_firmware_install_receipt(
        &mut self,
        _request_id: u32,
        _receipt: FirmwareInstallReceipt,
    ) -> Option<Result<FirmwareInstallReceipt, bridge_output::OutputError>> {
        None
    }
}

#[test]
fn a_lost_receipt_ack_retries_the_same_receipt_after_backoff() {
    let status = Arc::new(Mutex::new(BridgeStatus::default()));
    let (_, commands) = mpsc::channel();
    let supervisor = Supervisor::new(
        RuntimeConfig::default(),
        status,
        commands,
        Box::new(|_| {}),
        None,
    );
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let mut output = OutputSession {
        output: Box::new(DroppedReceiptAckOutput {
            attempts: Arc::clone(&attempts),
        }),
        serial_device: Some(serial_info("/dev/cu.test", "TESTSERIAL")),
        capabilities: OutputCapabilities::for_selection(&OutputSelection::Serial),
        first_observed_receipt: FirstObservedReceiptState::Idle,
    };

    supervisor.refresh_output_firmware(&mut output);
    let FirstObservedReceiptState::Waiting { request, .. } = output.first_observed_receipt else {
        panic!("receipt request did not start");
    };
    output.first_observed_receipt = FirstObservedReceiptState::Waiting {
        request,
        deadline: Instant::now(),
    };
    supervisor.refresh_output_firmware(&mut output);
    let FirstObservedReceiptState::Backoff { request, .. } = output.first_observed_receipt else {
        panic!("lost response did not enter backoff");
    };
    let request = request.unwrap();
    output.first_observed_receipt = FirstObservedReceiptState::Backoff {
        request: Some(request),
        retry_at: Instant::now(),
    };
    supervisor.refresh_output_firmware(&mut output);

    assert_eq!(*attempts.lock().unwrap(), [(request.request_id, request.receipt); 2]);
}

#[test]
fn hardware_release_finishes_before_command_acknowledgement() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let output = OutputSession {
        output: Box::new(DropOrderOutput(Arc::clone(&order))),
        serial_device: None,
        capabilities: OutputCapabilities::for_selection(&OutputSelection::Mock),
        first_observed_receipt: FirstObservedReceiptState::Idle,
    };
    let release_order = Arc::clone(&order);
    let ack_order = Arc::clone(&order);
    let (ack, receiver) = mpsc::channel::<Result<(), String>>();
    let observer = thread::spawn(move || {
        receiver.recv().unwrap().unwrap();
        ack_order.lock().unwrap().push("ack");
    });

    acknowledge_after_hardware_release(
        output,
        move || release_order.lock().unwrap().push("controllers"),
        &ack,
        Ok(()),
    );
    observer.join().unwrap();
    assert_eq!(*order.lock().unwrap(), ["output", "controllers", "ack"]);
}

#[test]
fn ordinary_stop_disconnects_virtual_hid_before_acknowledgement() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let output = OutputSession {
        output: Box::new(DropOrderOutput(Arc::clone(&order))),
        serial_device: None,
        capabilities: OutputCapabilities::for_selection(&OutputSelection::VirtualHid(
            VirtualHidConfig::new(std::path::PathBuf::from("helper")),
        )),
        first_observed_receipt: FirstObservedReceiptState::Idle,
    };
    let release_order = Arc::clone(&order);
    let ack_order = Arc::clone(&order);
    let (ack, receiver) = mpsc::channel::<Result<(), String>>();
    let observer = thread::spawn(move || {
        receiver.recv().unwrap().unwrap();
        ack_order.lock().unwrap().push("ack");
    });

    acknowledge_after_hardware_release(
        output,
        move || release_order.lock().unwrap().push("controllers"),
        &ack,
        Ok(()),
    );
    observer.join().unwrap();

    assert_eq!(*order.lock().unwrap(), ["output", "controllers", "ack"]);
}

#[test]
fn a_slow_desktop_operation_is_preceded_by_neutral_on_the_wire() {
    // Regression: constructing the desktop-input sink, or destroying it
    // after a backend failure, can block this thread beyond the firmware's
    // 100 ms controller-data watchdog. Healthy profile switches retain the
    // sink, but neutral remains the safety boundary around lifecycle work.
    let states = Arc::new(Mutex::new(Vec::new()));
    let mut session = OutputSession {
        output: Box::new(SharedOutput(Arc::clone(&states))),
        serial_device: None,
        capabilities: OutputCapabilities::for_selection(&OutputSelection::Mock),
        first_observed_receipt: FirstObservedReceiptState::Idle,
    };
    let mut engine = BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
    engine.connected();
    let mut idle = IdleActivityTracker::new(None);
    let started = Instant::now();
    let held = [SteamButton::A];

    let report = picker_report(1, &held, (0, 32_767));
    process_report(
        &report,
        &mut engine,
        &mut *session.output,
        &mut None,
        started,
        &mut idle,
    )
    .unwrap();
    let active = *states.lock().unwrap().last().unwrap();
    assert_ne!(active, gamepad_state::GamepadState::NEUTRAL);

    neutralize_before_desktop_work(&mut engine, &mut session);
    assert_eq!(
        states.lock().unwrap().last(),
        Some(&gamepad_state::GamepadState::NEUTRAL),
        "the device must be parked before the thread blocks"
    );

    // The controller has not moved, so the unchanged-output dedupe must
    // still know the wire is at neutral and resend the real state.
    process_report(
        &picker_report(2, &held, (0, 32_767)),
        &mut engine,
        &mut *session.output,
        &mut None,
        started,
        &mut idle,
    )
    .unwrap();
    assert_eq!(
        states.lock().unwrap().last(),
        Some(&active),
        "the real state must come back after the operation"
    );
}

#[test]
fn holding_quick_access_opens_the_wheel_without_firing_its_binding() {
    let keys = Arc::new(Mutex::new(Vec::new()));
    let mut harness = PickerHarness::new(
        quick_access_profile(),
        Box::new(SharedDesktopSink(Arc::clone(&keys))),
    );
    harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
    harness.feed(
        Duration::from_millis(10),
        &picker_report(2, &[SteamButton::QuickAccess], (0, 0)),
        TEST_ROSTER,
    );
    // Arming already hides the press, so the F5 chord never fires.
    assert!(keys.lock().unwrap().is_empty());

    harness.feed(
        Duration::from_millis(2_010),
        &picker_report(3, &[SteamButton::QuickAccess], (0, 0)),
        TEST_ROSTER,
    );
    assert_eq!(
        harness.events,
        vec![PickerEvent::Opened {
            selected: 0,
            page: 0,
            roster_revision: 0,
        }]
    );
    assert!(keys.lock().unwrap().is_empty());
    assert!(harness.engine.output_suppression().is_some());
}

#[test]
fn an_open_wheel_hides_its_controls_from_the_game_and_gives_them_back() {
    let keys = Arc::new(Mutex::new(Vec::new()));
    let mut harness = PickerHarness::new(
        quick_access_profile(),
        Box::new(SharedDesktopSink(Arc::clone(&keys))),
    );
    let held = [SteamButton::QuickAccess];
    harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
    harness.feed(
        Duration::from_millis(10),
        &picker_report(2, &held, (0, 0)),
        TEST_ROSTER,
    );
    harness.feed(
        Duration::from_millis(2_010),
        &picker_report(3, &held, (0, 0)),
        TEST_ROSTER,
    );
    assert!(harness.picker.is_open());

    // Steering the wheel must not also steer the game.
    harness.feed(
        Duration::from_millis(2_100),
        &picker_report(4, &held, (0, 32_767)),
        TEST_ROSTER,
    );
    let hidden = *harness.output.states.last().unwrap();
    assert_eq!((hidden.right_x, hidden.right_y), (0.0, 0.0));
    assert!(!hidden.buttons.contains(gamepad_state::Button::Extra3));

    // A commits and the wheel closes, but A is still physically down.
    harness.feed(
        Duration::from_millis(2_200),
        &picker_report(5, &[SteamButton::A], (0, 32_767)),
        TEST_ROSTER,
    );
    assert!(!harness.picker.is_open());
    assert_eq!(
        harness.events.last(),
        Some(&PickerEvent::Commit {
            index: 0,
            roster_revision: 0,
        })
    );

    // The sticks come back immediately -- the user can play again -- but the
    // press that closed the wheel must not reach the game just because it is
    // still held. Regression: it used to, on this very report.
    harness.feed(
        Duration::from_millis(2_250),
        &picker_report(6, &[SteamButton::A], (0, 32_767)),
        TEST_ROSTER,
    );
    let after_commit = *harness.output.states.last().unwrap();
    assert!(after_commit.right_y > 0.0, "the game is playable again");
    assert!(
        !after_commit.buttons.contains(gamepad_state::Button::South),
        "the commit press must not leak into the game"
    );

    // Released, so a later, deliberate press does reach the game.
    harness.feed(
        Duration::from_millis(2_300),
        &picker_report(7, &[], (0, 32_767)),
        TEST_ROSTER,
    );
    assert!(harness.engine.output_suppression().is_none());
    harness.feed(
        Duration::from_millis(2_400),
        &picker_report(8, &[SteamButton::A], (0, 32_767)),
        TEST_ROSTER,
    );
    let deliberate = *harness.output.states.last().unwrap();
    assert!(deliberate.buttons.contains(gamepad_state::Button::South));
}

#[test]
fn a_quick_access_tap_still_fires_its_desktop_binding() {
    let keys = Arc::new(Mutex::new(Vec::new()));
    let mut harness = PickerHarness::new(
        quick_access_profile(),
        Box::new(SharedDesktopSink(Arc::clone(&keys))),
    );
    harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
    harness.feed(
        Duration::from_millis(10),
        &picker_report(2, &[SteamButton::QuickAccess], (0, 0)),
        TEST_ROSTER,
    );
    harness.feed(
        Duration::from_millis(500),
        &picker_report(3, &[], (0, 0)),
        TEST_ROSTER,
    );
    assert_eq!(harness.events, vec![PickerEvent::TriggerTapped]);
    assert_eq!(
        *keys.lock().unwrap(),
        ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
    );
}

#[test]
fn dismissing_with_a_second_quick_access_press_does_not_fire_its_binding() {
    // Regression: the dismissing press returns the picker to Idle on the
    // very report that carries the down edge. Without the latch-aware mask
    // the bindings engine saw that edge as a fresh press and fired the
    // binding the wheel exists to protect.
    let keys = Arc::new(Mutex::new(Vec::new()));
    let mut harness = PickerHarness::new(
        quick_access_profile(),
        Box::new(SharedDesktopSink(Arc::clone(&keys))),
    );
    let held = [SteamButton::QuickAccess];
    harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
    harness.feed(
        Duration::from_millis(10),
        &picker_report(2, &held, (0, 0)),
        TEST_ROSTER,
    );
    harness.feed(
        Duration::from_millis(2_010),
        &picker_report(3, &held, (0, 0)),
        TEST_ROSTER,
    );
    assert!(harness.picker.is_open());

    // Release, then press Quick Access again to cancel.
    harness.feed(
        Duration::from_millis(2_100),
        &picker_report(4, &[], (0, 0)),
        TEST_ROSTER,
    );
    harness.feed(
        Duration::from_millis(2_200),
        &picker_report(5, &held, (0, 0)),
        TEST_ROSTER,
    );
    assert_eq!(harness.events.last(), Some(&PickerEvent::Dismissed));
    assert!(
        keys.lock().unwrap().is_empty(),
        "cancelling the wheel must not fire the Quick Access binding"
    );

    // Still held: still nothing. Released: still nothing.
    harness.feed(
        Duration::from_millis(2_300),
        &picker_report(6, &held, (0, 0)),
        TEST_ROSTER,
    );
    harness.feed(
        Duration::from_millis(2_400),
        &picker_report(7, &[], (0, 0)),
        TEST_ROSTER,
    );
    assert!(keys.lock().unwrap().is_empty());
    assert!(harness.engine.output_suppression().is_none());

    // A later deliberate tap fires the binding as normal.
    harness.feed(
        Duration::from_secs(3),
        &picker_report(8, &held, (0, 0)),
        TEST_ROSTER,
    );
    harness.feed(
        Duration::from_millis(3_100),
        &picker_report(9, &[], (0, 0)),
        TEST_ROSTER,
    );
    assert_eq!(
        *keys.lock().unwrap(),
        ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
    );
}

#[test]
fn a_config_change_mid_hold_cancels_the_wheel_and_swallows_the_press() {
    // Past the halfway mark the overlay child is already running, so the
    // caller must be told the hold is off (it answers with `Dismissed`),
    // and the withheld press must not become a fresh edge for the
    // bindings engine.
    let keys = Arc::new(Mutex::new(Vec::new()));
    let mut harness = PickerHarness::new(
        quick_access_profile(),
        Box::new(SharedDesktopSink(Arc::clone(&keys))),
    );
    let held = [SteamButton::QuickAccess];
    harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
    harness.feed(
        Duration::from_millis(10),
        &picker_report(2, &held, (0, 0)),
        TEST_ROSTER,
    );
    harness.feed(
        Duration::from_millis(1_200),
        &picker_report(3, &held, (0, 0)),
        TEST_ROSTER,
    );
    assert_eq!(harness.events, vec![PickerEvent::Preparing]);

    assert!(
        harness.picker.set_config(Some(PickerConfig {
            hold: Duration::from_secs(3),
            ..PickerConfig::default()
        })),
        "a cancelled hold must be reported so the overlay child is stopped"
    );

    // The press stays swallowed while held, and its release is not a tap.
    harness.feed(
        Duration::from_millis(1_300),
        &picker_report(4, &held, (0, 0)),
        TEST_ROSTER,
    );
    harness.feed(
        Duration::from_millis(1_400),
        &picker_report(5, &[], (0, 0)),
        TEST_ROSTER,
    );
    assert_eq!(harness.events, vec![PickerEvent::Preparing]);
    assert!(keys.lock().unwrap().is_empty());
    assert!(harness.engine.output_suppression().is_none());
}
