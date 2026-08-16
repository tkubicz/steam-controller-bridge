use std::collections::BTreeMap;
use std::time::Duration;

use steam_controller_protocol::SteamButtons;

use crate::model::{
    BindableControl, BindingAction, BindingProfile, DesktopInputSnapshot, Modifier, MouseButton,
    PadConfig, PadFeedbackConfig, PadFeedbackRequest, PadFeedbackStrength, PadMotionMode,
    PadSample, PadSide, PadTrigger, FEEDBACK_DISPLACEMENT_COUNTS, FEEDBACK_FAST_INTERVAL,
    FEEDBACK_SLOW_INTERVAL, MOMENTUM_FRAME_MAX_SECONDS, MOTION_DEFAULT_SECONDS, MOTION_MIN_SECONDS,
    MOTION_SPEED_FULL_COUNTS_PER_SECOND, MOTION_SPEED_MAX_SECONDS,
    MOTION_SPEED_START_COUNTS_PER_SECOND, MOUSE_COUNTS_PER_PIXEL, MOUSE_EDGE_DEADZONE_COUNTS,
    MOUSE_EDGE_STOP_PROGRESS_COUNTS, MOUSE_MOTION_DEADZONE_COUNTS, MOUSE_STOP_PROGRESS_COUNTS,
    MOUSE_STOP_WINDOW, PAD_DRAG_THRESHOLD_COUNTS, PAD_EDGE_DEADZONE_COUNTS,
    PAD_EDGE_DEADZONE_START_COUNTS, PAD_EDGE_STOP_PROGRESS_COUNTS, PAD_MAX_DELTA_COUNTS,
    PAD_MOTION_DEADZONE_COUNTS, PAD_PRESSURE_FREEZE_ENTER, PAD_PRESSURE_FREEZE_EXIT,
    PAD_RELEASE_GUARD, PAD_STOP_PROGRESS_COUNTS, PAD_STOP_WINDOW, SCROLL_COUNTS_PER_PIXEL,
    SCROLL_MAX_ACCELERATION, SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND,
    SCROLL_MOMENTUM_DECAY_PER_SECOND, SCROLL_MOMENTUM_STOP_PIXELS_PER_SECOND,
    SCROLL_VELOCITY_BLEND,
};
use crate::region::resolve_region;
use crate::sink::{DesktopInputSink, OutputKey};

/// The pad motion filter's states. Raw captures show the reported centroid
/// wandering by hundreds or thousands of counts as a pressed fingertip
/// flattens and rolls, so a press enters a frozen state that only deliberate
/// held travel can escape.
#[derive(Debug)]
enum MotionFilter {
    /// Unpressed rest: displacement from the anchor accumulates and bounded
    /// wander inside the noise radius emits nothing; escaping forwards only
    /// the radial excess.
    Parked { deadzone_x: i32, deadzone_y: i32 },
    /// Unpressed motion: deltas pass through while each stop-window shows net
    /// radial progress; an elapsed window without it re-parks.
    Moving {
        window_start: Duration,
        window_x: i32,
        window_y: i32,
    },
    /// Clicked: frozen. Displacement from the press point accumulates and only
    /// crossing the drag threshold escapes into `Dragging`.
    PressHeld { offset_x: i32, offset_y: i32 },
    /// An intentional drag that paused. It uses the smaller position-aware
    /// stop-progress envelope to resume instead of the full contact or drag
    /// threshold.
    DragParked { deadzone_x: i32, deadzone_y: i32 },
    /// Clicked and deliberately dragging: deltas pass through until a stalled
    /// window parks the drag without forgetting its drag latch.
    Dragging {
        window_start: Duration,
        window_x: i32,
        window_y: i32,
    },
}

#[derive(Debug)]
struct PadMotion {
    raw_x: i32,
    raw_y: i32,
    filtered: Option<(i32, i32)>,
    speed: f64,
}

#[derive(Debug, Clone, Copy)]
struct MotionThresholds {
    deadzone_counts: i32,
    edge_deadzone_counts: i32,
    stop_progress_counts: i32,
    edge_stop_progress_counts: i32,
    stop_window: Duration,
}

impl MotionThresholds {
    const fn drag_resume(self) -> Self {
        Self {
            deadzone_counts: self.stop_progress_counts,
            edge_deadzone_counts: self.edge_stop_progress_counts,
            ..self
        }
    }
}

struct StopWindow<'a> {
    start: &'a mut Duration,
    x: &'a mut i32,
    y: &'a mut i32,
}

const POINTER_THRESHOLDS: MotionThresholds = MotionThresholds {
    deadzone_counts: MOUSE_MOTION_DEADZONE_COUNTS,
    edge_deadzone_counts: MOUSE_EDGE_DEADZONE_COUNTS,
    stop_progress_counts: MOUSE_STOP_PROGRESS_COUNTS,
    edge_stop_progress_counts: MOUSE_EDGE_STOP_PROGRESS_COUNTS,
    stop_window: MOUSE_STOP_WINDOW,
};

const SCROLL_THRESHOLDS: MotionThresholds = MotionThresholds {
    deadzone_counts: PAD_MOTION_DEADZONE_COUNTS,
    edge_deadzone_counts: PAD_EDGE_DEADZONE_COUNTS,
    stop_progress_counts: PAD_STOP_PROGRESS_COUNTS,
    edge_stop_progress_counts: PAD_EDGE_STOP_PROGRESS_COUNTS,
    stop_window: PAD_STOP_WINDOW,
};

impl Default for MotionFilter {
    fn default() -> Self {
        Self::Parked {
            deadzone_x: 0,
            deadzone_y: 0,
        }
    }
}

/// A change to what one trigger holds down. Absent from [`PadEvents`] means the
/// trigger is unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PadLatch {
    /// Release anything the trigger holds, then hold this action.
    Hold(BindingAction),
    /// Release whatever the trigger holds.
    Clear,
}

/// Region-trigger transitions produced by one report, for the engine to turn
/// into reference-counted desktop output.
///
/// Gestures are not implemented; when they are, they become further fields here
/// and further arms in [`BindingEngine::dispatch_pad_events`], rather than a
/// second dispatch path alongside this one.
#[derive(Debug, Default, Clone)]
struct PadEvents {
    click: Option<PadLatch>,
    touch: Option<PadLatch>,
}

/// The physical click-bit edges observed in one report, reported separately from
/// motion so region actions still fire when the pad drives no motion at all.
#[derive(Debug, Default, Clone, Copy)]
struct PadClickEdges {
    pressed: bool,
    released: bool,
}

// Five genuinely independent latches: contact, safety block, effective press,
// physical click, and post-click pressure suppression. Packing them into enums
// would obscure that each is set and cleared on its own schedule.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default)]
struct PadMotionState {
    previous: Option<(i16, i16)>,
    touched: bool,
    blocked: bool,
    /// Effective press latch: the click bit OR pressure past the freeze
    /// threshold. Drives the motion freeze.
    held: bool,
    /// Physical click-bit latch. Distinguishes press, hold, and release edges.
    clicked: bool,
    /// After a physical release, do not let residual analog pressure start a
    /// second freeze before it first drops below the hysteresis exit point.
    pressure_blocked: bool,
    guard_until: Option<Duration>,
    filter: MotionFilter,
    x_residual: i32,
    y_residual: i32,
    feedback_x: i32,
    feedback_y: i32,
    last_feedback: Option<Duration>,
    last_motion: Option<Duration>,
    scroll_fraction_x: f64,
    scroll_fraction_y: f64,
    scroll_velocity_x: f64,
    scroll_velocity_y: f64,
    scroll_last_update: Option<Duration>,
    /// Coordinates at the effective-press edge. Pressure crosses its freeze
    /// threshold tens of milliseconds before the click bit, so anchoring here
    /// resolves the click's region before most of the fingertip roll.
    press_anchor: Option<(i16, i16)>,
    /// Region whose click action is held, latched at the press edge so sliding
    /// during a held click cannot swap the action.
    click_region: Option<usize>,
    /// Region the finger currently occupies. Tracked whether or not that region
    /// binds anything, so boundary hysteresis works at every seam.
    touch_region: Option<usize>,
    last_touch_feedback: Option<Duration>,
}

impl PadMotionState {
    // The click latch and guard window survive `reset_motion_tracking` on purpose:
    // the guard rebaselines every report, which calls into the reset path,
    // and clearing the guard there would end it after a single report.
    fn reset_motion_tracking(&mut self) {
        self.previous = None;
        self.filter = MotionFilter::default();
        self.x_residual = 0;
        self.y_residual = 0;
        self.feedback_x = 0;
        self.feedback_y = 0;
        self.last_feedback = None;
        self.last_motion = None;
    }

    fn reset_motion(&mut self) {
        self.reset_motion_tracking();
        clear_scroll_momentum(self);
    }

    // Region latches are deliberately untouched by `reset_motion_tracking`: the
    // release guard rebaselines every report through that path, and a held
    // click must survive it.
    fn clear_regions(&mut self) {
        self.press_anchor = None;
        self.click_region = None;
        self.touch_region = None;
        self.last_touch_feedback = None;
    }

    fn block_if_touched(&mut self) {
        self.blocked = self.touched;
        self.reset_motion();
        self.clear_regions();
    }

    fn end_contact(&mut self, sample: PadSample) {
        self.touched = false;
        self.blocked = false;
        self.held = false;
        self.clicked = sample.pressed;
        self.pressure_blocked = false;
        self.guard_until = None;
        self.clear_regions();
    }
}

/// The desktop actions one pad currently holds down. At most one per trigger,
/// which is the truth of the hardware: a pad has one finger and one switch.
#[derive(Debug, Default)]
struct PadLatchState {
    click: Option<BindingAction>,
    touch: Option<BindingAction>,
}

impl PadLatchState {
    fn get_mut(&mut self, trigger: PadTrigger) -> &mut Option<BindingAction> {
        match trigger {
            PadTrigger::Click => &mut self.click,
            PadTrigger::Touch => &mut self.touch,
        }
    }
}

pub struct BindingEngine {
    profile: BindingProfile,
    previous_mask: Option<u8>,
    blocked_mask: u8,
    active: BTreeMap<BindableControl, BindingAction>,
    key_counts: BTreeMap<OutputKey, u16>,
    mouse_counts: BTreeMap<MouseButton, u16>,
    left_pad: PadMotionState,
    right_pad: PadMotionState,
    left_latch: PadLatchState,
    right_latch: PadLatchState,
}

impl BindingEngine {
    #[must_use]
    pub fn new(profile: BindingProfile) -> Self {
        Self {
            profile,
            previous_mask: None,
            blocked_mask: 0,
            active: BTreeMap::new(),
            key_counts: BTreeMap::new(),
            mouse_counts: BTreeMap::new(),
            left_pad: PadMotionState::default(),
            right_pad: PadMotionState::default(),
            left_latch: PadLatchState::default(),
            right_latch: PadLatchState::default(),
        }
    }

    #[must_use]
    pub const fn profile(&self) -> &BindingProfile {
        &self.profile
    }

    #[must_use]
    pub fn held_output_count(&self) -> usize {
        self.key_counts.len() + self.mouse_counts.len()
    }

    /// Reports whether time-based output must continue without a new snapshot.
    ///
    /// Callers may sleep indefinitely while this is false because button and
    /// direct pad output are entirely snapshot-driven. It becomes true only
    /// while released left-pad scroll momentum still needs periodic ticks.
    #[must_use]
    pub fn needs_tick(&self) -> bool {
        self.previous_mask.is_some()
            && PadSide::ALL
                .into_iter()
                .any(|side| pad_momentum_pending(self.pad_state(side), self.profile.pads.get(side)))
    }

    const fn pad_state(&self, side: PadSide) -> &PadMotionState {
        match side {
            PadSide::Left => &self.left_pad,
            PadSide::Right => &self.right_pad,
        }
    }

    /// Observes a button-only snapshot and emits its binding edges.
    ///
    /// The first snapshot is a non-emitting baseline. Any sink error triggers a
    /// best-effort release and blocks held controls until they are released.
    ///
    /// # Errors
    /// Returns the first desktop-input injection failure.
    pub fn observe(
        &mut self,
        buttons: SteamButtons,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        self.observe_snapshot(
            DesktopInputSnapshot::buttons_only(buttons),
            Duration::ZERO,
            sink,
        )
        .map(|_| ())
    }

    /// Observes buttons and pads, emitting desktop actions and returning any
    /// finite pad-feedback ticks requested by movement or physical clicks.
    ///
    /// The first snapshot is a non-emitting baseline. Any sink error releases
    /// held outputs and blocks controls and pads until their physical release.
    ///
    /// # Errors
    /// Returns the first desktop-input injection failure.
    pub fn observe_snapshot(
        &mut self,
        snapshot: DesktopInputSnapshot,
        now: Duration,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<PadFeedbackRequest, String> {
        let mask = bindable_mask(snapshot.buttons);
        let Some(previous) = self.previous_mask else {
            self.previous_mask = Some(mask);
            self.blocked_mask = mask;
            baseline_pad(&mut self.left_pad, snapshot.left_pad);
            baseline_pad(&mut self.right_pad, snapshot.right_pad);
            return Ok(PadFeedbackRequest::NONE);
        };
        self.blocked_mask &= mask;
        let changed = previous ^ mask;
        let result = self
            .apply_changes(changed, mask, sink)
            .and_then(|()| self.process_pads(snapshot, now, sink));
        self.previous_mask = Some(mask);
        match result {
            Ok(feedback) => Ok(feedback),
            Err(error) => {
                let _ = self.release_all(sink);
                self.blocked_mask = mask;
                self.left_pad.block_if_touched();
                self.right_pad.block_if_touched();
                Err(error)
            }
        }
    }

    /// Runs both pads' motion and region processing, emitting their desktop
    /// output and collecting any feedback ticks they earned.
    fn process_pads(
        &mut self,
        snapshot: DesktopInputSnapshot,
        now: Duration,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<PadFeedbackRequest, String> {
        let (left, left_events) = process_pad(
            &mut self.left_pad,
            snapshot.left_pad,
            &self.profile.pads.left,
            now,
            sink,
        )?;
        self.dispatch_pad_events(PadSide::Left, left_events, sink)?;
        let (right, right_events) = process_pad(
            &mut self.right_pad,
            snapshot.right_pad,
            &self.profile.pads.right,
            now,
            sink,
        )?;
        self.dispatch_pad_events(PadSide::Right, right_events, sink)?;
        Ok(PadFeedbackRequest { left, right })
    }

    /// Turns one report's region transitions into reference-counted output,
    /// releasing before pressing so a hand-off between two regions never holds
    /// both actions at once.
    fn dispatch_pad_events(
        &mut self,
        side: PadSide,
        events: PadEvents,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        for (trigger, latch) in [
            (PadTrigger::Click, events.click),
            (PadTrigger::Touch, events.touch),
        ] {
            let Some(latch) = latch else {
                continue;
            };
            if let Some(action) = self.pad_latch(side).get_mut(trigger).take() {
                self.release_action(&action, sink)?;
            }
            if let PadLatch::Hold(action) = latch {
                self.press_action(&action, sink)?;
                *self.pad_latch(side).get_mut(trigger) = Some(action);
            }
        }
        Ok(())
    }

    fn pad_latch(&mut self, side: PadSide) -> &mut PadLatchState {
        match side {
            PadSide::Left => &mut self.left_latch,
            PadSide::Right => &mut self.right_latch,
        }
    }

    /// Drops every pad region latch without emitting releases. Only safe beside
    /// a `release_all`, which unwinds the reference counts those latches hold.
    fn forget_pad_latches(&mut self) {
        self.left_latch = PadLatchState::default();
        self.right_latch = PadLatchState::default();
    }

    /// Advances time-based desktop output such as left-pad scroll momentum.
    ///
    /// This is intentionally independent of controller reports so inertia can
    /// finish even when the HID transport becomes quiet after touch release.
    ///
    /// # Errors
    /// Returns a desktop-input injection failure after clearing pending motion.
    pub fn tick(&mut self, now: Duration, sink: &mut dyn DesktopInputSink) -> Result<(), String> {
        if !self.needs_tick() {
            return Ok(());
        }
        for side in PadSide::ALL {
            if !pad_momentum_pending(self.pad_state(side), self.profile.pads.get(side)) {
                continue;
            }
            let state = match side {
                PadSide::Left => &mut self.left_pad,
                PadSide::Right => &mut self.right_pad,
            };
            if let Err(error) = advance_scroll_momentum(state, now, sink) {
                let _ = self.release_all(sink);
                // A pad running momentum is by definition untouched, so this
                // clears its motion without blocking it; a pad the user is
                // holding stays inert until they let go.
                self.left_pad.block_if_touched();
                self.right_pad.block_if_touched();
                return Err(error);
            }
        }
        Ok(())
    }

    /// Releases the old profile and installs a replacement without synthesizing
    /// presses for controls already held.
    ///
    /// # Errors
    /// Returns an error if releasing an old desktop input fails.
    pub fn replace_profile(
        &mut self,
        profile: BindingProfile,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        if self.profile.id.eq_ignore_ascii_case(&profile.id)
            && self.profile.bindings == profile.bindings
            && self.profile.pads == profile.pads
        {
            self.profile = profile;
            return Ok(());
        }
        let held = self.previous_mask.unwrap_or_default();
        let release = self.release_all(sink);
        self.profile = profile;
        self.blocked_mask = held;
        self.left_pad.block_if_touched();
        self.right_pad.block_if_touched();
        release
    }

    /// Releases all outputs and forgets the source baseline.
    ///
    /// # Errors
    /// Returns the first failed release after attempting every held output.
    pub fn disconnect(&mut self, sink: &mut dyn DesktopInputSink) -> Result<(), String> {
        let result = self.release_all(sink);
        self.previous_mask = None;
        self.blocked_mask = 0;
        self.left_pad = PadMotionState::default();
        self.right_pad = PadMotionState::default();
        result
    }

    /// Reports whether a pad currently holds a region action down.
    #[must_use]
    pub fn held_pad_action_count(&self) -> usize {
        [&self.left_latch, &self.right_latch]
            .into_iter()
            .map(|latch| usize::from(latch.click.is_some()) + usize::from(latch.touch.is_some()))
            .sum()
    }

    fn apply_changes(
        &mut self,
        changed: u8,
        current: u8,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        for control in BindableControl::ALL {
            if changed & control.mask() != 0 && current & control.mask() == 0 {
                if let Some(action) = self.active.remove(&control) {
                    self.release_action(&action, sink)?;
                }
            }
        }
        for control in BindableControl::ALL {
            if changed & control.mask() == 0
                || current & control.mask() == 0
                || self.blocked_mask & control.mask() != 0
            {
                continue;
            }
            if let Some(action) = self.profile.bindings.get(control).cloned() {
                self.press_action(&action, sink)?;
                self.active.insert(control, action);
            }
        }
        Ok(())
    }

    fn press_action(
        &mut self,
        action: &BindingAction,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        match action {
            BindingAction::KeyChord { key, modifiers } => {
                for modifier in Modifier::ALL {
                    if modifiers.contains(&modifier) {
                        self.press_key(OutputKey::Modifier(modifier), sink)?;
                    }
                }
                self.press_key(OutputKey::Key(*key), sink)
            }
            BindingAction::MouseButton { button } => self.press_mouse(*button, sink),
        }
    }

    fn release_action(
        &mut self,
        action: &BindingAction,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        match action {
            BindingAction::KeyChord { key, modifiers } => {
                self.release_key(OutputKey::Key(*key), sink)?;
                for modifier in Modifier::ALL.into_iter().rev() {
                    if modifiers.contains(&modifier) {
                        self.release_key(OutputKey::Modifier(modifier), sink)?;
                    }
                }
                Ok(())
            }
            BindingAction::MouseButton { button } => self.release_mouse(*button, sink),
        }
    }

    fn press_key(
        &mut self,
        output: OutputKey,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        let count = self.key_counts.entry(output).or_default();
        if *count == 0 {
            emit_key(sink, output, true)?;
        }
        *count = count.saturating_add(1);
        Ok(())
    }

    fn release_key(
        &mut self,
        output: OutputKey,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        let Some(count) = self.key_counts.get_mut(&output) else {
            return Ok(());
        };
        *count -= 1;
        if *count == 0 {
            emit_key(sink, output, false)?;
            self.key_counts.remove(&output);
        }
        Ok(())
    }

    fn press_mouse(
        &mut self,
        button: MouseButton,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        let count = self.mouse_counts.entry(button).or_default();
        if *count == 0 {
            sink.mouse_button(button, true)?;
        }
        *count = count.saturating_add(1);
        Ok(())
    }

    fn release_mouse(
        &mut self,
        button: MouseButton,
        sink: &mut dyn DesktopInputSink,
    ) -> Result<(), String> {
        let Some(count) = self.mouse_counts.get_mut(&button) else {
            return Ok(());
        };
        *count -= 1;
        if *count == 0 {
            sink.mouse_button(button, false)?;
            self.mouse_counts.remove(&button);
        }
        Ok(())
    }

    fn release_all(&mut self, sink: &mut dyn DesktopInputSink) -> Result<(), String> {
        let mut first_error = None;
        for (button, _) in std::mem::take(&mut self.mouse_counts) {
            if let Err(error) = sink.mouse_button(button, false) {
                first_error.get_or_insert(error);
            }
        }
        let keys = std::mem::take(&mut self.key_counts);
        for (key, _) in keys.iter().rev() {
            if let Err(error) = emit_key(sink, *key, false) {
                first_error.get_or_insert(error);
            }
        }
        self.active.clear();
        self.forget_pad_latches();
        first_error.map_or(Ok(()), Err)
    }
}

/// Reports whether released scroll momentum on this pad still owes ticks.
fn pad_momentum_pending(state: &PadMotionState, config: &PadConfig) -> bool {
    !state.touched
        && !state.blocked
        && config.motion == PadMotionMode::Scroll
        && config.momentum
        && state.scroll_last_update.is_some()
}

fn baseline_pad(state: &mut PadMotionState, sample: PadSample) {
    state.touched = sample.touched;
    state.blocked = sample.touched;
    state.held = effectively_pressed(false, false, sample);
    state.clicked = sample.pressed;
    state.pressure_blocked = false;
    state.guard_until = None;
    state.reset_motion();
    state.clear_regions();
}

/// Effective press: the physical click bit OR the analog pressure crossing
/// the freeze threshold, with hysteresis. Pressure typically leads switch
/// actuation by tens of milliseconds, so the freeze catches the approach roll
/// and light presses that never reach the click bit.
fn effectively_pressed(previously_held: bool, pressure_blocked: bool, sample: PadSample) -> bool {
    sample.pressed
        || (!pressure_blocked
            && sample.pressure
                >= if previously_held {
                    PAD_PRESSURE_FREEZE_EXIT
                } else {
                    PAD_PRESSURE_FREEZE_ENTER
                })
}

/// Latches the pad's press state and applies its filter transitions. Pressure
/// freezes the approach, the physical click edge establishes a fresh drag
/// anchor, and the falling edge immediately guards the un-flattening tail even
/// while analog pressure remains high.
///
/// Returns the physical click edges alongside whether the release guard is
/// active, so region actions can fire even for a pad that drives no motion.
fn apply_click_transitions(
    state: &mut PadMotionState,
    sample: PadSample,
    now: Duration,
) -> (PadClickEdges, bool) {
    if state.pressure_blocked && sample.pressure < PAD_PRESSURE_FREEZE_EXIT {
        state.pressure_blocked = false;
    }
    let was_clicked = state.clicked;
    let pressed_edge = sample.pressed && !was_clicked;
    let released_edge = !sample.pressed && was_clicked;
    state.clicked = sample.pressed;
    let was_held = state.held;
    state.held = effectively_pressed(was_held, state.pressure_blocked, sample);

    if pressed_edge {
        state.pressure_blocked = false;
        state.held = true;
        rebaseline_placement(state, sample);
        state.guard_until = None;
    } else if released_edge {
        state.pressure_blocked = true;
        state.held = false;
        rebaseline_placement(state, sample);
        state.guard_until = Some(now + PAD_RELEASE_GUARD);
    } else if state.held && !was_held {
        rebaseline_placement(state, sample);
        state.guard_until = None;
    } else if !state.held && was_held {
        rebaseline_placement(state, sample);
        state.guard_until = Some(now + PAD_RELEASE_GUARD);
    }
    // The anchor is claimed by whichever press edge arrives first, which is
    // normally the pressure freeze; the later click bit keeps it rather than
    // re-reading a coordinate the fingertip has already rolled away from.
    if state.held {
        state.press_anchor.get_or_insert((sample.x, sample.y));
    } else {
        state.press_anchor = None;
    }
    let guard_active = match state.guard_until {
        Some(until) if now < until => true,
        Some(_) => {
            state.guard_until = None;
            false
        }
        None => false,
    };
    (
        PadClickEdges {
            pressed: pressed_edge,
            released: released_edge,
        },
        guard_active,
    )
}

/// Runs one pad's touch/press/release machine, emitting whichever motion its
/// configured mode calls for and reporting the region transitions the caller
/// must turn into desktop output.
fn process_pad(
    state: &mut PadMotionState,
    sample: PadSample,
    config: &PadConfig,
    now: Duration,
    sink: &mut dyn DesktopInputSink,
) -> Result<(Option<PadFeedbackStrength>, PadEvents), String> {
    let was_touched = state.touched;
    if !sample.touched {
        // Every latched region is released by lifting off, whatever the pad's
        // motion mode is doing.
        let events = PadEvents {
            click: state.click_region.is_some().then_some(PadLatch::Clear),
            touch: state.touch_region.is_some().then_some(PadLatch::Clear),
        };
        state.end_contact(sample);
        match config.motion {
            PadMotionMode::Scroll => {
                if was_touched {
                    state.reset_motion_tracking();
                    if config.momentum && scroll_velocity_pending(state) {
                        state.scroll_last_update = Some(now);
                    } else {
                        clear_scroll_momentum(state);
                    }
                } else if config.momentum {
                    advance_scroll_momentum(state, now, sink)?;
                } else {
                    clear_scroll_momentum(state);
                }
            }
            PadMotionMode::None | PadMotionMode::Pointer => state.reset_motion(),
        }
        return Ok((None, events));
    }

    state.touched = true;
    if state.blocked {
        state.held = effectively_pressed(state.held, state.pressure_blocked, sample);
        state.clicked = sample.pressed;
        state.pressure_blocked = false;
        state.reset_motion();
        return Ok((None, PadEvents::default()));
    }
    if config.motion == PadMotionMode::Scroll && !was_touched {
        clear_scroll_momentum(state);
    }

    let (edges, guard_active) = apply_click_transitions(state, sample, now);
    let events = track_regions(state, sample, config, edges, guard_active);
    let click_feedback =
        (edges.pressed && config.feedback.enabled).then_some(config.feedback.strength);
    let touch_feedback = touch_region_feedback(state, config, &events, now);

    let motion_feedback = if let Some(motion) = pad_motion(state, sample, config, guard_active, now)
    {
        match config.motion {
            PadMotionMode::Pointer => emit_pointer_motion(state, config, &motion, sink)?,
            PadMotionMode::Scroll => emit_scroll_motion(state, config, &motion, now, sink)?,
            // `pad_motion` yields nothing for an unbound pad, so this arm is
            // unreachable rather than a silently dropped behavior.
            PadMotionMode::None => {}
        }
        process_motion_feedback(state, config.feedback, &motion, now)
    } else {
        // Seed velocity timing from contact rather than assuming the default
        // frame interval for the first accepted scroll sample. This timestamp
        // alone cannot schedule momentum: release also requires real velocity.
        if config.motion == PadMotionMode::Scroll && state.scroll_last_update.is_none() {
            state.scroll_last_update = Some(now);
        }
        None
    };

    Ok((
        motion_feedback.or(click_feedback).or(touch_feedback),
        events,
    ))
}

/// Resolves which region the finger and the click belong to.
///
/// Tracking freezes while the pad is effectively pressed or inside the release
/// guard: a flattening or un-flattening fingertip's reported centroid wanders by
/// hundreds to thousands of counts, which would otherwise walk across seams and
/// fire actions the user never aimed at.
fn track_regions(
    state: &mut PadMotionState,
    sample: PadSample,
    config: &PadConfig,
    edges: PadClickEdges,
    guard_active: bool,
) -> PadEvents {
    let mut events = PadEvents::default();
    if edges.released && state.click_region.take().is_some() {
        events.click = Some(PadLatch::Clear);
    }
    if !state.held && !guard_active {
        let region = resolve_region(&config.regions, (sample.x, sample.y), state.touch_region);
        if region != state.touch_region {
            state.touch_region = region;
            events.touch = Some(latch_for(config, region, PadTrigger::Touch));
        }
    }
    if edges.pressed {
        // The press anchor is where pressure first crossed the freeze threshold,
        // which precedes the click bit and therefore most of the roll.
        let anchor = state.press_anchor.unwrap_or((sample.x, sample.y));
        let region = resolve_region(&config.regions, anchor, state.touch_region);
        state.click_region = region;
        events.click = Some(latch_for(config, region, PadTrigger::Click));
    }
    events
}

/// A region index becomes the action it binds for that trigger, or a release
/// when it is unbound or there is no region under the finger at all.
fn latch_for(config: &PadConfig, region: Option<usize>, trigger: PadTrigger) -> PadLatch {
    region
        .and_then(|index| config.regions.get(index))
        .and_then(|region| region.action(trigger))
        .cloned()
        .map_or(PadLatch::Clear, PadLatch::Hold)
}

/// One tick when the finger crosses into a region that binds a touch action.
/// Unlike a click, which the user commits to deliberately, boundary traffic can
/// be dense, so this shares the motion texture's fastest interval as a floor.
fn touch_region_feedback(
    state: &mut PadMotionState,
    config: &PadConfig,
    events: &PadEvents,
    now: Duration,
) -> Option<PadFeedbackStrength> {
    if !config.feedback.enabled || !matches!(events.touch, Some(PadLatch::Hold(_))) {
        return None;
    }
    if state
        .last_touch_feedback
        .is_some_and(|last| now.saturating_sub(last) < FEEDBACK_FAST_INTERVAL)
    {
        return None;
    }
    state.last_touch_feedback = Some(now);
    Some(config.feedback.strength)
}

/// The accepted per-report travel for the pad's mode, or `None` when the mode
/// consumes no motion or the filter is currently swallowing it.
fn pad_motion(
    state: &mut PadMotionState,
    sample: PadSample,
    config: &PadConfig,
    guard_active: bool,
    now: Duration,
) -> Option<PadMotion> {
    let thresholds = match config.motion {
        PadMotionMode::None => {
            state.reset_motion();
            return None;
        }
        PadMotionMode::Pointer => POINTER_THRESHOLDS,
        PadMotionMode::Scroll => SCROLL_THRESHOLDS,
    };
    if guard_active {
        rebaseline_placement(state, sample);
        return None;
    }

    let Some((previous_x, previous_y)) = state.previous.replace((sample.x, sample.y)) else {
        state.last_motion = Some(now);
        return None;
    };
    let raw_x = i32::from(sample.x) - i32::from(previous_x);
    let raw_y = i32::from(sample.y) - i32::from(previous_y);
    if raw_x.abs() > PAD_MAX_DELTA_COUNTS || raw_y.abs() > PAD_MAX_DELTA_COUNTS {
        rebaseline_placement(state, sample);
        return None;
    }

    let speed = update_motion_speed(state, raw_x, raw_y, now);
    let filtered = filter_pad_motion(state, sample, raw_x, raw_y, thresholds, now);
    Some(PadMotion {
        raw_x,
        raw_y,
        filtered,
        speed,
    })
}

fn emit_pointer_motion(
    state: &mut PadMotionState,
    config: &PadConfig,
    motion: &PadMotion,
    sink: &mut dyn DesktopInputSink,
) -> Result<(), String> {
    let Some((delta_x, delta_y)) = motion.filtered else {
        return Ok(());
    };
    let gain = mouse_gain(config.speed_percent);
    state.x_residual += scale_counts(delta_x, gain);
    state.y_residual -= scale_counts(delta_y, gain);
    let pixels_x = take_pixels(&mut state.x_residual, MOUSE_COUNTS_PER_PIXEL);
    let pixels_y = take_pixels(&mut state.y_residual, MOUSE_COUNTS_PER_PIXEL);
    if pixels_x != 0 || pixels_y != 0 {
        sink.mouse_move(pixels_x, pixels_y)?;
    }
    Ok(())
}

fn emit_scroll_motion(
    state: &mut PadMotionState,
    config: &PadConfig,
    motion: &PadMotion,
    now: Duration,
    sink: &mut dyn DesktopInputSink,
) -> Result<(), String> {
    let Some((delta_x, delta_y)) = motion.filtered else {
        if matches!(
            state.filter,
            MotionFilter::Parked { .. } | MotionFilter::DragParked { .. }
        ) {
            clear_scroll_momentum(state);
        }
        return Ok(());
    };
    let acceleration = scroll_acceleration(motion.speed);
    let profile_scale = f64::from(config.speed_percent) / 100.0;
    let scale = profile_scale * acceleration / f64::from(SCROLL_COUNTS_PER_PIXEL);
    let scroll_x = f64::from(delta_x) * scale;
    let scroll_y = -f64::from(delta_y) * scale;
    emit_fractional_scroll(state, scroll_x, scroll_y, sink)?;

    let seconds = motion_seconds(state.scroll_last_update, now);
    state.scroll_last_update = Some(now);
    let instantaneous_x = (scroll_x / seconds).clamp(
        -SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND,
        SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND,
    );
    let instantaneous_y = (scroll_y / seconds).clamp(
        -SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND,
        SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND,
    );
    state.scroll_velocity_x = blend_velocity(state.scroll_velocity_x, instantaneous_x);
    state.scroll_velocity_y = blend_velocity(state.scroll_velocity_y, instantaneous_y);
    Ok(())
}

fn update_motion_speed(
    state: &mut PadMotionState,
    delta_x: i32,
    delta_y: i32,
    now: Duration,
) -> f64 {
    let seconds = motion_seconds(state.last_motion, now);
    state.last_motion = Some(now);
    f64::from(delta_x).hypot(f64::from(delta_y)) / seconds
}

fn motion_seconds(previous: Option<Duration>, now: Duration) -> f64 {
    previous.map_or(MOTION_DEFAULT_SECONDS, |last| {
        now.saturating_sub(last)
            .as_secs_f64()
            .clamp(MOTION_MIN_SECONDS, MOTION_SPEED_MAX_SECONDS)
    })
}

fn normalized_motion_speed(speed: f64) -> f64 {
    ((speed - MOTION_SPEED_START_COUNTS_PER_SECOND)
        / (MOTION_SPEED_FULL_COUNTS_PER_SECOND - MOTION_SPEED_START_COUNTS_PER_SECOND))
        .clamp(0.0, 1.0)
}

fn scroll_acceleration(speed: f64) -> f64 {
    1.0 + normalized_motion_speed(speed) * (SCROLL_MAX_ACCELERATION - 1.0)
}

fn blend_velocity(previous: f64, instantaneous: f64) -> f64 {
    previous * (1.0 - SCROLL_VELOCITY_BLEND) + instantaneous * SCROLL_VELOCITY_BLEND
}

fn emit_fractional_scroll(
    state: &mut PadMotionState,
    x: f64,
    y: f64,
    sink: &mut dyn DesktopInputSink,
) -> Result<(), String> {
    state.scroll_fraction_x += x;
    state.scroll_fraction_y += y;
    let pixels_x = take_fractional_pixels(&mut state.scroll_fraction_x);
    let pixels_y = take_fractional_pixels(&mut state.scroll_fraction_y);
    if pixels_x != 0 || pixels_y != 0 {
        sink.scroll(pixels_x, pixels_y)?;
    }
    Ok(())
}

#[allow(clippy::cast_possible_truncation)]
fn take_fractional_pixels(residual: &mut f64) -> i32 {
    // Direct motion and momentum are bounded far below i32 limits. Truncation
    // deliberately retains the sub-pixel remainder for the next update.
    let whole = residual
        .trunc()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX));
    let pixels = whole as i32;
    *residual -= f64::from(pixels);
    pixels
}

fn advance_scroll_momentum(
    state: &mut PadMotionState,
    now: Duration,
    sink: &mut dyn DesktopInputSink,
) -> Result<(), String> {
    let Some(last) = state.scroll_last_update else {
        return Ok(());
    };
    let seconds = now
        .saturating_sub(last)
        .as_secs_f64()
        .min(MOMENTUM_FRAME_MAX_SECONDS);
    state.scroll_last_update = Some(now);
    if seconds == 0.0 {
        return Ok(());
    }
    emit_fractional_scroll(
        state,
        state.scroll_velocity_x * seconds,
        state.scroll_velocity_y * seconds,
        sink,
    )?;
    let decay = (-SCROLL_MOMENTUM_DECAY_PER_SECOND * seconds).exp();
    state.scroll_velocity_x *= decay;
    state.scroll_velocity_y *= decay;
    if !scroll_velocity_pending(state) {
        clear_scroll_momentum(state);
    }
    Ok(())
}

fn scroll_velocity_pending(state: &PadMotionState) -> bool {
    state.scroll_velocity_x.hypot(state.scroll_velocity_y) >= SCROLL_MOMENTUM_STOP_PIXELS_PER_SECOND
}

fn clear_scroll_momentum(state: &mut PadMotionState) {
    state.scroll_fraction_x = 0.0;
    state.scroll_fraction_y = 0.0;
    state.scroll_velocity_x = 0.0;
    state.scroll_velocity_y = 0.0;
    state.scroll_last_update = None;
}

fn process_feedback(
    state: &mut PadMotionState,
    config: PadFeedbackConfig,
    delta_x: i32,
    delta_y: i32,
    speed: f64,
    now: Duration,
) -> Option<PadFeedbackStrength> {
    if !config.enabled {
        state.feedback_x = 0;
        state.feedback_y = 0;
        return None;
    }
    // Measure displacement from the last consumed texture point, not total
    // path length. Back-and-forth coordinate noise therefore cancels instead
    // of eventually producing feedback while a finger is stationary.
    state.feedback_x += delta_x;
    state.feedback_y += delta_y;
    let feedback_x = i64::from(state.feedback_x);
    let feedback_y = i64::from(state.feedback_y);
    let feedback_threshold = i64::from(FEEDBACK_DISPLACEMENT_COUNTS);
    if feedback_x * feedback_x + feedback_y * feedback_y < feedback_threshold * feedback_threshold {
        return None;
    }

    let speed_factor = normalized_motion_speed(speed);
    let slow_ms = FEEDBACK_SLOW_INTERVAL.as_secs_f64() * 1_000.0;
    let fast_ms = FEEDBACK_FAST_INTERVAL.as_secs_f64() * 1_000.0;
    let interval =
        Duration::from_secs_f64((slow_ms + (fast_ms - slow_ms) * speed_factor) / 1_000.0);
    let interval_ready = state
        .last_feedback
        .is_none_or(|last| now.saturating_sub(last) >= interval);
    // Each threshold crossing is a complete microtick opportunity. When the
    // rate limiter is closed, drop it instead of retaining a delayed backlog.
    state.feedback_x = 0;
    state.feedback_y = 0;
    if interval_ready {
        state.last_feedback = Some(now);
        Some(config.strength)
    } else {
        None
    }
}

fn process_motion_feedback(
    state: &mut PadMotionState,
    config: PadFeedbackConfig,
    motion: &PadMotion,
    now: Duration,
) -> Option<PadFeedbackStrength> {
    // Texture feedback follows accepted pointer/scroll motion. Raw centroid
    // noise while parked or press-frozen must remain silent as well as still.
    if motion.filtered.is_none() {
        state.feedback_x = 0;
        state.feedback_y = 0;
        return None;
    }
    process_feedback(state, config, motion.raw_x, motion.raw_y, motion.speed, now)
}

/// Treats an impossibly large per-report delta as a lift-and-replace: motion,
/// deadzone, feedback, and momentum restart from the new contact point.
fn rebaseline_placement(state: &mut PadMotionState, sample: PadSample) {
    state.reset_motion();
    if state.held {
        state.filter = MotionFilter::PressHeld {
            offset_x: 0,
            offset_y: 0,
        };
    }
    state.previous = Some((sample.x, sample.y));
}

/// Anchored motion filter with hysteresis; see [`MotionFilter`] for the state
/// semantics. Escapes forward only the radial excess beyond the escaped
/// radius (rescaled along the accumulated direction, like the stick dead
/// zones), and the pass-through states re-anchor whenever a stop-window
/// elapses without net radial progress - oscillating press-noise nets out to
/// nothing, so it can never hold the filter open the way it defeated the
/// previous per-crossing progress counter.
fn filter_pad_motion(
    state: &mut PadMotionState,
    sample: PadSample,
    delta_x: i32,
    delta_y: i32,
    thresholds: MotionThresholds,
    now: Duration,
) -> Option<(i32, i32)> {
    match &mut state.filter {
        MotionFilter::Parked {
            deadzone_x,
            deadzone_y,
        } => {
            let escaped =
                escape_parked_motion(deadzone_x, deadzone_y, sample, delta_x, delta_y, thresholds);
            if escaped.is_some() {
                state.filter = MotionFilter::Moving {
                    window_start: now,
                    window_x: 0,
                    window_y: 0,
                };
            }
            escaped
        }
        MotionFilter::PressHeld { offset_x, offset_y } => {
            *offset_x += delta_x;
            *offset_y += delta_y;
            if !state.clicked {
                return None;
            }
            let escaped = escape_excess(*offset_x, *offset_y, PAD_DRAG_THRESHOLD_COUNTS);
            if escaped.is_some() {
                state.filter = MotionFilter::Dragging {
                    window_start: now,
                    window_x: 0,
                    window_y: 0,
                };
            }
            escaped
        }
        MotionFilter::DragParked {
            deadzone_x,
            deadzone_y,
        } => {
            let escaped = escape_parked_motion(
                deadzone_x,
                deadzone_y,
                sample,
                delta_x,
                delta_y,
                thresholds.drag_resume(),
            );
            if escaped.is_some() {
                state.filter = MotionFilter::Dragging {
                    window_start: now,
                    window_x: 0,
                    window_y: 0,
                };
            }
            escaped
        }
        MotionFilter::Moving {
            window_start,
            window_x,
            window_y,
        } => {
            if window_stalled(
                &mut StopWindow {
                    start: window_start,
                    x: window_x,
                    y: window_y,
                },
                (delta_x, delta_y),
                sample,
                thresholds,
                now,
            ) {
                state.filter = MotionFilter::default();
                return None;
            }
            Some((delta_x, delta_y))
        }
        MotionFilter::Dragging {
            window_start,
            window_x,
            window_y,
        } => {
            if window_stalled(
                &mut StopWindow {
                    start: window_start,
                    x: window_x,
                    y: window_y,
                },
                (delta_x, delta_y),
                sample,
                thresholds,
                now,
            ) {
                state.filter = MotionFilter::DragParked {
                    deadzone_x: 0,
                    deadzone_y: 0,
                };
                return None;
            }
            Some((delta_x, delta_y))
        }
    }
}

fn escape_parked_motion(
    deadzone_x: &mut i32,
    deadzone_y: &mut i32,
    sample: PadSample,
    delta_x: i32,
    delta_y: i32,
    thresholds: MotionThresholds,
) -> Option<(i32, i32)> {
    *deadzone_x += delta_x;
    *deadzone_y += delta_y;
    escape_excess(
        *deadzone_x,
        *deadzone_y,
        position_aware_threshold(
            sample,
            thresholds.deadzone_counts,
            thresholds.edge_deadzone_counts,
        ),
    )
}

/// Forwards the radial excess beyond an anchored radius once accumulated
/// displacement escapes it; `None` while still inside.
fn escape_excess(x: i32, y: i32, radius_counts: i32) -> Option<(i32, i32)> {
    let r2 = i64::from(x) * i64::from(x) + i64::from(y) * i64::from(y);
    if r2 <= i64::from(radius_counts).pow(2) {
        return None;
    }
    let radius = f64::from(x).hypot(f64::from(y));
    let scale = (radius - f64::from(radius_counts)) / radius;
    Some((scale_counts(x, scale), scale_counts(y, scale)))
}

/// Reports whether a stop-window elapsed without net radial progress, and
/// refreshes the window when it elapsed with progress.
fn window_stalled(
    window: &mut StopWindow<'_>,
    delta: (i32, i32),
    sample: PadSample,
    thresholds: MotionThresholds,
    now: Duration,
) -> bool {
    *window.x += delta.0;
    *window.y += delta.1;
    if now.saturating_sub(*window.start) < thresholds.stop_window {
        return false;
    }
    let x = i64::from(*window.x);
    let y = i64::from(*window.y);
    let threshold = position_aware_threshold(
        sample,
        thresholds.stop_progress_counts,
        thresholds.edge_stop_progress_counts,
    );
    if x * x + y * y < i64::from(threshold).pow(2) {
        return true;
    }
    *window.start = now;
    *window.x = 0;
    *window.y = 0;
    false
}

/// Capacitive centroid noise grows sharply near the pad rim, especially when
/// one axis is clamped. Grow each center threshold toward its separate edge
/// cap: the parked envelope rejects a fresh contact's wander, while the much
/// smaller stop threshold preserves deliberate slow travel already in motion.
fn position_aware_threshold(sample: PadSample, center_counts: i32, edge_counts: i32) -> i32 {
    let edge = i32::from(sample.x).abs().max(i32::from(sample.y).abs());
    if edge <= PAD_EDGE_DEADZONE_START_COUNTS {
        return center_counts;
    }
    let edge_span = i32::from(i16::MAX) - PAD_EDGE_DEADZONE_START_COUNTS;
    let edge_progress = edge - PAD_EDGE_DEADZONE_START_COUNTS;
    center_counts + (edge_counts - center_counts) * edge_progress / edge_span
}

/// Lizard-mode-compatible linear pointer gain plus the profile speed control.
fn mouse_gain(speed_percent: u16) -> f64 {
    f64::from(speed_percent) / 100.0
}

#[allow(clippy::cast_possible_truncation)]
fn scale_counts(counts: i32, factor: f64) -> i32 {
    // Per-report deltas and configured gain are bounded far below i32 limits.
    (f64::from(counts) * factor).round() as i32
}

fn take_pixels(residual: &mut i32, counts_per_pixel: i32) -> i32 {
    let pixels = *residual / counts_per_pixel;
    *residual -= pixels * counts_per_pixel;
    pixels
}

fn emit_key(
    sink: &mut dyn DesktopInputSink,
    output: OutputKey,
    pressed: bool,
) -> Result<(), String> {
    match output {
        OutputKey::Modifier(modifier) => sink.modifier(modifier, pressed),
        OutputKey::Key(key) => sink.key(key, pressed),
    }
}

#[must_use]
pub fn bindable_mask(buttons: SteamButtons) -> u8 {
    BindableControl::ALL
        .into_iter()
        .fold(0_u8, |mask, control| {
            if buttons.contains(control.steam_button()) {
                mask | control.mask()
            } else {
                mask
            }
        })
}
