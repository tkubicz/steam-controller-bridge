#[test]
fn active_source_selection_is_order_independent_and_rejects_ambiguity() {
    assert_eq!(choose_unique_active(&[]), Ok(None));
    assert_eq!(choose_unique_active(&[3]), Ok(Some(3)));
    assert_eq!(choose_unique_active(&[0]), Ok(Some(0)));
    assert_eq!(choose_unique_active(&[1, 3]), Err(vec![1, 3]));
}

#[test]
fn controller_inventory_scans_quickly_only_until_candidates_are_open() {
    assert_eq!(
        controller_inventory_scan_interval(false, MAX_STABLE_CONTROLLER_SCAN_INTERVAL,),
        DISCOVERY_INTERVAL
    );
    assert_eq!(
        controller_inventory_scan_interval(true, MIN_STABLE_CONTROLLER_SCAN_INTERVAL,),
        MIN_STABLE_CONTROLLER_SCAN_INTERVAL
    );
    assert_eq!(
        next_stable_controller_scan_interval(MIN_STABLE_CONTROLLER_SCAN_INTERVAL),
        Duration::from_secs(4)
    );
    assert_eq!(
        next_stable_controller_scan_interval(Duration::from_secs(8)),
        MAX_STABLE_CONTROLLER_SCAN_INTERVAL
    );
    assert_eq!(
        next_stable_controller_scan_interval(MAX_STABLE_CONTROLLER_SCAN_INTERVAL),
        MAX_STABLE_CONTROLLER_SCAN_INTERVAL
    );
}

#[test]
fn indexed_controller_discovery_caches_the_global_selection_between_scans() {
    let selected = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
        "USB",
    );
    let mut unrelated = selected.clone();
    unrelated.id = "unrelated".to_owned();
    unrelated.path = "unrelated".to_owned();
    unrelated.vendor_id = 0x1234;
    unrelated.product_id = 0x5678;
    let mut global = vec![unrelated; 44];
    global[43] = selected.clone();

    let mut discovery = IndexedControllerDiscoveryState::new();
    discovery.refresh(43, Ok(global.clone()));

    assert_eq!(discovery.info(), Some(&selected));
    assert_eq!(discovery.scan_error(), None);
    assert_eq!(
        discovery.stable_scan_interval,
        MIN_STABLE_CONTROLLER_SCAN_INTERVAL
    );
    assert!(!discovery.scan_due());
    assert!(
        discovery
            .next_scan
            .saturating_duration_since(Instant::now())
            > Duration::from_secs(1)
    );

    discovery.refresh(43, Ok(global));
    assert_eq!(discovery.stable_scan_interval, Duration::from_secs(4));
}

#[test]
fn controller_discovery_loop_keeps_nonblocking_probes_at_two_hertz() {
    assert_eq!(
        controller_discovery_loop_delay(Duration::ZERO),
        DISCOVERY_INTERVAL
    );
    assert_eq!(
        controller_discovery_loop_delay(DISCOVERY_INTERVAL + Duration::from_millis(1)),
        Duration::ZERO
    );
    assert_eq!(
        controller_discovery_loop_delay(Duration::from_millis(100)),
        Duration::from_millis(400)
    );
}

#[test]
fn idle_controller_discovery_reuses_sessions_across_scans_and_index_reordering() {
    let first = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
        "USB",
    );
    let second = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE + 1,
        "USB",
    );
    let mut discovery = ControllerDiscoveryState::new();
    let mut next_session = 0;
    let first_refresh = discovery.refresh(
        Ok(vec![(40, first.clone()), (41, second.clone())]),
        |_, _| {
            next_session += 1;
            Ok(next_session)
        },
    );
    assert_eq!(
        first_refresh,
        ControllerReconcileMetrics {
            opened: 2,
            ..ControllerReconcileMetrics::default()
        }
    );

    let second_refresh = discovery.refresh(
        Ok(vec![(12, second.clone()), (13, first.clone())]),
        |_, _| {
            next_session += 1;
            Ok(next_session)
        },
    );
    assert_eq!(
        second_refresh,
        ControllerReconcileMetrics {
            reused: 2,
            ..ControllerReconcileMetrics::default()
        }
    );
    assert_eq!(next_session, 2);
    assert_eq!(discovery.stable_scan_interval(), Duration::from_secs(4));
    assert_eq!(discovery.candidate(0).enumeration_index(), 12);
    assert_eq!(*discovery.candidate(0).session(), 2);
    assert_eq!(discovery.candidate(1).enumeration_index(), 13);
    assert_eq!(*discovery.candidate(1).session(), 1);
}

#[test]
fn controller_discovery_reconciles_arrival_removal_and_changed_paths() {
    let first = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
        "USB",
    );
    let second = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE + 1,
        "USB",
    );
    let bluetooth = controller_info(
        steam_controller_device::STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID,
        steam_controller_device::BLUETOOTH_CONTROLLER_INTERFACE,
        "Bluetooth",
    );
    let mut discovery = ControllerDiscoveryState::new();
    let mut next_session = 0;
    discovery.refresh(Ok(vec![(40, first), (41, second.clone())]), |_, _| {
        next_session += 1;
        Ok(next_session)
    });

    let mut moved_second = second;
    moved_second.id = "new-device-service-id".to_owned();
    moved_second.path = "new-device-service-id".to_owned();
    let refresh = discovery.refresh(Ok(vec![(8, moved_second), (58, bluetooth)]), |_, _| {
        next_session += 1;
        Ok(next_session)
    });
    assert_eq!(
        refresh,
        ControllerReconcileMetrics {
            opened: 1,
            reused: 1,
            removed: 1,
            ..ControllerReconcileMetrics::default()
        }
    );
    assert_eq!(next_session, 3);
    assert_eq!(*discovery.candidate(0).session(), 2);
    assert_eq!(*discovery.candidate(1).session(), 3);
}

#[test]
fn ambiguity_indices_are_resolved_against_the_global_hid_list() {
    let puck = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
        "USB",
    );
    let bluetooth = controller_info(
        steam_controller_device::STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID,
        steam_controller_device::BLUETOOTH_CONTROLLER_INTERFACE,
        "Bluetooth",
    );
    let mut discovery = ControllerDiscoveryState::new();
    discovery.refresh(
        Ok(vec![(0, puck.clone()), (1, bluetooth.clone())]),
        |index, _| Ok(index),
    );

    let mut unrelated = puck.clone();
    unrelated.id = "unrelated".to_owned();
    unrelated.path = "unrelated".to_owned();
    unrelated.vendor_id = 0x1234;
    unrelated.product_id = 0x5678;
    let mut global = vec![unrelated.clone(); 7];
    global[3] = bluetooth;
    global[6] = puck;
    discovery.resolve_global_indices(&global).unwrap();

    assert_eq!(discovery.candidate(0).enumeration_index(), 6);
    assert_eq!(discovery.candidate(1).enumeration_index(), 3);
}

#[test]
fn failed_global_index_resolution_does_not_partially_mutate_candidates() {
    let puck = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
        "USB",
    );
    let bluetooth = controller_info(
        steam_controller_device::STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID,
        steam_controller_device::BLUETOOTH_CONTROLLER_INTERFACE,
        "Bluetooth",
    );
    let mut discovery = ControllerDiscoveryState::new();
    discovery.refresh(Ok(vec![(0, puck.clone()), (1, bluetooth)]), |index, _| {
        Ok(index)
    });

    let mut unrelated = puck.clone();
    unrelated.id = "unrelated".to_owned();
    unrelated.path = "unrelated".to_owned();
    unrelated.vendor_id = 0x1234;
    unrelated.product_id = 0x5678;
    let mut incomplete_global = vec![unrelated; 7];
    incomplete_global[6] = puck;

    assert!(discovery
        .resolve_global_indices(&incomplete_global)
        .is_err());
    assert_eq!(discovery.candidate(0).enumeration_index(), 0);
    assert_eq!(discovery.candidate(1).enumeration_index(), 1);
}

#[test]
fn discovery_probe_uses_nonblocking_reads_for_idle_candidates() {
    let timeouts = Arc::new(Mutex::new(Vec::new()));
    let mut discovery = ControllerDiscoveryState::new();
    let candidates = (0..4)
        .map(|offset| {
            (
                40 + usize::try_from(offset).unwrap(),
                controller_info(
                    steam_controller_device::PROTEUS_PRODUCT_ID,
                    steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE + offset,
                    "USB",
                ),
            )
        })
        .collect();
    discovery.refresh(Ok(candidates), |_, info| {
        if info.interface_number == steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE + 2 {
            Ok(FakeDiscoverySession::with_report(
                controller_state_report(&info.id),
                Arc::clone(&timeouts),
            ))
        } else {
            Ok(FakeDiscoverySession::idle(Arc::clone(&timeouts)))
        }
    });

    let probe = discovery.probe();
    assert_eq!(probe.active_indices, vec![2]);
    assert!(probe.failures.is_empty());
    assert_eq!(
        *timeouts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![Duration::ZERO; 4]
    );
}

#[test]
fn discovery_probe_drains_a_bounded_prefix_to_find_fresh_state() {
    let timeouts = Arc::new(Mutex::new(Vec::new()));
    let puck = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
        "USB",
    );
    let connected = DeviceEvent::Connected(puck.clone());
    let state = DeviceEvent::Report(controller_state_report(&puck.id));
    let mut discovery = ControllerDiscoveryState::new();
    discovery.refresh(Ok(vec![(40, puck)]), |_, _| {
        Ok(FakeDiscoverySession::with_events(
            vec![Ok(Some(connected.clone())), Ok(Some(state.clone()))],
            Arc::clone(&timeouts),
        ))
    });

    let probe = discovery.probe();
    assert_eq!(probe.active_indices, vec![0]);
    assert_eq!(
        *timeouts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![Duration::ZERO; 2]
    );
}

#[test]
fn discovery_probe_never_drains_more_than_the_fixed_limit() {
    let timeouts = Arc::new(Mutex::new(Vec::new()));
    let puck = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
        "USB",
    );
    let mut events = (0..MAX_DISCOVERY_REPORTS_PER_CANDIDATE + 3)
        .map(|_| Ok(Some(DeviceEvent::Connected(puck.clone()))))
        .collect::<Vec<_>>();
    let mut discovery = ControllerDiscoveryState::new();
    discovery.refresh(Ok(vec![(40, puck)]), |_, _| {
        Ok(FakeDiscoverySession::with_events(
            std::mem::take(&mut events),
            Arc::clone(&timeouts),
        ))
    });

    let probe = discovery.probe();
    assert!(probe.active_indices.is_empty());
    assert_eq!(
        timeouts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        MAX_DISCOVERY_REPORTS_PER_CANDIDATE
    );
}

#[test]
fn discovery_probe_failures_use_identity_not_filtered_indices() {
    let timeouts = Arc::new(Mutex::new(Vec::new()));
    let puck = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
        "USB",
    );
    let mut discovery = ControllerDiscoveryState::new();
    discovery.refresh(Ok(vec![(2, puck)]), |_, _| {
        Ok(FakeDiscoverySession::with_error(
            "injected read failure",
            Arc::clone(&timeouts),
        ))
    });

    let probe = discovery.probe();
    assert_eq!(probe.failures.len(), 1);
    assert!(probe.failures[0].starts_with("Puck product"));
    assert!(probe.failures[0].contains("injected read failure"));
    assert!(!probe.failures[0].contains("index 2"));
}

#[test]
fn ambiguity_descriptions_retain_global_indices_and_transports() {
    let puck = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
        "USB",
    );
    let bluetooth = controller_info(
        steam_controller_device::STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID,
        steam_controller_device::BLUETOOTH_CONTROLLER_INTERFACE,
        "Bluetooth",
    );
    let puck_description = controller_source_description(43, &puck);
    let bluetooth_description = controller_source_description(58, &bluetooth);
    assert!(puck_description.contains("index 43 Puck"));
    assert!(bluetooth_description.contains("index 58 Bluetooth"));
    assert!(bluetooth_description.contains("interface -1"));
}

#[test]
fn remembered_output_serial_survives_a_changed_port_path() {
    let valid = vec![
        (bridge_endpoint("serial:new", "remembered"), ()),
        (bridge_endpoint("serial:other", "other"), ()),
    ];
    let ambiguity = choose_output_index(&valid, None).unwrap_err();
    assert!(ambiguity.contains("device ****ered"));
    assert!(ambiguity.contains("restart with --port PATH"));
    assert_eq!(choose_output_index(&valid, Some("remembered")), Ok(0));
    assert_eq!(choose_output_index(&valid, Some("other")), Ok(1));
}

#[test]
fn raw_usb_output_ambiguity_never_recommends_a_serial_only_selector() {
    let raw = BridgeEndpoint::official_linux_usb(3, 4, "raw-device");
    let valid = vec![
        (bridge_endpoint("serial:one", "serial-device"), ()),
        (raw, ()),
    ];

    let message = output_ambiguity_message(&valid);

    assert!(message.contains("disconnect all but one bridge device"));
    assert!(!message.contains("--port"));
}

#[test]
fn automatic_output_requires_the_marker_but_an_explicit_port_bypasses_it() {
    let explicit_only = BridgeEndpoint::serial_port("serial:explicit", 115_200)
        .with_stable_id("explicit")
        .with_usb_identity(bridge_output::BridgeUsbIdentity {
            vendor_id: 0xbeef,
            product_id: 0x1234,
            manufacturer: Some("Community implementation".to_owned()),
            product: Some("Custom development firmware".to_owned()),
        });

    assert_eq!(
        output_candidates(
            vec![explicit_only.clone()],
            &BridgeEndpointSelection::AutoBridgeDevice,
        ),
        Vec::new()
    );
    assert_eq!(
        output_candidates(
            vec![explicit_only.clone()],
            &BridgeEndpointSelection::SerialPort("serial:explicit".to_owned()),
        ),
        vec![explicit_only]
    );
}

#[test]
fn explicit_serial_output_bypasses_raw_usb_discovery() {
    let enumerated = BridgeEndpoint::serial_port("serial:explicit", 115_200)
        .with_stable_id("explicit-device")
        .with_usb_identity(bridge_output::BridgeUsbIdentity {
            vendor_id: 0xbeef,
            product_id: 0x1234,
            manufacturer: Some("Community implementation".to_owned()),
            product: Some("Custom development firmware".to_owned()),
        });
    let discovery = discover_output_endpoints_with(
        &BridgeEndpointSelection::SerialPort("serial:explicit".to_owned()),
        || -> Result<BridgeEndpointDiscovery, BridgeTransportError> {
            panic!("raw USB discovery must not run")
        },
        || Ok(vec![enumerated.clone()]),
    )
    .unwrap();

    assert_eq!(discovery.endpoints, vec![enumerated]);
    assert!(discovery.warnings.is_empty());
}

#[test]
fn explicit_serial_output_survives_serial_enumeration_failure() {
    let discovery = discover_output_endpoints_with(
        &BridgeEndpointSelection::SerialPort("serial:explicit".to_owned()),
        || -> Result<BridgeEndpointDiscovery, BridgeTransportError> {
            panic!("raw USB discovery must not run")
        },
        || {
            Err(BridgeTransportError::InvalidTopology(
                "serial enumeration failed".to_owned(),
            ))
        },
    )
    .unwrap();

    assert_eq!(
        discovery.endpoints,
        vec![BridgeEndpoint::serial_port("serial:explicit", 115_200)]
    );
    assert_eq!(discovery.warnings.len(), 1);
    assert!(discovery.warnings[0]
        .to_string()
        .contains("serial enumeration failed"));
}

#[test]
fn matching_serial_failure_falls_back_to_raw_usb_without_duplicate_output() {
    let serial = bridge_endpoint("serial:primary", "same-device");
    let raw = BridgeEndpoint::official_linux_usb(3, 4, "same-device");
    let mut attempts = Vec::new();

    let (valid, failures) =
        open_output_candidates_with(vec![raw.clone(), serial], None, |candidate| {
            attempts.push(candidate.kind());
            if candidate.serial_path().is_some() {
                Err("serial endpoint is busy")
            } else {
                Ok(())
            }
        });

    assert_eq!(
        attempts,
        vec![
            bridge_output::BridgeTransportKind::SerialPort,
            bridge_output::BridgeTransportKind::LinuxUsb,
        ]
    );
    assert_eq!(valid, vec![(raw, ())]);
    assert_eq!(failures.len(), 1);
}

#[test]
fn duplicate_serial_id_remains_ambiguous_and_does_not_open_raw_fallback() {
    let first = bridge_endpoint("serial:first", "duplicate");
    let second = bridge_endpoint("serial:second", "duplicate");
    let raw = BridgeEndpoint::official_linux_usb(3, 4, "duplicate");
    let mut attempts = Vec::new();

    let (valid, failures) =
        open_output_candidates_with(vec![raw, first, second], Some("duplicate"), |candidate| {
            attempts.push(candidate.kind());
            Ok::<_, &str>(())
        });

    assert_eq!(
        attempts,
        vec![
            bridge_output::BridgeTransportKind::SerialPort,
            bridge_output::BridgeTransportKind::SerialPort,
        ]
    );
    assert_eq!(valid.len(), 2);
    assert!(failures.is_empty());
    assert!(choose_output_index(&valid, Some("duplicate")).is_err());
}

#[test]
fn remembered_output_opens_without_handshaking_unrelated_devices() {
    let other = bridge_endpoint("serial:other", "other");
    let preferred = bridge_endpoint("serial:preferred", "preferred");
    let mut attempts = Vec::new();

    let (valid, failures) =
        open_output_candidates_with(vec![other, preferred.clone()], Some("preferred"), |candidate| {
            attempts.push(candidate.stable_id().unwrap().to_owned());
            Ok::<_, &str>(())
        });

    assert_eq!(attempts, ["preferred"]);
    assert_eq!(valid, vec![(preferred, ())]);
    assert!(failures.is_empty());
}

#[test]
fn charge_states_follow_the_sdl_triton_values() {
    assert_eq!(
        ControllerChargeState::from_raw(1),
        ControllerChargeState::Discharging
    );
    assert_eq!(
        ControllerChargeState::from_raw(2),
        ControllerChargeState::Charging
    );
    assert_eq!(
        ControllerChargeState::from_raw(4),
        ControllerChargeState::Charged
    );
    assert_eq!(
        ControllerChargeState::from_raw(3),
        ControllerChargeState::Unknown(3)
    );
}

#[test]
fn puck_dock_shutdown_is_exact_edge_triggered_and_one_shot() {
    let config = RuntimeConfig {
        puck_dock_action: PuckDockAction::PowerOff,
        ..RuntimeConfig::default()
    };
    let puck = controller_info(
        steam_controller_device::PROTEUS_PRODUCT_ID,
        steam_controller_device::FIRST_PROTEUS_SLOT_INTERFACE,
        "USB",
    );
    let bluetooth = controller_info(
        steam_controller_device::STEAM_CONTROLLER_BLUETOOTH_PRODUCT_ID,
        steam_controller_device::BLUETOOTH_CONTROLLER_INTERFACE,
        "Bluetooth",
    );
    let mut automatic = AutomaticShutdownRuntime::new(&config);
    automatic.source_selected(&puck, &config);
    assert!(!automatic.observe_charge_state(
        &puck,
        ControllerChargeState::Discharging,
        PuckDockAction::PowerOff
    ));
    assert!(automatic.observe_charge_state(
        &puck,
        ControllerChargeState::Charging,
        PuckDockAction::PowerOff
    ));
    automatic.succeeded(Instant::now(), ShutdownTrigger::PuckDock);
    assert!(!automatic.observe_charge_state(
        &puck,
        ControllerChargeState::Charged,
        PuckDockAction::PowerOff
    ));

    automatic.source_selected(&puck, &config);
    assert!(!automatic.observe_charge_state(
        &puck,
        ControllerChargeState::Charging,
        PuckDockAction::PowerOff
    ));
    automatic.observe_charge_state(
        &puck,
        ControllerChargeState::Discharging,
        PuckDockAction::PowerOff,
    );
    assert!(automatic.observe_charge_state(
        &puck,
        ControllerChargeState::Charging,
        PuckDockAction::PowerOff
    ));
    assert!(!automatic.observe_charge_state(
        &bluetooth,
        ControllerChargeState::Charging,
        PuckDockAction::PowerOff
    ));
    assert!(!automatic.observe_charge_state(
        &puck,
        ControllerChargeState::Unknown(3),
        PuckDockAction::PowerOff
    ));
}

#[test]
fn power_off_burst_is_scheduled_and_one_success_is_sufficient() {
    let (ack, _receiver) = mpsc::channel();
    let writer =
        FakePowerOffWriter::new([Err("first".to_owned()), Ok(()), Err("third".to_owned())]);
    let mut sequence = PowerOffSequence::new(ack, Duration::ZERO);
    assert_eq!(sequence.service(Duration::ZERO, &writer), None);
    assert_eq!(sequence.service(Duration::from_millis(9), &writer), None);
    assert_eq!(writer.writes.load(Ordering::Relaxed), 1);
    assert_eq!(sequence.service(Duration::from_millis(10), &writer), None);
    assert_eq!(
        sequence.service(Duration::from_millis(20), &writer),
        Some(Ok(()))
    );
    assert_eq!(writer.writes.load(Ordering::Relaxed), 3);
}

#[test]
fn all_failed_power_off_writes_report_the_last_error() {
    let (ack, _receiver) = mpsc::channel();
    let writer = FakePowerOffWriter::new([
        Err("one".to_owned()),
        Err("two".to_owned()),
        Err("three".to_owned()),
    ]);
    let mut sequence = PowerOffSequence::new(ack, Duration::ZERO);
    assert_eq!(sequence.service(Duration::ZERO, &writer), None);
    assert_eq!(sequence.service(Duration::from_millis(10), &writer), None);
    assert_eq!(
        sequence.service(Duration::from_millis(20), &writer),
        Some(Err("three".to_owned()))
    );
}

#[test]
fn start_stop_and_shutdown_are_idempotent_while_waiting() {
    let handle = BridgeRuntime::spawn(RuntimeConfig {
        controller: ControllerSelection::Index(usize::MAX),
        output: OutputSelection::Mock,
        ..RuntimeConfig::default()
    });
    assert!(!handle.is_terminated());
    handle
        .set_idle_shutdown_timeout(Some(Duration::from_mins(5)))
        .unwrap();
    handle
        .set_idle_shutdown_timeout(Some(Duration::from_mins(5)))
        .unwrap();
    handle
        .set_puck_dock_action(PuckDockAction::PowerOff)
        .unwrap();
    let status = handle.status();
    assert_eq!(
        status.automatic_shutdown.configured_timeout,
        Some(Duration::from_mins(5))
    );
    assert_eq!(
        status.automatic_shutdown.puck_dock_action,
        PuckDockAction::PowerOff
    );
    handle.stop().unwrap();
    handle.stop().unwrap();
    assert_eq!(handle.status().state, RuntimeState::Stopped);
    handle.start().unwrap();
    handle.start().unwrap();
    handle.suspend_for_sleep().unwrap();
    let suspended = handle.status();
    assert_eq!(suspended.state, RuntimeState::Stopped);
    assert_eq!(suspended.detail, "Suspended for system sleep");
    handle.request_resume_from_wake().unwrap();
    // Stopping during the wake-settle window must cancel the pending
    // automatic restart just as an explicit user stop would.
    handle.stop().unwrap();
    handle.shutdown().unwrap();
    assert!(handle.is_terminated());
}

#[test]
fn updater_suspension_preserves_user_intent_and_composes_with_sleep() {
    let handle = BridgeRuntime::spawn(RuntimeConfig {
        controller: ControllerSelection::Index(usize::MAX),
        output: OutputSelection::Mock,
        ..RuntimeConfig::default()
    });

    handle.suspend_for_update().unwrap();
    let suspended = handle.status();
    assert_eq!(suspended.state, RuntimeState::Stopped);
    assert_eq!(suspended.detail, "Suspended for application update");

    handle.stop().unwrap();
    handle.resume_from_update().unwrap();
    assert_eq!(handle.status().detail, "Bridge stopped");

    handle.start().unwrap();
    handle.suspend_for_update().unwrap();
    handle.suspend_for_sleep().unwrap();
    handle.resume_from_update().unwrap();
    assert_eq!(handle.status().detail, "Suspended for system sleep");

    handle.request_resume_from_wake().unwrap();
    handle.stop().unwrap();
    handle.shutdown().unwrap();
}
