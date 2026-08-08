#[test]
fn a_stick_between_the_dead_zones_keeps_steering_once_engaged() {
    let mut picker = opened(ROSTER);
    let config = *picker.config();
    picker.update(
        ms(2_100),
        &PickerInput {
            left_stick: stick(0.0, -1.0),
            ..PickerInput::default()
        },
        ROSTER,
    );
    // Still engaged, so drifting to a neighbour at reduced deflection works.
    let partial = config.engage_dead_zone.midpoint(config.track_dead_zone);
    let events: Vec<_> = picker
        .update(
            ms(2_200),
            &PickerInput {
                left_stick: stick(-partial, 0.0),
                ..PickerInput::default()
            },
            ROSTER,
        )
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Selection {
            selected: 3,
            page: 0,
            roster_revision: 0,
        }]
    );
}

#[test]
fn the_stick_pushed_furthest_wins() {
    let mut picker = opened(ROSTER);
    let events: Vec<_> = picker
        .update(
            ms(2_100),
            &PickerInput {
                // The left stick rests just past the dead zone pointing up;
                // the right one is slammed down and must win.
                left_stick: stick(0.0, 0.6),
                right_stick: stick(0.0, -1.0),
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
fn either_stick_steers() {
    for build in [
        (|s| PickerInput {
            left_stick: s,
            ..PickerInput::default()
        }) as fn((i16, i16)) -> PickerInput,
        |s| PickerInput {
            right_stick: s,
            ..PickerInput::default()
        },
    ] {
        let mut picker = opened(ROSTER);
        let events: Vec<_> = picker
            .update(ms(2_100), &build(stick(1.0, 0.0)), ROSTER)
            .into_iter()
            .collect();
        assert_eq!(
            events,
            vec![PickerEvent::Selection {
                selected: 1,
                page: 0,
                roster_revision: 0,
            }]
        );
    }
}

#[test]
fn pages_cover_the_roster_without_gaps_or_overlap() {
    for len in 0..40_usize {
        for per_page in MIN_SECTORS_PER_PAGE..=MAX_SECTORS_PER_PAGE {
            let pages = page_count(len, per_page);
            assert!(pages >= 1);
            if len == 0 {
                continue;
            }
            let covered: usize = (0..pages)
                .map(|page| sectors_on_page(len, per_page, page))
                .sum();
            assert_eq!(covered, len, "len {len} per_page {per_page}");
        }
    }
}

#[test]
fn the_shoulders_page_and_the_commit_index_accounts_for_the_page() {
    let roster = PickerRoster::new(11, Some(0));
    let mut picker = opened(roster);
    let events: Vec<_> = picker
        .update(ms(2_100), &input(&[PAGE_NEXT]), roster)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Selection {
            selected: 0,
            page: 1,
            roster_revision: 0,
        }]
    );
    // Page 1 is a short page: indices 8..11 spread over three sectors, so
    // the arcs are 120 degrees and sector 2 points down-left.
    let angle = TAU / 3.0 * 2.0;
    picker.update(ms(2_200), &input(&[]), roster);
    picker.update(
        ms(2_300),
        &PickerInput {
            left_stick: stick(angle.sin(), angle.cos()),
            ..PickerInput::default()
        },
        roster,
    );
    let events: Vec<_> = picker
        .update(ms(2_400), &input(&[COMMIT]), roster)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Commit {
            index: 10,
            roster_revision: 0,
        }]
    );
}

#[test]
fn paging_wraps_in_both_directions() {
    let roster = PickerRoster::new(11, Some(0));
    let mut picker = opened(roster);
    let events: Vec<_> = picker
        .update(ms(2_100), &input(&[PAGE_PREVIOUS]), roster)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Selection {
            selected: 0,
            page: 1,
            roster_revision: 0,
        }]
    );
    picker.update(ms(2_200), &input(&[]), roster);
    let events: Vec<_> = picker
        .update(ms(2_300), &input(&[PAGE_NEXT]), roster)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Selection {
            selected: 0,
            page: 0,
            roster_revision: 0,
        }]
    );
}

#[test]
fn a_single_page_roster_ignores_the_shoulders() {
    let mut picker = opened(ROSTER);
    assert!(picker
        .update(ms(2_100), &input(&[PAGE_NEXT]), ROSTER)
        .is_empty());
    assert!(picker
        .update(ms(2_200), &input(&[PAGE_PREVIOUS]), ROSTER)
        .is_empty());
}

#[test]
fn a_roster_that_shrinks_while_open_clamps_the_selection() {
    let roster = PickerRoster::new(11, Some(9));
    let mut picker = opened(roster);
    assert!(picker.is_open());
    // Opened on page 1, sector 1. Losing the last page must pull the
    // selection back onto a page that still exists.
    let smaller = PickerRoster::new(3, Some(0));
    let events: Vec<_> = picker
        .update(ms(2_100), &input(&[]), smaller)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Selection {
            selected: 1,
            page: 0,
            roster_revision: 0,
        }]
    );
    let events: Vec<_> = picker
        .update(ms(2_200), &input(&[COMMIT]), smaller)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Commit {
            index: 1,
            roster_revision: 0,
        }]
    );
}

#[test]
fn a_roster_that_drops_below_two_while_open_dismisses_the_wheel() {
    let mut picker = opened(ROSTER);
    let events: Vec<_> = picker
        .update(ms(2_100), &input(&[]), PickerRoster::new(1, Some(0)))
        .into_iter()
        .collect();
    assert_eq!(events, vec![PickerEvent::Dismissed]);
    assert!(!picker.is_open());
    assert!(picker.suppression().is_none());
}

#[test]
fn steering_and_committing_in_one_report_reports_both_in_order() {
    let mut picker = opened(ROSTER);
    let events: Vec<_> = picker
        .update(
            ms(2_100),
            &PickerInput {
                buttons: buttons(&[COMMIT]),
                left_stick: stick(0.0, -1.0),
                ..PickerInput::default()
            },
            ROSTER,
        )
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![
            PickerEvent::Selection {
                selected: 2,
                page: 0,
                roster_revision: 0,
            },
            PickerEvent::Commit {
                index: 2,
                roster_revision: 0,
            },
        ]
    );
}

#[test]
fn nonsensical_configuration_is_clamped_into_something_usable() {
    let config = PickerConfig {
        hold: MAX_HOLD + Duration::from_secs(1),
        engage_dead_zone: f32::NAN,
        track_dead_zone: 9.0,
        sectors_per_page: 0,
    }
    .sanitized();
    assert_eq!(config.hold, MAX_HOLD);
    assert_eq!(config.sectors_per_page, MIN_SECTORS_PER_PAGE);
    assert_eq!(config.engage_dead_zone, DEFAULT_ENGAGE_DEAD_ZONE);
    assert!(config.track_dead_zone <= config.engage_dead_zone);
    assert!(config.track_dead_zone > 0.0);

    let config = PickerConfig {
        hold: Duration::from_millis(1),
        ..PickerConfig::default()
    }
    .sanitized();
    assert_eq!(config.hold, MIN_HOLD);
}

#[test]
fn replacing_the_configuration_closes_the_wheel() {
    let mut picker = opened(ROSTER);
    picker.set_config(PickerConfig {
        hold: Duration::from_secs(3),
        ..PickerConfig::default()
    });
    assert!(!picker.is_open());
    assert_eq!(picker.config().hold, Duration::from_secs(3));

    // The trigger was still physically down when the configuration change
    // closed the wheel, so it stays withheld — from the game and from the
    // bindings engine alike — until the user lets go. Reports keep arriving
    // here, unlike a forced close, so the latch can drain normally.
    let Some(OutputSuppression::Buttons(withheld)) = picker.suppression() else {
        panic!("a still-held trigger must stay withheld across a config change");
    };
    assert!(withheld.contains(Button::Extra3));
    assert_eq!(picker.mask_trigger(buttons(&[TRIGGER])), SteamButtons(0));

    picker.update(ms(3_000), &input(&[]), ROSTER);
    assert!(picker.suppression().is_none());
    assert_eq!(
        picker.mask_trigger(buttons(&[TRIGGER])),
        buttons(&[TRIGGER])
    );
}

#[test]
fn a_config_change_mid_hold_swallows_the_withheld_press() {
    // Halfway through a hold the trigger has been masked from the bindings
    // engine the whole time. The configuration change abandons the hold
    // without an event, so the press must stay swallowed until release —
    // unmasking it here would hand the engine a fresh down edge instead.
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), ROSTER);
    picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
    assert!(picker.is_arming());

    picker.set_config(PickerConfig {
        hold: Duration::from_secs(3),
        ..PickerConfig::default()
    });
    assert!(!picker.is_arming());
    assert_eq!(
        picker.mask_trigger(buttons(&[TRIGGER])),
        SteamButtons(0),
        "the still-held press must not become a fresh edge"
    );

    // Releasing drains the latch; the next deliberate press is the host's.
    assert!(picker.update(ms(1_000), &input(&[]), ROSTER).is_empty());
    assert_eq!(
        picker.mask_trigger(buttons(&[TRIGGER])),
        buttons(&[TRIGGER])
    );
}

#[test]
fn dismissing_with_the_trigger_keeps_its_binding_masked_until_release() {
    // Regression: the second Quick Access press closes the wheel on its
    // down edge, which returns the picker to Idle on that same report. The
    // trigger must stay hidden from the bindings engine while latched, or
    // cancelling the wheel fires the user's Quick Access binding.
    let mut picker = opened(ROSTER);
    picker.update(ms(2_100), &input(&[]), ROSTER);
    let events: Vec<_> = picker
        .update(ms(2_200), &input(&[TRIGGER]), ROSTER)
        .into_iter()
        .collect();
    assert_eq!(events, vec![PickerEvent::Dismissed]);
    assert!(!picker.owns_trigger());
    assert_eq!(picker.mask_trigger(buttons(&[TRIGGER])), SteamButtons(0));

    // Held for a few more reports: still masked.
    picker.update(ms(2_250), &input(&[TRIGGER]), ROSTER);
    assert_eq!(picker.mask_trigger(buttons(&[TRIGGER])), SteamButtons(0));

    // Released and pressed again deliberately: the binding is back.
    picker.update(ms(2_300), &input(&[]), ROSTER);
    assert_eq!(
        picker.mask_trigger(buttons(&[TRIGGER])),
        buttons(&[TRIGGER])
    );
}

#[test]
fn a_resting_thumb_inside_the_hysteresis_band_cannot_steal_the_wheel() {
    // Regression: with one shared steering flag, a stick that never crossed
    // the engage dead zone could take over the moment it was pushed a hair
    // further than the stick that did — flipping the selection to the
    // opposite side of the wheel because of a resting thumb.
    let mut picker = opened(ROSTER);
    picker.update(
        ms(2_100),
        &PickerInput {
            left_stick: stick(0.0, 1.0),
            ..PickerInput::default()
        },
        ROSTER,
    );

    // The left stick relaxes into the hysteresis band; the right thumb
    // rests slightly further out but never crossed engage. The selection
    // must stay with the left stick.
    assert!(
        picker
            .update(
                ms(2_200),
                &PickerInput {
                    left_stick: stick(0.0, 0.40),
                    right_stick: stick(0.0, -0.45),
                    ..PickerInput::default()
                },
                ROSTER,
            )
            .is_empty(),
        "a stick that never engaged must not steer"
    );

    // Slammed past engage, the other stick does take over.
    let events: Vec<_> = picker
        .update(
            ms(2_300),
            &PickerInput {
                left_stick: stick(0.0, 0.40),
                right_stick: stick(0.0, -1.0),
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
fn a_stall_that_jumps_past_the_hold_still_opens_the_wheel() {
    // A host that stalls between reports can present a `now` that is past
    // the full hold without the halfway warning ever having fired. The
    // wheel must open regardless; `Preparing` is an optimization, not a
    // precondition.
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), ROSTER);
    picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
    let events: Vec<_> = picker
        .update(ms(10_000), &input(&[TRIGGER]), ROSTER)
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
fn a_roster_with_no_active_profile_opens_on_the_first_sector() {
    let roster = PickerRoster::new(4, None);
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), roster);
    picker.update(ms(10), &input(&[TRIGGER]), roster);
    let events: Vec<_> = picker
        .update(ms(2_010), &input(&[TRIGGER]), roster)
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
fn an_out_of_range_active_profile_clamps_to_the_last_sector() {
    // The roster and the active index come from the host over a channel,
    // so a stale pair must degrade to a sane selection, not a panic.
    let roster = PickerRoster::new(4, Some(99));
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), roster);
    picker.update(ms(10), &input(&[TRIGGER]), roster);
    let events: Vec<_> = picker
        .update(ms(2_010), &input(&[TRIGGER]), roster)
        .into_iter()
        .collect();
    assert_eq!(
        events,
        vec![PickerEvent::Opened {
            selected: 3,
            page: 0,
            roster_revision: 0,
        }]
    );
}

#[test]
fn a_shoulder_held_across_a_commit_is_withheld_until_released() {
    let roster = PickerRoster::new(11, Some(0));
    let mut picker = opened(roster);
    picker.update(ms(2_100), &input(&[]), roster);
    // Page with L1 still held while committing with A.
    picker.update(ms(2_200), &input(&[PAGE_NEXT]), roster);
    picker.update(ms(2_300), &input(&[PAGE_NEXT, COMMIT]), roster);
    assert!(!picker.is_open());
    let Some(OutputSuppression::Buttons(withheld)) = picker.suppression() else {
        panic!("held consumed controls must stay withheld after the close");
    };
    assert!(withheld.contains(Button::RightShoulder));
    assert!(withheld.contains(Button::South));
}

#[test]
fn latched_controls_are_released_one_at_a_time() {
    // Dismiss with a second trigger press while A is also held: both are
    // consumed controls, and each must come back to the game individually
    // as the user lets go of it.
    let mut picker = opened(ROSTER);
    picker.update(ms(2_100), &input(&[]), ROSTER);
    picker.update(ms(2_200), &input(&[COMMIT, TRIGGER]), ROSTER);
    assert!(!picker.is_open());
    let Some(OutputSuppression::Buttons(withheld)) = picker.suppression() else {
        panic!("both held controls must be latched");
    };
    assert!(withheld.contains(Button::South));
    assert!(withheld.contains(Button::Extra3));

    // Let go of A first: only the trigger stays withheld.
    picker.update(ms(2_300), &input(&[TRIGGER]), ROSTER);
    let Some(OutputSuppression::Buttons(withheld)) = picker.suppression() else {
        panic!("the still-held trigger must stay latched");
    };
    assert!(!withheld.contains(Button::South));
    assert!(withheld.contains(Button::Extra3));

    picker.update(ms(2_400), &input(&[]), ROSTER);
    assert!(picker.suppression().is_none());
}

#[test]
fn inverted_dead_zones_clamp_track_below_engage() {
    let config = PickerConfig {
        engage_dead_zone: 0.2,
        track_dead_zone: 0.5,
        ..PickerConfig::default()
    }
    .sanitized();
    assert_eq!(config.engage_dead_zone, 0.2);
    assert_eq!(config.track_dead_zone, 0.2);
}

#[test]
fn geometry_backstops_hold_for_out_of_range_input() {
    // Callers subtract one from `sectors_on_page`, so a page past the end
    // must report one sector, never zero.
    assert_eq!(sectors_on_page(3, 8, 9), 1);
    assert_eq!(page_count(0, 8), 1);
    assert_eq!(page_count(8, 0), 1);
}

#[test]
fn a_clock_that_does_not_advance_never_opens_the_wheel() {
    let mut picker = Picker::new(PickerConfig::default());
    picker.update(ms(0), &input(&[]), ROSTER);
    picker.update(ms(10), &input(&[TRIGGER]), ROSTER);
    // saturating_sub keeps a non-monotonic clock from wrapping into a hold.
    for _ in 0..10 {
        assert!(picker.update(ms(5), &input(&[TRIGGER]), ROSTER).is_empty());
    }
    assert!(!picker.is_open());
}
