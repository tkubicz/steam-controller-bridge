use std::collections::BTreeMap;
use std::time::Duration;

use steam_controller_protocol::SteamButtons;

use crate::model::{
    BindableControl, BindingAction, BindingProfile, DesktopInputSnapshot, Modifier, MouseButton,
    PadFeedbackConfig, PadFeedbackRequest, PadFeedbackStrength, PadFunctionConfig, PadSample,
    ScrollPadConfig, FEEDBACK_DISPLACEMENT_COUNTS, FEEDBACK_FAST_INTERVAL, FEEDBACK_SLOW_INTERVAL,
    MOMENTUM_FRAME_MAX_SECONDS, MOTION_DEFAULT_SECONDS, MOTION_MIN_SECONDS,
    MOTION_SPEED_FULL_COUNTS_PER_SECOND, MOTION_SPEED_MAX_SECONDS,
    MOTION_SPEED_START_COUNTS_PER_SECOND, MOUSE_COUNTS_PER_PIXEL, PAD_MAX_DELTA_COUNTS,
    PAD_MOTION_DEADZONE_COUNTS, SCROLL_COUNTS_PER_PIXEL, SCROLL_MAX_ACCELERATION,
    SCROLL_MAX_MOMENTUM_PIXELS_PER_SECOND, SCROLL_MOMENTUM_DECAY_PER_SECOND,
    SCROLL_MOMENTUM_STOP_PIXELS_PER_SECOND, SCROLL_VELOCITY_BLEND,
};
use crate::sink::{DesktopInputSink, OutputKey};

#[derive(Debug, Default)]
struct PadMotionState {
    previous: Option<(i16, i16)>,
    touched: bool,
    blocked: bool,
    deadzone_x: i32,
    deadzone_y: i32,
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
}

impl PadMotionState {
    fn reset_contact(&mut self) {
        self.previous = None;
        self.deadzone_x = 0;
        self.deadzone_y = 0;
        self.x_residual = 0;
        self.y_residual = 0;
        self.feedback_x = 0;
        self.feedback_y = 0;
        self.last_feedback = None;
        self.last_motion = None;
    }

    fn reset_motion(&mut self) {
        self.reset_contact();
        clear_scroll_momentum(self);
    }

    fn block_if_touched(&mut self) {
        self.blocked = self.touched;
        self.reset_motion();
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
            && !self.left_pad.touched
            && !self.left_pad.blocked
            && self.profile.pads.left_scroll.enabled
            && self.profile.pads.left_scroll.momentum
            && self.left_pad.scroll_last_update.is_some()
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
    /// finite pad-feedback ticks requested by movement.
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
        let pads = self.profile.pads;
        let result = self.apply_changes(changed, mask, sink).and_then(|()| {
            let left = process_scroll_pad(
                &mut self.left_pad,
                snapshot.left_pad,
                pads.left_scroll,
                now,
                sink,
            )?;
            let right = process_mouse_pad(
                &mut self.right_pad,
                snapshot.right_pad,
                pads.right_mouse,
                now,
                sink,
            )?;
            Ok(PadFeedbackRequest { left, right })
        });
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
        if let Err(error) = advance_scroll_momentum(&mut self.left_pad, now, sink) {
            let _ = self.release_all(sink);
            self.left_pad.reset_motion();
            self.right_pad.block_if_touched();
            return Err(error);
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
        first_error.map_or(Ok(()), Err)
    }
}

fn baseline_pad(state: &mut PadMotionState, sample: PadSample) {
    state.touched = sample.touched;
    state.blocked = sample.touched;
    state.reset_motion();
}

fn process_mouse_pad(
    state: &mut PadMotionState,
    sample: PadSample,
    config: PadFunctionConfig,
    now: Duration,
    sink: &mut dyn DesktopInputSink,
) -> Result<Option<PadFeedbackStrength>, String> {
    state.touched = sample.touched;
    if !sample.touched {
        state.blocked = false;
        state.reset_motion();
        return Ok(None);
    }
    if state.blocked || !config.enabled {
        state.reset_motion();
        return Ok(None);
    }

    let Some((previous_x, previous_y)) = state.previous.replace((sample.x, sample.y)) else {
        state.last_motion = Some(now);
        return Ok(None);
    };
    let raw_x = i32::from(sample.x) - i32::from(previous_x);
    let raw_y = i32::from(sample.y) - i32::from(previous_y);
    if raw_x.abs() > PAD_MAX_DELTA_COUNTS || raw_y.abs() > PAD_MAX_DELTA_COUNTS {
        rebaseline_placement(state, sample);
        return Ok(None);
    }

    let Some((delta_x, delta_y)) = accumulate_deadzone_motion(state, raw_x, raw_y) else {
        return Ok(None);
    };

    state.x_residual += delta_x;
    state.y_residual -= delta_y;
    let pixels_x = take_pixels(&mut state.x_residual, MOUSE_COUNTS_PER_PIXEL);
    let pixels_y = take_pixels(&mut state.y_residual, MOUSE_COUNTS_PER_PIXEL);
    if pixels_x != 0 || pixels_y != 0 {
        sink.mouse_move(pixels_x, pixels_y)?;
    }

    let speed = update_motion_speed(state, delta_x, delta_y, now);
    Ok(process_feedback(
        state,
        config.feedback,
        delta_x,
        delta_y,
        speed,
        now,
    ))
}

fn process_scroll_pad(
    state: &mut PadMotionState,
    sample: PadSample,
    config: ScrollPadConfig,
    now: Duration,
    sink: &mut dyn DesktopInputSink,
) -> Result<Option<PadFeedbackStrength>, String> {
    let was_touched = state.touched;
    state.touched = sample.touched;
    if !config.enabled || state.blocked {
        if !sample.touched {
            state.blocked = false;
        }
        state.reset_motion();
        return Ok(None);
    }
    if !sample.touched {
        if was_touched {
            state.reset_contact();
            state.scroll_last_update = Some(now);
            if !config.momentum {
                clear_scroll_momentum(state);
            }
            return Ok(None);
        }
        if config.momentum {
            advance_scroll_momentum(state, now, sink)?;
        } else {
            clear_scroll_momentum(state);
        }
        return Ok(None);
    }

    if !was_touched {
        clear_scroll_momentum(state);
    }
    let Some((previous_x, previous_y)) = state.previous.replace((sample.x, sample.y)) else {
        state.last_motion = Some(now);
        state.scroll_last_update = Some(now);
        return Ok(None);
    };
    let raw_x = i32::from(sample.x) - i32::from(previous_x);
    let raw_y = i32::from(sample.y) - i32::from(previous_y);
    if raw_x.abs() > PAD_MAX_DELTA_COUNTS || raw_y.abs() > PAD_MAX_DELTA_COUNTS {
        rebaseline_placement(state, sample);
        return Ok(None);
    }
    let Some((delta_x, delta_y)) = accumulate_deadzone_motion(state, raw_x, raw_y) else {
        return Ok(None);
    };

    let speed = update_motion_speed(state, delta_x, delta_y, now);
    let acceleration = scroll_acceleration(speed);
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

    Ok(process_feedback(
        state,
        config.feedback,
        delta_x,
        delta_y,
        speed,
        now,
    ))
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
    if state.scroll_velocity_x.hypot(state.scroll_velocity_y)
        < SCROLL_MOMENTUM_STOP_PIXELS_PER_SECOND
    {
        clear_scroll_momentum(state);
    }
    Ok(())
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

/// Treats an impossibly large per-report delta as a lift-and-replace: motion,
/// deadzone, feedback, and momentum restart from the new contact point.
fn rebaseline_placement(state: &mut PadMotionState, sample: PadSample) {
    state.reset_motion();
    state.previous = Some((sample.x, sample.y));
}

fn accumulate_deadzone_motion(
    state: &mut PadMotionState,
    delta_x: i32,
    delta_y: i32,
) -> Option<(i32, i32)> {
    // Accumulate slow intentional motion, but require its radial displacement
    // to leave the stationary-noise region before forwarding it. Recenter
    // after every accepted vector so a stopped finger gets a fresh deadzone.
    state.deadzone_x += delta_x;
    state.deadzone_y += delta_y;
    let x = i64::from(state.deadzone_x);
    let y = i64::from(state.deadzone_y);
    if x * x + y * y < i64::from(PAD_MOTION_DEADZONE_COUNTS).pow(2) {
        None
    } else {
        let filtered = (state.deadzone_x, state.deadzone_y);
        state.deadzone_x = 0;
        state.deadzone_y = 0;
        Some(filtered)
    }
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
