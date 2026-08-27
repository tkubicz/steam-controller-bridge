use super::*;
use std::fs;
use std::time::Duration;
use steam_controller_protocol::SteamButtons;

#[derive(Default)]
struct MockSink {
    events: Vec<String>,
    fail_next: bool,
    fail_flush: bool,
    flushes: usize,
}

impl DesktopInputSink for MockSink {
    fn key(&mut self, key: KeyboardKey, pressed: bool) -> Result<(), String> {
        self.push(format!("key:{key:?}:{pressed}"))
    }

    fn modifier(&mut self, modifier: Modifier, pressed: bool) -> Result<(), String> {
        self.push(format!("modifier:{modifier:?}:{pressed}"))
    }

    fn mouse_button(&mut self, button: MouseButton, pressed: bool) -> Result<(), String> {
        self.push(format!("mouse:{button:?}:{pressed}"))
    }

    fn mouse_move(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.push(format!("move:{x}:{y}"))
    }

    fn scroll(&mut self, x: i32, y: i32) -> Result<(), String> {
        self.push(format!("scroll:{x}:{y}"))
    }

    fn flush(&mut self) -> Result<(), String> {
        self.flushes += 1;
        if self.fail_flush {
            self.fail_flush = false;
            Err("injected flush failure".to_owned())
        } else {
            Ok(())
        }
    }
}

impl MockSink {
    fn push(&mut self, event: String) -> Result<(), String> {
        if self.fail_next {
            self.fail_next = false;
            Err("injected failure".to_owned())
        } else {
            self.events.push(event);
            Ok(())
        }
    }
}

fn buttons(pressed: &[BindableControl]) -> SteamButtons {
    SteamButtons(pressed.iter().fold(0_u32, |mask, control| {
        mask | (1_u32 << control.steam_button() as u8)
    }))
}

fn chord(key: KeyboardKey, modifiers: &[Modifier]) -> BindingAction {
    BindingAction::KeyChord {
        key,
        modifiers: modifiers.iter().copied().collect(),
    }
}

#[test]
fn default_store_path_uses_app_path_policy() {
    let expected = app_paths::current().unwrap().bindings_file();
    assert_eq!(default_store_path().unwrap(), expected);
}

fn pad_snapshot(
    buttons: SteamButtons,
    left: Option<(i16, i16)>,
    right: Option<(i16, i16)>,
) -> DesktopInputSnapshot {
    DesktopInputSnapshot {
        buttons,
        left_pad: left.map_or_else(PadSample::default, |(x, y)| PadSample {
            x,
            y,
            touched: true,
            ..PadSample::default()
        }),
        right_pad: right.map_or_else(PadSample::default, |(x, y)| PadSample {
            x,
            y,
            touched: true,
            ..PadSample::default()
        }),
    }
}

fn touching(x: i16, y: i16) -> PadSample {
    PadSample {
        x,
        y,
        touched: true,
        ..PadSample::NEUTRAL
    }
}

fn clicking(x: i16, y: i16) -> PadSample {
    PadSample {
        pressed: true,
        ..touching(x, y)
    }
}

fn side_snapshot(side: PadSide, sample: PadSample) -> DesktopInputSnapshot {
    let mut snapshot = DesktopInputSnapshot::buttons_only(SteamButtons::default());
    *match side {
        PadSide::Left => &mut snapshot.left_pad,
        PadSide::Right => &mut snapshot.right_pad,
    } = sample;
    snapshot
}

/// One region covering the whole pad, binding `action` to `trigger`. This is
/// what a pre-region pad-click binding becomes.
fn whole_pad(trigger: PadTrigger, action: BindingAction) -> Vec<PadRegion> {
    let mut regions = PadRegion::whole();
    *regions[0].action_mut(trigger) = Some(action);
    regions
}

/// Feeds a pad one timed sample per entry and returns the last feedback request.
fn drive_pad(
    engine: &mut BindingEngine,
    sink: &mut MockSink,
    side: PadSide,
    steps: &[(u64, PadSample)],
) -> PadFeedbackRequest {
    let mut last = PadFeedbackRequest::NONE;
    for (millis, sample) in steps {
        last = engine
            .observe_snapshot(
                side_snapshot(side, *sample),
                Duration::from_millis(*millis),
                sink,
            )
            .unwrap();
    }
    last
}

#[test]
fn store_round_trips_and_defaults_are_unbound() {
    let store = BindingStore::default();
    assert_eq!(store.profiles[0].bindings.configured_count(), 0);
    assert_eq!(store.profiles[0].configured_output_count(), 0);
    for side in PadSide::ALL {
        let pad = store.profiles[0].pads.get(side);
        assert_eq!(pad.motion, PadMotionMode::None);
        assert!(pad.regions.is_empty());
        assert!(pad.feedback.enabled);
        assert_eq!(pad.feedback.strength, PadFeedbackStrength::Medium);
        assert_eq!(pad.speed_percent, DEFAULT_PAD_SPEED_PERCENT);
        assert!(pad.momentum);
    }
    let bytes = serde_json::to_vec(&store).unwrap();
    let decoded: BindingStore = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, store);
    decoded.validate().unwrap();
}

#[test]
fn store_rejects_unknown_pad_feedback_strength() {
    let json = br#"{
          "version": 3,
          "profiles": [{
            "id": "default",
            "name": "Default",
            "bindings": {},
            "pads": {
              "right_mouse": {
                "enabled": true,
                "feedback": {"enabled": true, "strength": "extreme"}
              }
            }
          }]
        }"#;
    assert!(parse_store(json).is_err());
}

#[test]
fn documented_version_one_json_parses_with_stable_action_names() {
    let json = r#"{
          "version": 1,
          "profiles": [{
            "id": "default",
            "name": "Default",
            "bindings": {
              "l4": null,
              "l5": null,
              "r4": {"kind": "key_chord", "key": "F5", "modifiers": []},
              "r5": {"kind": "key_chord", "key": "F9", "modifiers": ["command"]},
              "quick_access": {"kind": "mouse_button", "button": "middle"}
            }
          }]
        }"#;
    let store = parse_store(json.as_bytes()).unwrap();
    assert_eq!(store.version, BINDINGS_VERSION);
    assert_eq!(store.profiles[0].pads, PadBindings::default());
    assert_eq!(store.profiles[0].bindings.configured_count(), 3);
    assert_eq!(
        store.profiles[0].bindings.r4.as_ref().unwrap().label(),
        "F5"
    );
    assert_eq!(
        store.profiles[0]
            .bindings
            .quick_access
            .as_ref()
            .unwrap()
            .label(),
        "Mouse Middle"
    );
    assert!(
        serde_json::from_str::<BindingStore>(&json.replace("key_chord", "raw_keycode")).is_err()
    );
    assert!(serde_json::from_str::<BindingStore>(&json.replace(
        "\"modifiers\": []",
        "\"modifiers\": [], \"raw_keycode\": 96"
    ))
    .is_err());
}

#[test]
fn store_rejects_duplicate_names_and_invalid_profile_counts() {
    let mut store = BindingStore::default();
    store.profiles.push(BindingProfile {
        id: "other".to_owned(),
        name: "default".to_owned(),
        bindings: ControlBindings::default(),
        pads: PadBindings::default(),
    });
    assert!(store
        .validate()
        .unwrap_err()
        .contains("duplicate profile name"));
    store.profiles.clear();
    assert!(store.validate().is_err());

    let mut oversized = BindingStore::default();
    for index in 1..=MAX_PROFILES {
        oversized.profiles.push(BindingProfile {
            id: format!("profile-{index}"),
            name: format!("Profile {index}"),
            bindings: ControlBindings::default(),
            pads: PadBindings::default(),
        });
    }
    assert!(oversized.validate().is_err());
}

#[test]
fn profile_operations_trim_names_preserve_ids_and_enforce_bounds() {
    let mut store = BindingStore::default();
    store.profiles[0].bindings.r4 = Some(chord(KeyboardKey::F5, &[]));
    let created = store.create_profile("  Gaming  ").unwrap();
    assert_eq!(store.profile_by_id(&created).unwrap().name, "Gaming");
    let duplicate = store
        .duplicate_profile(DEFAULT_PROFILE_ID, "Default Copy")
        .unwrap();
    assert_eq!(
        store
            .profile_by_id(&duplicate)
            .unwrap()
            .bindings
            .configured_count(),
        1
    );
    store.rename_profile(&created, "Games").unwrap();
    assert_eq!(store.profile_by_id(&created).unwrap().id, created);
    assert!(store.rename_profile(&created, "default").is_err());
    store.delete_profile(&duplicate).unwrap();
    store.delete_profile(&created).unwrap();
    assert!(store.delete_profile(DEFAULT_PROFILE_ID).is_err());
}

#[test]
fn profile_identity_rules_match_validation_and_lookup() {
    let mut store = BindingStore::default();
    store.create_profile("Ä").unwrap();
    assert!(store.create_profile("ä").is_err());

    store.profiles[1].id = "PROFILE-1".to_owned();
    store.validate().unwrap();
    assert_eq!(store.profile_by_id("profile-1").unwrap().name, "Ä");
    assert_eq!(store.next_profile_id(), "profile-2");
}

#[test]
fn store_is_created_and_atomically_persisted() {
    let directory = std::env::temp_dir().join(format!(
        "desktop-bindings-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("persistence")
    ));
    let path = directory.join("bindings.json");
    let mut store = load_or_create_store(&path).unwrap();
    assert_eq!(store, BindingStore::default());
    store.create_profile("Second").unwrap();
    save_store(&path, &store).unwrap();
    assert_eq!(load_store(&path).unwrap(), store);
    let _ = fs::remove_file(path);
    let _ = fs::remove_dir(directory);
}

#[test]
fn loading_version_one_atomically_migrates_to_the_current_version() {
    let directory =
        std::env::temp_dir().join(format!("desktop-bindings-migration-{}", std::process::id()));
    let path = directory.join("bindings.json");
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
            &path,
            br#"{"version":1,"profiles":[{"id":"default","name":"Default","bindings":{"r4":{"kind":"key_chord","key":"F5","modifiers":[]}}}]}"#,
        )
        .unwrap();

    let store = load_store(&path).unwrap();
    assert_eq!(store.version, BINDINGS_VERSION);
    assert_eq!(
        store.profiles[0].bindings.r4.as_ref().unwrap().label(),
        "F5"
    );
    assert_eq!(store.profiles[0].pads, PadBindings::default());
    let persisted: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(persisted["version"], BINDINGS_VERSION);
    assert!(persisted["profiles"][0]["pads"].is_object());
    let _ = fs::remove_file(path);
    let _ = fs::remove_dir(directory);
}

#[test]
fn loading_version_three_migrates_scroll_settings_and_leaves_the_pad_unregioned() {
    let json = br#"{
          "version": 3,
          "profiles": [{
            "id": "default",
            "name": "Default",
            "bindings": {
              "r4": {"kind": "key_chord", "key": "F5", "modifiers": []}
            },
            "pads": {
              "left_scroll": {
                "enabled": true,
                "feedback": {"enabled": true, "strength": "high"},
                "speed_percent": 150,
                "momentum": false
              }
            }
          }]
        }"#;
    let store = parse_store(json).unwrap();
    assert_eq!(store.version, BINDINGS_VERSION);
    assert_eq!(store.profiles[0].bindings.configured_count(), 1);
    let left = &store.profiles[0].pads.left;
    assert_eq!(left.motion, PadMotionMode::Scroll);
    assert_eq!(left.speed_percent, 150);
    assert!(!left.momentum);
    assert!(left.regions.is_empty());
    assert_eq!(store.profiles[0].pads.right.motion, PadMotionMode::None);
}

#[test]
fn a_version_four_pad_click_migrates_onto_a_whole_pad_region() {
    let json = br#"{
          "version": 4,
          "profiles": [{
            "id": "default",
            "name": "Default",
            "bindings": {
              "r4": {"kind": "key_chord", "key": "F5", "modifiers": []},
              "left_pad_click": {"kind": "key_chord", "key": "F9", "modifiers": ["shift"]},
              "right_pad_click": {"kind": "mouse_button", "button": "middle"}
            },
            "pads": {
              "right_mouse": {
                "enabled": true,
                "feedback": {"enabled": false, "strength": "low"},
                "speed_percent": 200
              },
              "left_scroll": {"enabled": false, "speed_percent": 175, "momentum": false}
            }
          }]
        }"#;
    let store = parse_store(json).unwrap();
    assert_eq!(store.version, BINDINGS_VERSION);
    // Untouched by the migration.
    assert_eq!(store.profiles[0].id, "default");
    assert_eq!(store.profiles[0].bindings.configured_count(), 1);

    let left = &store.profiles[0].pads.left;
    assert_eq!(left.motion, PadMotionMode::None);
    // A disabled pad keeps its settings, so re-enabling restores the tuning.
    assert_eq!(left.speed_percent, 175);
    assert!(!left.momentum);
    assert_eq!(left.regions.len(), 1);
    assert_eq!(left.regions[0].shape, PadRegionShape::WHOLE);
    assert_eq!(
        left.regions[0].click,
        Some(chord(KeyboardKey::F9, &[Modifier::Shift]))
    );
    assert!(left.regions[0].touch.is_none());

    let right = &store.profiles[0].pads.right;
    assert_eq!(right.motion, PadMotionMode::Pointer);
    assert_eq!(right.speed_percent, 200);
    assert!(!right.feedback.enabled);
    assert_eq!(right.feedback.strength, PadFeedbackStrength::Low);
    assert_eq!(
        right.regions[0].click,
        Some(BindingAction::MouseButton {
            button: MouseButton::Middle
        })
    );
    store.validate().unwrap();
}

#[test]
fn region_bindings_round_trip_and_count() {
    let mut store = BindingStore::default();
    let mut regions = PadRegion::four_way();
    regions[3].click = Some(chord(KeyboardKey::ArrowLeft, &[]));
    regions[3].touch = Some(BindingAction::MouseButton {
        button: MouseButton::Middle,
    });
    store.profiles[0].pads.left.regions = regions;
    store.profiles[0].pads.right.motion = PadMotionMode::Pointer;
    assert_eq!(store.profiles[0].configured_output_count(), 3);
    store.validate().unwrap();

    let bytes = serde_json::to_vec(&store).unwrap();
    let decoded: BindingStore = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, store);
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let left = &value["profiles"][0]["pads"]["left"];
    assert_eq!(left["motion"], "none");
    assert_eq!(left["regions"][3]["name"], "Left");
    assert_eq!(left["regions"][3]["click"]["key"], "ArrowLeft");
    assert_eq!(left["regions"][3]["touch"]["button"], "middle");
    assert_eq!(value["profiles"][0]["pads"]["right"]["motion"], "pointer");
}

#[test]
fn loading_version_two_preserves_pads_and_adds_scroll_defaults() {
    let json = br#"{
          "version": 2,
          "profiles": [{
            "id": "default",
            "name": "Default",
            "bindings": {},
            "pads": {
              "right_mouse": {
                "enabled": true,
                "feedback": {"enabled": false, "strength": "low"}
              },
              "left_scroll": {
                "enabled": true,
                "feedback": {"enabled": true, "strength": "high"}
              }
            }
          }]
        }"#;
    let store = parse_store(json).unwrap();
    assert_eq!(store.version, BINDINGS_VERSION);
    assert_eq!(store.profiles[0].pads.right.motion, PadMotionMode::Pointer);
    assert!(!store.profiles[0].pads.right.feedback.enabled);
    let scroll = &store.profiles[0].pads.left;
    assert_eq!(scroll.motion, PadMotionMode::Scroll);
    assert_eq!(scroll.feedback.strength, PadFeedbackStrength::High);
    assert_eq!(scroll.speed_percent, DEFAULT_PAD_SPEED_PERCENT);
    assert!(scroll.momentum);
}

#[test]
fn store_rejects_pad_speed_outside_supported_range() {
    for side in PadSide::ALL {
        let mut store = BindingStore::default();
        assert_eq!(
            store.profiles[0].pads.get(side).speed_percent,
            DEFAULT_PAD_SPEED_PERCENT
        );
        store.profiles[0].pads.get_mut(side).speed_percent = MIN_PAD_SPEED_PERCENT - 1;
        assert!(store.validate().unwrap_err().contains(side.label()));
        store.profiles[0].pads.get_mut(side).speed_percent = MAX_PAD_SPEED_PERCENT + 1;
        assert!(store.validate().is_err());
        store.profiles[0].pads.get_mut(side).speed_percent = 150;
        store.validate().unwrap();
    }
}

#[test]
fn store_rejects_malformed_regions() {
    /// A way to break a valid pad, and the word its rejection must contain.
    type Case = (&'static str, fn(&mut PadConfig));
    let cases: [Case; 5] = [
        ("region name", |pad| {
            pad.regions[0].name = " Top".to_owned();
        }),
        ("duplicate", |pad| {
            pad.regions[1].name = pad.regions[0].name.clone();
        }),
        ("degree sweep", |pad| {
            pad.regions[0].shape.sweep_degrees = 0;
        }),
        ("degree sweep", |pad| {
            pad.regions[0].shape.start_degrees = 360;
        }),
        ("degree sweep", |pad| {
            pad.regions[0].shape.inner_percent = pad.regions[0].shape.outer_percent;
        }),
    ];
    for (expected, break_it) in cases {
        let mut store = BindingStore::default();
        store.profiles[0].pads.left.regions = PadRegion::four_way();
        break_it(&mut store.profiles[0].pads.left);
        let error = store.validate().unwrap_err();
        assert!(
            error.contains(expected),
            "{error:?} should mention {expected}"
        );
    }

    let mut store = BindingStore::default();
    store.profiles[0].pads.left.regions = (0..=MAX_PAD_REGIONS)
        .map(|index| PadRegion::new(format!("R{index}"), PadRegionShape::WHOLE))
        .collect();
    assert!(store.validate().unwrap_err().contains("at most"));
}

#[test]
fn resetting_an_unreadable_store_keeps_the_original_beside_a_fresh_default() {
    let directory =
        std::env::temp_dir().join(format!("desktop-bindings-reset-{}", std::process::id()));
    let path = directory.join("bindings.json");
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    fs::write(&path, b"{ this is not a binding store }").unwrap();
    assert!(load_or_create_store(&path).is_err());

    let kept = reset_store(&path).unwrap();

    // Usable default; the original survives under a renameable name.
    assert_eq!(
        load_or_create_store(&path).unwrap(),
        BindingStore::default()
    );
    assert_eq!(kept, directory.join("bindings-invalid.json"));
    assert_eq!(fs::read(&kept).unwrap(), b"{ this is not a binding store }");

    // A second reset does not clobber the first.
    fs::write(&path, b"broken again").unwrap();
    let second = reset_store(&path).unwrap();
    assert_eq!(second, directory.join("bindings-invalid-1.json"));
    assert_eq!(fs::read(&kept).unwrap(), b"{ this is not a binding store }");
    assert_eq!(fs::read(&second).unwrap(), b"broken again");
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn resetting_a_store_that_cannot_be_moved_leaves_it_alone() {
    let directory = std::env::temp_dir().join(format!(
        "desktop-bindings-reset-fail-{}",
        std::process::id()
    ));
    let path = directory.join("bindings.json");
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    fs::write(&path, b"broken").unwrap();

    let mut permissions = fs::metadata(&directory).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&directory, permissions).unwrap();
    // Root ignores the write bit, so the failure cannot be produced.
    let read_only = fs::write(directory.join("probe"), b"").is_err();

    if read_only {
        assert!(reset_store(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"broken");
    }

    let mut permissions = fs::metadata(&directory).unwrap().permissions();
    #[allow(
        clippy::permissions_set_readonly_false,
        reason = "restoring the temporary directory so the test can clean up after itself"
    )]
    permissions.set_readonly(false);
    fs::set_permissions(&directory, permissions).unwrap();
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn failed_atomic_rename_cleans_up_the_temporary_file() {
    let directory = std::env::temp_dir().join(format!(
        "desktop-bindings-rename-failure-{}",
        std::process::id()
    ));
    let path = directory.join("bindings.json");
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&path).unwrap();

    assert!(save_store(&path, &BindingStore::default()).is_err());
    let leftovers = fs::read_dir(&directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() != "bindings.json")
        .count();
    assert_eq!(leftovers, 0);
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn first_snapshot_is_baseline_and_press_release_mirrors_chord() {
    let mut profile = BindingProfile::default();
    profile.bindings.r4 = Some(chord(KeyboardKey::F5, &[Modifier::Command]));
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::R4]), &mut sink)
        .unwrap();
    engine
        .observe(buttons(&[BindableControl::R4]), &mut sink)
        .unwrap();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    assert_eq!(
        sink.events,
        [
            "modifier:Command:true",
            "key:F5:true",
            "key:F5:false",
            "modifier:Command:false"
        ]
    );
}

#[test]
fn duplicate_bindings_reference_count_shared_outputs() {
    let mut profile = BindingProfile::default();
    profile.bindings.l4 = Some(chord(KeyboardKey::F9, &[]));
    profile.bindings.r4 = Some(chord(KeyboardKey::F9, &[]));
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::L4]), &mut sink)
        .unwrap();
    engine
        .observe(
            buttons(&[BindableControl::L4, BindableControl::R4]),
            &mut sink,
        )
        .unwrap();
    engine
        .observe(buttons(&[BindableControl::R4]), &mut sink)
        .unwrap();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    assert_eq!(sink.events, ["key:F9:true", "key:F9:false"]);
}

#[test]
fn overlapping_chords_do_not_release_a_shared_modifier_early() {
    let mut profile = BindingProfile::default();
    profile.bindings.l4 = Some(chord(KeyboardKey::S, &[Modifier::Command]));
    profile.bindings.r4 = Some(chord(KeyboardKey::F5, &[Modifier::Command]));
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::L4]), &mut sink)
        .unwrap();
    engine
        .observe(
            buttons(&[BindableControl::L4, BindableControl::R4]),
            &mut sink,
        )
        .unwrap();
    engine
        .observe(buttons(&[BindableControl::R4]), &mut sink)
        .unwrap();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    assert_eq!(
        sink.events,
        [
            "modifier:Command:true",
            "key:S:true",
            "key:F5:true",
            "key:S:false",
            "key:F5:false",
            "modifier:Command:false"
        ]
    );
}

#[test]
fn profile_switch_releases_and_blocks_controls_held_during_switch() {
    let mut first = BindingProfile::default();
    first.bindings.l4 = Some(chord(KeyboardKey::F5, &[]));
    let mut second = BindingProfile {
        id: "second".to_owned(),
        name: "Second".to_owned(),
        ..BindingProfile::default()
    };
    second.bindings.l4 = Some(chord(KeyboardKey::F9, &[]));
    let mut engine = BindingEngine::new(first);
    let mut sink = MockSink::default();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::L4]), &mut sink)
        .unwrap();
    engine.replace_profile(second, &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::L4]), &mut sink)
        .unwrap();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::L4]), &mut sink)
        .unwrap();
    assert_eq!(sink.events, ["key:F5:true", "key:F5:false", "key:F9:true"]);
}

#[test]
fn metadata_only_profile_update_preserves_held_outputs() {
    let mut profile = BindingProfile::default();
    profile.bindings.r4 = Some(chord(KeyboardKey::F5, &[]));
    let mut engine = BindingEngine::new(profile.clone());
    let mut sink = MockSink::default();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::R4]), &mut sink)
        .unwrap();

    profile.name = "Renamed".to_owned();
    engine.replace_profile(profile, &mut sink).unwrap();
    assert_eq!(sink.events, ["key:F5:true"]);
    engine.observe(buttons(&[]), &mut sink).unwrap();
    assert_eq!(sink.events, ["key:F5:true", "key:F5:false"]);
}

#[test]
fn sink_failure_releases_existing_outputs_and_rebaselines() {
    let mut profile = BindingProfile::default();
    profile.bindings.l4 = Some(chord(KeyboardKey::F5, &[]));
    profile.bindings.r4 = Some(chord(KeyboardKey::F9, &[]));
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::L4]), &mut sink)
        .unwrap();
    sink.fail_next = true;
    assert!(engine
        .observe(
            buttons(&[BindableControl::L4, BindableControl::R4]),
            &mut sink
        )
        .is_err());
    assert_eq!(engine.held_output_count(), 0);
    assert!(sink.events.contains(&"key:F5:false".to_owned()));
}

#[test]
fn disconnect_flushes_after_successful_and_failed_releases() {
    for fail_release in [false, true] {
        let mut profile = BindingProfile::default();
        profile.bindings.r4 = Some(chord(KeyboardKey::F5, &[]));
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        engine.observe(buttons(&[]), &mut sink).unwrap();
        engine
            .observe(buttons(&[BindableControl::R4]), &mut sink)
            .unwrap();
        sink.fail_next = fail_release;
        sink.fail_flush = fail_release;

        let result = engine.disconnect(&mut sink);
        assert_eq!(
            result,
            if fail_release {
                Err("injected failure".to_owned())
            } else {
                Ok(())
            }
        );
        assert_eq!(sink.flushes, 1);
        assert_eq!(engine.held_output_count(), 0);
    }
}

#[test]
fn flush_failure_is_reported_after_all_releases_are_attempted() {
    let mut profile = BindingProfile::default();
    profile.bindings.r4 = Some(chord(KeyboardKey::F5, &[]));
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::R4]), &mut sink)
        .unwrap();
    sink.fail_flush = true;

    assert_eq!(
        engine.disconnect(&mut sink),
        Err("injected flush failure".to_owned())
    );
    assert!(sink.events.contains(&"key:F5:false".to_owned()));
    assert_eq!(sink.flushes, 1);
}

#[test]
fn pad_click_press_release_mirrors_its_regions_binding() {
    let mut profile = BindingProfile::default();
    profile.pads.left.regions = whole_pad(
        PadTrigger::Click,
        chord(KeyboardKey::F5, &[Modifier::Command]),
    );
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Left,
        &[
            (0, PadSample::NEUTRAL),
            (10, touching(0, 0)),
            (20, clicking(0, 0)),
            (30, touching(0, 0)),
        ],
    );
    assert_eq!(
        sink.events,
        [
            "modifier:Command:true",
            "key:F5:true",
            "key:F5:false",
            "modifier:Command:false"
        ]
    );
}

#[test]
fn pad_click_fires_regardless_of_pad_motion_and_alongside_it() {
    let action = BindingAction::MouseButton {
        button: MouseButton::Left,
    };
    // Motion off: the click is still live.
    let mut disabled = BindingProfile::default();
    disabled.pads.right.regions = whole_pad(PadTrigger::Click, action.clone());
    let mut engine = BindingEngine::new(disabled.clone());
    let mut sink = MockSink::default();
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Right,
        &[
            (0, PadSample::NEUTRAL),
            (10, touching(0, 0)),
            (20, clicking(0, 0)),
            (30, touching(0, 0)),
        ],
    );
    assert_eq!(sink.events, ["mouse:Left:true", "mouse:Left:false"]);

    // Motion on: both reach the sink during one touch.
    let mut profile = disabled;
    profile.pads.right.motion = PadMotionMode::Pointer;
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Right,
        &[
            (0, PadSample::NEUTRAL),
            (1, touching(100, 100)),
            (20, touching(3_044, 292)),
            (40, clicking(3_044, 292)),
            (60, touching(3_044, 292)),
        ],
    );
    assert!(sink.events.contains(&"mouse:Left:true".to_owned()));
    assert!(sink.events.contains(&"mouse:Left:false".to_owned()));
    assert!(sink.events.iter().any(|event| event.starts_with("move:")));
}

#[test]
fn press_hold_freezes_wander_for_entire_hold() {
    // Raw captures show the centroid wandering up to ~2,400 counts while a pad
    // is physically pressed. None of it may reach the pointer, no matter how
    // long the click is held.
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((0, 0))),
            Duration::from_millis(1),
            &mut sink,
        )
        .unwrap();
    for (time_ms, x, y) in [
        (10, 500, 300),
        (60, 1800, -400),
        (140, 2200, 600),
        (260, 900, -100),
        (410, 1500, 800),
    ] {
        let mut snapshot = pad_snapshot(neutral, None, Some((x, y)));
        snapshot.right_pad.pressed = true;
        engine
            .observe_snapshot(snapshot, Duration::from_millis(time_ms), &mut sink)
            .unwrap();
    }
    assert!(sink.events.is_empty());
}

#[test]
fn pressure_freeze_engages_before_the_click_bit() {
    // "Slightly pressing" the pad raises the analog pressure long before the
    // click bit sets (and sometimes without ever setting it). The freeze must
    // key on pressure so that wander cannot reach the pointer.
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    let at_pressure = |x, pressure| {
        let mut snapshot = pad_snapshot(neutral, None, Some((x, 0)));
        snapshot.right_pad.pressure = pressure;
        snapshot
    };

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    engine
        .observe_snapshot(at_pressure(0, 0), Duration::from_millis(1), &mut sink)
        .unwrap();
    engine
        .observe_snapshot(at_pressure(3_000, 0), Duration::from_millis(20), &mut sink)
        .unwrap();
    let moved = sink.events.len();
    assert!(moved > 0);

    // Pressure past the freeze threshold with no click bit: wander is frozen.
    for (time_ms, x) in [(40, 1_400), (60, 2_000), (90, 1_200)] {
        engine
            .observe_snapshot(
                at_pressure(x, 3_000),
                Duration::from_millis(time_ms),
                &mut sink,
            )
            .unwrap();
    }
    assert_eq!(sink.events.len(), moved);

    // Pressure release behaves like a click release: guarded, then normal
    // pointing resumes.
    engine
        .observe_snapshot(at_pressure(1_300, 0), Duration::from_millis(120), &mut sink)
        .unwrap();
    engine
        .observe_snapshot(at_pressure(1_350, 0), Duration::from_millis(150), &mut sink)
        .unwrap();
    assert_eq!(sink.events.len(), moved);
    engine
        .observe_snapshot(at_pressure(1_350, 0), Duration::from_millis(400), &mut sink)
        .unwrap();
    engine
        .observe_snapshot(at_pressure(4_500, 0), Duration::from_millis(420), &mut sink)
        .unwrap();
    assert!(sink.events.len() > moved);
}

#[test]
fn pad_click_feedback_is_edge_triggered_when_the_pad_has_no_motion_mode() {
    let profile = BindingProfile::default();
    assert_eq!(profile.pads.right.motion, PadMotionMode::None);
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let step = |engine: &mut BindingEngine, sink: &mut MockSink, millis, sample| {
        engine
            .observe_snapshot(
                side_snapshot(PadSide::Right, sample),
                Duration::from_millis(millis),
                sink,
            )
            .unwrap()
    };

    step(&mut engine, &mut sink, 0, PadSample::NEUTRAL);
    step(&mut engine, &mut sink, 1, touching(0, 0));
    let press = step(&mut engine, &mut sink, 10, clicking(0, 0));
    assert_eq!(press.right, Some(PadFeedbackStrength::Medium));
    let hold = step(&mut engine, &mut sink, 40, clicking(0, 0));
    assert_eq!(hold, PadFeedbackRequest::NONE);
    let release = step(&mut engine, &mut sink, 60, touching(0, 0));
    assert_eq!(release, PadFeedbackRequest::NONE);
    assert!(sink.events.is_empty());
}

#[test]
fn pad_click_feedback_respects_each_pads_feedback_setting() {
    let mut profile = BindingProfile::default();
    profile.pads.left.feedback.enabled = false;
    profile.pads.right.feedback.strength = PadFeedbackStrength::High;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();

    let left = drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Left,
        &[
            (0, PadSample::NEUTRAL),
            (1, touching(0, 0)),
            (10, clicking(0, 0)),
        ],
    );
    assert_eq!(left, PadFeedbackRequest::NONE);
    let right = drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Right,
        &[(20, touching(0, 0)), (30, clicking(0, 0))],
    );
    assert_eq!(right.right, Some(PadFeedbackStrength::High));
}

#[test]
fn click_freeze_swallows_press_wander_but_drag_escapes() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    profile.pads.right.feedback.enabled = false;
    profile.pads.right.regions = whole_pad(
        PadTrigger::Click,
        BindingAction::MouseButton {
            button: MouseButton::Left,
        },
    );
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    let pressed_at = |x, y| side_snapshot(PadSide::Right, clicking(x, y));

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((0, 0))),
            Duration::from_millis(1),
            &mut sink,
        )
        .unwrap();
    // Press-roll wander stays below the drag threshold: frozen.
    engine
        .observe_snapshot(pressed_at(640, 0), Duration::from_millis(10), &mut sink)
        .unwrap();
    engine
        .observe_snapshot(pressed_at(1200, 0), Duration::from_millis(40), &mut sink)
        .unwrap();
    engine
        .observe_snapshot(pressed_at(400, 0), Duration::from_millis(80), &mut sink)
        .unwrap();
    assert_eq!(sink.events, ["mouse:Left:true"]);

    // A deliberate travel past the drag threshold engages dragging: the
    // radial excess beyond 2,800 counts is forwarded, then deltas flow.
    engine
        .observe_snapshot(pressed_at(4000, 0), Duration::from_millis(100), &mut sink)
        .unwrap();
    engine
        .observe_snapshot(pressed_at(4400, 0), Duration::from_millis(110), &mut sink)
        .unwrap();
    // Release: the click action releases while the un-flatten roll and the
    // guard window after it stay silent.
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((3800, 0))),
            Duration::from_millis(130),
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((3900, 0))),
            Duration::from_millis(200),
            &mut sink,
        )
        .unwrap();
    assert_eq!(
        sink.events,
        [
            "mouse:Left:true",
            "move:4:0",
            "move:3:0",
            "mouse:Left:false"
        ]
    );
}

#[test]
fn oscillating_noise_reparks_after_a_stop_window() {
    // Alternating jitter has near-zero net displacement per stop window, so
    // pass-through motion must re-park instead of leaking jitter forever.
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((0, 0))),
            Duration::from_millis(1),
            &mut sink,
        )
        .unwrap();
    // A real swipe unparks the filter.
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((3_000, 0))),
            Duration::from_millis(20),
            &mut sink,
        )
        .unwrap();
    assert!(!sink.events.is_empty());

    // ±60-count oscillation: passes through at first, then the first stop
    // window with sub-noise net progress re-parks the pad.
    let mut time_ms = 20;
    for step in 0..15 {
        time_ms += 10;
        let x = if step % 2 == 0 { 3_060 } else { 2_940 };
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((x, 0))),
                Duration::from_millis(time_ms),
                &mut sink,
            )
            .unwrap();
    }
    let settled = sink.events.len();

    // The same oscillation keeps going for a long time: parked, fully silent.
    for step in 0..40 {
        time_ms += 10;
        let x = if step % 2 == 0 { 3_060 } else { 2_940 };
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((x, 0))),
                Duration::from_millis(time_ms),
                &mut sink,
            )
            .unwrap();
    }
    assert_eq!(sink.events.len(), settled);
}

#[test]
fn a_pad_clicked_at_baseline_is_blocked_until_it_is_released() {
    let mut profile = BindingProfile::default();
    profile.pads.left.regions = whole_pad(PadTrigger::Click, chord(KeyboardKey::F5, &[]));
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    // The baseline snapshot already has the pad held down.
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Left,
        &[(0, clicking(0, 0)), (10, clicking(0, 0))],
    );
    assert!(sink.events.is_empty());
    // Lifting off clears the block.
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Left,
        &[
            (20, PadSample::NEUTRAL),
            (30, touching(0, 0)),
            (40, clicking(0, 0)),
        ],
    );
    assert_eq!(sink.events, ["key:F5:true"]);
}

#[test]
fn profile_switch_releases_and_blocks_a_held_pad_click() {
    let mut first = BindingProfile::default();
    first.pads.right.regions = whole_pad(PadTrigger::Click, chord(KeyboardKey::F5, &[]));
    let mut second = BindingProfile {
        id: "second".to_owned(),
        name: "Second".to_owned(),
        ..BindingProfile::default()
    };
    second.pads.right.regions = whole_pad(PadTrigger::Click, chord(KeyboardKey::F9, &[]));
    let mut engine = BindingEngine::new(first);
    let mut sink = MockSink::default();
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Right,
        &[
            (0, PadSample::NEUTRAL),
            (10, touching(0, 0)),
            (20, clicking(0, 0)),
        ],
    );
    engine.replace_profile(second, &mut sink).unwrap();
    assert_eq!(engine.held_pad_action_count(), 0);
    // Still-held: inert until physically released.
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Right,
        &[
            (30, clicking(0, 0)),
            (40, PadSample::NEUTRAL),
            (50, touching(0, 0)),
            (60, clicking(0, 0)),
        ],
    );
    assert_eq!(sink.events, ["key:F5:true", "key:F5:false", "key:F9:true"]);
}

#[test]
fn right_pad_feedback_cadence_increases_with_motion_speed_without_a_backlog() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    assert_eq!(profile.configured_output_count(), 1);
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((100, 100))),
            Duration::from_millis(1),
            &mut sink,
        )
        .unwrap();
    let first = engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((3_428, 100))),
            Duration::from_millis(500),
            &mut sink,
        )
        .unwrap();
    assert_eq!(first.right, Some(PadFeedbackStrength::Medium));
    let slow_limited = engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((4_196, 100))),
            Duration::from_millis(800),
            &mut sink,
        )
        .unwrap();
    assert_eq!(slow_limited, PadFeedbackRequest::NONE);
    let slow_ready = engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((4_964, 100))),
            Duration::from_millis(1_200),
            &mut sink,
        )
        .unwrap();
    assert_eq!(slow_ready.right, Some(PadFeedbackStrength::Medium));

    let mut fast_engine = BindingEngine::new(engine.profile().clone());
    let mut fast_sink = MockSink::default();
    fast_engine
        .observe_snapshot(
            pad_snapshot(neutral, None, None),
            Duration::ZERO,
            &mut fast_sink,
        )
        .unwrap();
    fast_engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((100, 100))),
            Duration::from_millis(1),
            &mut fast_sink,
        )
        .unwrap();
    let fast_first = fast_engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((3_428, 100))),
            Duration::from_millis(10),
            &mut fast_sink,
        )
        .unwrap();
    assert_eq!(fast_first.right, Some(PadFeedbackStrength::Medium));
    let fast_limited = fast_engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((4_196, 100))),
            Duration::from_millis(60),
            &mut fast_sink,
        )
        .unwrap();
    assert_eq!(fast_limited, PadFeedbackRequest::NONE);
    let fast_ready = fast_engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((4_964, 100))),
            Duration::from_millis(110),
            &mut fast_sink,
        )
        .unwrap();
    assert_eq!(fast_ready.right, Some(PadFeedbackStrength::Medium));
}

#[test]
fn stationary_pressed_pad_noise_does_not_emit_feedback() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    // The press edge earns one tick; the stationary noise after it must not.
    let press = engine
        .observe_snapshot(
            side_snapshot(PadSide::Right, clicking(0, 0)),
            Duration::ZERO,
            &mut sink,
        )
        .unwrap();
    assert_eq!(press.right, Some(PadFeedbackStrength::Medium));
    for (index, (x, y)) in [
        (0, 160),
        (0, -160),
        (96, 128),
        (-96, -128),
        (128, -96),
        (-128, 96),
        (0, 160),
        (0, -160),
    ]
    .into_iter()
    .enumerate()
    {
        let feedback = engine
            .observe_snapshot(
                side_snapshot(PadSide::Right, clicking(x, y)),
                Duration::from_millis(u64::try_from((index + 1) * 250).unwrap()),
                &mut sink,
            )
            .unwrap();
        assert_eq!(feedback, PadFeedbackRequest::NONE);
    }
    assert!(sink.events.is_empty());
}

#[test]
fn left_pad_scrolls_both_axes_and_can_disable_feedback() {
    let mut profile = BindingProfile::default();
    profile.pads.left.motion = PadMotionMode::Scroll;
    profile.pads.left.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, Some((0, 0)), None),
            Duration::from_millis(1),
            &mut sink,
        )
        .unwrap();
    let feedback = engine
        .observe_snapshot(
            pad_snapshot(neutral, Some((384, 192)), None),
            Duration::from_millis(20),
            &mut sink,
        )
        .unwrap();
    assert_eq!(feedback, PadFeedbackRequest::NONE);
    // The escape from the anchored deadzone forwards the excess beyond the
    // 192-count radius: (384, 192) rescales to (212, 106) before conversion.
    assert_eq!(sink.events, ["scroll:3:-1"]);
}

#[test]
fn left_pad_scroll_acceleration_and_profile_speed_scale_output() {
    fn scroll_once(duration_ms: u64, speed_percent: u16) -> Vec<String> {
        let mut profile = BindingProfile::default();
        profile.pads.left.motion = PadMotionMode::Scroll;
        profile.pads.left.feedback.enabled = false;
        profile.pads.left.speed_percent = speed_percent;
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        let neutral = SteamButtons::default();
        engine
            .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, Some((0, 0)), None),
                Duration::from_millis(1),
                &mut sink,
            )
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, Some((384, 0)), None),
                Duration::from_millis(duration_ms),
                &mut sink,
            )
            .unwrap();
        sink.events
    }

    // A 384-count swipe from rest forwards 192 counts of deadzone excess.
    assert_eq!(scroll_once(501, 100), ["scroll:1:0"]);
    assert_eq!(scroll_once(20, 100), ["scroll:3:0"]);
    assert_eq!(scroll_once(20, 50), ["scroll:1:0"]);
    assert_eq!(scroll_once(20, 200), ["scroll:6:0"]);
}

#[test]
fn left_pad_momentum_decays_after_release_and_can_be_disabled() {
    fn run(momentum: bool) -> Vec<String> {
        let mut profile = BindingProfile::default();
        profile.pads.left.motion = PadMotionMode::Scroll;
        profile.pads.left.feedback.enabled = false;
        profile.pads.left.momentum = momentum;
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        let neutral = SteamButtons::default();
        engine
            .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, Some((0, 0)), None),
                Duration::from_millis(1),
                &mut sink,
            )
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, Some((768, 0)), None),
                Duration::from_millis(21),
                &mut sink,
            )
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, None),
                Duration::from_millis(22),
                &mut sink,
            )
            .unwrap();
        for time_ms in (32..=2_032).step_by(10) {
            engine
                .tick(Duration::from_millis(time_ms), &mut sink)
                .unwrap();
        }
        sink.events
    }

    let with_momentum = run(true);
    let without_momentum = run(false);
    assert_eq!(without_momentum, ["scroll:9:0"]);
    assert!(with_momentum.len() > without_momentum.len());
    assert!(with_momentum
        .iter()
        .skip(1)
        .all(|event| event.starts_with("scroll:")));
    assert_eq!(with_momentum.last(), Some(&"scroll:1:0".to_owned()));
}

#[test]
fn ticks_are_needed_only_while_released_scroll_momentum_is_pending() {
    let mut profile = BindingProfile::default();
    profile.pads.left.motion = PadMotionMode::Scroll;
    profile.pads.left.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    assert!(!engine.needs_tick());
    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    assert!(!engine.needs_tick());
    engine
        .observe_snapshot(
            pad_snapshot(neutral, Some((0, 0)), None),
            Duration::from_millis(1),
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, Some((768, 0)), None),
            Duration::from_millis(21),
            &mut sink,
        )
        .unwrap();
    assert!(!engine.needs_tick());

    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, None),
            Duration::from_millis(22),
            &mut sink,
        )
        .unwrap();
    assert!(engine.needs_tick());

    for time_ms in (32..=2_032).step_by(10) {
        engine
            .tick(Duration::from_millis(time_ms), &mut sink)
            .unwrap();
        if !engine.needs_tick() {
            break;
        }
    }
    assert!(!engine.needs_tick());

    engine
        .observe_snapshot(
            pad_snapshot(neutral, Some((0, 0)), None),
            Duration::from_millis(2_101),
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, Some((768, 0)), None),
            Duration::from_millis(2_121),
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, None),
            Duration::from_millis(2_122),
            &mut sink,
        )
        .unwrap();
    assert!(engine.needs_tick());
    engine.disconnect(&mut sink).unwrap();
    assert!(!engine.needs_tick());
}

#[test]
fn stationary_scroll_touch_and_release_never_schedule_a_tick() {
    let mut profile = BindingProfile::default();
    profile.pads.left.motion = PadMotionMode::Scroll;
    profile.pads.left.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    for time_ms in [1, 100, 200] {
        engine
            .observe_snapshot(
                pad_snapshot(neutral, Some((0, 0)), None),
                Duration::from_millis(time_ms),
                &mut sink,
            )
            .unwrap();
    }
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, None),
            Duration::from_millis(201),
            &mut sink,
        )
        .unwrap();

    assert!(sink.events.is_empty());
    assert!(!engine.needs_tick());
}

#[test]
fn stalled_scroll_motion_cannot_launch_stale_momentum_on_lift() {
    let mut profile = BindingProfile::default();
    profile.pads.left.motion = PadMotionMode::Scroll;
    profile.pads.left.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, Some((0, 0)), None),
            Duration::from_millis(1),
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, Some((768, 0)), None),
            Duration::from_millis(21),
            &mut sink,
        )
        .unwrap();
    assert!(!sink.events.is_empty());

    // The elapsed stop window re-parks the pad and invalidates the velocity
    // estimate from the earlier swipe before contact ends.
    engine
        .observe_snapshot(
            pad_snapshot(neutral, Some((768, 0)), None),
            Duration::from_millis(200),
            &mut sink,
        )
        .unwrap();
    let event_count = sink.events.len();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, None),
            Duration::from_millis(201),
            &mut sink,
        )
        .unwrap();
    engine.tick(Duration::from_millis(301), &mut sink).unwrap();

    assert_eq!(sink.events.len(), event_count);
    assert!(!engine.needs_tick());
}

#[test]
fn pad_motion_deadzone_rejects_noise_and_recenters_after_large_jumps() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    for (time_ms, x) in [
        (1, 0),
        (2, 64),
        (3, -64),
        (4, 64),
        (5, -64),
        (6, 128),
        (7, -128),
        (8, 128),
    ] {
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((x, 0))),
                Duration::from_millis(time_ms),
                &mut sink,
            )
            .unwrap();
    }
    assert!(sink.events.is_empty());

    // The capture-derived pointer envelope is 2,560 counts at pad center.
    // Crossing it forwards only radial excess; subsequent motion passes.
    for (time_ms, x) in [(9, 2_560), (10, 2_944), (11, 3_200)] {
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((x, 0))),
                Duration::from_millis(time_ms),
                &mut sink,
            )
            .unwrap();
    }
    assert_eq!(sink.events, ["move:3:0", "move:2:0"]);

    // An impossible per-report jump rebaselines: the pad re-parks and the next
    // report remains parked inside the larger edge-noise envelope.
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((-32_700, 0))),
            Duration::from_millis(12),
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((-32_508, 0))),
            Duration::from_millis(13),
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((-32_316, 0))),
            Duration::from_millis(14),
            &mut sink,
        )
        .unwrap();
    assert_eq!(sink.events, ["move:3:0", "move:2:0"]);
}

#[test]
fn pointer_transfer_is_linear_in_displacement_not_report_speed() {
    fn swipe(elapsed_ms: u64, speed_percent: u16) -> Vec<String> {
        let mut profile = BindingProfile::default();
        profile.pads.right.motion = PadMotionMode::Pointer;
        profile.pads.right.feedback.enabled = false;
        profile.pads.right.speed_percent = speed_percent;
        let mut engine = BindingEngine::new(profile);
        let mut sink = MockSink::default();
        let neutral = SteamButtons::default();
        engine
            .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((0, 0))),
                Duration::from_millis(1),
                &mut sink,
            )
            .unwrap();
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((12_800, 0))),
                Duration::from_millis(elapsed_ms),
                &mut sink,
            )
            .unwrap();
        sink.events
    }

    assert_eq!(swipe(10, 100), ["move:80:0"]);
    assert_eq!(swipe(500, 100), ["move:80:0"]);
    assert_eq!(swipe(10, 200), ["move:160:0"]);
}

#[test]
fn recorded_center_hold_wander_stays_parked() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();

    // Bounds sampled from the reference center-hold stage. The centroid walks
    // almost 2,500 counts while the lizard pointer remains exactly still.
    for (index, (x, y)) in [
        (318, 406),
        (740, -320),
        (1_526, -1_780),
        (920, -1_220),
        (510, -280),
    ]
    .into_iter()
    .enumerate()
    {
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((x, y))),
                Duration::from_millis(u64::try_from(index * 20 + 1).unwrap()),
                &mut sink,
            )
            .unwrap();
    }
    assert!(sink.events.is_empty());
}

#[test]
fn recorded_bottom_edge_wander_stays_parked() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    // Capture-derived stationary corner episode: the Y coordinate is clamped
    // while X drifts about 2,000 counts out and back over 150 ms.
    for (index, x) in [
        -7_014, -6_668, -6_224, -5_766, -5_420, -5_120, -5_006, -5_446, -5_886, -6_490, -7_094,
    ]
    .into_iter()
    .enumerate()
    {
        let feedback = engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((x, -32_766))),
                Duration::from_millis(u64::try_from(index * 15 + 1).unwrap()),
                &mut sink,
            )
            .unwrap();
        assert_eq!(feedback, PadFeedbackRequest::NONE);
    }
    assert!(sink.events.is_empty());
}

#[test]
fn deliberate_slow_edge_motion_stays_unparked_across_stop_windows() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((0, -32_766))),
            Duration::from_millis(1),
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((4_000, -32_766))),
            Duration::from_millis(20),
            &mut sink,
        )
        .unwrap();
    let events_after_escape = sink.events.len();

    // Once intentional motion escapes the larger parked envelope, 800 counts
    // per stop window is meaningful travel even at the rim. It must not be
    // compared against the full contact deadzone again.
    for (time_ms, x) in [(180, 4_800), (340, 5_600), (500, 6_400)] {
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((x, -32_766))),
                Duration::from_millis(time_ms),
                &mut sink,
            )
            .unwrap();
    }
    assert_eq!(sink.events.len(), events_after_escape + 3);
}

#[test]
fn recorded_post_click_pressure_tail_cannot_become_a_drag() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    profile.pads.right.feedback.enabled = false;
    profile.pads.right.regions = whole_pad(
        PadTrigger::Click,
        BindingAction::MouseButton {
            button: MouseButton::Left,
        },
    );
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    let sample = |x, y, pressure, pressed| {
        side_snapshot(
            PadSide::Right,
            PadSample {
                pressure,
                pressed,
                ..touching(x, y)
            },
        )
    };

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    for (time_ms, snapshot) in [
        (1, sample(-23_322, 6_434, 1_532, false)),
        (12, sample(-23_216, 6_420, 1_649, false)),
        (64, sample(-22_946, 6_324, 3_988, true)),
        (212, sample(-22_076, 8_346, 3_696, true)),
        (244, sample(-22_298, 8_620, 2_370, false)),
        (266, sample(-22_714, 9_210, 1_201, false)),
        (287, sample(-22_848, 9_426, 928, false)),
        (470, sample(-22_946, 9_772, 421, false)),
    ] {
        engine
            .observe_snapshot(snapshot, Duration::from_millis(time_ms), &mut sink)
            .unwrap();
    }
    assert_eq!(sink.events, ["mouse:Left:true", "mouse:Left:false"]);
}

#[test]
fn recorded_left_pad_release_tail_cannot_start_scroll_momentum() {
    let mut profile = BindingProfile::default();
    profile.pads.left.motion = PadMotionMode::Scroll;
    profile.pads.left.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    let sample = |x, y, pressure, pressed| {
        side_snapshot(
            PadSide::Left,
            PadSample {
                pressure,
                pressed,
                ..touching(x, y)
            },
        )
    };

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    for (time_ms, snapshot) in [
        (1, sample(11_476, -2_014, 1_590, false)),
        (12, sample(11_454, -2_060, 1_619, false)),
        (72, sample(11_384, -2_176, 4_234, true)),
        (238, sample(12_506, -2_504, 2_437, false)),
        (310, sample(12_150, -3_092, 1_371, false)),
        (377, sample(11_032, -3_058, 1_444, false)),
        (448, sample(8_804, -4_220, 1_356, false)),
    ] {
        engine
            .observe_snapshot(snapshot, Duration::from_millis(time_ms), &mut sink)
            .unwrap();
    }
    assert!(sink.events.is_empty());
    assert!(!engine.needs_tick());
}

#[test]
fn paused_drag_resumes_without_crossing_the_full_drag_threshold_again() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    let pressed_at = |x| side_snapshot(PadSide::Right, clicking(x, 0));

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((0, 0))),
            Duration::from_millis(1),
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(pressed_at(0), Duration::from_millis(10), &mut sink)
        .unwrap();
    engine
        .observe_snapshot(pressed_at(4_000), Duration::from_millis(30), &mut sink)
        .unwrap();
    let before_pause = sink.events.len();
    engine
        .observe_snapshot(pressed_at(4_010), Duration::from_millis(220), &mut sink)
        .unwrap();
    engine
        .observe_snapshot(pressed_at(4_610), Duration::from_millis(240), &mut sink)
        .unwrap();

    assert!(sink.events.len() > before_pause);
}

#[test]
fn parked_pad_never_banks_bounded_wander_and_reparks_after_a_stall() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    // A resting finger wandering inside the noise radius forever emits nothing,
    // because displacement is measured from a fixed anchor rather than banked
    // and recentered.
    let mut time_ms = 1;
    for _ in 0..50 {
        for x in [0_i16, 150, -150, 100, -100] {
            engine
                .observe_snapshot(
                    pad_snapshot(neutral, None, Some((x, 0))),
                    Duration::from_millis(time_ms),
                    &mut sink,
                )
                .unwrap();
            time_ms += 4;
        }
    }
    assert!(sink.events.is_empty());

    // A deliberate swipe unparks and moves; a stall with sub-noise progress
    // re-parks, after which the same wander is silent again.
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((3_000, 0))),
            Duration::from_millis(time_ms),
            &mut sink,
        )
        .unwrap();
    assert!(!sink.events.is_empty());
    let moved = sink.events.len();
    time_ms += 200;
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((3_004, 0))),
            Duration::from_millis(time_ms),
            &mut sink,
        )
        .unwrap();
    for _ in 0..50 {
        time_ms += 4;
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((3_000, 0))),
                Duration::from_millis(time_ms),
                &mut sink,
            )
            .unwrap();
        time_ms += 4;
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((3_100, 0))),
                Duration::from_millis(time_ms),
                &mut sink,
            )
            .unwrap();
    }
    assert_eq!(sink.events.len(), moved);
}

#[test]
fn pad_touched_during_startup_or_profile_switch_waits_for_release() {
    let mut profile = BindingProfile::default();
    profile.pads.right.motion = PadMotionMode::Pointer;
    let mut engine = BindingEngine::new(profile.clone());
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((0, 0))),
            Duration::ZERO,
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((640, 0))),
            Duration::from_millis(20),
            &mut sink,
        )
        .unwrap();
    assert!(sink.events.is_empty());
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, None),
            Duration::from_millis(21),
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((0, 0))),
            Duration::from_millis(22),
            &mut sink,
        )
        .unwrap();
    let mut replacement = profile;
    replacement.pads.right.feedback.strength = PadFeedbackStrength::High;
    engine.replace_profile(replacement, &mut sink).unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((640, 0))),
            Duration::from_millis(40),
            &mut sink,
        )
        .unwrap();
    assert!(sink.events.is_empty());
}

#[test]
fn rapid_mouse_transitions_and_disconnect_never_leave_output_held() {
    let mut profile = BindingProfile::default();
    profile.bindings.quick_access = Some(BindingAction::MouseButton {
        button: MouseButton::Forward,
    });
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    for _ in 0..20 {
        engine
            .observe(buttons(&[BindableControl::QuickAccess]), &mut sink)
            .unwrap();
        engine.observe(buttons(&[]), &mut sink).unwrap();
    }
    engine
        .observe(buttons(&[BindableControl::QuickAccess]), &mut sink)
        .unwrap();
    engine.disconnect(&mut sink).unwrap();
    assert_eq!(engine.held_output_count(), 0);
    assert_eq!(
        sink.events.last().map(String::as_str),
        Some("mouse:Forward:false")
    );
}

/// A coordinate at `degrees` clockwise from twelve o'clock, at `percent` of the
/// pad's full-scale radius, matching the region geometry's own convention.
#[allow(
    clippy::cast_possible_truncation,
    reason = "test bearings stay far inside i16"
)]
fn bearing(degrees: f32, percent: f32) -> (i16, i16) {
    let radians = degrees.to_radians();
    let scale = f32::from(i16::MAX) * percent / 100.0;
    (
        (radians.sin() * scale) as i16,
        (radians.cos() * scale) as i16,
    )
}

/// Left and right sectors of an eight-way layout bound to the arrow keys, the
/// motivating example for regions.
fn arrow_regions(trigger: PadTrigger) -> Vec<PadRegion> {
    let mut regions = PadRegion::eight_way();
    for region in &mut regions {
        let key = match region.name.as_str() {
            "Left" => KeyboardKey::ArrowLeft,
            "Right" => KeyboardKey::ArrowRight,
            _ => continue,
        };
        *region.action_mut(trigger) = Some(chord(key, &[]));
    }
    regions
}

#[test]
fn clicking_opposite_sectors_of_one_pad_fires_their_own_bindings() {
    let mut profile = BindingProfile::default();
    profile.pads.left.regions = arrow_regions(PadTrigger::Click);
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let (west_x, west_y) = bearing(270.0, 70.0);
    let (east_x, east_y) = bearing(90.0, 70.0);
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Left,
        &[
            (0, PadSample::NEUTRAL),
            (10, touching(west_x, west_y)),
            (20, clicking(west_x, west_y)),
            (30, touching(west_x, west_y)),
            (400, PadSample::NEUTRAL),
            (410, touching(east_x, east_y)),
            (420, clicking(east_x, east_y)),
            (430, touching(east_x, east_y)),
        ],
    );
    assert_eq!(
        sink.events,
        [
            "key:ArrowLeft:true",
            "key:ArrowLeft:false",
            "key:ArrowRight:true",
            "key:ArrowRight:false"
        ]
    );
    assert_eq!(engine.held_pad_action_count(), 0);
}

#[test]
fn a_click_holds_the_region_it_was_pressed_in_even_if_the_finger_slides_away() {
    let mut profile = BindingProfile::default();
    profile.pads.left.regions = arrow_regions(PadTrigger::Click);
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let (west_x, west_y) = bearing(270.0, 70.0);
    let (east_x, east_y) = bearing(90.0, 70.0);
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Left,
        &[
            (0, PadSample::NEUTRAL),
            (10, touching(west_x, west_y)),
            (20, clicking(west_x, west_y)),
            // A drag while held must not swap to the opposite sector.
            (30, clicking(0, 0)),
            (40, clicking(east_x, east_y)),
        ],
    );
    assert_eq!(sink.events, ["key:ArrowLeft:true"]);
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Left,
        &[(50, touching(east_x, east_y))],
    );
    assert_eq!(sink.events, ["key:ArrowLeft:true", "key:ArrowLeft:false"]);
}

#[test]
fn a_touch_hands_off_between_regions_and_releases_on_lift() {
    let mut profile = BindingProfile::default();
    profile.pads.right.regions = arrow_regions(PadTrigger::Touch);
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let (west_x, west_y) = bearing(270.0, 70.0);
    let (east_x, east_y) = bearing(90.0, 70.0);
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Right,
        &[
            (0, PadSample::NEUTRAL),
            (10, touching(west_x, west_y)),
            // Through the unbound top sector, then into the opposite one.
            (20, touching(bearing(0.0, 70.0).0, bearing(0.0, 70.0).1)),
            (30, touching(east_x, east_y)),
            (40, PadSample::NEUTRAL),
        ],
    );
    assert_eq!(
        sink.events,
        [
            "key:ArrowLeft:true",
            "key:ArrowLeft:false",
            "key:ArrowRight:true",
            "key:ArrowRight:false"
        ]
    );
    assert_eq!(engine.held_pad_action_count(), 0);
}

#[test]
fn a_touch_crossing_between_regions_with_the_same_action_does_not_retrigger_it() {
    let mut profile = BindingProfile::default();
    profile.pads.right.regions = PadRegion::four_way();
    for region in &mut profile.pads.right.regions {
        region.touch = Some(chord(KeyboardKey::F9, &[]));
    }
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let top = bearing(0.0, 70.0);
    let right = bearing(90.0, 70.0);
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Right,
        &[
            (0, PadSample::NEUTRAL),
            (10, touching(top.0, top.1)),
            (20, touching(right.0, right.1)),
            (30, PadSample::NEUTRAL),
        ],
    );
    assert_eq!(sink.events, ["key:F9:true", "key:F9:false"]);
    assert_eq!(engine.held_pad_action_count(), 0);
}

#[test]
fn a_finger_resting_on_a_region_seam_does_not_alternate_between_bindings() {
    let mut profile = BindingProfile::default();
    profile.pads.right.regions = arrow_regions(PadTrigger::Touch);
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let mut steps = vec![
        (0, PadSample::NEUTRAL),
        (10, touching(bearing(270.0, 70.0).0, bearing(270.0, 70.0).1)),
    ];
    // The Left/Bottom Left seam sits at 292.5 degrees.
    for (index, degrees) in [291.0, 294.0, 291.5, 293.5, 292.0, 294.0]
        .into_iter()
        .enumerate()
    {
        let (x, y) = bearing(degrees, 70.0);
        steps.push((20 + index as u64 * 10, touching(x, y)));
    }
    drive_pad(&mut engine, &mut sink, PadSide::Right, &steps);
    assert_eq!(sink.events, ["key:ArrowLeft:true"]);
    assert_eq!(engine.held_pad_action_count(), 1);
}

#[test]
fn a_touch_region_action_is_released_by_disconnect_and_by_a_sink_failure() {
    let mut profile = BindingProfile::default();
    profile.pads.right.regions = arrow_regions(PadTrigger::Touch);
    profile.pads.right.feedback.enabled = false;
    let (west_x, west_y) = bearing(270.0, 70.0);

    let mut engine = BindingEngine::new(profile.clone());
    let mut sink = MockSink::default();
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Right,
        &[(0, PadSample::NEUTRAL), (10, touching(west_x, west_y))],
    );
    assert_eq!(engine.held_pad_action_count(), 1);
    engine.disconnect(&mut sink).unwrap();
    assert_eq!(engine.held_output_count(), 0);
    assert_eq!(engine.held_pad_action_count(), 0);
    assert_eq!(sink.events, ["key:ArrowLeft:true", "key:ArrowLeft:false"]);

    // A failing sink releases what it can and leaves nothing latched.
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    drive_pad(
        &mut engine,
        &mut sink,
        PadSide::Right,
        &[(0, PadSample::NEUTRAL), (10, touching(west_x, west_y))],
    );
    sink.fail_next = true;
    assert!(engine
        .observe_snapshot(
            side_snapshot(
                PadSide::Right,
                touching(bearing(90.0, 70.0).0, bearing(90.0, 70.0).1)
            ),
            Duration::from_millis(20),
            &mut sink
        )
        .is_err());
    assert_eq!(engine.held_output_count(), 0);
    assert_eq!(engine.held_pad_action_count(), 0);
}

#[test]
fn both_pads_can_scroll_with_momentum_at_the_same_time() {
    let mut profile = BindingProfile::default();
    for side in PadSide::ALL {
        let pad = profile.pads.get_mut(side);
        pad.motion = PadMotionMode::Scroll;
        pad.feedback.enabled = false;
    }
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    // A pad touched at the baseline is inert, so start from lifted.
    engine
        .observe_snapshot(
            DesktopInputSnapshot::buttons_only(SteamButtons::default()),
            Duration::ZERO,
            &mut sink,
        )
        .unwrap();
    for step in 0..8_i16 {
        engine
            .observe_snapshot(
                DesktopInputSnapshot {
                    buttons: SteamButtons::default(),
                    left_pad: touching(0, step * 3_000),
                    right_pad: touching(0, step * 3_000),
                },
                Duration::from_millis(1 + u64::try_from(step).unwrap() * 8),
                &mut sink,
            )
            .unwrap();
    }
    engine
        .observe_snapshot(
            DesktopInputSnapshot::buttons_only(SteamButtons::default()),
            Duration::from_millis(70),
            &mut sink,
        )
        .unwrap();
    assert!(engine.needs_tick());
    let before = sink.events.len();
    engine.tick(Duration::from_millis(80), &mut sink).unwrap();
    // One tick per pad.
    assert_eq!(sink.events.len(), before + 2);
}

#[test]
fn either_pad_can_take_either_motion_mode() {
    let mut profile = BindingProfile::default();
    profile.pads.left.motion = PadMotionMode::Pointer;
    profile.pads.left.feedback.enabled = false;
    profile.pads.right.motion = PadMotionMode::Scroll;
    profile.pads.right.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    engine
        .observe_snapshot(
            DesktopInputSnapshot::buttons_only(SteamButtons::default()),
            Duration::ZERO,
            &mut sink,
        )
        .unwrap();
    for (millis, position) in [(1_u64, 100_i16), (20, 6_000), (30, 12_000)] {
        engine
            .observe_snapshot(
                DesktopInputSnapshot {
                    buttons: SteamButtons::default(),
                    left_pad: touching(position, 0),
                    right_pad: touching(0, position),
                },
                Duration::from_millis(millis),
                &mut sink,
            )
            .unwrap();
    }
    // Pointer on the left pad, scrolling on the right.
    assert!(sink.events.iter().any(|event| event.starts_with("move:")));
    assert!(sink.events.iter().any(|event| event.starts_with("scroll:")));
}
