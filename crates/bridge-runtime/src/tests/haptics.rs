#[test]
fn disabling_the_wheel_while_open_keeps_held_controls_latched() {
    // Switching the feature off is not allowed to hand the game or the
    // bindings engine the buttons that were operating the wheel.
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

    assert!(harness.picker.set_config(None));
    harness.feed(
        Duration::from_millis(2_100),
        &picker_report(4, &held, (0, 0)),
        TEST_ROSTER,
    );
    assert!(
        keys.lock().unwrap().is_empty(),
        "the held trigger must not become a fresh press when the wheel is disabled"
    );
    assert!(
        harness.engine.output_suppression().is_some(),
        "the held trigger stays withheld from the game until released"
    );

    // Released: everything drains. A fresh press is an ordinary binding
    // press again, with no wheel to intercept it.
    harness.feed(
        Duration::from_millis(2_200),
        &picker_report(5, &[], (0, 0)),
        TEST_ROSTER,
    );
    assert!(harness.engine.output_suppression().is_none());
    let events_before_press = harness.events.len();
    harness.feed(
        Duration::from_secs(3),
        &picker_report(6, &held, (0, 0)),
        TEST_ROSTER,
    );
    assert_eq!(*keys.lock().unwrap(), ["key:F5:true".to_owned()]);
    assert_eq!(
        harness.events.len(),
        events_before_press,
        "a disabled wheel must not arm or open again"
    );
}

#[test]
fn a_disabled_wheel_leaves_quick_access_entirely_alone() {
    let keys = Arc::new(Mutex::new(Vec::new()));
    let mut harness = PickerHarness::new(
        quick_access_profile(),
        Box::new(SharedDesktopSink(Arc::clone(&keys))),
    );
    harness.picker = PickerRuntime::new(None);
    harness.feed(Duration::ZERO, &picker_report(1, &[], (0, 0)), TEST_ROSTER);
    harness.feed(
        Duration::from_millis(10),
        &picker_report(2, &[SteamButton::QuickAccess], (0, 0)),
        TEST_ROSTER,
    );
    // The binding fires on the press edge, exactly as before the wheel existed.
    assert_eq!(*keys.lock().unwrap(), ["key:F5:true".to_owned()]);
    harness.feed(
        Duration::from_secs(5),
        &picker_report(3, &[SteamButton::QuickAccess], (0, 0)),
        TEST_ROSTER,
    );
    assert!(harness.events.is_empty());
    assert!(harness.engine.output_suppression().is_none());
}

#[test]
fn closing_the_wheel_for_a_lost_controller_reports_it_once() {
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
    assert!(harness.picker.close());
    assert!(!harness.picker.close());
}

#[test]
fn runtime_binding_observation_does_not_change_gamepad_output() {
    let report = |sequence: u8, buttons: u32| {
        let mut data = vec![0; INPUT_REPORT_SIZE];
        data[0] = INPUT_REPORT_ID;
        data[1] = sequence;
        data[2..6].copy_from_slice(&buttons.to_le_bytes());
        RawHidReport {
            timestamp: Duration::ZERO,
            report_id: INPUT_REPORT_ID,
            data,
            source_device_id: "runtime-test".to_owned(),
            transport: "USB".to_owned(),
            dropped_reports: 0,
        }
    };
    let r4 = 1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8;
    let reports = [report(1, 0), report(2, r4), report(3, 0)];

    let mut profile = BindingProfile::default();
    profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F5,
        modifiers: std::collections::BTreeSet::new(),
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut bindings = DesktopBindingsRuntime::with_sink(
        profile,
        Box::new(SharedDesktopSink(Arc::clone(&events))),
    );
    let mut bound_engine =
        BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
    let mut control_engine =
        BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
    bound_engine.connected();
    control_engine.connected();
    let mut bound_output = MockOutput::default();
    let mut control_output = MockOutput::default();
    let mut bound_idle = IdleActivityTracker::new(None);
    let mut control_idle = IdleActivityTracker::new(None);
    let started = Instant::now();
    for report in &reports {
        let effect = process_report(
            report,
            &mut bound_engine,
            &mut bound_output,
            &mut None,
            started,
            &mut bound_idle,
        )
        .unwrap();
        if let ReportEffect::ControllerState { desktop_input, .. } = effect {
            let _ = bindings.observe(desktop_input, started.elapsed());
        }
        let _ = process_report(
            report,
            &mut control_engine,
            &mut control_output,
            &mut None,
            started,
            &mut control_idle,
        )
        .unwrap();
    }
    assert_eq!(bound_output.states, control_output.states);
    assert_eq!(
        *events.lock().unwrap(),
        ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
    );
}

#[test]
fn runtime_pad_observation_emits_mouse_and_feedback_without_changing_gamepad_output() {
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
            source_device_id: "runtime-pad-test".to_owned(),
            transport: "USB".to_owned(),
            dropped_reports: 0,
        }
    };
    let reports = [
        report(1, false, 0),
        report(2, true, 0),
        report(3, true, 3_968),
    ];
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut bindings = DesktopBindingsRuntime::with_sink(
        profile,
        Box::new(SharedDesktopSink(Arc::clone(&events))),
    );
    let mut bound_engine =
        BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
    let mut control_engine =
        BridgeEngine::new(BridgeConfig::default(), MapperConfig::default()).unwrap();
    bound_engine.connected();
    control_engine.connected();
    let mut bound_output = MockOutput::default();
    let mut control_output = MockOutput::default();
    let mut bound_idle = IdleActivityTracker::new(None);
    let mut control_idle = IdleActivityTracker::new(None);
    let started = Instant::now();
    let mut feedback = PadFeedbackRequest::NONE;
    for (index, report) in reports.iter().enumerate() {
        let effect = process_report(
            report,
            &mut bound_engine,
            &mut bound_output,
            &mut None,
            started,
            &mut bound_idle,
        )
        .unwrap();
        if let ReportEffect::ControllerState { desktop_input, .. } = effect {
            feedback = bindings.observe(
                desktop_input,
                Duration::from_millis(u64::try_from(index * 20).unwrap()),
            );
        }
        let _ = process_report(
            report,
            &mut control_engine,
            &mut control_output,
            &mut None,
            started,
            &mut control_idle,
        )
        .unwrap();
    }
    assert_eq!(bound_output.states, control_output.states);
    assert_eq!(*events.lock().unwrap(), ["move:11:0".to_owned()]);
    assert_eq!(
        feedback.right,
        Some(desktop_bindings::PadFeedbackStrength::Medium)
    );
}

#[test]
fn runtime_emits_one_feedback_tick_for_a_physical_pad_click() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut bindings = DesktopBindingsRuntime::with_sink(
        BindingProfile::default(),
        Box::new(SharedDesktopSink(Arc::clone(&events))),
    );
    let snapshot = |pressed| DesktopInputSnapshot {
        buttons: steam_controller_protocol::SteamButtons(if pressed {
            1_u32 << SteamButton::RightPadClick as u8
        } else {
            0
        }),
        right_pad: PadSample {
            touched: true,
            pressed,
            ..PadSample::default()
        },
        ..desktop_snapshot(steam_controller_protocol::SteamButtons::default())
    };

    let _ = bindings.observe(
        desktop_snapshot(steam_controller_protocol::SteamButtons::default()),
        Duration::ZERO,
    );
    let _ = bindings.observe(snapshot(false), Duration::from_millis(1));
    let press = bindings.observe(snapshot(true), Duration::from_millis(10));
    let hold = bindings.observe(snapshot(true), Duration::from_millis(20));

    assert_eq!(
        press.right,
        Some(desktop_bindings::PadFeedbackStrength::Medium)
    );
    assert_eq!(hold, PadFeedbackRequest::NONE);
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn runtime_tick_advances_scroll_momentum_without_more_hid_reports() {
    let mut profile = BindingProfile::default();
    profile.pads.left.motion = PadMotionMode::Scroll;
    profile.pads.left.feedback.enabled = false;
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut bindings = DesktopBindingsRuntime::with_sink(
        profile,
        Box::new(SharedDesktopSink(Arc::clone(&events))),
    );
    let snapshot = |x, touched| DesktopInputSnapshot {
        left_pad: PadSample {
            x,
            touched,
            ..PadSample::default()
        },
        ..desktop_snapshot(steam_controller_protocol::SteamButtons::default())
    };

    assert!(!bindings.needs_tick());
    let _ = bindings.observe(snapshot(0, false), Duration::ZERO);
    let _ = bindings.observe(snapshot(0, true), Duration::from_millis(1));
    let _ = bindings.observe(snapshot(768, true), Duration::from_millis(21));
    assert!(!bindings.needs_tick());
    let _ = bindings.observe(snapshot(0, false), Duration::from_millis(22));
    assert!(bindings.needs_tick());
    bindings.tick(Duration::from_millis(72));

    assert_eq!(
        *events.lock().unwrap(),
        ["scroll:9:0".to_owned(), "scroll:7:0".to_owned()]
    );
}

#[test]
fn desktop_motion_failure_requests_pending_feedback_discard() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    let mut bindings = DesktopBindingsRuntime::with_sink(profile, Box::new(FailingMotionSink));
    let snapshot = |x, touched| DesktopInputSnapshot {
        right_pad: PadSample {
            x,
            touched,
            ..PadSample::default()
        },
        ..desktop_snapshot(steam_controller_protocol::SteamButtons::default())
    };

    let _ = bindings.observe(snapshot(0, false), Duration::ZERO);
    let _ = bindings.observe(snapshot(0, true), Duration::from_millis(1));
    assert_eq!(
        bindings.observe(snapshot(3_200, true), Duration::from_millis(20)),
        PadFeedbackRequest::NONE
    );
    assert_eq!(bindings.status().state, DesktopBindingsState::Degraded);
    assert!(bindings.take_discard_pending_feedback());
    assert!(!bindings.take_discard_pending_feedback());
}

#[test]
fn desktop_status_is_published_only_when_semantics_change() {
    let mut profile = BindingProfile::default();
    profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F5,
        modifiers: std::collections::BTreeSet::new(),
    });
    let mut bindings = DesktopBindingsRuntime::with_sink(
        profile,
        Box::new(SharedDesktopSink(Arc::new(Mutex::new(Vec::new())))),
    );
    assert!(bindings.take_status_update().is_some());
    assert!(bindings.take_status_update().is_none());

    let neutral = SteamButtons::default();
    let r4 = SteamButtons(1_u32 << SteamButton::RightGrip4 as u8);
    let _ = bindings.observe(desktop_snapshot(neutral), Duration::ZERO);
    assert!(bindings.take_status_update().is_none());
    let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(1));
    assert_eq!(bindings.take_status_update().unwrap().held_output_count, 1);
    assert!(bindings.take_status_update().is_none());
}

#[test]
fn replacing_profile_keeps_an_existing_authorized_sink_ready() {
    let mut first = BindingProfile::default();
    first.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F5,
        modifiers: std::collections::BTreeSet::new(),
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut bindings =
        DesktopBindingsRuntime::with_sink(first, Box::new(SharedDesktopSink(Arc::clone(&events))));
    let mut second = BindingProfile {
        name: "Second".to_owned(),
        ..BindingProfile::default()
    };
    second.bindings.r5 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F9,
        modifiers: std::collections::BTreeSet::new(),
    });

    bindings.replace_profile(Some(second)).unwrap();

    let status = bindings.status();
    assert_eq!(status.state, DesktopBindingsState::Ready);
    assert_eq!(status.configured_binding_count, 1);
    assert!(status.last_error.is_none());
    assert!(events.lock().unwrap().is_empty());
}

#[test]
fn switching_through_an_unbound_profile_reuses_the_authorized_sink() {
    let mut first = BindingProfile::default();
    first.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F5,
        modifiers: std::collections::BTreeSet::new(),
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let drops = Arc::new(AtomicU64::new(0));
    let mut bindings = DesktopBindingsRuntime::with_sink(
        first,
        Box::new(DropTrackedDesktopSink::new(
            Arc::clone(&events),
            Arc::clone(&drops),
        )),
    );
    let neutral = steam_controller_protocol::SteamButtons::default();
    let r4 = steam_controller_protocol::SteamButtons(
        1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8,
    );
    let _ = bindings.observe(desktop_snapshot(neutral), Duration::ZERO);
    let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(1));

    bindings
        .replace_profile(Some(BindingProfile::default()))
        .unwrap();

    let status = bindings.status();
    assert_eq!(status.state, DesktopBindingsState::Disabled);
    assert_eq!(status.configured_binding_count, 0);
    assert_eq!(status.held_output_count, 0);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    assert_eq!(
        *events.lock().unwrap(),
        ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
    );

    let mut rebound = BindingProfile {
        name: "Rebound".to_owned(),
        ..BindingProfile::default()
    };
    rebound.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F9,
        modifiers: std::collections::BTreeSet::new(),
    });
    bindings.replace_profile(Some(rebound)).unwrap();
    assert_eq!(bindings.status().state, DesktopBindingsState::Ready);
    assert_eq!(drops.load(Ordering::Relaxed), 0);

    // The control stayed held through both switches. Reusing the sink must
    // not turn that into a synthetic press in the replacement profile.
    let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(2));
    assert_eq!(events.lock().unwrap().len(), 2);
    let _ = bindings.observe(desktop_snapshot(neutral), Duration::from_millis(3));
    let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(4));
    assert_eq!(
        *events.lock().unwrap(),
        [
            "key:F5:true".to_owned(),
            "key:F5:false".to_owned(),
            "key:F9:true".to_owned(),
        ]
    );

    drop(bindings);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn clearing_a_profile_retains_the_authorized_sink_for_later_reuse() {
    let mut first = BindingProfile::default();
    first.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F5,
        modifiers: std::collections::BTreeSet::new(),
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let drops = Arc::new(AtomicU64::new(0));
    let mut bindings = DesktopBindingsRuntime::with_sink(
        first,
        Box::new(DropTrackedDesktopSink::new(
            Arc::clone(&events),
            Arc::clone(&drops),
        )),
    );

    bindings.replace_profile(None).unwrap();
    assert_eq!(bindings.status(), DesktopBindingsStatus::default());
    assert!(bindings.engine.is_none());
    assert!(bindings.sink.is_some());
    assert_eq!(drops.load(Ordering::Relaxed), 0);

    let mut replacement = BindingProfile::default();
    replacement.bindings.r5 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F9,
        modifiers: std::collections::BTreeSet::new(),
    });
    bindings.replace_profile(Some(replacement)).unwrap();
    assert_eq!(bindings.status().state, DesktopBindingsState::Ready);
    assert!(bindings.engine.is_some());
    assert!(bindings.sink.is_some());
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    assert!(events.lock().unwrap().is_empty());

    drop(bindings);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn idle_profile_replacement_does_not_initialize_desktop_input() {
    let mut profile = BindingProfile::default();
    profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F5,
        modifiers: std::collections::BTreeSet::new(),
    });
    let mut bindings = DesktopBindingsRuntime::new(None);

    bindings.replace_profile(Some(profile)).unwrap();

    assert!(!bindings.activation_requested);
    assert!(bindings.sink.is_none());
    assert_eq!(
        bindings.status().state,
        DesktopBindingsState::PermissionRequired
    );
}

#[test]
fn enabling_an_already_ready_sink_preserves_held_output_state() {
    let mut profile = BindingProfile::default();
    profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F5,
        modifiers: std::collections::BTreeSet::new(),
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut bindings = DesktopBindingsRuntime::with_sink(
        profile,
        Box::new(SharedDesktopSink(Arc::clone(&events))),
    );
    let r4 = steam_controller_protocol::SteamButtons(
        1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8,
    );
    let _ = bindings.observe(
        desktop_snapshot(steam_controller_protocol::SteamButtons::default()),
        Duration::ZERO,
    );
    let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(1));

    bindings.enable().unwrap();
    let _ = bindings.observe(
        desktop_snapshot(steam_controller_protocol::SteamButtons::default()),
        Duration::from_millis(2),
    );

    assert_eq!(
        *events.lock().unwrap(),
        ["key:F5:true".to_owned(), "key:F5:false".to_owned()]
    );
}

#[test]
fn mailbox_overflow_recovers_after_a_non_emitting_baseline() {
    let mut profile = BindingProfile::default();
    profile.bindings.r4 = Some(desktop_bindings::BindingAction::KeyChord {
        key: desktop_bindings::KeyboardKey::F5,
        modifiers: std::collections::BTreeSet::new(),
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut bindings = DesktopBindingsRuntime::with_sink(
        profile,
        Box::new(SharedDesktopSink(Arc::clone(&events))),
    );
    let neutral = steam_controller_protocol::SteamButtons::default();
    let r4 = steam_controller_protocol::SteamButtons(
        1_u32 << steam_controller_protocol::SteamButton::RightGrip4 as u8,
    );
    let _ = bindings.observe(desktop_snapshot(neutral), Duration::ZERO);
    let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(1));

    bindings.overflow();
    assert_eq!(bindings.status().state, DesktopBindingsState::Degraded);
    let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(2));
    assert_eq!(bindings.status().state, DesktopBindingsState::Ready);
    let _ = bindings.observe(desktop_snapshot(neutral), Duration::from_millis(3));
    let _ = bindings.observe(desktop_snapshot(r4), Duration::from_millis(4));

    assert_eq!(
        *events.lock().unwrap(),
        [
            "key:F5:true".to_owned(),
            "key:F5:false".to_owned(),
            "key:F5:true".to_owned(),
        ]
    );
    assert_eq!(bindings.status().failures, 1);
}

#[test]
fn latest_rumble_slot_coalesces_to_one_command() {
    let slot = LatestRumbleSlot::default();
    assert!(!slot.publish(RumbleCommand {
        low_frequency: 1,
        high_frequency: 2,
    }));
    assert!(slot.publish(RumbleCommand {
        low_frequency: 3,
        high_frequency: 4,
    }));
    assert_eq!(
        slot.take(),
        Some(RumbleCommand {
            low_frequency: 3,
            high_frequency: 4,
        })
    );
    assert_eq!(slot.take(), None);
}

#[test]
fn pending_pad_feedback_coalesces_sides_and_preserves_strength() {
    let pending = PendingPadFeedback::default();
    assert_eq!(
        pending.publish(PadFeedbackRequest {
            left: Some(desktop_bindings::PadFeedbackStrength::Medium),
            right: None,
        }),
        0
    );
    assert_eq!(
        pending.publish(PadFeedbackRequest {
            left: Some(desktop_bindings::PadFeedbackStrength::Medium),
            right: Some(desktop_bindings::PadFeedbackStrength::Medium),
        }),
        1
    );
    assert_eq!(
        pending.take(),
        vec![PadFeedbackCommand {
            side: PadHapticSide::Both,
            gain: PadHapticGain::Medium,
        }]
    );

    let _ = pending.publish(PadFeedbackRequest {
        left: Some(desktop_bindings::PadFeedbackStrength::Low),
        right: Some(desktop_bindings::PadFeedbackStrength::High),
    });
    assert_eq!(
        pending.take(),
        vec![
            PadFeedbackCommand {
                side: PadHapticSide::Left,
                gain: PadHapticGain::Low,
            },
            PadFeedbackCommand {
                side: PadHapticSide::Right,
                gain: PadHapticGain::High,
            },
        ]
    );
    pending.clear();
    assert!(pending.take().is_empty());
}

#[test]
fn pad_feedback_failure_backs_off_without_changing_rumble_state() {
    let metrics = Arc::new(SharedHapticsMetrics::default());
    let writer = FakePadFeedbackWriter::default();
    let mut feedback = PadFeedbackSupervisor::new(Arc::clone(&metrics));
    feedback.connected();
    writer.fail.store(true, Ordering::Release);
    feedback.service(
        Duration::from_millis(10),
        &writer,
        vec![PadFeedbackCommand {
            side: PadHapticSide::Right,
            gain: PadHapticGain::Medium,
        }],
    );
    let failed = metrics.snapshot(Duration::from_millis(10));
    assert_eq!(failed.state, HapticsState::Degraded);
    assert_eq!(failed.pad_feedback_failures, 1);
    assert!(failed.pad_feedback_last_error.is_some());

    writer.fail.store(false, Ordering::Release);
    feedback.service(
        Duration::from_millis(509),
        &writer,
        vec![PadFeedbackCommand {
            side: PadHapticSide::Right,
            gain: PadHapticGain::Medium,
        }],
    );
    assert!(writer.writes.lock().unwrap().is_empty());
    feedback.service(
        Duration::from_millis(510),
        &writer,
        vec![PadFeedbackCommand {
            side: PadHapticSide::Right,
            gain: PadHapticGain::Medium,
        }],
    );
    let recovered = metrics.snapshot(Duration::from_millis(510));
    assert_eq!(recovered.state, HapticsState::Idle);
    assert_eq!(recovered.pad_feedback_ticks, 1);
    assert!(recovered.pad_feedback_last_error.is_none());
}

#[test]
fn haptics_refreshes_expires_and_recovers_after_backoff() {
    let metrics = Arc::new(SharedHapticsMetrics::default());
    let writer = FakeRumbleWriter::default();
    let mut haptics = HapticsSupervisor::new(Arc::clone(&metrics));

    haptics.connected(Duration::ZERO, &writer);
    haptics.command(
        Duration::from_millis(1),
        &writer,
        RumbleCommand {
            low_frequency: 0x1234,
            high_frequency: 0xabcd,
        },
    );
    haptics.service(Duration::from_millis(40), &writer);
    assert_eq!(metrics.snapshot(Duration::from_millis(40)).refreshes, 0);
    haptics.service(Duration::from_millis(41), &writer);
    assert_eq!(metrics.snapshot(Duration::from_millis(41)).refreshes, 1);

    haptics.command(
        Duration::from_millis(50),
        &writer,
        RumbleCommand {
            low_frequency: 0x1234,
            high_frequency: 0xabcd,
        },
    );
    haptics.service(Duration::from_millis(150), &writer);
    assert_eq!(
        writer
            .writes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .copied(),
        Some((0, 0))
    );
    assert_eq!(
        metrics.snapshot(Duration::from_millis(150)).state,
        HapticsState::Idle
    );

    writer.fail.store(true, Ordering::Release);
    haptics.command(
        Duration::from_millis(200),
        &writer,
        RumbleCommand {
            low_frequency: 1,
            high_frequency: 2,
        },
    );
    assert_eq!(
        metrics.snapshot(Duration::from_millis(200)).state,
        HapticsState::Degraded
    );
    writer.fail.store(false, Ordering::Release);
    for now in [250, 340, 430, 520, 610, 699] {
        haptics.command(
            Duration::from_millis(now),
            &writer,
            RumbleCommand {
                low_frequency: 1,
                high_frequency: 2,
            },
        );
        haptics.service(Duration::from_millis(now), &writer);
    }
    assert_eq!(
        metrics.snapshot(Duration::from_millis(699)).state,
        HapticsState::Degraded
    );
    haptics.command(
        Duration::from_millis(700),
        &writer,
        RumbleCommand {
            low_frequency: 1,
            high_frequency: 2,
        },
    );
    haptics.service(Duration::from_millis(700), &writer);
    let recovered = metrics.snapshot(Duration::from_millis(700));
    assert_eq!(recovered.state, HapticsState::Active);
    assert_eq!(recovered.failures, 1);
}
