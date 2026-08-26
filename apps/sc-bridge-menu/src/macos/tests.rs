use super::icons::{template_icon_rgba, ICON_HEIGHT, ICON_RENDER_SCALE, ICON_WIDTH};
use super::logging::diagnostics_text;
use super::permissions::{
    advance_permission_requirements, menu_capability_context, should_open_remedy, PermissionAdvance,
};
use super::*;
use platform_capabilities::Remedy;

#[test]
fn firmware_update_menu_labels_preserve_the_visible_ampersand() {
    let item = MenuItem::new(FIRMWARE_UPDATES_LABEL, true, None);
    assert_eq!(item.text(), "Firmware & Updates");
    item.set_text(UPDATE_AVAILABLE_LABEL);
    assert_eq!(item.text(), "Update Available");
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
        bridge.bridge_device_or_firmware_enabled,
    );
    assert!(bridge.controller_input_enabled);
    assert!(bridge.bridge_device_or_firmware_enabled);
    assert!(!bridge.virtual_output_enabled);
    assert!(bridge.desktop_bindings_enabled);
    assert_eq!(bridge.desktop_session, Some(DesktopSession::MacOs));

    let virtual_hid = menu_capability_context(OutputPreference::VirtualHid, true);
    assert_eq!(
        virtual_hid.bridge_device_or_firmware_enabled,
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
            ..bridge_runtime::OutputStatus::configured(
                &bridge_runtime::OutputSelection::BridgeDevice,
            )
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
        let rgba_pixels = pixels.as_chunks::<4>().0;
        assert!(rgba_pixels.iter().any(|pixel| pixel[3] != 0));
        assert!(
            rgba_pixels
                .iter()
                .any(|pixel| pixel[3] > 0 && pixel[3] < 255),
            "{state:?} should retain anti-aliased edges"
        );
        let occupied_rows: Vec<_> = rgba_pixels
            .chunks_exact(usize::try_from(ICON_WIDTH).unwrap())
            .enumerate()
            .filter_map(|(row, pixels)| pixels.iter().any(|pixel| pixel[3] > 8).then_some(row))
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
