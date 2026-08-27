use std::time::Duration;

use crate::{DeviceEvent, HidDeviceInfo, RawHidReport};

pub const CONTROLLER_RECONNECT_INTERVAL: Duration = Duration::from_millis(500);

/// The next platform operation required by a controller session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerSessionStep {
    Event(DeviceEvent),
    Read { timeout: Duration },
    Wait { duration: Duration },
    Retry,
    Close,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    PendingConnected,
    Connected,
    Disconnected { retry_at: Duration },
    Closing,
    Stopped,
}

/// Target-neutral controller connection and event-ordering state.
///
/// Platform HID code performs reads, reconnect attempts, and handle closure,
/// then records their outcomes here. Supplying elapsed time makes the complete
/// connect/report/disconnect/retry/shutdown policy deterministic in tests.
#[derive(Debug)]
pub struct ControllerSession {
    selected: HidDeviceInfo,
    state: ConnectionState,
}

impl ControllerSession {
    #[must_use]
    pub const fn new(selected: HidDeviceInfo) -> Self {
        Self {
            selected,
            state: ConnectionState::PendingConnected,
        }
    }

    #[must_use]
    pub const fn device_info(&self) -> &HidDeviceInfo {
        &self.selected
    }

    /// Returns the next platform operation for this session.
    #[must_use]
    pub fn next_step(&mut self, now: Duration, timeout: Duration) -> ControllerSessionStep {
        match self.state {
            ConnectionState::PendingConnected => {
                self.state = ConnectionState::Connected;
                ControllerSessionStep::Event(DeviceEvent::Connected(self.selected.clone()))
            }
            ConnectionState::Connected => ControllerSessionStep::Read { timeout },
            ConnectionState::Disconnected { retry_at } if now < retry_at => {
                ControllerSessionStep::Wait {
                    duration: timeout.min(retry_at.saturating_sub(now)),
                }
            }
            ConnectionState::Disconnected { .. } => ControllerSessionStep::Retry,
            ConnectionState::Closing => ControllerSessionStep::Close,
            ConnectionState::Stopped => ControllerSessionStep::Stopped,
        }
    }

    /// Converts a successful native read into the shared report event.
    #[must_use]
    pub fn report(&self, now: Duration, data: Vec<u8>, dropped_reports: u64) -> DeviceEvent {
        DeviceEvent::Report(RawHidReport {
            timestamp: now,
            report_id: data.first().copied().unwrap_or(0),
            data,
            source_device_id: self.selected.id.clone(),
            transport: self.selected.transport.clone(),
            dropped_reports,
        })
    }

    /// Records loss of the native device and schedules the first retry.
    #[must_use]
    pub fn disconnected(&mut self, now: Duration) -> DeviceEvent {
        self.state = ConnectionState::Disconnected {
            retry_at: now.saturating_add(CONTROLLER_RECONNECT_INTERVAL),
        };
        DeviceEvent::Disconnected
    }

    /// Records an unsuccessful reconnect attempt and schedules the next one.
    pub fn retry_failed(&mut self, now: Duration) {
        self.state = ConnectionState::Disconnected {
            retry_at: now.saturating_add(CONTROLLER_RECONNECT_INTERVAL),
        };
    }

    /// Records a successful reconnect and emits the refreshed identity.
    #[must_use]
    pub fn reconnected(&mut self, selected: HidDeviceInfo) -> DeviceEvent {
        self.selected = selected;
        self.state = ConnectionState::Connected;
        DeviceEvent::Connected(self.selected.clone())
    }

    /// Prevents further reads or retries and requests native handle closure.
    pub fn request_shutdown(&mut self) {
        if self.state != ConnectionState::Stopped {
            self.state = ConnectionState::Closing;
        }
    }

    /// Confirms that the platform closed its native handles.
    pub fn closed(&mut self) {
        debug_assert_eq!(self.state, ConnectionState::Closing);
        self.state = ConnectionState::Stopped;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(path: &str) -> HidDeviceInfo {
        HidDeviceInfo {
            id: path.to_owned(),
            path: path.to_owned(),
            vendor_id: crate::PROTEUS_VENDOR_ID,
            product_id: crate::PROTEUS_PRODUCT_ID,
            usage_page: crate::STEAM_USAGE_PAGE,
            usage: crate::STEAM_CONTROLLER_USAGE,
            interface_number: crate::FIRST_PROTEUS_SLOT_INTERFACE,
            serial_number: Some("fixture".to_owned()),
            manufacturer: Some("Valve Software".to_owned()),
            product: Some("Steam Controller Puck".to_owned()),
            transport: "USB".to_owned(),
        }
    }

    #[test]
    fn scripted_lifecycle_orders_connect_report_disconnect_retry_and_shutdown() {
        let initial = device("initial");
        let mut session = ControllerSession::new(initial.clone());
        assert_eq!(
            session.next_step(Duration::ZERO, Duration::from_millis(10)),
            ControllerSessionStep::Event(DeviceEvent::Connected(initial))
        );
        assert_eq!(
            session.next_step(Duration::ZERO, Duration::from_millis(10)),
            ControllerSessionStep::Read {
                timeout: Duration::from_millis(10)
            }
        );

        let data = vec![0x42, 0x01, 0x02];
        assert_eq!(
            session.report(Duration::from_millis(5), data.clone(), 0),
            DeviceEvent::Report(RawHidReport {
                timestamp: Duration::from_millis(5),
                report_id: 0x42,
                data,
                source_device_id: "initial".to_owned(),
                transport: "USB".to_owned(),
                dropped_reports: 0,
            })
        );

        assert_eq!(
            session.disconnected(Duration::from_millis(6)),
            DeviceEvent::Disconnected
        );
        assert_eq!(
            session.next_step(Duration::from_millis(100), Duration::from_secs(1)),
            ControllerSessionStep::Wait {
                duration: Duration::from_millis(406)
            }
        );
        assert_eq!(
            session.next_step(Duration::from_millis(506), Duration::from_secs(1)),
            ControllerSessionStep::Retry
        );

        session.retry_failed(Duration::from_millis(506));
        assert!(matches!(
            session.next_step(Duration::from_millis(700), Duration::from_secs(1)),
            ControllerSessionStep::Wait { .. }
        ));
        assert_eq!(
            session.next_step(Duration::from_millis(1_006), Duration::from_secs(1)),
            ControllerSessionStep::Retry
        );

        let reconnected = device("reconnected");
        assert_eq!(
            session.reconnected(reconnected.clone()),
            DeviceEvent::Connected(reconnected)
        );
        session.request_shutdown();
        assert_eq!(
            session.next_step(Duration::from_secs(2), Duration::from_millis(10)),
            ControllerSessionStep::Close
        );
        session.closed();
        assert_eq!(
            session.next_step(Duration::from_secs(2), Duration::from_millis(10)),
            ControllerSessionStep::Stopped
        );
    }

    #[test]
    fn shutdown_preempts_a_pending_retry() {
        let mut session = ControllerSession::new(device("fixture"));
        let _ = session.next_step(Duration::ZERO, Duration::ZERO);
        let _ = session.disconnected(Duration::ZERO);
        session.request_shutdown();
        assert_eq!(
            session.next_step(Duration::ZERO, Duration::from_secs(1)),
            ControllerSessionStep::Close
        );
    }
}
