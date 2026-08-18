#[allow(
    clippy::wildcard_imports,
    reason = "the mailbox shares private snapshots and synchronization types with its worker"
)]
use super::*;

#[derive(Debug, Clone, Copy)]
pub(crate) struct DesktopWorkerSnapshot {
    pub(crate) snapshot: DesktopInputSnapshot,
    pub(crate) now: Duration,
    pub(crate) generation: u64,
    pub(crate) feedback_epoch: u64,
}

pub(crate) enum DesktopWorkerMessage {
    Snapshot(DesktopWorkerSnapshot),
    Overflow,
    ReplaceProfile {
        profile: Option<Box<BindingProfile>>,
        ack: Option<CommandAck>,
    },
    Enable {
        ack: Option<CommandAck>,
    },
    Disconnect(CommandAck),
    Shutdown(CommandAck),
}

impl DesktopWorkerMessage {
    pub(crate) fn reject(self, error: &str) {
        let ack = match self {
            Self::ReplaceProfile { ack, .. } | Self::Enable { ack } => ack,
            Self::Disconnect(ack) | Self::Shutdown(ack) => Some(ack),
            Self::Snapshot(_) | Self::Overflow => None,
        };
        if let Some(ack) = ack {
            let _ = ack.send(Err(error.to_owned()));
        }
    }

    pub(crate) const fn is_snapshot_or_overflow(&self) -> bool {
        matches!(self, Self::Snapshot(_) | Self::Overflow)
    }

    pub(crate) const fn is_safety_control(&self) -> bool {
        matches!(self, Self::Disconnect(_) | Self::Shutdown(_))
    }
}

/// Tracks whether two equal transition masks have established a baseline that
/// makes an intermediate analog-only sample replaceable.
#[derive(Debug, Default)]
pub(crate) struct StableTransitionRun {
    pub(crate) previous: Option<u16>,
    pub(crate) latest: Option<u16>,
}

impl StableTransitionRun {
    pub(crate) fn can_replace_latest(&self, transition_mask: Option<u16>) -> bool {
        transition_mask.is_some()
            && self.latest == transition_mask
            && self.previous == transition_mask
    }

    pub(crate) fn push(&mut self, transition_mask: Option<u16>) {
        self.previous = self.latest;
        self.latest = transition_mask;
    }

    pub(crate) fn reset(&mut self) {
        self.previous = None;
        self.latest = None;
    }

    pub(crate) fn reset_with_latest(&mut self, transition_mask: Option<u16>) {
        self.previous = None;
        self.latest = transition_mask;
    }
}

#[derive(Default)]
pub(crate) struct DesktopWorkerMailboxState {
    pub(crate) messages: VecDeque<DesktopWorkerMessage>,
    pub(crate) snapshot_count: usize,
    pub(crate) control_count: usize,
    pub(crate) transition_run: StableTransitionRun,
    pub(crate) generation: u64,
    pub(crate) feedback_epoch: u64,
    pub(crate) accepting: bool,
}

pub(crate) struct DesktopWorkerMailbox {
    pub(crate) state: Mutex<DesktopWorkerMailboxState>,
    pub(crate) wake: Condvar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DesktopSnapshotPublish {
    Published,
    Overflowed,
    Closed,
}

impl Default for DesktopWorkerMailbox {
    fn default() -> Self {
        Self {
            state: Mutex::new(DesktopWorkerMailboxState {
                accepting: true,
                ..DesktopWorkerMailboxState::default()
            }),
            wake: Condvar::new(),
        }
    }
}

impl DesktopWorkerMailbox {
    // The supervisor is the sole producer. Snapshot runs may coalesce only
    // after preserving a baseline; controls reset that run and remain ordered
    // barriers. Overflow keeps controls, releases worker state, and retains the
    // newest snapshot as a non-emitting recovery baseline.
    pub(crate) fn publish_snapshot(
        &self,
        outputs: &DesktopWorkerOutputs,
        snapshot: DesktopInputSnapshot,
        now: Duration,
    ) -> DesktopSnapshotPublish {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.accepting {
            return DesktopSnapshotPublish::Closed;
        }
        let transition_mask = desktop_snapshot_transition_mask(snapshot);
        let result = if state
            .transition_run
            .can_replace_latest(Some(transition_mask))
        {
            let generation = state.generation;
            let feedback_epoch = state.feedback_epoch;
            let Some(DesktopWorkerMessage::Snapshot(latest)) = state.messages.back_mut() else {
                unreachable!("desktop snapshot coalescing state must describe the queue tail");
            };
            *latest = DesktopWorkerSnapshot {
                snapshot,
                now,
                generation,
                feedback_epoch,
            };
            DesktopSnapshotPublish::Published
        } else if state.snapshot_count == DESKTOP_INPUT_MAILBOX_CAPACITY {
            Self::reset_snapshots_for_overflow(&mut state);
            outputs.invalidate_feedback(state.feedback_epoch);
            state.messages.push_back(DesktopWorkerMessage::Overflow);
            Self::push_snapshot(&mut state, snapshot, now, transition_mask);
            DesktopSnapshotPublish::Overflowed
        } else {
            Self::push_snapshot(&mut state, snapshot, now, transition_mask);
            DesktopSnapshotPublish::Published
        };
        drop(state);
        self.wake.notify_one();
        result
    }

    pub(crate) fn publish_overflow(&self, outputs: &DesktopWorkerOutputs) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.accepting {
            return false;
        }
        Self::reset_snapshots_for_overflow(&mut state);
        outputs.invalidate_feedback(state.feedback_epoch);
        state.messages.push_back(DesktopWorkerMessage::Overflow);
        drop(state);
        self.wake.notify_one();
        true
    }

    pub(crate) fn push_control(
        &self,
        outputs: &DesktopWorkerOutputs,
        message: DesktopWorkerMessage,
        feedback_barrier: bool,
    ) -> Result<(), Box<DesktopWorkerMessage>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.accepting {
            return Err(Box::new(message));
        }
        if matches!(message, DesktopWorkerMessage::ReplaceProfile { .. })
            && matches!(
                state.messages.back(),
                Some(DesktopWorkerMessage::ReplaceProfile { .. })
            )
        {
            let Some(previous) = state.messages.pop_back() else {
                unreachable!("the queue tail was just matched")
            };
            state.messages.push_back(message);
            drop(state);
            previous.reject("desktop profile update superseded by a newer profile");
            self.wake.notify_one();
            return Ok(());
        }
        let reserved_capacity = DESKTOP_CONTROL_MAILBOX_CAPACITY - 1;
        let limit = if message.is_safety_control() {
            DESKTOP_CONTROL_MAILBOX_CAPACITY
        } else {
            reserved_capacity
        };
        if state.control_count >= limit {
            return Err(Box::new(message));
        }
        if feedback_barrier {
            state.feedback_epoch = state.feedback_epoch.wrapping_add(1);
            outputs.invalidate_feedback(state.feedback_epoch);
        }
        state.transition_run.reset();
        state.messages.push_back(message);
        state.control_count += 1;
        drop(state);
        self.wake.notify_one();
        Ok(())
    }

    pub(crate) fn take_batch(&self, timeout: Option<Duration>) -> VecDeque<DesktopWorkerMessage> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.messages.is_empty() && state.accepting {
            state = match timeout {
                Some(timeout) => {
                    let (returned, _) = self
                        .wake
                        .wait_timeout_while(state, timeout, |state| {
                            state.messages.is_empty() && state.accepting
                        })
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    returned
                }
                None => self
                    .wake
                    .wait_while(state, |state| state.messages.is_empty() && state.accepting)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            };
        }
        state.snapshot_count = 0;
        state.control_count = 0;
        state.transition_run.reset();
        std::mem::take(&mut state.messages)
    }

    pub(crate) fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accepting = false;
        let pending = std::mem::take(&mut state.messages);
        state.snapshot_count = 0;
        state.control_count = 0;
        state.transition_run.reset();
        drop(state);
        for message in pending {
            message.reject("desktop-input worker stopped before processing the command");
        }
        self.wake.notify_all();
    }

    pub(crate) fn generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation
    }

    pub(crate) fn push_snapshot(
        state: &mut DesktopWorkerMailboxState,
        snapshot: DesktopInputSnapshot,
        now: Duration,
        transition_mask: u16,
    ) {
        state
            .messages
            .push_back(DesktopWorkerMessage::Snapshot(DesktopWorkerSnapshot {
                snapshot,
                now,
                generation: state.generation,
                feedback_epoch: state.feedback_epoch,
            }));
        state.snapshot_count += 1;
        state.transition_run.push(Some(transition_mask));
    }

    pub(crate) fn reset_snapshots_for_overflow(state: &mut DesktopWorkerMailboxState) {
        state
            .messages
            .retain(|message| !message.is_snapshot_or_overflow());
        state.snapshot_count = 0;
        state.transition_run.reset();
        state.generation = state.generation.wrapping_add(1);
        state.feedback_epoch = state.feedback_epoch.wrapping_add(1);
    }
}

pub(crate) fn desktop_snapshot_transition_mask(snapshot: DesktopInputSnapshot) -> u16 {
    desktop_transition_mask(
        snapshot.buttons,
        snapshot.left_pad.touched,
        snapshot.right_pad.touched,
    )
}

/// Which bits, if they change, make a snapshot a transition that must not be
/// coalesced away. The pad click bits are explicit because `bindable_mask` no
/// longer carries them: pad clicks dispatch through the regions.
pub(crate) fn desktop_transition_mask(
    buttons: SteamButtons,
    left_touched: bool,
    right_touched: bool,
) -> u16 {
    let mut mask = u16::from(bindable_mask(buttons));
    mask |= u16::from(left_touched) << 8;
    mask |= u16::from(right_touched) << 9;
    mask |= u16::from(buttons.contains(SteamButton::LeftPadClick)) << 10;
    mask |= u16::from(buttons.contains(SteamButton::RightPadClick)) << 11;
    mask
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DesktopWorkerOutput {
    pub(crate) feedback: PadFeedbackRequest,
    pub(crate) discard_pending_feedback: bool,
}

impl Default for DesktopWorkerOutput {
    fn default() -> Self {
        Self {
            feedback: PadFeedbackRequest::NONE,
            discard_pending_feedback: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DesktopWorkerOutputState {
    pub(crate) output: DesktopWorkerOutput,
    pub(crate) feedback_epoch: u64,
}

#[derive(Default)]
pub(crate) struct DesktopWorkerOutputs {
    pub(crate) state: Mutex<DesktopWorkerOutputState>,
}

impl DesktopWorkerOutputs {
    pub(crate) fn publish_feedback(&self, feedback_epoch: u64, feedback: PadFeedbackRequest) {
        if feedback == PadFeedbackRequest::NONE {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if feedback_epoch != state.feedback_epoch {
            return;
        }
        if feedback.left.is_some() {
            state.output.feedback.left = feedback.left;
        }
        if feedback.right.is_some() {
            state.output.feedback.right = feedback.right;
        }
    }

    pub(crate) fn discard_feedback(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.output.feedback = PadFeedbackRequest::NONE;
        state.output.discard_pending_feedback = true;
    }

    pub(crate) fn invalidate_feedback(&self, feedback_epoch: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.output.feedback = PadFeedbackRequest::NONE;
        state.output.discard_pending_feedback = true;
        state.feedback_epoch = feedback_epoch;
    }

    pub(crate) fn take(&self) -> DesktopWorkerOutput {
        std::mem::take(
            &mut self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .output,
        )
    }
}
