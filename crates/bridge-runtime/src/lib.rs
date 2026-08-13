//! Reusable live Steam Controller 2 bridge orchestration.
//!
//! The runtime deliberately keeps discovery separate from ownership: candidate
//! Puck and Bluetooth collections are only read during discovery, and the
//! lizard-mode feature command is sent only after exactly one active source has
//! been identified.

mod api;
mod automatic_shutdown;
mod desktop;
mod idle_shutdown;
mod picker;
mod runtime;
mod supervisor;

pub use api::{
    AutomaticShutdownPhase, AutomaticShutdownStatus, BridgeStatus, ControllerChargeState,
    ControllerSelection, ControllerSourceStatus, ControllerStatus, DesktopBindingsState,
    DesktopBindingsStatus, HapticsState, HapticsStatus, LizardMode, LizardStatus, OutputBackend,
    OutputCapabilities, OutputSelection, OutputStatus, ProfilePickerStatus, PuckDockAction,
    RuntimeConfig, RuntimeError, RuntimeState, SerialSelection, ShutdownTrigger, VirtualHidStatus,
};
pub(crate) use automatic_shutdown::{
    automatic_shutdown_phase, binding_status_for_profile, validate_idle_shutdown_timeout,
    AutomaticShutdownRuntime, ControllerCooldown,
};
#[cfg(test)]
#[allow(clippy::wildcard_imports)]
pub(crate) use desktop::*;
pub(crate) use desktop::{
    bounded_error, desktop_transition_mask, DesktopBindingsWorker, StableTransitionRun,
};
pub(crate) use picker::PickerRuntime;
pub(crate) use runtime::{picker_status, CommandAck, RuntimeCommand};
pub use runtime::{
    BridgeHandle, BridgeRuntime, CommandPoll, OutputChangePoll, PendingOutputChange,
    PendingUpdateResume, PickerEventSink, UpdateResumePoll,
};
// Re-exported so frontends can render firmware status without depending on
// bridge-output directly.
pub use bridge_output::{
    new_firmware_install_receipt, FirmwareCapabilities, FirmwareInfo, FirmwareInstallReceipt,
    FirmwareInstallSource, FirmwareInstallState, FirmwareTarget, FirmwareTargetId,
    FirmwareTargetIdError, FirmwareVersion,
};
pub(crate) use supervisor::Supervisor;
#[cfg(test)]
#[allow(clippy::wildcard_imports)]
pub(crate) use supervisor::*;

use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bridge_core::{BridgeEngine, ProcessOutcome};
use bridge_output::{
    available_serial_devices, DumpOutput, FileOutput, GamepadOutput, MockOutput, OutputError,
    OutputFeedback, SerialDeviceInfo, SerialOutput,
};
#[cfg(test)]
use desktop_bindings::PadSample;
use desktop_bindings::{
    bindable_mask, BindingEngine, BindingProfile, DesktopInputSink, DesktopInputSnapshot,
    PadFeedbackRequest,
};
use gamepad_state::OutputSuppression;
#[cfg(target_os = "macos")]
use macos_power_monitor::{PowerEvent, PowerMonitor};
pub use macos_virtual_hid::VirtualHidConfig;
use profile_picker::{Picker, PickerEvents, PickerInput};
// Frontends drive the wheel and render it, so its vocabulary is part of the
// runtime's public surface.
pub use profile_picker::{PickerConfig, PickerEvent, PickerRoster};
use recording::{
    RecordingError, RecordingEvent, RecordingWriter, KIND_DEVICE_CONNECTED,
    KIND_DEVICE_DISCONNECTED,
};
use serde_json::json;
use steam_controller_device::{
    masked_serial, ControllerEnumerator, ControllerTransport, DeviceError, DeviceEvent,
    HidDeviceInfo, HidSession, LizardModeHeartbeat, RawHidReport,
};
use steam_controller_discovery::{
    choose_unique_active, inventory_scan_interval as controller_inventory_scan_interval,
    next_stable_scan_interval as next_stable_controller_scan_interval, same_controller_collection,
    ControllerDiscoveryState, MIN_STABLE_SCAN_INTERVAL as MIN_STABLE_CONTROLLER_SCAN_INTERVAL,
};
#[cfg(test)]
use steam_controller_discovery::{
    ControllerProbeSession, ControllerReconcileMetrics,
    MAX_REPORTS_PER_PROBE as MAX_DISCOVERY_REPORTS_PER_CANDIDATE,
    MAX_STABLE_SCAN_INTERVAL as MAX_STABLE_CONTROLLER_SCAN_INTERVAL,
};

// Frontends render status without depending on the device crate directly.
pub use steam_controller_device::masked_serial as mask_serial_for_display;
use steam_controller_protocol::{
    ConnectionState, DecodedReport, PadHapticGain, PadHapticSide, SteamButton, SteamButtons,
    EXTENDED_INPUT_REPORT_ID, EXTENDED_INPUT_REPORT_SIZE, INPUT_REPORT_ID, INPUT_REPORT_SIZE,
};

mod status_log;

pub use status_log::{
    format_status_diagnostics, StatusLogChange, StatusLogLevel, StatusLogRecord,
    StatusLogRecordKind, StatusLogTracker, StatusSnapshotReason, STATUS_SNAPSHOT_INTERVAL,
};

use idle_shutdown::IdleActivityTracker;

const DISCOVERY_INTERVAL: Duration = Duration::from_millis(500);
const OUTPUT_RETRY_INITIAL: Duration = Duration::from_secs(1);
const OUTPUT_RETRY_MAX: Duration = Duration::from_secs(30);
/// The longest a `WillSleep` callback waits for the hardware teardown before
/// acknowledging the sleep anyway. Far above every bounded teardown step, and
/// safely under macOS's ~30-second forced-sleep cap.
#[cfg(target_os = "macos")]
const SLEEP_TEARDOWN_ACK_TIMEOUT: Duration = Duration::from_secs(25);
/// How long after a system wake before hardware discovery may reopen ports.
/// A bridge device's CDC interface can re-enumerate for a couple of seconds after a
/// wake, and touching it mid-setup is the window the sleep suspension exists
/// to stay out of.
const WAKE_SETTLE_DELAY: Duration = Duration::from_secs(2);
const ACTIVE_SLOT_TIMEOUT: Duration = Duration::from_secs(1);
const INPUT_MAILBOX_CAPACITY: usize = 64;
const DESKTOP_INPUT_MAILBOX_CAPACITY: usize = 64;
const DESKTOP_CONTROL_MAILBOX_CAPACITY: usize = 32;
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
const RUNTIME_POLL_INTERVAL: Duration = Duration::from_millis(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const SUPERVISOR_STALL_THRESHOLD: Duration = Duration::from_millis(50);
const RUMBLE_REFRESH_INTERVAL: Duration = Duration::from_millis(40);
const RUMBLE_LEASE_TIMEOUT: Duration = Duration::from_millis(100);
const RUMBLE_RETRY_INTERVAL: Duration = Duration::from_millis(500);
const PAD_FEEDBACK_RETRY_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_IDLE_SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(15);
const AUTOMATIC_SHUTDOWN_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const POWER_OFF_COOLDOWN: Duration = Duration::from_millis(2_500);
const BATTERY_STATUS_FRESHNESS: Duration = Duration::from_secs(30);
const POWER_OFF_BURST_WRITES: u8 = 3;
const POWER_OFF_BURST_INTERVAL: Duration = Duration::from_millis(10);
const MIN_IDLE_SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(1);
pub const MAX_IDLE_SHUTDOWN_TIMEOUT: Duration = Duration::from_hours(24);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SupervisorStall {
    elapsed: Duration,
    phase: &'static str,
    phase_elapsed: Duration,
}

struct SupervisorIterationTimer {
    started: Instant,
    phase_started: Instant,
    phase: &'static str,
    slowest_phase: &'static str,
    slowest_phase_elapsed: Duration,
    reported: bool,
}

impl SupervisorIterationTimer {
    fn new(initial_phase: &'static str) -> Self {
        Self::new_at(initial_phase, Instant::now())
    }

    fn new_at(initial_phase: &'static str, now: Instant) -> Self {
        Self {
            started: now,
            phase_started: now,
            phase: initial_phase,
            slowest_phase: initial_phase,
            slowest_phase_elapsed: Duration::ZERO,
            reported: false,
        }
    }

    fn enter(&mut self, phase: &'static str) {
        self.enter_at(phase, Instant::now());
    }

    fn enter_at(&mut self, phase: &'static str, now: Instant) {
        self.record_current_phase(now);
        self.phase = phase;
        self.phase_started = now;
    }

    fn take_stall_at(&mut self, now: Instant) -> Option<SupervisorStall> {
        if self.reported {
            return None;
        }
        self.record_current_phase(now);
        self.reported = true;
        let elapsed = now.saturating_duration_since(self.started);
        (elapsed >= SUPERVISOR_STALL_THRESHOLD).then_some(SupervisorStall {
            elapsed,
            phase: self.slowest_phase,
            phase_elapsed: self.slowest_phase_elapsed,
        })
    }

    fn record_current_phase(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.phase_started);
        if elapsed > self.slowest_phase_elapsed {
            self.slowest_phase = self.phase;
            self.slowest_phase_elapsed = elapsed;
        }
    }
}

impl Drop for SupervisorIterationTimer {
    fn drop(&mut self) {
        if let Some(stall) = self.take_stall_at(Instant::now()) {
            eprintln!(
                "level=warn event=supervisor_stall elapsed_ms={} phase={} phase_elapsed_ms={}",
                stall.elapsed.as_millis(),
                stall.phase,
                stall.phase_elapsed.as_millis()
            );
        }
    }
}

#[cfg(test)]
mod tests;
