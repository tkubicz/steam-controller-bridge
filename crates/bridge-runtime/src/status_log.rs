use std::fmt;
use std::time::Duration;

use crate::{BridgeStatus, RuntimeState};

pub const STATUS_SNAPSHOT_INTERVAL: Duration = Duration::from_mins(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLogLevel {
    Info,
    Warn,
    Error,
}

impl fmt::Display for StatusLogLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSnapshotReason {
    Startup,
    Periodic,
    Error,
}

impl fmt::Display for StatusSnapshotReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Startup => "startup",
            Self::Periodic => "periodic",
            Self::Error => "error",
        })
    }
}

/// Copyable discriminant for callers that only need to classify a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLogRecordKind {
    Snapshot(StatusSnapshotReason),
    Change,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLogRecord {
    level: StatusLogLevel,
    revision: u64,
    body: StatusLogRecordBody,
}

/// Each variant carries its own payload so a record cannot be built with a
/// kind that disagrees with its contents. Formatting a log line must never be
/// able to panic.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StatusLogRecordBody {
    Snapshot {
        reason: StatusSnapshotReason,
        status: String,
    },
    Change {
        changes: Vec<StatusLogChange>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLogChange {
    field: &'static str,
    previous: String,
    current: String,
}

impl StatusLogChange {
    #[must_use]
    pub const fn field(&self) -> &'static str {
        self.field
    }

    #[must_use]
    pub fn previous(&self) -> &str {
        &self.previous
    }

    #[must_use]
    pub fn current(&self) -> &str {
        &self.current
    }
}

impl StatusLogRecord {
    #[must_use]
    pub const fn kind(&self) -> StatusLogRecordKind {
        match &self.body {
            StatusLogRecordBody::Snapshot { reason, .. } => StatusLogRecordKind::Snapshot(*reason),
            StatusLogRecordBody::Change { .. } => StatusLogRecordKind::Change,
        }
    }

    #[must_use]
    pub const fn level(&self) -> StatusLogLevel {
        self.level
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn changes(&self) -> &[StatusLogChange] {
        match &self.body {
            StatusLogRecordBody::Change { changes } => changes,
            StatusLogRecordBody::Snapshot { .. } => &[],
        }
    }
}

impl fmt::Display for StatusLogRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "level={} event=", self.level)?;
        match &self.body {
            StatusLogRecordBody::Snapshot { reason, status } => write!(
                formatter,
                "status_snapshot reason={reason} revision={} status={status}",
                self.revision
            ),
            StatusLogRecordBody::Change { changes } => {
                write!(formatter, "status_change revision={}", self.revision)?;
                for change in changes {
                    // `last_error` records only a transition, and its own
                    // snapshot carries the message, so the arrow form would add
                    // nothing readable here.
                    if change.field == "last_error" {
                        write!(formatter, " last_error={}", change.current)?;
                    } else {
                        write!(
                            formatter,
                            " {}={}->{}",
                            change.field, change.previous, change.current
                        )?;
                    }
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct StatusLogTracker {
    previous: Option<BridgeStatus>,
    last_snapshot_at: Option<Duration>,
}

impl StatusLogTracker {
    #[must_use]
    pub fn observe(&mut self, now: Duration, current: &BridgeStatus) -> Vec<StatusLogRecord> {
        let Some(previous) = self.previous.as_ref() else {
            let is_error = current.state == RuntimeState::Error
                || current.last_error.is_some()
                || has_failures(current);
            let reason = if is_error {
                StatusSnapshotReason::Error
            } else {
                StatusSnapshotReason::Startup
            };
            let record = snapshot_record(reason, current);
            self.previous = Some(current.clone());
            self.last_snapshot_at = Some(now);
            return vec![record];
        };

        let periodic_due = self
            .last_snapshot_at
            .is_none_or(|last| now.saturating_sub(last) >= STATUS_SNAPSHOT_INTERVAL);
        if previous.revision == current.revision {
            if periodic_due {
                self.last_snapshot_at = Some(now);
                return vec![snapshot_record(StatusSnapshotReason::Periodic, current)];
            }
            return Vec::new();
        }

        let tracked_change = tracked_status_changed(previous, current);
        if !tracked_change {
            // Volatile ages and successful-operation counters intentionally do not
            // participate in deltas. Remember the observation without cloning all
            // of their updated strings and counters every 250 ms.
            if let Some(previous) = self.previous.as_mut() {
                previous.revision = current.revision;
            }
            if periodic_due {
                self.last_snapshot_at = Some(now);
                return vec![snapshot_record(StatusSnapshotReason::Periodic, current)];
            }
            return Vec::new();
        }

        let error_detected = current.state == RuntimeState::Error
            && previous.state != RuntimeState::Error
            || current.last_error.is_some() && current.last_error != previous.last_error
            || failures_increased(previous, current);
        let changes = status_changes(previous, current);
        let mut records = Vec::new();
        if !changes.is_empty() {
            let level = if current.state == RuntimeState::Error {
                StatusLogLevel::Error
            } else if error_detected {
                StatusLogLevel::Warn
            } else {
                StatusLogLevel::Info
            };
            records.push(change_record(level, current.revision, changes));
        }

        if error_detected {
            records.push(snapshot_record(StatusSnapshotReason::Error, current));
            self.last_snapshot_at = Some(now);
        } else if periodic_due {
            records.push(snapshot_record(StatusSnapshotReason::Periodic, current));
            self.last_snapshot_at = Some(now);
        }

        self.previous = Some(current.clone());
        records
    }
}

fn change_record(
    level: StatusLogLevel,
    revision: u64,
    changes: Vec<StatusLogChange>,
) -> StatusLogRecord {
    StatusLogRecord {
        level,
        revision,
        body: StatusLogRecordBody::Change { changes },
    }
}

fn snapshot_record(reason: StatusSnapshotReason, status: &BridgeStatus) -> StatusLogRecord {
    let level = match reason {
        StatusSnapshotReason::Error if status.state == RuntimeState::Error => StatusLogLevel::Error,
        StatusSnapshotReason::Error => StatusLogLevel::Warn,
        StatusSnapshotReason::Startup | StatusSnapshotReason::Periodic => StatusLogLevel::Info,
    };
    StatusLogRecord {
        level,
        revision: status.revision,
        body: StatusLogRecordBody::Snapshot {
            reason,
            status: format!("{status:?}"),
        },
    }
}

#[must_use]
pub fn format_status_diagnostics(status: &BridgeStatus) -> String {
    format!("Steam Controller Bridge diagnostics\nstatus: {status:#?}\n")
}

fn status_changes(previous: &BridgeStatus, current: &BridgeStatus) -> Vec<StatusLogChange> {
    let mut changes = Vec::new();
    core_status_changes(&mut changes, previous, current);
    safety_status_changes(&mut changes, previous, current);
    failure_status_changes(&mut changes, previous, current);
    // Deliberately records only the transition, not the message: setting or
    // changing an error always emits a snapshot alongside this record, and that
    // snapshot carries the text. Keeping it out here keeps change lines short
    // and stops the message being duplicated on consecutive lines.
    if previous.last_error != current.last_error {
        let (previous, current) = match (&previous.last_error, &current.last_error) {
            (_, None) => ("set", "cleared"),
            (None, Some(_)) => ("cleared", "set"),
            (Some(_), Some(_)) => ("set", "changed"),
        };
        changes.push(StatusLogChange {
            field: "last_error",
            previous: previous.to_owned(),
            current: current.to_owned(),
        });
    }
    changes
}

fn core_status_changes(
    changes: &mut Vec<StatusLogChange>,
    previous: &BridgeStatus,
    current: &BridgeStatus,
) {
    push_change(changes, "state", &previous.state, &current.state);
    push_change(changes, "detail", &previous.detail, &current.detail);
    push_change(
        changes,
        "source_identity",
        &previous.source.identity,
        &current.source.identity,
    );
    push_change(
        changes,
        "source_transport",
        &previous.source.transport,
        &current.source.transport,
    );
    push_change(
        changes,
        "source_connected",
        &previous.source.connected,
        &current.source.connected,
    );
    push_change(
        changes,
        "source_active",
        &previous.source.active,
        &current.source.active,
    );
    push_change(
        changes,
        "controller_connected",
        &previous.controller.connected,
        &current.controller.connected,
    );
    push_change(
        changes,
        "xiao_path",
        &previous.xiao.path,
        &current.xiao.path,
    );
    push_masked_serial_change(
        changes,
        "xiao_serial",
        previous.xiao.usb_serial.as_deref(),
        current.xiao.usb_serial.as_deref(),
    );
    push_change(
        changes,
        "xiao_handshake",
        &previous.xiao.handshake_complete,
        &current.xiao.handshake_complete,
    );
    push_change(
        changes,
        "battery",
        &previous.battery_percent,
        &current.battery_percent,
    );
    push_change(
        changes,
        "charge_state",
        &previous.battery_charge_state,
        &current.battery_charge_state,
    );
}

fn safety_status_changes(
    changes: &mut Vec<StatusLogChange>,
    previous: &BridgeStatus,
    current: &BridgeStatus,
) {
    push_change(
        changes,
        "lizard_suppressed",
        &previous.lizard.suppressed,
        &current.lizard.suppressed,
    );
    push_change(
        changes,
        "lizard_failures",
        &previous.lizard.failures,
        &current.lizard.failures,
    );
    push_change(
        changes,
        "haptics_state",
        &previous.haptics.state,
        &current.haptics.state,
    );
    push_change(
        changes,
        "haptics_failures",
        &previous.haptics.failures,
        &current.haptics.failures,
    );
    push_change(
        changes,
        "bindings_state",
        &previous.bindings.state,
        &current.bindings.state,
    );
    push_change(
        changes,
        "binding_profile",
        &previous.bindings.active_profile_name,
        &current.bindings.active_profile_name,
    );
    push_change(
        changes,
        "configured_bindings",
        &previous.bindings.configured_binding_count,
        &current.bindings.configured_binding_count,
    );
    push_change(
        changes,
        "binding_failures",
        &previous.bindings.failures,
        &current.bindings.failures,
    );
    push_change(
        changes,
        "binding_last_error",
        &previous.bindings.last_error,
        &current.bindings.last_error,
    );
    push_change(
        changes,
        "idle_timeout",
        &previous.automatic_shutdown.configured_timeout,
        &current.automatic_shutdown.configured_timeout,
    );
    push_change(
        changes,
        "puck_dock_action",
        &previous.automatic_shutdown.puck_dock_action,
        &current.automatic_shutdown.puck_dock_action,
    );
    push_change(
        changes,
        "puck_dock_handled",
        &previous.automatic_shutdown.puck_dock_episode_handled,
        &current.automatic_shutdown.puck_dock_episode_handled,
    );
    push_change(
        changes,
        "automatic_shutdown_phase",
        &previous.automatic_shutdown.phase,
        &current.automatic_shutdown.phase,
    );
    push_change(
        changes,
        "automatic_shutdown_trigger",
        &previous.automatic_shutdown.trigger,
        &current.automatic_shutdown.trigger,
    );
    push_change(
        changes,
        "automatic_shutdown_failures",
        &previous.automatic_shutdown.failures,
        &current.automatic_shutdown.failures,
    );
}

fn failure_status_changes(
    changes: &mut Vec<StatusLogChange>,
    previous: &BridgeStatus,
    current: &BridgeStatus,
) {
    push_change(
        changes,
        "decode_failures",
        &previous.bridge_metrics.decode_failures,
        &current.bridge_metrics.decode_failures,
    );
    push_change(
        changes,
        "framing_failures",
        &previous.output_diagnostics.framing_failures,
        &current.output_diagnostics.framing_failures,
    );
    push_change(
        changes,
        "checksum_failures",
        &previous.output_diagnostics.checksum_failures,
        &current.output_diagnostics.checksum_failures,
    );
}

fn push_change<T: fmt::Debug + PartialEq>(
    changes: &mut Vec<StatusLogChange>,
    field: &'static str,
    previous: &T,
    current: &T,
) {
    if previous != current {
        changes.push(StatusLogChange {
            field,
            previous: format!("{previous:?}"),
            current: format!("{current:?}"),
        });
    }
}

fn push_masked_serial_change(
    changes: &mut Vec<StatusLogChange>,
    field: &'static str,
    previous: Option<&str>,
    current: Option<&str>,
) {
    if previous != current {
        changes.push(StatusLogChange {
            field,
            previous: format!("{:?}", crate::mask_serial_for_display(previous)),
            current: format!("{:?}", crate::mask_serial_for_display(current)),
        });
    }
}

fn tracked_status_changed(previous: &BridgeStatus, current: &BridgeStatus) -> bool {
    previous.state != current.state
        || previous.detail != current.detail
        || previous.source != current.source
        || previous.controller.connected != current.controller.connected
        || previous.xiao != current.xiao
        || previous.battery_percent != current.battery_percent
        || previous.battery_charge_state != current.battery_charge_state
        || previous.lizard.suppressed != current.lizard.suppressed
        || previous.lizard.failures != current.lizard.failures
        || previous.haptics.state != current.haptics.state
        || previous.haptics.failures != current.haptics.failures
        || previous.bindings.state != current.bindings.state
        || previous.bindings.active_profile_id != current.bindings.active_profile_id
        || previous.bindings.active_profile_name != current.bindings.active_profile_name
        || previous.bindings.configured_binding_count != current.bindings.configured_binding_count
        || previous.bindings.failures != current.bindings.failures
        || previous.bindings.last_error != current.bindings.last_error
        || previous.automatic_shutdown.configured_timeout
            != current.automatic_shutdown.configured_timeout
        || previous.automatic_shutdown.puck_dock_action
            != current.automatic_shutdown.puck_dock_action
        || previous.automatic_shutdown.puck_dock_episode_handled
            != current.automatic_shutdown.puck_dock_episode_handled
        || previous.automatic_shutdown.phase != current.automatic_shutdown.phase
        || previous.automatic_shutdown.trigger != current.automatic_shutdown.trigger
        || previous.automatic_shutdown.failures != current.automatic_shutdown.failures
        || previous.bridge_metrics.decode_failures != current.bridge_metrics.decode_failures
        || previous.output_diagnostics.framing_failures
            != current.output_diagnostics.framing_failures
        || previous.output_diagnostics.checksum_failures
            != current.output_diagnostics.checksum_failures
        || previous.last_error != current.last_error
}

fn has_failures(status: &BridgeStatus) -> bool {
    status.bridge_metrics.decode_failures > 0
        || status.output_diagnostics.framing_failures > 0
        || status.output_diagnostics.checksum_failures > 0
        || status.lizard.failures > 0
        || status.haptics.failures > 0
        || status.bindings.failures > 0
        || status.automatic_shutdown.failures > 0
}

fn failures_increased(previous: &BridgeStatus, current: &BridgeStatus) -> bool {
    current.bridge_metrics.decode_failures > previous.bridge_metrics.decode_failures
        || current.output_diagnostics.framing_failures
            > previous.output_diagnostics.framing_failures
        || current.output_diagnostics.checksum_failures
            > previous.output_diagnostics.checksum_failures
        || current.lizard.failures > previous.lizard.failures
        || current.haptics.failures > previous.haptics.failures
        || current.bindings.failures > previous.bindings.failures
        || current.automatic_shutdown.failures > previous.automatic_shutdown.failures
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_and_periodic_snapshots_include_complete_state() {
        let status = BridgeStatus::default();
        let mut tracker = StatusLogTracker::default();
        let startup = tracker.observe(Duration::ZERO, &status);
        assert_eq!(startup.len(), 1);
        assert_eq!(
            startup[0].kind(),
            StatusLogRecordKind::Snapshot(StatusSnapshotReason::Startup)
        );
        let text = startup[0].to_string();
        for field in [
            "status=BridgeStatus",
            "source:",
            "controller:",
            "xiao:",
            "lizard:",
            "haptics:",
            "bindings:",
            "automatic_shutdown:",
            "bridge_metrics:",
            "output_diagnostics:",
            "last_error:",
        ] {
            assert!(text.contains(field), "missing {field} from {text}");
        }

        assert!(tracker
            .observe(
                STATUS_SNAPSHOT_INTERVAL
                    .checked_sub(Duration::from_millis(1))
                    .unwrap(),
                &status,
            )
            .is_empty());
        let periodic = tracker.observe(STATUS_SNAPSHOT_INTERVAL, &status);
        assert_eq!(periodic.len(), 1);
        assert_eq!(
            periodic[0].kind(),
            StatusLogRecordKind::Snapshot(StatusSnapshotReason::Periodic)
        );
    }

    #[test]
    fn metric_and_age_only_revisions_do_not_log_immediately() {
        let initial = BridgeStatus::default();
        let mut tracker = StatusLogTracker::default();
        let _ = tracker.observe(Duration::ZERO, &initial);
        let mut current = initial;
        current.revision += 1;
        current.bridge_metrics.input_reports = 10_000;
        current.bridge_metrics.dropped_input_reports = 20;
        current.output_diagnostics.state_refreshes = 500;
        current.lizard.refreshes = 40;
        current.lizard.last_refresh_age = Some(Duration::from_secs(1));
        current.haptics.refreshes = 30;
        current.haptics.last_command_age = Some(Duration::from_millis(50));
        current.controller.last_state_age = Some(Duration::from_millis(2));
        current.automatic_shutdown.neutral_idle_age = Some(Duration::from_secs(30));
        assert!(tracker.observe(Duration::from_secs(1), &current).is_empty());
    }

    #[test]
    fn meaningful_changes_are_concise_and_errors_get_context() {
        let initial = BridgeStatus::default();
        let mut tracker = StatusLogTracker::default();
        let _ = tracker.observe(Duration::ZERO, &initial);

        let mut running = initial.clone();
        running.revision = 1;
        running.state = RuntimeState::Running;
        running.detail = "Bridge running".to_owned();
        running.controller.connected = true;
        running.battery_percent = Some(98);
        let records = tracker.observe(Duration::from_secs(1), &running);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind(), StatusLogRecordKind::Change);
        let text = records[0].to_string();
        assert!(text.contains("state=Stopped->Running"));
        assert!(text.contains("battery=None->Some(98)"));
        assert!(!text.contains("bridge_metrics="));
        assert!(!text.contains("last_state_age"));

        let mut failed = running;
        failed.revision = 2;
        failed.last_error = Some("controller failed".to_owned());
        let records = tracker.observe(Duration::from_secs(2), &failed);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].kind(), StatusLogRecordKind::Change);
        assert!(records[0].to_string().contains("last_error=set"));
        assert!(!records[0].to_string().contains("controller failed"));
        assert_eq!(
            records[1].kind(),
            StatusLogRecordKind::Snapshot(StatusSnapshotReason::Error)
        );
        assert!(records[1].to_string().contains("controller failed"));

        assert!(tracker.observe(Duration::from_secs(3), &failed).is_empty());
        let mut recovered = failed;
        recovered.revision = 3;
        recovered.last_error = None;
        let records = tracker.observe(Duration::from_secs(4), &recovered);
        assert_eq!(records.len(), 1);
        assert!(records[0].to_string().contains("last_error=cleared"));
        assert!(!records[0].to_string().contains("controller failed"));
    }

    #[test]
    fn hardware_and_safety_categories_are_reported_as_deltas() {
        let initial = BridgeStatus::default();
        let mut tracker = StatusLogTracker::default();
        let _ = tracker.observe(Duration::ZERO, &initial);
        let mut current = initial;
        current.revision = 1;
        current.source.connected = true;
        current.xiao.path = Some("/dev/cu.usbmodem1".to_owned());
        current.xiao.handshake_complete = true;
        current.lizard.suppressed = true;
        current.haptics.state = crate::HapticsState::Active;
        current.bindings.state = crate::DesktopBindingsState::Ready;
        current.bindings.active_profile_name = Some("Default".to_owned());
        current.bindings.configured_binding_count = 2;
        current.automatic_shutdown.configured_timeout = None;
        current.automatic_shutdown.phase = crate::AutomaticShutdownPhase::Monitoring;
        current.automatic_shutdown.puck_dock_action = crate::PuckDockAction::PowerOff;

        let records = tracker.observe(Duration::from_secs(1), &current);
        assert_eq!(records.len(), 1);
        let text = records[0].to_string();
        for field in [
            "source_connected=",
            "xiao_path=",
            "lizard_suppressed=",
            "haptics_state=",
            "bindings_state=",
            "binding_profile=",
            "configured_bindings=",
            "idle_timeout=",
            "automatic_shutdown_phase=",
            "puck_dock_action=",
        ] {
            assert!(text.contains(field), "missing {field} from {text}");
        }
    }

    #[test]
    fn each_failure_category_forces_an_error_snapshot() {
        for mutate in [
            |status: &mut BridgeStatus| status.bridge_metrics.decode_failures += 1,
            |status: &mut BridgeStatus| status.output_diagnostics.framing_failures += 1,
            |status: &mut BridgeStatus| status.output_diagnostics.checksum_failures += 1,
            |status: &mut BridgeStatus| status.lizard.failures += 1,
            |status: &mut BridgeStatus| status.haptics.failures += 1,
            |status: &mut BridgeStatus| status.bindings.failures += 1,
            |status: &mut BridgeStatus| status.automatic_shutdown.failures += 1,
        ] {
            let initial = BridgeStatus::default();
            let mut tracker = StatusLogTracker::default();
            let _ = tracker.observe(Duration::ZERO, &initial);
            let mut failed = initial;
            failed.revision = 1;
            mutate(&mut failed);
            let records = tracker.observe(Duration::from_secs(1), &failed);
            assert_eq!(records.len(), 2);
            assert!(matches!(
                records[1].kind(),
                StatusLogRecordKind::Snapshot(StatusSnapshotReason::Error)
            ));
        }
    }

    #[test]
    fn snapshots_and_source_changes_mask_serial_numbers() {
        let xiao_secret = "5E6EF905E5468F85";
        let controller_secret = "a1:b2:c3:d4:e5:f6";
        let mut status = BridgeStatus::default();
        status.xiao.usb_serial = Some(xiao_secret.to_owned());
        status.source.identity = Some(steam_controller_device::HidDeviceInfo {
            id: "bluetooth-controller".to_owned(),
            path: "bluetooth-controller".to_owned(),
            vendor_id: 0x28de,
            product_id: 0x1303,
            usage_page: 0xff00,
            usage: 1,
            interface_number: -1,
            serial_number: Some(controller_secret.to_owned()),
            manufacturer: Some("Valve Corporation".to_owned()),
            product: Some("Steam Ctrl (BT)".to_owned()),
            transport: "Bluetooth".to_owned(),
        });
        let mut tracker = StatusLogTracker::default();
        let records = tracker.observe(Duration::ZERO, &status);
        let text = records[0].to_string();
        assert!(!text.contains(xiao_secret));
        assert!(!text.contains(controller_secret));
        assert!(text.contains("****8F85"));
        assert!(text.contains("****5:f6"));

        let previous = status.clone();
        status.revision = 1;
        status.xiao.handshake_complete = true;
        status.source.connected = true;
        let records = tracker.observe(Duration::from_secs(1), &status);
        assert_eq!(records.len(), 1);
        let text = records[0].to_string();
        assert!(!text.contains(xiao_secret));
        assert!(!text.contains(controller_secret));
        assert!(!text.contains("usb_serial"));
        assert!(!text.contains("serial_number"));
        assert_ne!(previous.xiao, status.xiao);

        status.revision = 2;
        status.xiao.usb_serial = Some("1122334455667788".to_owned());
        status.source.identity.as_mut().unwrap().serial_number =
            Some("11:22:33:44:55:66".to_owned());
        let records = tracker.observe(Duration::from_secs(2), &status);
        let text = records[0].to_string();
        assert!(!text.contains(xiao_secret));
        assert!(!text.contains(controller_secret));
        assert!(!text.contains("1122334455667788"));
        assert!(!text.contains("11:22:33:44:55:66"));
        assert!(text.contains("****8F85"));
        assert!(text.contains("****7788"));
        assert!(text.contains("****5:f6"));
        assert!(text.contains("****5:66"));
    }
}
