use super::icons::{template_icon_rgba, ICON_HEIGHT, ICON_RENDER_SCALE, ICON_WIDTH};
use super::logging::diagnostics_text;
use super::support::{default_overlay_hold_ms, SETTINGS_VERSION};
use super::system::privacy_pane_url;
use super::*;

#[test]
fn a_refused_permission_sends_the_user_to_the_pane_that_grants_it() {
    // macOS shows no dialog once it has recorded a refusal, so the pane is
    // the only remaining route, and each permission has its own.
    assert_ne!(
        privacy_pane_url(PrivacyPane::InputMonitoring),
        privacy_pane_url(PrivacyPane::Accessibility),
    );
    for pane in [PrivacyPane::InputMonitoring, PrivacyPane::Accessibility] {
        let url = privacy_pane_url(pane);
        assert!(
            url.starts_with("x-apple.systempreferences:"),
            "{pane:?} must open System Settings, got {url}",
        );
    }
}

#[test]
fn permission_requests_never_skip_input_monitoring_or_post_event() {
    assert_eq!(
        permission_stage(false, false, false),
        PermissionStage::InputMonitoring
    );
    assert_eq!(
        permission_stage(false, true, true),
        PermissionStage::InputMonitoring
    );
    assert_eq!(
        permission_stage(true, false, false),
        PermissionStage::PostEvent
    );
    assert_eq!(
        permission_stage(true, false, true),
        PermissionStage::PostEvent
    );
    assert_eq!(
        permission_stage(true, true, false),
        PermissionStage::Accessibility
    );
    assert_eq!(permission_stage(true, true, true), PermissionStage::Ready);
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
    assert!(text.contains("xiao:"));
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
        xiao: bridge_runtime::XiaoStatus {
            path: Some("/dev/cu.usbmodem11201".to_owned()),
            usb_serial: Some("5E6EF905E5468F85".to_owned()),
            handshake_complete: true,
        },
        ..BridgeStatus::default()
    });
    assert!(!text.contains("a1b2c3d4e5f6"));
    assert!(text.contains("****e5f6"));
    // The XIAO's MCU serial is a stable hardware identifier too.
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
