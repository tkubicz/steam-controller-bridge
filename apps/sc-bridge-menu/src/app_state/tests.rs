use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use desktop_bindings::BindingStore;

use super::*;

fn temporary_settings_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "steam-controller-bridge-{name}-{}-settings.json",
        std::process::id()
    ))
}

#[test]
fn settings_path_uses_app_path_policy() {
    let expected = app_paths::current().unwrap().settings_file();
    assert_eq!(settings_path().unwrap(), expected);
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
    assert!(path.is_dir());
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

    assert!(!stale.exists());
    assert!(unrelated.exists());
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
    assert_eq!(settings.idle_shutdown_minutes, Some(10));
    assert_eq!(settings.active_binding_profile, "gaming");
    assert!(!settings.profile_overlay_enabled);
    assert_eq!(settings.profile_overlay_hold_ms, OVERLAY_HOLD_CHOICES[0]);
    assert!(settings.picker_config().is_none());
    let _ = fs::remove_file(path);
}

#[test]
fn a_hold_duration_the_menu_cannot_offer_falls_back_alone() {
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
    assert_eq!(mailbox.len(), 1);

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
    assert_eq!(mailbox.len(), 2);
    assert!(!mailbox.publish(PickerEvent::Commit {
        index: 2,
        roster_revision: 4,
    }));
    assert_eq!(
        mailbox.pop(),
        Some(PickerEvent::Commit {
            index: 2,
            roster_revision: 4,
        })
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
