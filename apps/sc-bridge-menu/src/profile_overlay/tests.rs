use super::wheel::point_on_wheel;
use super::window::{choose_screen, index_of_frame, screen_containing, Rect};
use super::*;

fn roster(count: usize, sectors_per_page: usize) -> OverlayState {
    let mut state = OverlayState::default();
    state.apply(OverlayMessage::Roster {
        names: (0..count).map(|index| format!("Profile {index}")).collect(),
        active: Some(0),
        sectors_per_page,
    });
    state
}

/// A primary display with a second one placed to its left, which puts the
/// second at a negative origin -- the case that breaks any code assuming
/// screen coordinates start at zero.
const LEFT_OF_PRIMARY: [Rect; 2] = [(0.0, 0.0, 2560.0, 1440.0), (-1920.0, 0.0, 1920.0, 1080.0)];

#[test]
fn a_point_resolves_to_the_screen_it_falls_in() {
    assert_eq!(screen_containing((100.0, 100.0), &LEFT_OF_PRIMARY), Some(0));
    assert_eq!(
        screen_containing((-100.0, 100.0), &LEFT_OF_PRIMARY),
        Some(1),
        "a display left of the primary has a negative origin"
    );
    assert_eq!(
        screen_containing((-1920.0, 0.0), &LEFT_OF_PRIMARY),
        Some(1),
        "the far corner of that display is still inside it"
    );
}

#[test]
fn a_shared_edge_belongs_to_exactly_one_screen() {
    // x = 0 is the primary's left edge and one past the second's right
    // edge. Half-open ranges keep it from matching both.
    assert_eq!(screen_containing((0.0, 500.0), &LEFT_OF_PRIMARY), Some(0));
    assert_eq!(
        screen_containing((-0.001, 500.0), &LEFT_OF_PRIMARY),
        Some(1)
    );
}

#[test]
fn a_point_outside_every_screen_matches_nothing() {
    // The cursor can sit in a gap between displays that are not aligned.
    assert_eq!(screen_containing((0.0, 2000.0), &LEFT_OF_PRIMARY), None);
    assert_eq!(screen_containing((-5000.0, 0.0), &LEFT_OF_PRIMARY), None);
    assert_eq!(
        screen_containing((2560.0, 0.0), &LEFT_OF_PRIMARY),
        None,
        "just past the primary's right edge"
    );
}

#[test]
fn an_empty_screen_list_matches_nothing() {
    assert_eq!(screen_containing((0.0, 0.0), &[]), None);
}

#[test]
fn a_non_primary_focused_window_wins_because_only_a_real_one_reports_it() {
    assert_eq!(
        choose_screen(Some(2), Some(1)),
        (2, ScreenSource::FocusedWindow)
    );
    assert_eq!(
        choose_screen(Some(3), None),
        (3, ScreenSource::FocusedWindow)
    );
}

#[test]
fn a_primary_focused_window_yields_to_the_cursor() {
    // `mainScreen` returning the primary is indistinguishable from its
    // no-key-window fallback, so it must not outvote a cursor that is
    // somewhere else. Trusting it here is exactly the bug being fixed.
    assert_eq!(choose_screen(Some(0), Some(4)), (4, ScreenSource::Cursor));
    assert_eq!(choose_screen(None, Some(1)), (1, ScreenSource::Cursor));
}

#[test]
fn the_primary_is_used_when_nothing_points_anywhere_else() {
    assert_eq!(
        choose_screen(Some(0), Some(0)),
        (0, ScreenSource::FocusedWindow)
    );
    assert_eq!(
        choose_screen(Some(0), None),
        (0, ScreenSource::FocusedWindow)
    );
    assert_eq!(choose_screen(None, Some(0)), (0, ScreenSource::Cursor));
    assert_eq!(choose_screen(None, None), (0, ScreenSource::Primary));
}

#[test]
fn a_screen_frame_resolves_to_its_index() {
    assert_eq!(
        index_of_frame(&LEFT_OF_PRIMARY, LEFT_OF_PRIMARY[1]),
        Some(1)
    );
    // AppKit hands back the same numbers on both sides, but a sub-pixel
    // difference must not silently fall through to the wrong display.
    assert_eq!(
        index_of_frame(&LEFT_OF_PRIMARY, (-1920.2, 0.1, 1920.0, 1080.0)),
        Some(1)
    );
    assert_eq!(
        index_of_frame(&LEFT_OF_PRIMARY, (500.0, 0.0, 800.0, 600.0)),
        None
    );
    assert_eq!(index_of_frame(&[], LEFT_OF_PRIMARY[0]), None);
}

#[test]
fn every_screen_source_has_a_distinct_label() {
    let labels: Vec<_> = [
        ScreenSource::FocusedWindow,
        ScreenSource::Cursor,
        ScreenSource::Primary,
    ]
    .into_iter()
    .map(ScreenSource::label)
    .collect();
    // The log line is the only way to tell which branch won on real
    // hardware, so the labels have to be distinguishable.
    assert_eq!(labels, ["focused_window", "cursor", "primary"]);
}

#[test]
fn the_wheel_can_be_shown_again_after_it_has_been_hidden() {
    // Regression: the overlay used to hide by ordering the window out.
    // macOS then stopped delivering redraws, the overlay's only route to
    // the main thread, so the second open could never be acted on and the
    // wheel opened exactly once per process.
    let mut presentation = Presentation::default();
    assert_eq!(
        presentation.update(false, false),
        PresentationChange::default(),
        "nothing happens before the window has been sized"
    );
    assert_eq!(
        presentation.update(false, true),
        PresentationChange {
            order_in: true,
            wheel: None
        }
    );

    for round in 0..3 {
        assert_eq!(
            presentation.update(true, true),
            PresentationChange {
                order_in: false,
                wheel: Some(true)
            },
            "round {round} must be able to show the wheel"
        );
        assert_eq!(
            presentation.update(false, true),
            PresentationChange {
                order_in: false,
                wheel: Some(false)
            },
            "round {round} must be able to hide the wheel"
        );
    }
}

#[test]
fn the_window_is_ordered_in_exactly_once() {
    let mut presentation = Presentation::default();
    let mut order_ins = 0;
    for open in [false, true, false, true, true, false, false, true] {
        if presentation.update(open, true).order_in {
            order_ins += 1;
        }
    }
    // More than one would mean the window had been ordered out in between,
    // which is the state the wheel cannot recover from.
    assert_eq!(order_ins, 1);
}

#[test]
fn an_unchanged_wheel_state_asks_for_no_work() {
    let mut presentation = Presentation::default();
    presentation.update(false, true);
    presentation.update(true, true);
    assert_eq!(
        presentation.update(true, true),
        PresentationChange::default(),
        "a repeated frame must not re-apply the alpha"
    );
}

#[test]
fn open_drives_visibility_and_moves_the_highlight() {
    // There is no close message: a closed wheel is a killed process.
    let mut state = roster(4, 8);
    assert!(state.open.is_none());
    state.apply(OverlayMessage::Open {
        selected: 2,
        page: 0,
    });
    assert_eq!(state.open, Some((2, 0)));
    state.apply(OverlayMessage::Open {
        selected: 3,
        page: 0,
    });
    assert_eq!(state.open, Some((3, 0)));
}

#[test]
fn a_roster_update_while_open_keeps_the_wheel_up() {
    let mut state = roster(4, 8);
    state.apply(OverlayMessage::Open {
        selected: 1,
        page: 0,
    });
    state.apply(OverlayMessage::Roster {
        names: vec!["Solo".to_owned()],
        active: None,
        sectors_per_page: 8,
    });
    // Only the runtime closes the wheel; the overlay must not decide that
    // for itself or the two would disagree about what is on screen.
    assert_eq!(state.open, Some((1, 0)));
}

#[test]
fn pages_slice_the_roster_without_gaps_or_overlap() {
    let state = roster(11, 8);
    assert_eq!(state.page_count(), 2);
    let (first, first_offset) = state.page_entries(0);
    let (second, second_offset) = state.page_entries(1);
    assert_eq!(first.len(), 8);
    assert_eq!(first_offset, 0);
    assert_eq!(second.len(), 3);
    assert_eq!(second_offset, 8);
    assert_eq!(second[0], "Profile 8");
}

#[test]
fn a_page_past_the_end_yields_nothing_rather_than_panicking() {
    let state = roster(3, 8);
    let (entries, offset) = state.page_entries(9);
    assert!(entries.is_empty());
    assert_eq!(offset, 3);
}

#[test]
fn an_empty_roster_reports_one_page_and_no_entries() {
    let state = roster(0, 8);
    assert_eq!(state.page_count(), 1);
    assert!(state.page_entries(0).0.is_empty());
}

#[test]
fn a_zero_sector_roster_cannot_divide_by_zero() {
    let mut state = OverlayState::default();
    state.apply(OverlayMessage::Roster {
        names: vec!["One".to_owned(), "Two".to_owned()],
        active: None,
        sectors_per_page: 0,
    });
    assert_eq!(state.sectors_per_page, 1);
    assert_eq!(state.page_count(), 2);
    assert_eq!(state.page_entries(1).0, ["Two".to_owned()]);
}

#[test]
fn sector_zero_is_up_and_the_wheel_runs_clockwise() {
    let center = egui::pos2(100.0, 100.0);
    let up = point_on_wheel(center, 0.0, 10.0);
    assert!((up.x - 100.0).abs() < 1.0e-4);
    assert!((up.y - 90.0).abs() < 1.0e-4, "sector zero must point up");

    let right = point_on_wheel(center, std::f32::consts::FRAC_PI_2, 10.0);
    assert!(
        (right.x - 110.0).abs() < 1.0e-4,
        "a quarter turn must point right"
    );
    assert!((right.y - 100.0).abs() < 1.0e-4);
}
