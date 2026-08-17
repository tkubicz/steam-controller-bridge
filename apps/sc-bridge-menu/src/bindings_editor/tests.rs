use super::*;

#[test]
fn callouts_cover_each_bindable_control_once() {
    // Pads are reached through their own hotspots, so every remaining bindable
    // control needs exactly one callout.
    let controls = CONTROL_CALLOUTS
        .iter()
        .map(|callout| callout.control)
        .collect::<BTreeSet<_>>();
    assert_eq!(controls.len(), CONTROL_CALLOUTS.len());
    for control in BindableControl::ALL {
        assert!(controls.contains(&control), "{control:?} has no callout");
    }
}

#[test]
fn rear_callouts_match_physical_view_orientation() {
    // The paddle geometry half of this now lives in `controller-art` as
    // `the_rear_view_keeps_physical_handedness`; what stays here is that
    // the callouts agree with it.
    for callout in CONTROL_CALLOUTS {
        let rear = callout.view == ControllerView::Rear;
        assert_eq!(rear, callout.control != BindableControl::QuickAccess);
    }
}

#[test]
fn binding_summaries_are_compact_and_mac_native() {
    let modifiers = [Modifier::Command, Modifier::Shift]
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        binding_summary(Some(&BindingAction::KeyChord {
            key: KeyboardKey::F5,
            modifiers,
        })),
        "⌘⇧F5"
    );
    assert_eq!(
        binding_summary(Some(&BindingAction::MouseButton {
            button: MouseButton::Middle,
        })),
        "Middle Mouse"
    );
    assert_eq!(binding_summary(None), "Unbound");
}

#[test]
fn duplicating_a_maximum_length_profile_keeps_a_valid_name() {
    let mut store = BindingStore::default();
    store.profiles[0].name = "A".repeat(MAX_PROFILE_NAME_CHARS);
    let mut editor = BindingsEditor::new(std::path::PathBuf::new(), store);

    editor.duplicate_profile();

    assert_eq!(editor.store.profiles.len(), 2);
    assert!(editor.store.profiles[1].name.chars().count() <= MAX_PROFILE_NAME_CHARS);
    editor.store.validate().unwrap();
    assert!(editor.message.is_none());
}

#[test]
fn duplicated_profiles_preserve_independent_pad_settings() {
    let mut store = BindingStore::default();
    store.profiles[0].pads.right.motion = PadMotionMode::Pointer;
    store.profiles[0].pads.right.feedback.enabled = false;
    store.profiles[0].pads.left.feedback.strength = PadFeedbackStrength::High;
    store.profiles[0].pads.left.speed_percent = 175;
    store.profiles[0].pads.left.momentum = false;
    store.profiles[0].pads.left.regions = PadRegion::four_way();
    let mut editor = BindingsEditor::new(std::path::PathBuf::new(), store);

    editor.duplicate_profile();

    assert_eq!(editor.store.profiles[1].pads, editor.store.profiles[0].pads);
    assert_eq!(
        editor.selection,
        EditorSelection::Button(BindableControl::QuickAccess)
    );
}

#[test]
fn pad_edits_participate_in_dirty_state_detection() {
    let mut editor = BindingsEditor::new(std::path::PathBuf::new(), BindingStore::default());
    assert!(!editor.is_dirty());
    editor.store.profiles[0].pads.right.motion = PadMotionMode::Pointer;
    assert!(editor.is_dirty());
    editor.store.profiles[0].pads.right.motion = PadMotionMode::None;
    assert!(!editor.is_dirty());
    editor.store.profiles[0].pads.left.regions = PadRegion::four_way();
    assert!(editor.is_dirty());
}

#[test]
fn pad_selections_describe_the_pad_rather_than_a_fixed_role() {
    // Neither pad owns a behavior any more, so neither description may name one.
    for side in [PadSide::Left, PadSide::Right] {
        for selection in [
            EditorSelection::Pad(side),
            EditorSelection::PadRegion(side, 0),
        ] {
            assert_eq!(
                selection_description(selection),
                selection_description(EditorSelection::Pad(side))
            );
        }
    }
    assert_eq!(
        selection_description(EditorSelection::Pad(PadSide::Left)),
        "Left trackpad motion and regions"
    );
    let pads = desktop_bindings::PadBindings::default();
    for pad in [&pads.left, &pads.right] {
        assert_eq!(pad.motion, PadMotionMode::None);
        assert!(pad.regions.is_empty());
        assert!(pad.feedback.enabled);
        assert_eq!(pad.feedback.strength, PadFeedbackStrength::Medium);
        assert_eq!(pad.speed_percent, 100);
        assert!(pad.momentum);
    }
}

#[test]
fn adding_regions_generates_unique_names_the_store_accepts() {
    let mut editor = BindingsEditor::new(std::path::PathBuf::new(), BindingStore::default());
    editor.pad(PadSide::Left).regions = PadRegion::four_way();
    for _ in 0..3 {
        editor.add_region(PadSide::Left);
    }
    // Each add selects what it created, so the shape editor opens on it.
    assert_eq!(
        editor.selection,
        EditorSelection::PadRegion(PadSide::Left, 6)
    );
    let regions = &editor.store.profiles[0].pads.left.regions;
    assert_eq!(regions.len(), 7);
    let names = regions
        .iter()
        .map(|region| region.name.to_lowercase())
        .collect::<BTreeSet<_>>();
    assert_eq!(names.len(), regions.len());
    editor.store.validate().unwrap();
}

#[test]
fn save_normalization_trims_profile_and_region_names() {
    let mut editor = BindingsEditor::new(std::path::PathBuf::new(), BindingStore::default());
    editor.store.profiles[0].name = "  Default  ".to_owned();
    editor.store.profiles[0].pads.left.regions = PadRegion::whole();
    editor.store.profiles[0].pads.left.regions[0].name = "  Whole Pad  ".to_owned();

    editor.normalize_names();

    assert_eq!(editor.store.profiles[0].name, "Default");
    assert_eq!(
        editor.store.profiles[0].pads.left.regions[0].name,
        "Whole Pad"
    );
    editor.store.validate().unwrap();
}

#[test]
fn deleting_a_region_drops_the_stale_selection_instead_of_indexing_past_the_list() {
    let mut editor = BindingsEditor::new(std::path::PathBuf::new(), BindingStore::default());
    editor.pad(PadSide::Left).regions = PadRegion::four_way();
    editor.selection = EditorSelection::PadRegion(PadSide::Left, 3);
    assert_eq!(editor.selected_region(PadSide::Left), Some(3));
    editor.pad(PadSide::Left).regions.truncate(2);
    assert_eq!(editor.selected_region(PadSide::Left), None);
    // A region selected on one pad is not a region selected on the other.
    editor.selection = EditorSelection::PadRegion(PadSide::Left, 0);
    assert_eq!(editor.selected_region(PadSide::Right), None);
}

#[test]
fn an_action_slot_survives_a_target_that_names_a_deleted_region() {
    let mut editor = BindingsEditor::new(std::path::PathBuf::new(), BindingStore::default());
    editor.pad(PadSide::Left).regions = PadRegion::four_way();
    let target = ActionTarget::Region(PadSide::Left, 3, PadTrigger::Click);
    *editor.action_slot(target).unwrap() = Some(BindingAction::MouseButton {
        button: MouseButton::Middle,
    });
    assert!(editor.pad(PadSide::Left).regions[3].click.is_some());
    editor.pad(PadSide::Left).regions.clear();
    assert!(editor.action_slot(target).is_none());
}

#[test]
fn canvas_layout_keeps_both_views_square_and_leaves_room_for_labels() {
    let canvas = egui::Rect::from_min_size(egui::pos2(20.0, 40.0), egui::vec2(830.0, 560.0));
    let layout = CanvasLayout::new(canvas);
    for view in [layout.front, layout.rear] {
        // Square to within float noise, so the artwork keeps its aspect.
        assert!((view.width() - view.height()).abs() < 1e-3);
    }
    let (front, rear) = (
        layout.body(ControllerView::Front),
        layout.body(ControllerView::Rear),
    );
    for body in [front, rear] {
        assert!(canvas.contains_rect(body));
    }
    assert!((front.top() - rear.top()).abs() < f32::EPSILON);
    assert!(front.right() < rear.left());
    for callout in CONTROL_CALLOUTS {
        let label = layout.label(callout);
        assert!(
            canvas.contains_rect(label),
            "the {} label at {label:?} escapes the canvas",
            callout.control.label(),
        );
        assert!(
            !label.intersects(layout.body(callout.view)),
            "the {} label overlaps the controller",
            callout.control.label(),
        );
    }
    let upper = layout.label(CONTROL_CALLOUTS[1]);
    let lower = layout.label(CONTROL_CALLOUTS[2]);
    assert!(upper.bottom() < lower.top(), "stacked labels overlap");

    // Every leader has to start on its label's edge and run at the middle
    // of the control it names.
    for callout in CONTROL_CALLOUTS {
        let label = layout.label(callout);
        let target = control_rect(layout.view(callout.view), art_control(callout.control)).center();
        let start = rect_edge_towards(label, target);
        let on_edge = (start.x - label.left()).abs() < 0.01
            || (start.x - label.right()).abs() < 0.01
            || (start.y - label.top()).abs() < 0.01
            || (start.y - label.bottom()).abs() < 0.01;
        assert!(
            on_edge,
            "the {} leader starts at {start:?}, off its label's edge",
            callout.control.label()
        );
        // Start, label centre and target are collinear, so the leader
        // points straight at the middle of the control.
        let along = target - label.center();
        let out = start - label.center();
        let cross = along.x.mul_add(out.y, -(along.y * out.x));
        assert!(
            cross.abs() < 0.5,
            "the {} leader does not aim at the control's middle",
            callout.control.label(),
        );
    }
}

#[test]
fn the_diagram_and_the_inspector_fit_inside_the_window() {
    for window in [MIN_WINDOW_SIZE[0], WINDOW_SIZE[0]] {
        let content_width = window - 40.0;
        let canvas_width = (content_width - INSPECTOR_WIDTH - COLUMN_GAP).max(CANVAS_MIN_WIDTH);
        // Exactly, not merely within: the inspector's right edge is what
        // the Save and Cancel buttons line up against.
        assert!(
            (canvas_width + COLUMN_GAP + INSPECTOR_WIDTH - content_width).abs() < f32::EPSILON,
            "at a {window}pt window the row does not fill the content width",
        );
    }
}
