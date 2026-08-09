use super::*;
use std::fs;
use std::time::Duration;
use steam_controller_protocol::SteamButtons;

#[derive(Default)]
struct MockSink {
    events: Vec<String>,
    fail_next: bool,
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

#[test]
fn store_round_trips_and_defaults_are_unbound() {
    let store = BindingStore::default();
    assert_eq!(store.profiles[0].bindings.configured_count(), 0);
    assert_eq!(store.profiles[0].configured_output_count(), 0);
    assert!(!store.profiles[0].pads.left_scroll.enabled);
    assert!(!store.profiles[0].pads.right_mouse.enabled);
    assert!(store.profiles[0].pads.left_scroll.feedback.enabled);
    assert_eq!(
        store.profiles[0].pads.left_scroll.speed_percent,
        DEFAULT_SCROLL_SPEED_PERCENT
    );
    assert!(store.profiles[0].pads.left_scroll.momentum);
    assert_eq!(
        store.profiles[0].pads.right_mouse.feedback.strength,
        PadFeedbackStrength::Medium
    );
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
    assert!(store.profiles[0].bindings.left_pad_click.is_none());
    assert!(store.profiles[0].bindings.right_pad_click.is_none());
    let persisted: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(persisted["version"], BINDINGS_VERSION);
    assert!(persisted["profiles"][0]["pads"].is_object());
    let _ = fs::remove_file(path);
    let _ = fs::remove_dir(directory);
}

#[test]
fn loading_version_three_migrates_with_pad_clicks_unbound() {
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
    assert!(store.profiles[0].bindings.left_pad_click.is_none());
    assert!(store.profiles[0].bindings.right_pad_click.is_none());
    assert_eq!(store.profiles[0].bindings.configured_count(), 1);
    assert_eq!(store.profiles[0].pads.left_scroll.speed_percent, 150);
}

#[test]
fn pad_click_bindings_round_trip_and_count() {
    let mut store = BindingStore::default();
    store.profiles[0].bindings.left_pad_click = Some(chord(KeyboardKey::F5, &[Modifier::Shift]));
    store.profiles[0].bindings.right_pad_click = Some(BindingAction::MouseButton {
        button: MouseButton::Middle,
    });
    assert_eq!(store.profiles[0].bindings.configured_count(), 2);
    let bytes = serde_json::to_vec(&store).unwrap();
    let decoded: BindingStore = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, store);
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["profiles"][0]["bindings"]["left_pad_click"]["kind"],
        "key_chord"
    );
    assert_eq!(
        value["profiles"][0]["bindings"]["right_pad_click"]["button"],
        "middle"
    );
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
    assert!(store.profiles[0].pads.right_mouse.enabled);
    assert!(!store.profiles[0].pads.right_mouse.feedback.enabled);
    let scroll = store.profiles[0].pads.left_scroll;
    assert!(scroll.enabled);
    assert_eq!(scroll.feedback.strength, PadFeedbackStrength::High);
    assert_eq!(scroll.speed_percent, DEFAULT_SCROLL_SPEED_PERCENT);
    assert!(scroll.momentum);
}

#[test]
fn store_rejects_scroll_speed_outside_supported_range() {
    let mut store = BindingStore::default();
    store.profiles[0].pads.left_scroll.speed_percent = MIN_SCROLL_SPEED_PERCENT - 1;
    assert!(store.validate().is_err());
    store.profiles[0].pads.left_scroll.speed_percent = MAX_SCROLL_SPEED_PERCENT + 1;
    assert!(store.validate().is_err());
}

#[test]
fn store_rejects_pointer_speed_outside_supported_range_and_defaults_it() {
    let mut store = BindingStore::default();
    assert_eq!(
        store.profiles[0].pads.right_mouse.speed_percent,
        DEFAULT_SCROLL_SPEED_PERCENT
    );
    store.profiles[0].pads.right_mouse.speed_percent = MIN_SCROLL_SPEED_PERCENT - 1;
    assert!(store.validate().unwrap_err().contains("pointer speed"));
    store.profiles[0].pads.right_mouse.speed_percent = MAX_SCROLL_SPEED_PERCENT + 1;
    assert!(store.validate().is_err());
    store.profiles[0].pads.right_mouse.speed_percent = 150;
    store.validate().unwrap();
    let bytes = serde_json::to_vec(&store).unwrap();
    let decoded: BindingStore = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, store);

    // A v4 document written before the pointer-speed field existed still
    // parses, with the speed defaulted.
    let json = br#"{
          "version": 4,
          "profiles": [{
            "id": "default",
            "name": "Default",
            "bindings": {},
            "pads": {
              "right_mouse": {
                "enabled": true,
                "feedback": {"enabled": true, "strength": "medium"}
              }
            }
          }]
        }"#;
    let parsed = parse_store(json).unwrap();
    assert_eq!(
        parsed.profiles[0].pads.right_mouse.speed_percent,
        DEFAULT_SCROLL_SPEED_PERCENT
    );
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
fn pad_click_press_release_mirrors_binding() {
    let mut profile = BindingProfile::default();
    profile.bindings.left_pad_click = Some(chord(KeyboardKey::F5, &[Modifier::Command]));
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::LeftPadClick]), &mut sink)
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
fn pad_click_fires_regardless_of_pad_function_and_alongside_motion() {
    // With the pad function disabled, the click is still a live binding.
    let mut disabled = BindingProfile::default();
    disabled.bindings.right_pad_click = Some(BindingAction::MouseButton {
        button: MouseButton::Left,
    });
    let mut engine = BindingEngine::new(disabled.clone());
    let mut sink = MockSink::default();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::RightPadClick]), &mut sink)
        .unwrap();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    assert_eq!(sink.events, ["mouse:Left:true", "mouse:Left:false"]);

    // With the pad function enabled, pointer motion and a click both reach the
    // sink during one continuous touch.
    let mut profile = disabled;
    profile.pads.right_mouse.enabled = true;
    profile.pads.right_mouse.feedback.enabled = false;
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
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((3_044, 292))),
            Duration::from_millis(20),
            &mut sink,
        )
        .unwrap();
    let clicked = buttons(&[BindableControl::RightPadClick]);
    let mut snapshot = pad_snapshot(clicked, None, Some((3_044, 292)));
    snapshot.right_pad.pressed = true;
    engine
        .observe_snapshot(snapshot, Duration::from_millis(40), &mut sink)
        .unwrap();
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((3_044, 292))),
            Duration::from_millis(60),
            &mut sink,
        )
        .unwrap();
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
    profile.pads.right_mouse.enabled = true;
    profile.pads.right_mouse.feedback.enabled = false;
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
    profile.pads.right_mouse.enabled = true;
    profile.pads.right_mouse.feedback.enabled = false;
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
fn pad_click_feedback_is_edge_triggered_when_the_pad_function_is_disabled() {
    let profile = BindingProfile::default();
    assert!(!profile.pads.right_mouse.enabled);
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    let clicked = buttons(&[BindableControl::RightPadClick]);
    let pressed_at = |pressed| {
        let mut snapshot =
            pad_snapshot(if pressed { clicked } else { neutral }, None, Some((0, 0)));
        snapshot.right_pad.pressed = pressed;
        snapshot
    };

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    engine
        .observe_snapshot(pressed_at(false), Duration::from_millis(1), &mut sink)
        .unwrap();
    let press = engine
        .observe_snapshot(pressed_at(true), Duration::from_millis(10), &mut sink)
        .unwrap();
    assert_eq!(press.right, Some(PadFeedbackStrength::Medium));
    let hold = engine
        .observe_snapshot(pressed_at(true), Duration::from_millis(40), &mut sink)
        .unwrap();
    assert_eq!(hold, PadFeedbackRequest::NONE);
    let release = engine
        .observe_snapshot(pressed_at(false), Duration::from_millis(60), &mut sink)
        .unwrap();
    assert_eq!(release, PadFeedbackRequest::NONE);
    assert!(sink.events.is_empty());
}

#[test]
fn pad_click_feedback_respects_each_pads_feedback_setting() {
    let mut profile = BindingProfile::default();
    profile.pads.left_scroll.feedback.enabled = false;
    profile.pads.right_mouse.feedback.strength = PadFeedbackStrength::High;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    let left = engine
        .observe_snapshot(
            pad_snapshot(
                buttons(&[BindableControl::LeftPadClick]),
                Some((0, 0)),
                None,
            ),
            Duration::from_millis(1),
            &mut sink,
        )
        .unwrap();
    assert_eq!(left, PadFeedbackRequest::NONE);
    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, None),
            Duration::from_millis(2),
            &mut sink,
        )
        .unwrap();
    let right = engine
        .observe_snapshot(
            pad_snapshot(
                buttons(&[BindableControl::RightPadClick]),
                None,
                Some((0, 0)),
            ),
            Duration::from_millis(3),
            &mut sink,
        )
        .unwrap();
    assert_eq!(right.right, Some(PadFeedbackStrength::High));
}

#[test]
fn click_freeze_swallows_press_wander_but_drag_escapes() {
    let mut profile = BindingProfile::default();
    profile.pads.right_mouse.enabled = true;
    profile.pads.right_mouse.feedback.enabled = false;
    profile.bindings.right_pad_click = Some(BindingAction::MouseButton {
        button: MouseButton::Left,
    });
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    let clicked = buttons(&[BindableControl::RightPadClick]);
    let pressed_at = |x, y| {
        let mut snapshot = pad_snapshot(clicked, None, Some((x, y)));
        snapshot.right_pad.pressed = true;
        snapshot
    };

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
    profile.pads.right_mouse.enabled = true;
    profile.pads.right_mouse.feedback.enabled = false;
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
fn pad_click_held_at_baseline_is_blocked_until_released() {
    let mut profile = BindingProfile::default();
    profile.bindings.left_pad_click = Some(chord(KeyboardKey::F5, &[]));
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    engine
        .observe(buttons(&[BindableControl::LeftPadClick]), &mut sink)
        .unwrap();
    assert!(sink.events.is_empty());
    engine.observe(buttons(&[]), &mut sink).unwrap();
    assert!(sink.events.is_empty());
    engine
        .observe(buttons(&[BindableControl::LeftPadClick]), &mut sink)
        .unwrap();
    assert_eq!(sink.events, ["key:F5:true"]);
}

#[test]
fn profile_switch_releases_and_blocks_a_held_pad_click() {
    let mut first = BindingProfile::default();
    first.bindings.right_pad_click = Some(chord(KeyboardKey::F5, &[]));
    let mut second = BindingProfile {
        id: "second".to_owned(),
        name: "Second".to_owned(),
        ..BindingProfile::default()
    };
    second.bindings.right_pad_click = Some(chord(KeyboardKey::F9, &[]));
    let mut engine = BindingEngine::new(first);
    let mut sink = MockSink::default();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::RightPadClick]), &mut sink)
        .unwrap();
    engine.replace_profile(second, &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::RightPadClick]), &mut sink)
        .unwrap();
    engine.observe(buttons(&[]), &mut sink).unwrap();
    engine
        .observe(buttons(&[BindableControl::RightPadClick]), &mut sink)
        .unwrap();
    assert_eq!(sink.events, ["key:F5:true", "key:F5:false", "key:F9:true"]);
}

#[test]
fn right_pad_feedback_cadence_increases_with_motion_speed_without_a_backlog() {
    let mut profile = BindingProfile::default();
    profile.pads.right_mouse.enabled = true;
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
    profile.pads.right_mouse.enabled = true;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    for (index, (x, y)) in [
        (0, 0),
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
        let mut snapshot = pad_snapshot(neutral, None, Some((x, y)));
        snapshot.right_pad.pressed = true;
        let feedback = engine
            .observe_snapshot(
                snapshot,
                Duration::from_millis(u64::try_from(index * 250).unwrap()),
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
    profile.pads.left_scroll.enabled = true;
    profile.pads.left_scroll.feedback.enabled = false;
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
        profile.pads.left_scroll.enabled = true;
        profile.pads.left_scroll.feedback.enabled = false;
        profile.pads.left_scroll.speed_percent = speed_percent;
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
        profile.pads.left_scroll.enabled = true;
        profile.pads.left_scroll.feedback.enabled = false;
        profile.pads.left_scroll.momentum = momentum;
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
    profile.pads.left_scroll.enabled = true;
    profile.pads.left_scroll.feedback.enabled = false;
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
    profile.pads.left_scroll.enabled = true;
    profile.pads.left_scroll.feedback.enabled = false;
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
    profile.pads.left_scroll.enabled = true;
    profile.pads.left_scroll.feedback.enabled = false;
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
    profile.pads.right_mouse.enabled = true;
    profile.pads.right_mouse.feedback.enabled = false;
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
        profile.pads.right_mouse.enabled = true;
        profile.pads.right_mouse.feedback.enabled = false;
        profile.pads.right_mouse.speed_percent = speed_percent;
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
    profile.pads.right_mouse.enabled = true;
    profile.pads.right_mouse.feedback.enabled = false;
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
    profile.pads.right_mouse.enabled = true;
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
    profile.pads.right_mouse.enabled = true;
    profile.pads.right_mouse.feedback.enabled = false;
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
    profile.pads.right_mouse.enabled = true;
    profile.pads.right_mouse.feedback.enabled = false;
    profile.bindings.right_pad_click = Some(BindingAction::MouseButton {
        button: MouseButton::Left,
    });
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    let clicked = buttons(&[BindableControl::RightPadClick]);
    let sample = |buttons, x, y, pressure, pressed| {
        let mut snapshot = pad_snapshot(buttons, None, Some((x, y)));
        snapshot.right_pad.pressure = pressure;
        snapshot.right_pad.pressed = pressed;
        snapshot
    };

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    for (time_ms, snapshot) in [
        (1, sample(neutral, -23_322, 6_434, 1_532, false)),
        (12, sample(neutral, -23_216, 6_420, 1_649, false)),
        (64, sample(clicked, -22_946, 6_324, 3_988, true)),
        (212, sample(clicked, -22_076, 8_346, 3_696, true)),
        (244, sample(neutral, -22_298, 8_620, 2_370, false)),
        (266, sample(neutral, -22_714, 9_210, 1_201, false)),
        (287, sample(neutral, -22_848, 9_426, 928, false)),
        (470, sample(neutral, -22_946, 9_772, 421, false)),
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
    profile.pads.left_scroll.enabled = true;
    profile.pads.left_scroll.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    let clicked = buttons(&[BindableControl::LeftPadClick]);
    let sample = |buttons, x, y, pressure, pressed| {
        let mut snapshot = pad_snapshot(buttons, Some((x, y)), None);
        snapshot.left_pad.pressure = pressure;
        snapshot.left_pad.pressed = pressed;
        snapshot
    };

    engine
        .observe_snapshot(pad_snapshot(neutral, None, None), Duration::ZERO, &mut sink)
        .unwrap();
    for (time_ms, snapshot) in [
        (1, sample(neutral, 11_476, -2_014, 1_590, false)),
        (12, sample(neutral, 11_454, -2_060, 1_619, false)),
        (72, sample(clicked, 11_384, -2_176, 4_234, true)),
        (238, sample(neutral, 12_506, -2_504, 2_437, false)),
        (310, sample(neutral, 12_150, -3_092, 1_371, false)),
        (377, sample(neutral, 11_032, -3_058, 1_444, false)),
        (448, sample(neutral, 8_804, -4_220, 1_356, false)),
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
    profile.pads.right_mouse.enabled = true;
    profile.pads.right_mouse.feedback.enabled = false;
    let mut engine = BindingEngine::new(profile);
    let mut sink = MockSink::default();
    let neutral = SteamButtons::default();
    let clicked = buttons(&[BindableControl::RightPadClick]);
    let pressed_at = |x| {
        let mut snapshot = pad_snapshot(clicked, None, Some((x, 0)));
        snapshot.right_pad.pressed = true;
        snapshot
    };

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
    profile.pads.right_mouse.enabled = true;
    profile.pads.right_mouse.feedback.enabled = false;
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
    profile.pads.right_mouse.enabled = true;
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
    replacement.pads.right_mouse.feedback.strength = PadFeedbackStrength::High;
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
