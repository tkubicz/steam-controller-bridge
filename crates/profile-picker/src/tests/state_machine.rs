use super::*;
use crate::geometry::bit;
use crate::types::AXIS_FULL_SCALE;
use controller_mapper::gamepad_button;
use gamepad_state::{Button, OutputSuppression};
use std::f32::consts::TAU;
use std::time::Duration;
use steam_controller_protocol::{SteamButton, SteamButtons};

const ROSTER: PickerRoster = PickerRoster {
    len: 4,
    active: Some(0),
    revision: 0,
};

fn buttons(pressed: &[SteamButton]) -> SteamButtons {
    SteamButtons(pressed.iter().fold(0, |mask, button| mask | bit(*button)))
}

fn input(pressed: &[SteamButton]) -> PickerInput {
    PickerInput {
        buttons: buttons(pressed),
        ..PickerInput::default()
    }
}

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

/// Drives a picker to the open state and returns it, baseline established.
fn opened(roster: PickerRoster) -> Picker {
    let mut picker = Picker::new(PickerConfig::default());
    assert!(picker.update(ms(0), &input(&[]), roster).is_empty());
    picker.update(ms(10), &input(&[TRIGGER]), roster);
    let events = picker.update(ms(2_010), &input(&[TRIGGER]), roster);
    assert!(picker.is_open(), "picker should have opened");
    assert_eq!(events.len(), 1);
    picker
}

#[test]
fn the_first_update_is_a_baseline_that_cannot_open_the_wheel() {
    let mut picker = Picker::new(PickerConfig::default());
    // A trigger already held when the controller connects must not arm.
    assert!(picker.update(ms(0), &input(&[TRIGGER]), ROSTER).is_empty());
    assert!(!picker.is_arming());
    assert!(picker
        .update(ms(5_000), &input(&[TRIGGER]), ROSTER)
        .is_empty());
    assert!(!picker.is_open());
}

#[test]
fn holding_the_trigger_past_the_threshold_opens_the_wheel() {
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), ROSTER);
    assert!(picker.update(ms(10), &input(&[TRIGGER]), ROSTER).is_empty());
    assert!(picker.is_arming());
    // One millisecond short of the hold has not opened anything; the only
    // thing reported by then is the halfway warning.
    let events: Vec<_> = picker
        .update(ms(2_009), &input(&[TRIGGER]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(events, vec![PickerEvent::Preparing]);
    assert!(!picker.is_open());

    let events: Vec<_> = picker
        .update(ms(2_010), &input(&[TRIGGER]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Opened {
            selected: 0,
            page: 0,
            roster_revision: 0,
        }]
    );
}

#[test]
fn events_echo_the_roster_revision_that_defined_their_indices() {
    let roster = PickerRoster::with_revision(4, Some(0), 37);
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), roster);
    picker.update(ms(10), &input(&[TRIGGER]), roster);
    assert_eq!(
        picker
            .update(ms(2_010), &input(&[TRIGGER]), roster)
            .into_iter()
            .collect::<Vec<_>>(),
        vec![PickerEvent::Opened {
            selected: 0,
            page: 0,
            roster_revision: 37,
        }]
    );
    assert_eq!(
        picker
            .update(ms(2_020), &input(&[COMMIT]), roster)
            .into_iter()
            .collect::<Vec<_>>(),
        vec![PickerEvent::Commit {
            index: 0,
            roster_revision: 37,
        }]
    );
}

#[test]
fn the_halfway_warning_arrives_once_with_time_left_to_act_on_it() {
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), ROSTER);
    picker.update(ms(0), &input(&[TRIGGER]), ROSTER);

    // Nothing at all for the first half of the hold: an ordinary press is
    // far shorter than this, and must not start the host's overlay.
    assert!(picker
        .update(ms(999), &input(&[TRIGGER]), ROSTER)
        .is_empty());

    let events: Vec<_> = picker
        .update(ms(1_000), &input(&[TRIGGER]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(events, vec![PickerEvent::Preparing]);
    // Reported once, not on every report for the rest of the hold.
    for at in [1_100, 1_500, 1_999] {
        assert!(picker.update(ms(at), &input(&[TRIGGER]), ROSTER).is_empty());
    }
    let events: Vec<_> = picker
        .update(ms(2_000), &input(&[TRIGGER]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Opened {
            selected: 0,
            page: 0,
            roster_revision: 0,
        }]
    );
}

#[test]
fn a_tap_never_warns_and_a_fresh_hold_warns_again() {
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), ROSTER);
    picker.update(ms(0), &input(&[TRIGGER]), ROSTER);
    let events: Vec<_> = picker
        .update(ms(200), &input(&[]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::TriggerTapped],
        "a tap must not ask the host to prepare anything"
    );

    // The warning is per hold, so the next one gets its own.
    picker.update(ms(300), &input(&[TRIGGER]), ROSTER);
    let events: Vec<_> = picker
        .update(ms(1_300), &input(&[TRIGGER]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(events, vec![PickerEvent::Preparing]);
}

#[test]
fn a_hold_abandoned_after_the_warning_still_reports_the_tap() {
    // The host started its overlay on the warning, so it has to be told the
    // wheel is not coming after all or the overlay would be left running.
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), ROSTER);
    picker.update(ms(0), &input(&[TRIGGER]), ROSTER);
    assert_eq!(
        picker.update(ms(1_200), &input(&[TRIGGER]), ROSTER).len(),
        1
    );
    let events: Vec<_> = picker
        .update(ms(1_500), &input(&[]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(events, vec![PickerEvent::TriggerTapped]);
    assert!(!picker.is_open());
}

#[test]
fn releasing_before_the_threshold_reports_a_tap() {
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), ROSTER);
    picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
    let events: Vec<_> = picker
        .update(ms(500), &input(&[]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(events, vec![PickerEvent::TriggerTapped]);
    assert!(!picker.is_open());
    assert!(!picker.owns_trigger());
}

#[test]
fn the_wheel_opens_on_the_active_profile() {
    let roster = PickerRoster::new(20, Some(9));
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), roster);
    picker.update(ms(10), &input(&[TRIGGER]), roster);
    let events: Vec<_> = picker
        .update(ms(2_010), &input(&[TRIGGER]), roster)
        .into_iter()
        .collect();
    // Index 9 with eight sectors per page is page 1, sector 1.
    assert_eq!(
        events,
        vec![PickerEvent::Opened {
            selected: 1,
            page: 1,
            roster_revision: 0,
        }]
    );
}

#[test]
fn a_roster_too_small_to_choose_from_never_arms() {
    for roster in [PickerRoster::new(0, None), PickerRoster::new(1, Some(0))] {
        let mut picker = Picker::new(PickerConfig::default());
        picker.update(ms(0), &input(&[]), roster);
        picker.update(ms(10), &input(&[TRIGGER]), roster);
        assert!(!picker.is_arming(), "{roster:?} should not arm");
        // The trigger stays the host's, so its binding still works.
        assert!(!picker.owns_trigger());
        assert!(picker
            .update(ms(5_000), &input(&[TRIGGER]), roster)
            .is_empty());
    }
}

#[test]
fn a_roster_that_empties_mid_hold_hands_the_press_back() {
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), ROSTER);
    picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
    let events: Vec<_> = picker
        .update(ms(500), &input(&[TRIGGER]), PickerRoster::new(1, Some(0)))
        .into_iter()
        .collect();
    assert_eq!(events, vec![PickerEvent::TriggerTapped]);
}

#[test]
fn the_wheel_stays_open_after_the_trigger_is_released() {
    let mut picker = opened(ROSTER);
    assert!(picker.update(ms(2_100), &input(&[]), ROSTER).is_empty());
    assert!(picker.is_open());
}

#[test]
fn a_applies_the_pointed_at_profile_and_b_does_not() {
    let mut picker = opened(ROSTER);
    picker.update(ms(2_100), &input(&[]), ROSTER);
    let events: Vec<_> = picker
        .update(ms(2_200), &input(&[COMMIT]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Commit {
            index: 0,
            roster_revision: 0,
        }]
    );
    assert!(!picker.is_open());

    let mut picker = opened(ROSTER);
    picker.update(ms(2_100), &input(&[]), ROSTER);
    let events: Vec<_> = picker
        .update(ms(2_200), &input(&[DISMISS]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(events, vec![PickerEvent::Dismissed]);
    assert!(!picker.is_open());
}

#[test]
fn the_wheel_can_be_opened_again_after_a_commit() {
    let mut picker = opened(ROSTER);
    // Commit while the trigger is still held, then release it.
    picker.update(ms(2_100), &input(&[TRIGGER, COMMIT]), ROSTER);
    assert!(!picker.is_open());
    picker.update(ms(2_200), &input(&[]), ROSTER);

    // A fresh hold must open the wheel a second time.
    picker.update(ms(3_000), &input(&[TRIGGER]), ROSTER);
    assert!(picker.is_arming(), "a fresh press must re-arm");
    let events: Vec<_> = picker
        .update(ms(5_010), &input(&[TRIGGER]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Opened {
            selected: 0,
            page: 0,
            roster_revision: 0,
        }]
    );
}

#[test]
fn the_wheel_can_be_opened_again_after_the_trigger_is_released_first() {
    // The gesture the user actually performs: hold, let go, flick, press A.
    let mut picker = opened(ROSTER);
    picker.update(ms(2_100), &input(&[]), ROSTER);
    picker.update(ms(2_200), &input(&[COMMIT]), ROSTER);
    assert!(!picker.is_open());
    picker.update(ms(2_300), &input(&[]), ROSTER);

    picker.update(ms(3_000), &input(&[TRIGGER]), ROSTER);
    let events: Vec<_> = picker
        .update(ms(5_010), &input(&[TRIGGER]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Opened {
            selected: 0,
            page: 0,
            roster_revision: 0,
        }]
    );
}

#[test]
fn a_second_trigger_press_dismisses_without_reopening() {
    let mut picker = opened(ROSTER);
    picker.update(ms(2_100), &input(&[]), ROSTER);
    let events: Vec<_> = picker
        .update(ms(2_200), &input(&[TRIGGER]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(events, vec![PickerEvent::Dismissed]);
    // Holding that same press must not arm a fresh hold, and releasing it
    // must not be reported as a tap.
    assert!(picker
        .update(ms(5_000), &input(&[TRIGGER]), ROSTER)
        .is_empty());
    assert!(picker.update(ms(5_100), &input(&[]), ROSTER).is_empty());
    assert!(!picker.is_open());
}

#[test]
fn the_trigger_is_hidden_from_the_host_only_while_the_picker_owns_it() {
    let mut picker = Picker::new(PickerConfig::default());
    let held = buttons(&[TRIGGER, SteamButton::X]);
    picker.update(ms(0), &input(&[]), ROSTER);
    assert_eq!(picker.mask_trigger(held), held);

    picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
    assert!(picker.is_arming());
    assert_eq!(picker.mask_trigger(held), buttons(&[SteamButton::X]));

    picker.update(ms(2_010), &input(&[TRIGGER]), ROSTER);
    assert!(picker.is_open());
    assert_eq!(picker.mask_trigger(held), buttons(&[SteamButton::X]));

    picker.update(ms(2_100), &input(&[COMMIT]), ROSTER);
    assert_eq!(picker.mask_trigger(held), held);
}

#[test]
fn the_button_that_applied_a_profile_is_held_back_until_released() {
    // Regression: the wheel closes on the press edge, so A is still down on
    // the next report. Lifting suppression there sent that press straight to
    // the game -- a press the user aimed at the overlay.
    let mut picker = opened(ROSTER);
    picker.update(ms(2_100), &input(&[]), ROSTER);
    picker.update(ms(2_200), &input(&[COMMIT]), ROSTER);
    assert!(!picker.is_open());

    let Some(OutputSuppression::Buttons(buttons)) = picker.suppression() else {
        panic!("a still-held commit must keep being withheld");
    };
    assert!(buttons.contains(Button::South));
    assert!(
        !buttons.contains(Button::North),
        "only what the wheel consumed is withheld; the rest of the pad works"
    );

    // Still down a few reports later.
    picker.update(ms(2_250), &input(&[COMMIT]), ROSTER);
    assert!(matches!(
        picker.suppression(),
        Some(OutputSuppression::Buttons(_))
    ));

    // Released, so the game gets the button back.
    picker.update(ms(2_300), &input(&[]), ROSTER);
    assert_eq!(picker.suppression(), None);

    // And a deliberate later press does reach the game.
    picker.update(ms(2_400), &input(&[COMMIT]), ROSTER);
    assert_eq!(picker.suppression(), None);
}

#[test]
fn the_button_that_dismissed_the_wheel_is_held_back_too() {
    for closing in [DISMISS, TRIGGER] {
        let mut picker = opened(ROSTER);
        picker.update(ms(2_100), &input(&[]), ROSTER);
        picker.update(ms(2_200), &input(&[closing]), ROSTER);
        assert!(!picker.is_open());
        let Some(OutputSuppression::Buttons(buttons)) = picker.suppression() else {
            panic!("{closing:?} must keep being withheld while held");
        };
        assert!(buttons
            .contains(gamepad_button(closing).expect("consumed controls are directly mapped")));
        picker.update(ms(2_300), &input(&[]), ROSTER);
        assert_eq!(picker.suppression(), None, "{closing:?}");
    }
}

#[test]
fn a_control_released_before_the_wheel_closed_is_never_latched() {
    // Committing with A while Quick Access was long since released must not
    // withhold Quick Access from the game.
    let mut picker = opened(ROSTER);
    picker.update(ms(2_100), &input(&[]), ROSTER);
    picker.update(ms(2_200), &input(&[COMMIT]), ROSTER);
    let Some(OutputSuppression::Buttons(buttons)) = picker.suppression() else {
        panic!("the commit is still held");
    };
    assert!(!buttons.contains(Button::Extra3));
}

#[test]
fn a_forced_close_latches_nothing() {
    // The controller is gone, so no report will ever arrive to clear a
    // latch. Holding one would withhold those buttons forever.
    let mut picker = opened(ROSTER);
    assert!(picker.close());
    assert_eq!(picker.suppression(), None);
}

#[test]
fn only_an_open_wheel_takes_the_output_from_the_game() {
    let mut picker = Picker::new(PickerConfig::default());
    assert!(picker.suppression().is_none());

    picker.update(ms(0), &input(&[]), ROSTER);
    picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
    // Arming still passes everything through; only opening takes over.
    assert!(picker.suppression().is_none());
    picker.update(ms(1_000), &input(&[TRIGGER]), ROSTER);
    assert!(
        picker.suppression().is_none(),
        "the halfway warning must not touch the game's input"
    );

    picker.update(ms(2_010), &input(&[TRIGGER]), ROSTER);
    assert!(picker.is_open());
    assert_eq!(picker.suppression(), Some(OutputSuppression::Neutral));

    // Closing hands the pad back, save for the still-held button that closed
    // it; releasing that clears the last of it.
    picker.update(ms(2_100), &input(&[DISMISS]), ROSTER);
    picker.update(ms(2_200), &input(&[]), ROSTER);
    assert!(picker.suppression().is_none());
}

#[test]
fn closing_reports_whether_anything_was_active() {
    let mut picker = opened(ROSTER);
    assert!(picker.close());
    assert!(!picker.is_open());
    assert!(!picker.close());

    // After a forced close the next update is a fresh baseline, so a still
    // held trigger cannot immediately reopen the wheel.
    assert!(picker
        .update(ms(3_000), &input(&[TRIGGER]), ROSTER)
        .is_empty());
    assert!(!picker.is_arming());
}

fn stick(x: f32, y: f32) -> (i16, i16) {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "test helper builds in-range axis values"
    )]
    ((x * AXIS_FULL_SCALE) as i16, (y * AXIS_FULL_SCALE) as i16)
}

#[test]
fn every_sector_centre_selects_its_own_sector() {
    for sectors in MIN_SECTORS_PER_PAGE..=MAX_SECTORS_PER_PAGE {
        let arc = TAU / sectors as f32;
        for sector in 0..sectors {
            let angle = arc * sector as f32;
            // Sector 0 is up, running clockwise.
            let (x, y) = (angle.sin(), angle.cos());
            assert_eq!(
                sector_for(x, y, sectors),
                sector,
                "sector {sector} of {sectors}"
            );
        }
    }
}

#[test]
fn sector_boundaries_fall_between_neighbours() {
    let sectors = 8;
    let arc = TAU / sectors as f32;
    for sector in 0..sectors {
        let boundary = arc * (sector as f32 + 0.5);
        let nudge = arc * 0.05;
        let before = boundary - nudge;
        let after = boundary + nudge;
        assert_eq!(sector_for(before.sin(), before.cos(), sectors), sector);
        assert_eq!(
            sector_for(after.sin(), after.cos(), sectors),
            (sector + 1) % sectors
        );
    }
}

#[test]
fn straight_up_is_sector_zero_and_straight_right_is_a_quarter_turn() {
    assert_eq!(sector_for(0.0, 1.0, 8), 0);
    assert_eq!(sector_for(1.0, 0.0, 8), 2);
    assert_eq!(sector_for(0.0, -1.0, 8), 4);
    assert_eq!(sector_for(-1.0, 0.0, 8), 6);
    assert_eq!(sector_for(0.0, 0.0, 1), 0);
}

#[test]
fn the_stick_must_be_pushed_past_the_engage_dead_zone_to_steer() {
    let mut picker = opened(ROSTER);
    let config = *picker.config();
    // Below the engage dead zone nothing moves, even pointing elsewhere.
    let weak = config.engage_dead_zone - 0.05;
    let events = picker.update(
        ms(2_100),
        &PickerInput {
            left_stick: stick(0.0, -weak),
            ..PickerInput::default()
        },
        ROSTER,
    );
    assert!(events.is_empty(), "a light push must not steer");

    let events: Vec<_> = picker
        .update(
            ms(2_200),
            &PickerInput {
                left_stick: stick(0.0, -1.0),
                ..PickerInput::default()
            },
            ROSTER,
        )
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Selection {
            selected: 2,
            page: 0,
            roster_revision: 0,
        }]
    );
}

#[test]
fn the_selection_stays_put_when_the_stick_is_released() {
    let mut picker = opened(ROSTER);
    picker.update(
        ms(2_100),
        &PickerInput {
            left_stick: stick(0.0, -1.0),
            ..PickerInput::default()
        },
        ROSTER,
    );
    // Recentering must not snap the selection back to where it started;
    // the user still has to reach for A.
    assert!(picker.update(ms(2_200), &input(&[]), ROSTER).is_empty());
    let events: Vec<_> = picker
        .update(ms(2_300), &input(&[COMMIT]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Commit {
            index: 2,
            roster_revision: 0,
        }]
    );
}

