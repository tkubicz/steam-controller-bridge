//! Active Steam Controller source discovery shared by runtimes and tools.
//!
//! A Puck exposes several sibling HID collections, but only one carries input.
//! This crate owns the scan, retained-session, probe, and unique-selection
//! policy without pulling bridge orchestration into diagnostic applications.

use std::time::{Duration, Instant};

use steam_controller_device::{
    masked_serial, ControllerEnumerator, DeviceError, DeviceEvent, HidDeviceInfo, HidSession,
    RawHidReport,
};
use steam_controller_protocol::{
    DecodedReport, SteamControllerDecoder, EXTENDED_INPUT_REPORT_ID, INPUT_REPORT_ID,
};

pub const EMPTY_SCAN_INTERVAL: Duration = Duration::from_millis(500);
pub const MIN_STABLE_SCAN_INTERVAL: Duration = Duration::from_secs(2);
pub const MAX_STABLE_SCAN_INTERVAL: Duration = Duration::from_secs(10);
pub const MAX_REPORTS_PER_PROBE: usize = 4;

/// The narrow polling surface discovery needs from an opened collection.
pub trait ControllerProbeSession {
    /// Polls without blocking during candidate probing.
    ///
    /// # Errors
    ///
    /// Returns a display-ready error when the candidate cannot be polled.
    fn poll_for_discovery(&mut self, timeout: Duration) -> Result<Option<DeviceEvent>, String>;
}

impl ControllerProbeSession for HidSession {
    fn poll_for_discovery(&mut self, timeout: Duration) -> Result<Option<DeviceEvent>, String> {
        self.poll(timeout).map_err(|error| error.to_string())
    }
}

/// One retained opened candidate.
pub struct ControllerCandidate<S> {
    enumeration_index: usize,
    info: HidDeviceInfo,
    session: S,
}

impl<S> ControllerCandidate<S> {
    #[must_use]
    pub const fn enumeration_index(&self) -> usize {
        self.enumeration_index
    }

    #[must_use]
    pub const fn info(&self) -> &HidDeviceInfo {
        &self.info
    }

    #[must_use]
    pub const fn session(&self) -> &S {
        &self.session
    }

    #[must_use]
    pub fn into_parts(self) -> (HidDeviceInfo, S) {
        (self.info, self.session)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ControllerReconcileMetrics {
    pub opened: usize,
    pub reused: usize,
    pub removed: usize,
    pub failures: usize,
}

pub struct ControllerProbe {
    pub active_indices: Vec<usize>,
    pub failures: Vec<String>,
}

/// Retains inactive candidate sessions and reconciles them against HID scans.
///
/// Keeping these sessions open avoids repeatedly creating native reader
/// threads and retained IOHID report buffers while waiting for input.
pub struct ControllerDiscoveryState<S> {
    candidates: Vec<ControllerCandidate<S>>,
    next_scan: Instant,
    stable_scan_interval: Duration,
    supported_devices_seen: bool,
    open_failures: Vec<String>,
    scan_error: Option<String>,
}

impl<S> Default for ControllerDiscoveryState<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> ControllerDiscoveryState<S> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            candidates: Vec::new(),
            next_scan: Instant::now(),
            stable_scan_interval: MIN_STABLE_SCAN_INTERVAL,
            supported_devices_seen: false,
            open_failures: Vec::new(),
            scan_error: None,
        }
    }

    #[must_use]
    pub fn scan_due(&self) -> bool {
        Instant::now() >= self.next_scan
    }

    pub fn refresh(
        &mut self,
        discovered: Result<Vec<(usize, HidDeviceInfo)>, String>,
        mut open: impl FnMut(usize, &HidDeviceInfo) -> Result<S, String>,
    ) -> ControllerReconcileMetrics {
        let Ok(discovered) = discovered else {
            self.scan_error = discovered.err();
            self.stable_scan_interval = MIN_STABLE_SCAN_INTERVAL;
            self.next_scan = Instant::now()
                + inventory_scan_interval(!self.candidates.is_empty(), self.stable_scan_interval);
            return ControllerReconcileMetrics::default();
        };

        self.scan_error = None;
        self.supported_devices_seen = !discovered.is_empty();
        self.open_failures.clear();

        let old_count = self.candidates.len();
        let mut existing: Vec<_> = self.candidates.drain(..).map(Some).collect();
        let mut reconciled = Vec::with_capacity(discovered.len());
        let mut metrics = ControllerReconcileMetrics::default();

        for (enumeration_index, info) in discovered {
            let existing_candidate = existing.iter_mut().find(|candidate| {
                candidate
                    .as_ref()
                    .is_some_and(|candidate| same_controller_collection(&candidate.info, &info))
            });
            if let Some(mut candidate) = existing_candidate.and_then(Option::take) {
                candidate.enumeration_index = enumeration_index;
                candidate.info = info;
                reconciled.push(candidate);
                metrics.reused += 1;
                continue;
            }

            match open(enumeration_index, &info) {
                Ok(session) => {
                    reconciled.push(ControllerCandidate {
                        enumeration_index,
                        info,
                        session,
                    });
                    metrics.opened += 1;
                }
                Err(error) => {
                    self.open_failures.push(error);
                    metrics.failures += 1;
                }
            }
        }

        metrics.removed = old_count.saturating_sub(metrics.reused);
        self.candidates = reconciled;
        let inventory_changed = metrics.opened > 0 || metrics.removed > 0 || metrics.failures > 0;
        self.stable_scan_interval = if inventory_changed {
            MIN_STABLE_SCAN_INTERVAL
        } else {
            next_stable_scan_interval(self.stable_scan_interval)
        };
        self.next_scan = Instant::now()
            + inventory_scan_interval(!self.candidates.is_empty(), self.stable_scan_interval);
        metrics
    }

    pub fn clear(&mut self) {
        self.candidates.clear();
        self.next_scan = Instant::now();
        self.stable_scan_interval = MIN_STABLE_SCAN_INTERVAL;
        self.supported_devices_seen = false;
        self.open_failures.clear();
        self.scan_error = None;
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    #[must_use]
    pub const fn supported_devices_seen(&self) -> bool {
        self.supported_devices_seen
    }

    #[must_use]
    pub fn scan_error(&self) -> Option<&str> {
        self.scan_error.as_deref()
    }

    #[must_use]
    pub fn current_errors(&self, probe_failures: &[String]) -> Option<String> {
        let errors = self
            .scan_error
            .iter()
            .chain(&self.open_failures)
            .chain(probe_failures)
            .map(String::as_str)
            .collect::<Vec<_>>();
        (!errors.is_empty()).then(|| errors.join("; "))
    }

    #[must_use]
    pub fn candidate(&self, index: usize) -> &ControllerCandidate<S> {
        &self.candidates[index]
    }

    /// Resolves filtered candidates back to the stable global HID indices a
    /// user can pass to command-line tools.
    ///
    /// # Errors
    ///
    /// Returns an error without mutating any candidate if one identity is no
    /// longer present in the supplied inventory.
    pub fn resolve_global_indices(&mut self, devices: &[HidDeviceInfo]) -> Result<(), String> {
        let resolved = self
            .candidates
            .iter()
            .map(|candidate| {
                devices
                    .iter()
                    .position(|info| same_controller_collection(&candidate.info, info))
                    .ok_or_else(|| {
                        format!(
                            "cannot resolve the global index for {}",
                            controller_source_identity(&candidate.info)
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (candidate, index) in self.candidates.iter_mut().zip(resolved) {
            candidate.enumeration_index = index;
        }
        Ok(())
    }

    #[must_use]
    pub fn select(&mut self, index: usize) -> ControllerCandidate<S> {
        let selected = self.candidates.swap_remove(index);
        self.clear();
        selected
    }

    #[must_use]
    pub const fn stable_scan_interval(&self) -> Duration {
        self.stable_scan_interval
    }
}

impl<S: ControllerProbeSession> ControllerDiscoveryState<S> {
    pub fn probe(&mut self) -> ControllerProbe {
        let mut decoder = SteamControllerDecoder::new();
        let mut active_indices = Vec::new();
        let mut failures = Vec::new();
        for (index, candidate) in self.candidates.iter_mut().enumerate() {
            for _ in 0..MAX_REPORTS_PER_PROBE {
                match candidate.session.poll_for_discovery(Duration::ZERO) {
                    Ok(Some(DeviceEvent::Report(report)))
                        if is_valid_controller_state(&mut decoder, &report) =>
                    {
                        active_indices.push(index);
                        break;
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(error) => {
                        failures.push(format!(
                            "{}: {error}",
                            controller_source_identity(&candidate.info)
                        ));
                        break;
                    }
                }
            }
        }
        ControllerProbe {
            active_indices,
            failures,
        }
    }
}

#[must_use]
pub const fn inventory_scan_interval(
    has_open_candidates: bool,
    stable_scan_interval: Duration,
) -> Duration {
    if has_open_candidates {
        stable_scan_interval
    } else {
        EMPTY_SCAN_INTERVAL
    }
}

#[must_use]
pub fn next_stable_scan_interval(current: Duration) -> Duration {
    current.saturating_mul(2).min(MAX_STABLE_SCAN_INTERVAL)
}

#[must_use]
pub fn same_controller_collection(left: &HidDeviceInfo, right: &HidDeviceInfo) -> bool {
    let same_stable_serial = left
        .serial_number
        .as_deref()
        .filter(|value| !value.is_empty())
        .zip(
            right
                .serial_number
                .as_deref()
                .filter(|value| !value.is_empty()),
        )
        .is_some_and(|(left, right)| left == right);
    (left.path == right.path || same_stable_serial)
        && left.vendor_id == right.vendor_id
        && left.product_id == right.product_id
        && left.usage_page == right.usage_page
        && left.usage == right.usage
        && left.interface_number == right.interface_number
}

/// Chooses the only active candidate, if there is one.
///
/// # Errors
///
/// Returns every active candidate index when more than one source is active.
pub fn choose_unique_active(active_indices: &[usize]) -> Result<Option<usize>, Vec<usize>> {
    match active_indices {
        [] => Ok(None),
        [selected] => Ok(Some(*selected)),
        multiple => Err(multiple.to_vec()),
    }
}

fn is_valid_controller_state(decoder: &mut SteamControllerDecoder, report: &RawHidReport) -> bool {
    matches!(report.report_id, INPUT_REPORT_ID | EXTENDED_INPUT_REPORT_ID)
        && matches!(
            decoder.decode(report.report_id, &report.data),
            Ok(DecodedReport::ControllerState(_))
        )
}

fn controller_source_identity(info: &HidDeviceInfo) -> String {
    let transport = info
        .controller_transport()
        .map_or_else(|| "Unknown".to_owned(), |value| value.to_string());
    format!(
        "{transport} product {:?} serial {} interface {}",
        info.product.as_deref().unwrap_or("<unknown>"),
        masked_serial(info.serial_number.as_deref()),
        info.interface_number
    )
}

fn ownership_guidance(error: &DeviceError) -> String {
    format!(
        "{error}. Fully quit Steam and other controller tools; if Steam's ipcserver remains, \
         stop its LaunchAgent manually"
    )
}

/// Why one discovery attempt did not produce a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerSearch {
    NoController,
    Backend(String),
    CannotOpen(String),
    NoInputYet,
    Ambiguous(usize),
}

impl std::fmt::Display for ControllerSearch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoController => {
                write!(formatter, "Waiting for a Steam Controller 2 connection")
            }
            Self::Backend(detail) => {
                write!(
                    formatter,
                    "Cannot enumerate Steam Controller input: {detail}"
                )
            }
            Self::CannotOpen(detail) => write!(
                formatter,
                "Steam Controller input found, but no collection can be opened: {detail}"
            ),
            Self::NoInputYet => write!(
                formatter,
                "Steam Controller input found; waiting for valid controller state"
            ),
            Self::Ambiguous(count) => write!(
                formatter,
                "{count} Steam Controller sources are active at once; disconnect all but one"
            ),
        }
    }
}

/// Finds the unique supported collection that is producing controller state.
pub struct ActiveControllerFinder {
    enumerator: ControllerEnumerator,
    discovery: ControllerDiscoveryState<HidSession>,
}

impl ActiveControllerFinder {
    /// Builds the reusable HID context.
    ///
    /// # Errors
    ///
    /// Returns the native error when HID initialization fails.
    pub fn new() -> Result<Self, DeviceError> {
        Ok(Self {
            enumerator: ControllerEnumerator::new()?,
            discovery: ControllerDiscoveryState::new(),
        })
    }

    /// Releases every retained candidate session while preserving the native
    /// enumeration context for a later scan on the same thread.
    pub fn clear_candidates(&mut self) {
        self.discovery.clear();
    }

    /// Probes retained candidates and refreshes their inventory only when due.
    ///
    /// # Errors
    ///
    /// Returns a displayable search state when no unique active source exists.
    pub fn find(&mut self) -> Result<(HidDeviceInfo, HidSession), ControllerSearch> {
        self.find_with_index()
            .map(|(_, info, session)| (info, session))
    }

    /// Finds the active source while retaining its path-sorted global HID
    /// index for diagnostics that need to report the exact selected
    /// collection.
    ///
    /// # Errors
    ///
    /// Returns a displayable search state when no unique active source exists.
    pub fn find_with_index(
        &mut self,
    ) -> Result<(usize, HidDeviceInfo, HidSession), ControllerSearch> {
        if self.discovery.scan_due() {
            let discovered = self
                .enumerator
                .enumerate()
                .map(|devices| devices.into_iter().enumerate().collect::<Vec<_>>())
                .map_err(|error| error.to_string());
            let enumerator = &self.enumerator;
            self.discovery.refresh(discovered, |_, info| {
                enumerator.open(info).map_err(|error| {
                    format!(
                        "{}: {}",
                        controller_source_identity(info),
                        ownership_guidance(&error)
                    )
                })
            });
        }

        if self.discovery.is_empty() {
            return Err(if let Some(detail) = self.discovery.scan_error() {
                ControllerSearch::Backend(detail.to_owned())
            } else if self.discovery.supported_devices_seen() {
                ControllerSearch::CannotOpen(
                    self.discovery
                        .current_errors(&[])
                        .unwrap_or_else(|| "no detail available".to_owned()),
                )
            } else {
                ControllerSearch::NoController
            });
        }

        let probe = self.discovery.probe();
        match choose_unique_active(&probe.active_indices) {
            Ok(Some(selected)) => {
                let enumeration_index = self.discovery.candidate(selected).enumeration_index();
                let (info, session) = self.discovery.select(selected).into_parts();
                Ok((enumeration_index, info, session))
            }
            Ok(None) => Err(match self.discovery.current_errors(&probe.failures) {
                Some(detail) => ControllerSearch::CannotOpen(detail),
                None => ControllerSearch::NoInputYet,
            }),
            Err(active) => Err(ControllerSearch::Ambiguous(active.len())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(report_id: u8, data: Vec<u8>) -> RawHidReport {
        RawHidReport {
            timestamp: Duration::ZERO,
            report_id,
            data,
            source_device_id: "slot".to_owned(),
            transport: "USB".to_owned(),
            dropped_reports: 0,
        }
    }

    #[test]
    fn active_source_selection_requires_exactly_one_candidate() {
        assert_eq!(choose_unique_active(&[]), Ok(None));
        assert_eq!(choose_unique_active(&[3]), Ok(Some(3)));
        assert_eq!(choose_unique_active(&[1, 2]), Err(vec![1, 2]));
    }

    #[test]
    fn inventory_backoff_is_bounded() {
        assert_eq!(
            inventory_scan_interval(false, MAX_STABLE_SCAN_INTERVAL),
            EMPTY_SCAN_INTERVAL
        );
        assert_eq!(
            inventory_scan_interval(true, MIN_STABLE_SCAN_INTERVAL),
            MIN_STABLE_SCAN_INTERVAL
        );
        assert_eq!(
            next_stable_scan_interval(MIN_STABLE_SCAN_INTERVAL),
            Duration::from_secs(4)
        );
        assert_eq!(
            next_stable_scan_interval(Duration::from_secs(8)),
            MAX_STABLE_SCAN_INTERVAL
        );
    }

    #[test]
    fn search_states_are_distinct() {
        let states = [
            ControllerSearch::NoController,
            ControllerSearch::Backend("IOKit unavailable".to_owned()),
            ControllerSearch::CannotOpen("held by Steam".to_owned()),
            ControllerSearch::NoInputYet,
            ControllerSearch::Ambiguous(2),
        ];
        let rendered: Vec<String> = states.iter().map(ToString::to_string).collect();
        for (index, text) in rendered.iter().enumerate() {
            assert!(!text.is_empty());
            for other in &rendered[index + 1..] {
                assert_ne!(text, other, "two search states read the same");
            }
        }
        assert!(rendered[1].contains("IOKit unavailable"));
        assert!(rendered[2].contains("held by Steam"));
        assert!(rendered[4].contains("2 Steam Controller sources"));
    }

    #[test]
    fn only_complete_controller_states_mark_a_candidate_active() {
        let mut decoder = SteamControllerDecoder::new();
        let mut state = vec![0; steam_controller_protocol::INPUT_REPORT_SIZE];
        state[0] = INPUT_REPORT_ID;
        assert!(is_valid_controller_state(
            &mut decoder,
            &report(INPUT_REPORT_ID, state)
        ));

        let mut battery = vec![0; 15];
        battery[0] = steam_controller_protocol::BATTERY_REPORT_ID;
        assert!(!is_valid_controller_state(
            &mut decoder,
            &report(steam_controller_protocol::BATTERY_REPORT_ID, battery)
        ));
        assert!(!is_valid_controller_state(
            &mut decoder,
            &report(INPUT_REPORT_ID, vec![INPUT_REPORT_ID])
        ));
    }
}
