use super::*;

#[test]
fn callouts_cover_each_bindable_control_once() {
    // Pad clicks are the only bindable controls without a callout: they are
    // edited through the pad hotspots' inspector instead.
    let controls = CONTROL_CALLOUTS
        .iter()
        .map(|callout| callout.control)
        .collect::<BTreeSet<_>>();
    assert_eq!(controls.len(), CONTROL_CALLOUTS.len());
    for control in BindableControl::ALL {
        let pad_click = matches!(
            control,
            BindableControl::LeftPadClick | BindableControl::RightPadClick
        );
        assert_eq!(controls.contains(&control), !pad_click);
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
    store.profiles[0].pads.right_mouse.enabled = true;
    store.profiles[0].pads.right_mouse.feedback.enabled = false;
    store.profiles[0].pads.left_scroll.feedback.strength = PadFeedbackStrength::High;
    store.profiles[0].pads.left_scroll.speed_percent = 175;
    store.profiles[0].pads.left_scroll.momentum = false;
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
    editor.store.profiles[0].pads.right_mouse.enabled = true;
    assert!(editor.is_dirty());
    editor.store.profiles[0].pads.right_mouse.enabled = false;
    assert!(!editor.is_dirty());
}

#[test]
fn pad_selections_have_fixed_roles_and_default_feedback() {
    assert_eq!(
        selection_description(EditorSelection::Pad(PadSide::Left)),
        "Two-axis smooth desktop scrolling, bindable click"
    );
    assert_eq!(
        selection_description(EditorSelection::Pad(PadSide::Right)),
        "Relative desktop pointer movement, bindable click"
    );
    let pads = desktop_bindings::PadBindings::default();
    assert!(!pads.left_scroll.enabled);
    assert!(!pads.right_mouse.enabled);
    assert!(pads.left_scroll.feedback.enabled);
    assert_eq!(pads.left_scroll.speed_percent, 100);
    assert!(pads.left_scroll.momentum);
    assert_eq!(
        pads.right_mouse.feedback.strength,
        PadFeedbackStrength::Medium
    );
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
