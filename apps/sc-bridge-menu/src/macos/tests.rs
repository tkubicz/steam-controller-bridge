use super::icons::{template_icon_rgba, ICON_HEIGHT, ICON_RENDER_SCALE, ICON_WIDTH};
use super::logging::diagnostics_text;
use super::permissions::{
    advance_permission_requirements, menu_capability_context, should_open_remedy, PermissionAdvance,
};
use super::support::{default_overlay_hold_ms, SETTINGS_VERSION};
use super::tray::{hardware_status_rows, HardwareStatusRow};
use super::*;
use platform_capabilities::Remedy;

#[test]
fn settings_path_uses_app_path_policy() {
    let expected = app_paths::current().unwrap().settings_file();
    assert_eq!(settings_path().unwrap(), expected);
}

#[test]
fn app_center_menu_actions_follow_build_capability() {
    assert_eq!(
        app_center_page_for_menu(ABOUT_ID),
        app_center_available().then_some(AppCenterPage::About)
    );
    assert_eq!(
        app_center_page_for_menu(UPDATES_ID),
        app_center_available().then_some(AppCenterPage::Updates)
    );
    assert_eq!(app_center_page_for_menu("not-an-app-center-item"), None);
}

#[test]
fn firmware_update_menu_labels_preserve_the_visible_ampersand() {
    let item = MenuItem::new(FIRMWARE_UPDATES_LABEL, true, None);
    assert_eq!(item.text(), "Firmware & Updates");
    item.set_text(UPDATE_AVAILABLE_LABEL);
    assert_eq!(item.text(), "Update Available");
}

#[test]
fn optional_hardware_rows_have_the_requested_pipeline_order() {
    let hidden = HardwareRowVisibility {
        section: false,
        firmware: true,
        controller_details: true,
    };
    assert!(hardware_status_rows(hidden).is_empty());

    assert_eq!(
        hardware_status_rows(HardwareRowVisibility {
            section: true,
            firmware: false,
            controller_details: false,
        }),
        [
            HardwareStatusRow::Input,
            HardwareStatusRow::Output,
            HardwareStatusRow::Controller,
        ]
    );
    assert_eq!(
        hardware_status_rows(HardwareRowVisibility {
            section: true,
            firmware: true,
            controller_details: true,
        }),
        [
            HardwareStatusRow::Input,
            HardwareStatusRow::Output,
            HardwareStatusRow::Firmware,
            HardwareStatusRow::Controller,
            HardwareStatusRow::Battery,
            HardwareStatusRow::Haptics,
        ]
    );
}

#[test]
fn a_blocked_capability_sends_the_user_to_the_pane_that_grants_it() {
    // macOS shows no dialog once it has recorded a refusal, so the pane is
    // the only remaining route, and each permission has its own.
    let provider = platform_capabilities::MacOsCapabilities::new();
    let input = provider
        .remedy(CapabilityId::InputMonitoring)
        .expect("input monitoring should have a remedy");
    let accessibility = provider
        .remedy(CapabilityId::Accessibility)
        .expect("accessibility should have a remedy");

    assert_ne!(input, accessibility);
    for remedy in [input, accessibility] {
        let Remedy::OpenUrl(url) = remedy else {
            panic!("macOS permission remedies must open System Settings");
        };
        assert!(
            url.starts_with("x-apple.systempreferences:"),
            "permission remedy must open System Settings, got {url}",
        );
    }
}

#[test]
fn menu_context_preserves_the_selected_output_and_desktop_requirements() {
    let bridge = menu_capability_context(OutputPreference::BridgeDevice, true);
    assert_ne!(
        bridge.virtual_output_enabled,
        bridge.serial_output_or_firmware_enabled,
    );
    assert!(bridge.controller_input_enabled);
    assert!(bridge.serial_output_or_firmware_enabled);
    assert!(!bridge.virtual_output_enabled);
    assert!(bridge.desktop_bindings_enabled);
    assert_eq!(bridge.desktop_session, Some(DesktopSession::MacOs));

    let virtual_hid = menu_capability_context(OutputPreference::VirtualHid, true);
    assert_eq!(
        virtual_hid.serial_output_or_firmware_enabled,
        cfg!(feature = "updater"),
    );
    assert!(virtual_hid.virtual_output_enabled);
}

#[test]
fn menu_permission_flow_stops_before_a_later_ordered_requirement() {
    let requirements = vec![platform_capabilities::RequirementGroup::Ordered(vec![
        CapabilityId::InputMonitoring,
        CapabilityId::PostEvent,
        CapabilityId::Accessibility,
    ])];
    let mut provider = platform_capabilities::ScriptedProvider::new(requirements)
        .with_state(CapabilityId::InputMonitoring, CapabilityState::Satisfied)
        .with_state(
            CapabilityId::PostEvent,
            CapabilityState::Blocked {
                reason: "post-event denied".to_owned(),
            },
        )
        .with_state(
            CapabilityId::Accessibility,
            CapabilityState::Blocked {
                reason: "accessibility denied".to_owned(),
            },
        )
        .with_request_state(
            CapabilityId::PostEvent,
            CapabilityState::Blocked {
                reason: "post-event denied".to_owned(),
            },
        );

    let result = advance_permission_requirements(&mut provider, &CapabilityContext::default())
        .expect("the scripted request should succeed");
    assert!(matches!(
        result,
        PermissionAdvance::Waiting(CapabilityRequestOutcome {
            id: CapabilityId::PostEvent,
            current: CapabilityState::Blocked { .. },
            ..
        })
    ));
    assert!(!provider
        .calls()
        .contains(&platform_capabilities::CapabilityCall::Probe(
            CapabilityId::Accessibility
        )));
}

#[test]
fn only_explicit_non_pending_requests_open_a_settings_remedy() {
    let pending = CapabilityRequestOutcome {
        id: CapabilityId::InputMonitoring,
        previous: CapabilityState::Undecided,
        current: CapabilityState::Pending,
        remedy: Some(Remedy::OpenUrl("settings://input".to_owned())),
    };
    let blocked = CapabilityRequestOutcome {
        id: CapabilityId::PostEvent,
        previous: CapabilityState::Undecided,
        current: CapabilityState::Blocked {
            reason: "denied".to_owned(),
        },
        remedy: Some(Remedy::OpenUrl("settings://post-event".to_owned())),
    };

    assert!(!should_open_remedy(false, &blocked));
    assert!(!should_open_remedy(true, &pending));
    assert!(should_open_remedy(true, &blocked));
}

#[test]
fn satisfied_requirements_activate_without_requesting_any_capability() {
    let requirements = vec![platform_capabilities::RequirementGroup::Ordered(vec![
        CapabilityId::InputMonitoring,
        CapabilityId::PostEvent,
        CapabilityId::Accessibility,
    ])];
    let mut provider = platform_capabilities::ScriptedProvider::new(requirements)
        .with_state(CapabilityId::InputMonitoring, CapabilityState::Satisfied)
        .with_state(CapabilityId::PostEvent, CapabilityState::Satisfied)
        .with_state(CapabilityId::Accessibility, CapabilityState::Satisfied);

    assert_eq!(
        advance_permission_requirements(&mut provider, &CapabilityContext::default())
            .expect("satisfied requirements should not fail"),
        PermissionAdvance::Ready,
    );
    assert!(provider
        .calls()
        .iter()
        .all(|call| !matches!(call, platform_capabilities::CapabilityCall::Request(_))));
}

fn temporary_settings_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "steam-controller-bridge-{name}-{}-settings.json",
        std::process::id()
    ))
}

fn temporary_log_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "steam-controller-bridge-{name}-{}-sc-bridge.log",
        std::process::id()
    ))
}

fn test_logger(path: PathBuf) -> StatusLogger {
    StatusLogger {
        directory: path.parent().unwrap().to_path_buf(),
        path,
        started: Instant::now(),
        tracker: StatusLogTracker::default(),
        pending_batch: None,
    }
}

#[test]
fn menu_settings_round_trip_and_invalid_data_falls_back() {
    let path = temporary_settings_path("round-trip");
    let settings = AppSettings {
        version: SETTINGS_VERSION,
        idle_shutdown_minutes: None,
        power_off_on_puck: true,
        output: OutputPreference::VirtualHid,
        active_binding_profile: "gaming".to_owned(),
        profile_overlay_enabled: true,
        profile_overlay_hold_ms: 3_000,
    };
    save_settings(&path, &settings).unwrap();
    assert_eq!(load_settings(&path), (settings, None));

    fs::write(&path, b"not json").unwrap();
    let (fallback, warning) = load_settings(&path);
    assert_eq!(fallback, AppSettings::default());
    assert!(warning.is_some());

    save_settings(
        &path,
        &AppSettings {
            version: SETTINGS_VERSION + 1,
            idle_shutdown_minutes: Some(1),
            power_off_on_puck: true,
            active_binding_profile: "default".to_owned(),
            ..AppSettings::default()
        },
    )
    .unwrap();
    let (fallback, warning) = load_settings(&path);
    assert_eq!(fallback, AppSettings::default());
    assert!(warning.is_some());
    let _ = fs::remove_file(path);
}

#[test]
fn version_three_settings_migrate_to_the_bridge_device_and_version_four_preserves_virtual_hid() {
    let path = temporary_settings_path("output-migration");
    fs::write(
        &path,
        br#"{"version":3,"idle_shutdown_minutes":15,"power_off_on_puck":false}"#,
    )
    .unwrap();
    let (migrated, warning) = load_settings(&path);
    assert!(warning.is_none());
    assert_eq!(migrated.version, SETTINGS_VERSION);
    assert_eq!(migrated.output, OutputPreference::BridgeDevice);

    let virtual_hid = AppSettings {
        output: OutputPreference::VirtualHid,
        ..AppSettings::default()
    };
    save_settings(&path, &virtual_hid).unwrap();
    assert_eq!(load_settings(&path), (virtual_hid, None));
    let _ = fs::remove_file(path);
}

#[test]
fn a_disabled_virtual_hid_gate_uses_serial_without_destroying_the_saved_preference() {
    let saved = OutputPreference::VirtualHid;
    assert_eq!(
        saved.when_virtual_hid_enabled(false),
        OutputPreference::BridgeDevice
    );
    assert_eq!(
        saved.when_virtual_hid_enabled(true),
        OutputPreference::VirtualHid
    );
    assert_eq!(saved, OutputPreference::VirtualHid);
}

#[test]
fn bundled_helper_resolution_never_searches_path() {
    let executable =
        Path::new("/Applications/Steam Controller Bridge.app/Contents/MacOS/sc-bridge-menu");
    assert_eq!(
        bundled_virtual_hid_helper_path_from(executable).unwrap(),
        PathBuf::from(
            "/Applications/Steam Controller Bridge.app/Contents/Helpers/Steam Controller Bridge Virtual HID Helper.app/Contents/MacOS/sc-virtual-hid-helper"
        )
    );
    assert!(bundled_virtual_hid_helper_path_from(Path::new("/tmp/sc-bridge-menu")).is_err());
}

#[test]
fn failed_settings_rename_preserves_the_target_and_removes_the_temporary_file() {
    let directory = std::env::temp_dir().join(format!(
        "steam-controller-bridge-settings-failure-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("settings.json");
    fs::create_dir(&path).unwrap();

    assert!(save_settings(&path, &AppSettings::default()).is_err());
    assert!(
        path.is_dir(),
        "the existing settings target must be preserved"
    );
    let entries = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, [std::ffi::OsString::from("settings.json")]);

    let _ = fs::remove_dir_all(directory);
}

#[test]
fn save_settings_sweeps_stale_temporaries_from_earlier_crashes() {
    let directory = std::env::temp_dir().join(format!(
        "steam-controller-bridge-settings-sweep-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("settings.json");
    let stale = directory.join("settings.json.999.123456.0.tmp");
    fs::write(&stale, b"{}").unwrap();
    let unrelated = directory.join("bindings.json");
    fs::write(&unrelated, b"{}").unwrap();

    save_settings(&path, &AppSettings::default()).unwrap();

    assert!(!stale.exists(), "stale temporaries must be swept");
    assert!(unrelated.exists(), "unrelated files must be preserved");
    assert_eq!(load_settings(&path), (AppSettings::default(), None));

    let _ = fs::remove_dir_all(directory);
}

#[test]
fn version_one_settings_migrate_without_losing_shutdown_choices() {
    let path = temporary_settings_path("migration");
    fs::write(
        &path,
        br#"{"version":1,"idle_shutdown_minutes":30,"power_off_on_puck":true}"#,
    )
    .unwrap();
    let (settings, warning) = load_settings(&path);
    assert!(warning.is_none());
    assert_eq!(settings.version, SETTINGS_VERSION);
    assert_eq!(settings.idle_shutdown_minutes, Some(30));
    assert!(settings.power_off_on_puck);
    assert_eq!(settings.active_binding_profile, "default");
    let _ = fs::remove_file(path);
}

#[test]
fn version_two_settings_migrate_with_the_wheel_switched_off() {
    let path = temporary_settings_path("overlay-migration");
    fs::write(
            &path,
            br#"{"version":2,"idle_shutdown_minutes":10,"power_off_on_puck":false,"active_binding_profile":"gaming"}"#,
        )
        .unwrap();
    let (settings, warning) = load_settings(&path);
    assert!(warning.is_none());
    assert_eq!(settings.version, SETTINGS_VERSION);
    // Existing choices survive, and the new feature stays off so it cannot
    // take Quick Access away from a binding the user already relies on.
    assert_eq!(settings.idle_shutdown_minutes, Some(10));
    assert_eq!(settings.active_binding_profile, "gaming");
    assert!(!settings.profile_overlay_enabled);
    assert_eq!(settings.profile_overlay_hold_ms, OVERLAY_HOLD_CHOICES[0]);
    assert!(settings.picker_config().is_none());
    let _ = fs::remove_file(path);
}

#[test]
fn a_hold_duration_the_menu_cannot_offer_falls_back_alone() {
    // A hand-edited hold must not take idle shutdown, the active profile,
    // and the enablement down with it; only the bad field resets.
    let path = temporary_settings_path("overlay-bad-hold");
    fs::write(
            &path,
            br#"{"version":3,"idle_shutdown_minutes":null,"power_off_on_puck":false,"active_binding_profile":"default","profile_overlay_enabled":true,"profile_overlay_hold_ms":45000}"#,
        )
        .unwrap();
    let (settings, warning) = load_settings(&path);
    assert!(warning.is_none());
    assert_eq!(settings.profile_overlay_hold_ms, default_overlay_hold_ms());
    assert!(settings.profile_overlay_enabled);
    assert_eq!(settings.idle_shutdown_minutes, None);
    let _ = fs::remove_file(path);
}

#[test]
fn an_enabled_wheel_configures_the_chosen_hold() {
    let settings = AppSettings {
        profile_overlay_enabled: true,
        profile_overlay_hold_ms: 3_000,
        ..AppSettings::default()
    };
    let config = settings.picker_config().expect("the wheel is enabled");
    assert_eq!(config.hold, Duration::from_secs(3));
    assert_eq!(
        config.sectors_per_page,
        PickerConfig::default().sectors_per_page
    );
}

#[test]
fn the_roster_reports_the_active_profiles_position() {
    let mut store = BindingStore::default();
    store.create_profile("Gaming").unwrap();
    store.create_profile("Couch").unwrap();
    assert_eq!(store.profiles.len(), 3);

    let second = store.profiles[1].id.clone();
    let roster = picker_roster(&store, &second, 7);
    assert_eq!(roster.len, 3);
    assert_eq!(roster.active, Some(1));
    assert_eq!(roster.revision, 7);
    assert!(roster.is_openable());

    // A profile that no longer exists must not point the wheel somewhere
    // arbitrary; it opens on the first sector instead.
    let roster = picker_roster(&store, "deleted-profile", 8);
    assert_eq!(roster.len, 3);
    assert_eq!(roster.active, None);
    assert_eq!(roster.revision, 8);
}

#[test]
fn a_single_profile_store_cannot_open_the_wheel() {
    let store = BindingStore::default();
    assert_eq!(store.profiles.len(), 1);
    assert!(!picker_roster(&store, &store.profiles[0].id, 0).is_openable());
}

#[test]
fn picker_event_mailbox_coalesces_visual_updates_and_bounds_backlog() {
    let mailbox = PickerEventMailbox::default();
    assert!(mailbox.publish(PickerEvent::Preparing));
    assert!(!mailbox.publish(PickerEvent::Opened {
        selected: 0,
        page: 0,
        roster_revision: 4,
    }));
    assert_eq!(mailbox.len(), 1, "Opened replaces its pending preparation");

    assert!(!mailbox.publish(PickerEvent::Selection {
        selected: 1,
        page: 0,
        roster_revision: 4,
    }));
    assert!(!mailbox.publish(PickerEvent::Selection {
        selected: 2,
        page: 0,
        roster_revision: 4,
    }));
    assert_eq!(mailbox.len(), 2, "only the latest selection is useful");
    assert!(!mailbox.publish(PickerEvent::Commit {
        index: 2,
        roster_revision: 4,
    }));
    assert_eq!(
        mailbox.pop(),
        Some(PickerEvent::Commit {
            index: 2,
            roster_revision: 4,
        }),
        "a terminal event supersedes every pending visual update"
    );
    assert!(mailbox.pop().is_none());

    for _ in 0..=PICKER_EVENT_MAILBOX_CAPACITY {
        let _ = mailbox.publish(PickerEvent::Dismissed);
    }
    assert_eq!(mailbox.len(), PICKER_EVENT_MAILBOX_CAPACITY);
}

#[test]
fn picker_commits_only_resolve_against_the_roster_the_wheel_used() {
    let ids = vec!["default".to_owned(), "gaming".to_owned()];
    assert_eq!(resolve_picker_commit(&ids, 7, 7, 1), Some("gaming"));
    assert_eq!(resolve_picker_commit(&ids, 8, 7, 1), None);
    assert_eq!(resolve_picker_commit(&ids, 7, 7, 2), None);
}

#[test]
fn diagnostics_include_hardware_and_safety_state() {
    let text = diagnostics_text(&BridgeStatus::default());
    assert!(text.contains("source:"));
    assert!(text.contains("output:"));
    assert!(text.contains("lizard:"));
    assert!(text.contains("haptics:"));
    assert!(text.contains("automatic_shutdown:"));
    assert!(text.contains("output_diagnostics:"));
}

#[test]
fn menu_logger_writes_periodic_snapshots_without_a_revision_change() {
    let path = temporary_log_path("periodic");
    let _ = fs::remove_file(&path);
    let mut logger = test_logger(path.clone());
    let status = BridgeStatus::default();
    logger
        .write_status_at(&status, Duration::ZERO, 100)
        .unwrap();
    logger
        .write_status_at(&status, bridge_runtime::STATUS_SNAPSHOT_INTERVAL, 400)
        .unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("event=status_snapshot").count(), 2);
    assert!(text.contains("reason=startup"));
    assert!(text.contains("reason=periodic"));
    let _ = fs::remove_file(path);
}

#[test]
fn menu_logger_writes_overlay_diagnostics_with_timestamps() {
    let path = temporary_log_path("overlay-diagnostics");
    let _ = fs::remove_file(&path);
    let mut logger = test_logger(path.clone());
    logger
        .write_diagnostics(&[
            "level=info event=profile_overlay_started".to_owned(),
            "level=info event=overlay_window_shown level=1001".to_owned(),
        ])
        .unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("timestamp=").count(), 2);
    assert!(text.contains("event=profile_overlay_started"));
    assert!(text.contains("event=overlay_window_shown level=1001"));
    let _ = fs::remove_file(path);
}

#[test]
fn rotation_keeps_an_error_change_and_snapshot_in_the_same_file() {
    let path = temporary_log_path("rotation");
    let rotated = path.with_extension("log.1");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(&rotated);
    let mut logger = test_logger(path.clone());
    let initial = BridgeStatus::default();
    logger
        .write_status_at(&initial, Duration::ZERO, 100)
        .unwrap();
    fs::write(
        &path,
        vec![b'x'; usize::try_from(LOG_LIMIT_BYTES - 16).unwrap()],
    )
    .unwrap();

    let mut failed = initial;
    failed.revision = 1;
    failed.last_error = Some("controller failed".to_owned());
    logger
        .write_status_at(&failed, Duration::from_secs(1), 101)
        .unwrap();

    let active = fs::read_to_string(&path).unwrap();
    assert!(active.contains("event=status_change"));
    assert!(active.contains("event=status_snapshot reason=error"));
    assert!(rotated.metadata().unwrap().len() >= LOG_LIMIT_BYTES - 16);
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(rotated);
}

#[test]
fn failed_writes_are_retried_without_losing_the_record() {
    let directory = std::env::temp_dir().join(format!(
        "steam-controller-bridge-retry-{}",
        std::process::id()
    ));
    let path = directory.join("sc-bridge.log");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir(&directory);
    let mut logger = test_logger(path.clone());
    let status = BridgeStatus::default();

    assert!(logger
        .write_status_at(&status, Duration::ZERO, 100)
        .is_err());
    fs::create_dir_all(&directory).unwrap();
    logger
        .write_status_at(&status, Duration::from_secs(1), 101)
        .unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("event=status_snapshot").count(), 1);
    assert!(text.contains("timestamp=100"));
    let _ = fs::remove_file(path);
    let _ = fs::remove_dir(directory);
}

#[test]
fn oversized_error_batches_are_explicitly_truncated_to_the_log_limit() {
    let path = temporary_log_path("oversized");
    let rotated = path.with_extension("log.1");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(&rotated);
    let mut logger = test_logger(path.clone());
    let initial = BridgeStatus::default();
    logger
        .write_status_at(&initial, Duration::ZERO, 100)
        .unwrap();

    let mut failed = initial;
    failed.revision = 1;
    failed.last_error = Some("x".repeat(usize::try_from(LOG_LIMIT_BYTES).unwrap() + 1_024));
    logger
        .write_status_at(&failed, Duration::from_secs(1), 101)
        .unwrap();

    let active = fs::read(&path).unwrap();
    assert_eq!(active.len(), usize::try_from(LOG_LIMIT_BYTES).unwrap());
    assert!(active.ends_with(LOG_TRUNCATION_MARKER.as_bytes()));
    assert!(rotated.metadata().unwrap().len() < LOG_LIMIT_BYTES);
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(rotated);
}

/// Copy Diagnostics is what the troubleshooting guide tells users to paste
/// into a public issue, so no whole serial may reach it. On Bluetooth that
/// value is the controller's MAC address.
#[test]
fn diagnostics_never_expose_a_whole_device_serial() {
    let text = diagnostics_text(&BridgeStatus {
        source: bridge_runtime::ControllerSourceStatus {
            identity: Some(steam_controller_device::HidDeviceInfo {
                id: "controller-source".to_owned(),
                path: "controller-source".to_owned(),
                vendor_id: 0x28de,
                product_id: 0x1303,
                usage_page: 0xff00,
                usage: 1,
                interface_number: -1,
                serial_number: Some("a1b2c3d4e5f6".to_owned()),
                manufacturer: Some("Valve Corporation".to_owned()),
                product: Some("Steam Ctrl (BT)".to_owned()),
                transport: "Bluetooth".to_owned(),
            }),
            transport: Some(steam_controller_device::ControllerTransport::Bluetooth),
            connected: true,
            active: true,
        },
        output: bridge_runtime::OutputStatus {
            endpoint: Some("/dev/cu.usbmodem11201".to_owned()),
            stable_id: Some("5E6EF905E5468F85".to_owned()),
            ready: true,
            firmware: Some(bridge_runtime::FirmwareInfo {
                version: bridge_runtime::FirmwareVersion::Reported(1),
                ..bridge_runtime::FirmwareInfo::default()
            }),
            ..bridge_runtime::OutputStatus::configured(&bridge_runtime::OutputSelection::Serial)
        },
        ..BridgeStatus::default()
    });
    assert!(!text.contains("a1b2c3d4e5f6"));
    assert!(text.contains("****e5f6"));
    // A bridge device's MCU serial is a stable hardware identifier too.
    assert!(!text.contains("5E6EF905E5468F85"));
    assert!(text.contains("****8F85"));
    // Transport, product, and port still have to be diagnosable.
    assert!(text.contains("Steam Ctrl (BT)"));
    assert!(text.contains("/dev/cu.usbmodem11201"));
}

#[test]
fn template_icons_are_valid_and_distinct_for_every_state() {
    let states = [
        TrayState::Off,
        TrayState::Waiting,
        TrayState::Ready,
        TrayState::Error,
    ];
    let images: Vec<_> = states
        .iter()
        .map(|state| template_icon_rgba(*state))
        .collect();
    for (state, pixels) in states.iter().zip(&images) {
        assert!(template_icon(*state).is_ok());
        assert_eq!(
            pixels.len(),
            usize::try_from(ICON_WIDTH * ICON_HEIGHT * 4).unwrap()
        );
        assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
        assert!(
            pixels
                .chunks_exact(4)
                .any(|pixel| pixel[3] > 0 && pixel[3] < 255),
            "{state:?} should retain anti-aliased edges"
        );
        let occupied_rows: Vec<_> = pixels
            .chunks_exact(usize::try_from(ICON_WIDTH * 4).unwrap())
            .enumerate()
            .filter_map(|(row, pixels)| {
                pixels
                    .chunks_exact(4)
                    .any(|pixel| pixel[3] > 8)
                    .then_some(row)
            })
            .collect();
        assert!(
            occupied_rows.last().unwrap() - occupied_rows.first().unwrap()
                >= usize::try_from(14 * ICON_RENDER_SCALE).unwrap(),
            "{state:?} should fill the native menu-bar height"
        );
    }
    for left in 0..images.len() {
        for right in left + 1..images.len() {
            assert_ne!(images[left], images[right]);
        }
    }
}
