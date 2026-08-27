//! A bounded mailbox between the polling thread and the UI.
//!
//! The old `sync_channel(64)` + `try_send` dropped the *newest* report when it
//! filled, so under pressure the display could sit on a stale press while the
//! controller had already moved on. This keeps the newest instead.
//!
//! The shape mirrors `TransitionReportMailbox` in `crates/bridge/bridge-runtime`
//! (see its `transition_mailbox_*` tests). It is not shared code: the runtime
//! coalesces on an 8-bit desktop-bindings mask, and the visualizer has to see
//! every one of the 32 Steam button bits.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use steam_controller_device::{DeviceEvent, RawHidReport};
use steam_controller_discovery::ControllerSearch;
use steam_controller_protocol::{
    EXTENDED_INPUT_REPORT_ID, EXTENDED_INPUT_REPORT_SIZE, INPUT_REPORT_ID, INPUT_REPORT_SIZE,
};

pub(crate) const CAPACITY: usize = 64;

/// What the worker hands over. Reports may be coalesced; everything else is an
/// ordered barrier that is never discarded.
#[derive(Debug, Clone)]
pub(crate) enum InputEvent {
    Report {
        report: RawHidReport,
        /// Worker-side receive time. The UI must not treat an old report as
        /// fresh merely because it was delayed in a repaint backlog.
        received_at: Instant,
    },
    /// Connected, disconnected, or a worker failure.
    Lifecycle(Box<Result<DeviceEvent, String>>),
    /// Typed controller-discovery status for structured diagnostics.
    Search(ControllerSearch),
}

impl InputEvent {
    pub(crate) fn report(report: RawHidReport) -> Self {
        Self::Report {
            report,
            received_at: Instant::now(),
        }
    }
}

/// Counters describing what the mailbox had to throw away.
#[derive(Debug, Default)]
pub(crate) struct MailboxCounters {
    /// Every report offered by the producer, before any pressure policy.
    pub published: AtomicU64,
    /// Redundant analog reports replaced in place. No transition was lost.
    pub coalesced: AtomicU64,
    /// Reports discarded outright, only ever on overflow.
    pub dropped: AtomicU64,
    /// Times the queue filled with un-coalescible traffic.
    pub overflows: AtomicU64,
}

impl MailboxCounters {
    pub(crate) fn published(&self) -> u64 {
        self.published.load(Ordering::Relaxed)
    }

    pub(crate) fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.coalesced.load(Ordering::Relaxed),
            self.dropped.load(Ordering::Relaxed),
            self.overflows.load(Ordering::Relaxed),
        )
    }
}

/// The last two button masks seen. A report is only redundant when its mask
/// matches both, which keeps the first and newest sample of every run plus
/// every transition between runs.
#[derive(Default)]
struct StableRun {
    previous: Option<u32>,
    latest: Option<u32>,
}

impl StableRun {
    fn can_replace_latest(&self, mask: Option<u32>) -> bool {
        mask.is_some() && self.latest == mask && self.previous == mask
    }

    fn push(&mut self, mask: Option<u32>) {
        self.previous = self.latest;
        self.latest = mask;
    }

    fn reset(&mut self) {
        self.previous = None;
        self.latest = None;
    }

    fn reset_with_latest(&mut self, mask: Option<u32>) {
        self.previous = None;
        self.latest = mask;
    }
}

#[derive(Default)]
struct MailboxState {
    events: VecDeque<InputEvent>,
    run: StableRun,
    overflowed: bool,
}

#[derive(Default)]
pub(crate) struct InputMailbox {
    state: Mutex<MailboxState>,
    pub(crate) counters: MailboxCounters,
}

/// The full 32-bit button mask, but only for a report whose id and length both
/// validate. Anything else is non-coalescible and still reaches the decoder.
fn button_mask(report: &RawHidReport) -> Option<u32> {
    let valid = match report.report_id {
        INPUT_REPORT_ID => report.data.len() == INPUT_REPORT_SIZE,
        EXTENDED_INPUT_REPORT_ID => report.data.len() == EXTENDED_INPUT_REPORT_SIZE,
        _ => false,
    };
    if !valid {
        return None;
    }
    Some(u32::from_le_bytes([
        report.data[2],
        report.data[3],
        report.data[4],
        report.data[5],
    ]))
}

impl InputMailbox {
    pub(crate) fn publish(&self, event: InputEvent) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if matches!(&event, InputEvent::Report { .. }) {
            self.counters.published.fetch_add(1, Ordering::Relaxed);
        }

        let InputEvent::Report { report, .. } = &event else {
            // A persistent backend error is status, not a distinct lifecycle
            // transition. Keep one copy instead of allowing a fast failing
            // poll loop to grow the queue without bound.
            if let InputEvent::Lifecycle(new) = &event {
                if let (Err(new_error), Some(InputEvent::Lifecycle(previous))) =
                    (&**new, state.events.back())
                {
                    if matches!(&**previous, Err(previous_error) if previous_error == new_error) {
                        return;
                    }
                }
            }
            if let InputEvent::Search(new) = &event {
                if matches!(state.events.back(), Some(InputEvent::Search(previous)) if previous == new)
                {
                    return;
                }
            }
            // Lifecycle events keep their place and end any coalescing run.
            state.run.reset();
            state.events.push_back(event);
            return;
        };
        let mask = button_mask(report);

        if state.events.len() < CAPACITY {
            // Fidelity is more important than compression while the consumer
            // is keeping up: report rate, smoothing, raw recording, and serial
            // output must all see every sample.
            state.events.push_back(event);
            state.run.push(mask);
            return;
        }

        let replaceable = state.run.can_replace_latest(mask)
            && matches!(state.events.back(), Some(InputEvent::Report { .. }));
        if replaceable {
            let _ = state.events.pop_back();
            state.events.push_back(event);
            self.counters.coalesced.fetch_add(1, Ordering::Relaxed);
        } else if let Some(redundant) = redundant_report_index(&state.events) {
            // The queue is full, but an established same-button run has a
            // middle analog sample that can be removed without losing either
            // edge or the newest coordinates.
            let _ = state.events.remove(redundant);
            state.events.push_back(event);
            rebuild_run(&mut state);
            self.counters.coalesced.fetch_add(1, Ordering::Relaxed);
        } else {
            // Nothing safe to coalesce. Keep every lifecycle event and the
            // newest report as an explicit recovery baseline, so the display
            // cannot stay stuck on an old press.
            let before = state.events.len();
            state.events.retain(|queued| {
                matches!(queued, InputEvent::Lifecycle(_) | InputEvent::Search(_))
            });
            let discarded = before - state.events.len();
            state.events.push_back(event);
            state.run.reset_with_latest(mask);
            state.overflowed = true;
            self.counters
                .dropped
                .fetch_add(discarded as u64, Ordering::Relaxed);
            self.counters.overflows.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Drains into caller-owned scratch storage while retaining both buffers.
    pub(crate) fn take_all(&self, destination: &mut Vec<InputEvent>) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.run.reset();
        destination.clear();
        destination.reserve(state.events.len());
        destination.extend(state.events.drain(..));
        std::mem::take(&mut state.overflowed)
    }
}

/// Finds the middle of an established three-report run. Malformed reports and
/// lifecycle events are barriers because their button state is unknown.
fn redundant_report_index(events: &VecDeque<InputEvent>) -> Option<usize> {
    let mut last: Option<(usize, u32)> = None;
    let mut run_len = 0_usize;
    for (index, event) in events.iter().enumerate() {
        let InputEvent::Report { report, .. } = event else {
            last = None;
            run_len = 0;
            continue;
        };
        let Some(mask) = button_mask(report) else {
            last = None;
            run_len = 0;
            continue;
        };
        if last.is_some_and(|(_, previous)| previous == mask) {
            run_len += 1;
            if run_len >= 3 {
                return last.map(|(previous_index, _)| previous_index);
            }
        } else {
            run_len = 1;
        }
        last = Some((index, mask));
    }
    None
}

fn rebuild_run(state: &mut MailboxState) {
    state.run.reset();
    for event in &state.events {
        match event {
            InputEvent::Report { report, .. } => state.run.push(button_mask(report)),
            InputEvent::Lifecycle(_) | InputEvent::Search(_) => state.run.reset(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{button_mask, InputEvent, InputMailbox, CAPACITY};
    use std::time::Duration;
    use steam_controller_device::{DeviceEvent, RawHidReport};
    use steam_controller_protocol::{INPUT_REPORT_ID, INPUT_REPORT_SIZE};

    fn report(buttons: u32) -> RawHidReport {
        let mut data = vec![0u8; INPUT_REPORT_SIZE];
        data[2..6].copy_from_slice(&buttons.to_le_bytes());
        RawHidReport {
            timestamp: Duration::ZERO,
            report_id: INPUT_REPORT_ID,
            data,
            source_device_id: "test".to_owned(),
            transport: "test".to_owned(),
            dropped_reports: 0,
        }
    }

    fn queued(mailbox: &InputMailbox) -> usize {
        let mut events = Vec::new();
        let _ = mailbox.take_all(&mut events);
        events.len()
    }

    #[test]
    fn under_capacity_every_report_is_retained() {
        let mailbox = InputMailbox::default();
        // Even identical analog samples stay intact while there is room.
        for _ in 0..CAPACITY {
            mailbox.publish(InputEvent::report(report(0b1)));
        }
        let (coalesced, dropped, overflows) = mailbox.counters.snapshot();
        assert_eq!((coalesced, dropped, overflows), (0, 0, 0));
        assert_eq!(queued(&mailbox), CAPACITY);
    }

    #[test]
    fn nominal_device_rate_with_sixty_hz_drains_loses_no_reports() {
        let mailbox = InputMailbox::default();
        let mut drained = Vec::with_capacity(CAPACITY);
        let mut retained = 0_usize;
        for sample in 0..250 {
            mailbox.publish(InputEvent::report(report(0b1)));
            // Approximate a 250 Hz producer drained by a 60 Hz UI.
            if sample % 4 == 3 {
                assert!(!mailbox.take_all(&mut drained));
                retained += drained.len();
            }
        }
        let _ = mailbox.take_all(&mut drained);
        retained += drained.len();
        assert_eq!(retained, 250);
        assert_eq!(mailbox.counters.published(), 250);
        assert_eq!(mailbox.counters.snapshot(), (0, 0, 0));
    }

    #[test]
    fn mailbox_coalesces_analog_runs_but_preserves_button_edges() {
        let mailbox = InputMailbox::default();
        // Fill the queue with one stable run. Only the next report creates
        // pressure and replaces the previous newest sample.
        for _ in 0..=CAPACITY {
            mailbox.publish(InputEvent::report(report(0b1)));
        }
        mailbox.publish(InputEvent::report(report(0b11)));
        let (coalesced, dropped, _) = mailbox.counters.snapshot();
        assert_eq!(coalesced, 2, "only pressure removed redundant samples");
        assert_eq!(dropped, 0, "coalescing never drops a transition");

        let mut events = Vec::new();
        assert!(!mailbox.take_all(&mut events));
        assert_eq!(events.len(), CAPACITY);
        let masks: Vec<Option<u32>> = events
            .iter()
            .map(|event| match event {
                InputEvent::Report { report, .. } => button_mask(report),
                InputEvent::Lifecycle(_) | InputEvent::Search(_) => None,
            })
            .collect();
        assert_eq!(masks.last(), Some(&Some(0b11)));
        assert!(masks[..masks.len() - 1]
            .iter()
            .all(|mask| *mask == Some(0b1)));
    }

    #[test]
    fn lifecycle_events_are_never_discarded_and_keep_their_order() {
        let mailbox = InputMailbox::default();
        mailbox.publish(InputEvent::Lifecycle(Box::new(Ok(
            DeviceEvent::Disconnected,
        ))));
        // Enough identical reports to force an overflow.
        for _ in 0..CAPACITY * 3 {
            mailbox.publish(InputEvent::report(report(0b1)));
        }
        mailbox.publish(InputEvent::Lifecycle(Box::new(Err("boom".to_owned()))));

        let mut events = Vec::new();
        let _ = mailbox.take_all(&mut events);
        let lifecycles: Vec<&InputEvent> = events
            .iter()
            .filter(|event| matches!(event, InputEvent::Lifecycle(_)))
            .collect();
        assert_eq!(lifecycles.len(), 2, "both lifecycle events survived");
        assert!(
            matches!(events.first(), Some(InputEvent::Lifecycle(_))),
            "the disconnect stayed at the front"
        );
        assert!(
            matches!(events.last(), Some(InputEvent::Lifecycle(_))),
            "the failure stayed at the back"
        );
    }

    #[test]
    fn overflow_retains_the_newest_report_as_a_recovery_baseline() {
        let mailbox = InputMailbox::default();
        // Alternating masks defeat coalescing, so the queue really fills.
        for index in 0..u32::try_from(CAPACITY).unwrap() {
            mailbox.publish(InputEvent::report(report(index)));
        }
        mailbox.publish(InputEvent::report(report(0xDEAD_BEEF)));

        let (_, dropped, overflows) = mailbox.counters.snapshot();
        assert_eq!(overflows, 1);
        assert_eq!(dropped, CAPACITY as u64);

        let mut events = Vec::new();
        let overflowed = mailbox.take_all(&mut events);
        assert!(overflowed, "the overflow is reported to the UI");
        assert_eq!(events.len(), 1, "only the recovery baseline remains");
        match &events[0] {
            InputEvent::Report { report, .. } => {
                assert_eq!(button_mask(report), Some(0xDEAD_BEEF));
            }
            InputEvent::Lifecycle(_) | InputEvent::Search(_) => {
                panic!("expected the newest report")
            }
        }
    }

    #[test]
    fn malformed_reports_are_never_coalesced() {
        let mailbox = InputMailbox::default();
        let mut short = report(0b1);
        short.data.truncate(4);
        for _ in 0..3 {
            mailbox.publish(InputEvent::report(short.clone()));
        }
        let (coalesced, ..) = mailbox.counters.snapshot();
        assert_eq!(coalesced, 0, "an unvalidated report has no usable mask");
        assert_eq!(queued(&mailbox), 3);
    }

    #[test]
    fn draining_always_exposes_the_newest_report() {
        let mailbox = InputMailbox::default();
        for _ in 0..CAPACITY * 5 {
            mailbox.publish(InputEvent::report(report(0b1)));
        }
        mailbox.publish(InputEvent::report(report(0b1000)));
        let mut events = Vec::new();
        let _ = mailbox.take_all(&mut events);
        match events.last() {
            Some(InputEvent::Report { report, .. }) => {
                assert_eq!(button_mask(report), Some(0b1000));
            }
            _ => panic!("the newest report must survive"),
        }
    }
}
