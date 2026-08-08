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
fn loading_version_one_atomically_migrates_to_version_three() {
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
            pad_snapshot(neutral, None, Some((868, 100))),
            Duration::from_millis(500),
            &mut sink,
        )
        .unwrap();
    assert_eq!(first.right, Some(PadFeedbackStrength::Medium));
    let slow_limited = engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((1636, 100))),
            Duration::from_millis(800),
            &mut sink,
        )
        .unwrap();
    assert_eq!(slow_limited, PadFeedbackRequest::NONE);
    let slow_ready = engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((2404, 100))),
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
            pad_snapshot(neutral, None, Some((868, 100))),
            Duration::from_millis(10),
            &mut fast_sink,
        )
        .unwrap();
    assert_eq!(fast_first.right, Some(PadFeedbackStrength::Medium));
    let fast_limited = fast_engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((1636, 100))),
            Duration::from_millis(60),
            &mut fast_sink,
        )
        .unwrap();
    assert_eq!(fast_limited, PadFeedbackRequest::NONE);
    let fast_ready = fast_engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((2404, 100))),
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
    assert_eq!(sink.events, ["scroll:6:-3"]);
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

    assert_eq!(scroll_once(501, 100), ["scroll:2:0"]);
    assert_eq!(scroll_once(20, 100), ["scroll:6:0"]);
    assert_eq!(scroll_once(20, 50), ["scroll:3:0"]);
    assert_eq!(scroll_once(20, 200), ["scroll:12:0"]);
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
    assert_eq!(without_momentum, ["scroll:12:0"]);
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

    for (time_ms, x) in [(9, 192), (10, 384), (11, 448)] {
        engine
            .observe_snapshot(
                pad_snapshot(neutral, None, Some((x, 0))),
                Duration::from_millis(time_ms),
                &mut sink,
            )
            .unwrap();
    }
    assert_eq!(sink.events, ["move:3:0", "move:3:0"]);

    engine
        .observe_snapshot(
            pad_snapshot(neutral, None, Some((-32_700, 0))),
            Duration::from_millis(12),
            &mut sink,
        )
        .unwrap();
    engine
        .observe_snapshot(
            DesktopInputSnapshot {
                right_pad: PadSample {
                    x: -32_508,
                    pressure: i16::MAX,
                    touched: true,
                    pressed: true,
                    ..PadSample::default()
                },
                ..pad_snapshot(neutral, None, None)
            },
            Duration::from_millis(13),
            &mut sink,
        )
        .unwrap();
    assert_eq!(sink.events, ["move:3:0", "move:3:0", "move:3:0"]);
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
