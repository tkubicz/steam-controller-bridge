use std::collections::VecDeque;
use std::fs;
use std::io::BufReader;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bridge_output::{GamepadOutput, OutputDiagnostics, OutputError};
use gamepad_state::GamepadState;

use crate::contract::{
    read_json_line, write_json_line, HelperRequest, HelperResponse, HELPER_PROTOCOL_VERSION,
};
use crate::{VirtualHidConfig, VirtualHidError, VirtualHidErrorClass, VirtualHidHelperMetadata};

const RESPONSE_QUEUE_CAPACITY: usize = 64;

enum ReaderEvent {
    Response(HelperResponse),
    Error(VirtualHidError),
    Eof,
}

enum WorkItem {
    Report {
        bytes: Vec<u8>,
        neutral: bool,
        acknowledgement: Option<mpsc::Sender<Result<(), VirtualHidError>>>,
    },
    Shutdown {
        acknowledgement: mpsc::Sender<Result<(), VirtualHidError>>,
        deadline: Instant,
    },
}

enum MailboxReceive {
    Item(WorkItem),
    Timeout,
    Closed,
}

impl WorkItem {
    const fn is_ordinary_report(&self) -> bool {
        matches!(self, Self::Report { neutral: false, .. })
    }
}

struct MailboxState {
    queue: VecDeque<WorkItem>,
    closed: bool,
}

struct Mailbox {
    capacity: usize,
    state: Mutex<MailboxState>,
    available: Condvar,
}

impl Mailbox {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(MailboxState {
                queue: VecDeque::with_capacity(capacity),
                closed: false,
            }),
            available: Condvar::new(),
        }
    }

    fn enqueue_report(
        &self,
        bytes: Vec<u8>,
        neutral: bool,
        acknowledgement: Option<mpsc::Sender<Result<(), VirtualHidError>>>,
    ) -> Result<bool, VirtualHidError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return Err(VirtualHidError::new(
                VirtualHidErrorClass::HelperExited,
                "virtual HID worker is closed",
            ));
        }
        if neutral {
            state.queue.retain(|item| !item.is_ordinary_report());
        } else if let Some(WorkItem::Report {
            bytes: pending,
            neutral: false,
            acknowledgement: None,
        }) = state.queue.back_mut()
        {
            *pending = bytes;
            return Ok(true);
        }
        if state.queue.len() >= self.capacity {
            return Err(VirtualHidError::new(
                VirtualHidErrorClass::QueueOverflow,
                "virtual HID worker queue overflowed",
            ));
        }
        state.queue.push_back(WorkItem::Report {
            bytes,
            neutral,
            acknowledgement,
        });
        self.available.notify_one();
        Ok(false)
    }

    fn enqueue_shutdown(
        &self,
        acknowledgement: mpsc::Sender<Result<(), VirtualHidError>>,
        deadline: Instant,
    ) -> Result<(), VirtualHidError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return Err(VirtualHidError::new(
                VirtualHidErrorClass::HelperExited,
                "virtual HID worker is closed",
            ));
        }
        state.queue.retain(|item| !item.is_ordinary_report());
        state.queue.push_back(WorkItem::Shutdown {
            acknowledgement,
            deadline,
        });
        self.available.notify_one();
        Ok(())
    }

    fn receive_timeout(&self, timeout: Duration) -> MailboxReceive {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.queue.is_empty() && !state.closed {
            let (next, wait) = self
                .available
                .wait_timeout(state, timeout)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            if wait.timed_out() && state.queue.is_empty() && !state.closed {
                return MailboxReceive::Timeout;
            }
        }
        state
            .queue
            .pop_front()
            .map_or(MailboxReceive::Closed, MailboxReceive::Item)
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        for item in state.queue.drain(..) {
            let error = VirtualHidError::new(
                VirtualHidErrorClass::HelperExited,
                "virtual HID worker stopped before applying queued work",
            );
            match item {
                WorkItem::Report {
                    acknowledgement: Some(acknowledgement),
                    ..
                }
                | WorkItem::Shutdown {
                    acknowledgement, ..
                } => {
                    let _ = acknowledgement.send(Err(error));
                }
                WorkItem::Report { .. } => {}
            }
        }
        self.available.notify_all();
    }
}

#[derive(Default)]
struct SharedState {
    diagnostics: OutputDiagnostics,
    failure: Option<VirtualHidError>,
}

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("child guard is populated")
    }

    fn take(mut self) -> Child {
        self.0.take().expect("child guard is populated")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct VirtualHidOutput {
    config: VirtualHidConfig,
    metadata: VirtualHidHelperMetadata,
    mailbox: Arc<Mailbox>,
    state: Arc<Mutex<SharedState>>,
    worker: Option<JoinHandle<()>>,
}

impl VirtualHidOutput {
    /// Starts the exact configured helper and waits for its ready handshake.
    ///
    /// # Errors
    ///
    /// Returns a classified error when configuration validation, process
    /// startup, protocol negotiation, or initial virtual-device creation fails.
    pub fn open(config: VirtualHidConfig) -> Result<Self, VirtualHidError> {
        config.validate()?;
        validate_helper_path(&config.helper_path)?;

        let mut command = Command::new(&config.helper_path);
        if config.dry_run {
            command.arg("--dry-run");
        }
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| {
                VirtualHidError::new(
                    VirtualHidErrorClass::SpawnFailed,
                    format!("cannot start virtual HID helper: {error}"),
                )
            })?;
        let mut child = ChildGuard::new(child);
        let mut stdin = child.child_mut().stdin.take().ok_or_else(|| {
            VirtualHidError::new(
                VirtualHidErrorClass::SpawnFailed,
                "virtual HID helper stdin was not captured",
            )
        })?;
        let stdout = child.child_mut().stdout.take().ok_or_else(|| {
            VirtualHidError::new(
                VirtualHidErrorClass::SpawnFailed,
                "virtual HID helper stdout was not captured",
            )
        })?;
        let (reader_sender, reader_receiver) = mpsc::sync_channel(RESPONSE_QUEUE_CAPACITY);
        let reader = thread::Builder::new()
            .name("virtual-hid-response-reader".to_owned())
            .spawn(move || read_responses(BufReader::new(stdout), &reader_sender))
            .map_err(|error| {
                VirtualHidError::new(VirtualHidErrorClass::SpawnFailed, error.to_string())
            })?;

        if let Err(error) = write_json_line(
            &mut stdin,
            &HelperRequest::Create {
                protocol: HELPER_PROTOCOL_VERSION,
                vendor_id: config.vendor_id,
                product_id: config.product_id,
            },
        ) {
            drop(child);
            let _ = reader.join();
            return Err(error);
        }
        let state = Arc::new(Mutex::new(SharedState::default()));
        let ready = match wait_for_ready(
            &reader_receiver,
            config.startup_timeout,
            config.vendor_id,
            config.product_id,
            &state,
        ) {
            Ok(ready) => ready,
            Err(error) => {
                let error = classify_startup_failure(error, child.child_mut(), config.dry_run);
                drop(child);
                let _ = reader.join();
                return Err(error);
            }
        };

        let mailbox = Arc::new(Mailbox::new(config.queue_capacity));
        let worker_mailbox = Arc::clone(&mailbox);
        let worker_state = Arc::clone(&state);
        let acknowledgement_timeout = config.acknowledgement_timeout;
        let shutdown_timeout = config.shutdown_timeout;
        let worker = thread::Builder::new()
            .name("virtual-hid-output".to_owned())
            .spawn(move || {
                run_worker(
                    child.take(),
                    stdin,
                    &reader_receiver,
                    reader,
                    &worker_mailbox,
                    &worker_state,
                    acknowledgement_timeout,
                    shutdown_timeout,
                );
            })
            .map_err(|error| {
                VirtualHidError::new(VirtualHidErrorClass::SpawnFailed, error.to_string())
            })?;
        Ok(Self {
            config,
            metadata: ready,
            mailbox,
            state,
            worker: Some(worker),
        })
    }

    #[must_use]
    pub fn helper_metadata(&self) -> VirtualHidHelperMetadata {
        self.metadata.clone()
    }

    fn failure(&self) -> Option<VirtualHidError> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failure
            .clone()
    }

    fn latch_failure(&self, error: VirtualHidError) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.failure.get_or_insert(error);
        self.mailbox.close();
    }

    fn map_output_error(error: &VirtualHidError) -> OutputError {
        if error.is_permanent_configuration_failure() {
            OutputError::Configuration(error.to_string())
        } else {
            OutputError::Transport(error.to_string())
        }
    }
}

impl GamepadOutput for VirtualHidOutput {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        if let Some(error) = self.failure() {
            return Err(Self::map_output_error(&error));
        }
        let report = crate::encode_input_report(state)
            .map_err(|error| Self::map_output_error(&error))?
            .to_vec();
        match self.mailbox.enqueue_report(report, false, None) {
            Ok(coalesced) => {
                if coalesced {
                    self.state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .diagnostics
                        .virtual_reports_coalesced += 1;
                }
                Ok(())
            }
            Err(error) => {
                self.latch_failure(error.clone());
                Err(Self::map_output_error(&error))
            }
        }
    }

    fn send_neutral(&mut self) -> Result<(), OutputError> {
        if let Some(error) = self.failure() {
            return Err(Self::map_output_error(&error));
        }
        let (sender, receiver) = mpsc::channel();
        self.mailbox
            .enqueue_report(crate::NEUTRAL_INPUT_REPORT.to_vec(), true, Some(sender))
            .map_err(|error| Self::map_output_error(&error))?;
        if let Ok(result) = receiver.recv_timeout(self.config.acknowledgement_timeout) {
            result.map_err(|error| Self::map_output_error(&error))
        } else {
            let error = VirtualHidError::new(
                VirtualHidErrorClass::AcknowledgementTimeout,
                "virtual HID neutral report acknowledgement timed out",
            );
            self.latch_failure(error.clone());
            Err(Self::map_output_error(&error))
        }
    }

    fn service(&mut self) -> Result<(), OutputError> {
        self.failure()
            .map_or(Ok(()), |error| Err(Self::map_output_error(&error)))
    }

    fn diagnostics(&self) -> OutputDiagnostics {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .diagnostics
    }
}

impl Drop for VirtualHidOutput {
    fn drop(&mut self) {
        if self.worker.is_none() {
            return;
        }
        let deadline = Instant::now() + self.config.shutdown_timeout;
        let (sender, receiver) = mpsc::channel();
        if self.mailbox.enqueue_shutdown(sender, deadline).is_ok() {
            let _ = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        }
        self.mailbox.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn validate_helper_path(path: &std::path::Path) -> Result<(), VirtualHidError> {
    let metadata = fs::metadata(path).map_err(|error| {
        VirtualHidError::new(
            VirtualHidErrorClass::MissingHelper,
            format!("virtual HID helper is unavailable: {error}"),
        )
    })?;
    if metadata.is_file() {
        Ok(())
    } else {
        Err(VirtualHidError::new(
            VirtualHidErrorClass::MissingHelper,
            "virtual HID helper is not a regular file",
        ))
    }
}

fn read_responses(
    mut reader: BufReader<impl std::io::Read>,
    sender: &mpsc::SyncSender<ReaderEvent>,
) {
    loop {
        match read_json_line::<HelperResponse>(&mut reader) {
            Ok(Some(response)) => {
                if sender.send(ReaderEvent::Response(response)).is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = sender.send(ReaderEvent::Eof);
                return;
            }
            Err(error) => {
                let _ = sender.send(ReaderEvent::Error(error));
                return;
            }
        }
    }
}

fn wait_for_ready(
    receiver: &mpsc::Receiver<ReaderEvent>,
    timeout: Duration,
    expected_vendor_id: u16,
    expected_product_id: u16,
    state: &Mutex<SharedState>,
) -> Result<VirtualHidHelperMetadata, VirtualHidError> {
    let deadline = Instant::now() + timeout;
    loop {
        let event = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| {
                VirtualHidError::new(
                    VirtualHidErrorClass::StartupTimeout,
                    "virtual HID helper did not become ready in time",
                )
            })?;
        match event {
            ReaderEvent::Response(HelperResponse::Ready {
                protocol,
                vendor_id,
                product_id,
                dry_run,
                bundle_identifier,
                signing_identifier,
                entitlement_present,
            }) => {
                check_response_protocol(protocol)?;
                if (vendor_id, product_id) != (expected_vendor_id, expected_product_id) {
                    return Err(VirtualHidError::new(
                        VirtualHidErrorClass::ProtocolViolation,
                        format!(
                            "helper activated identity {vendor_id:04x}:{product_id:04x}, expected {expected_vendor_id:04x}:{expected_product_id:04x}"
                        ),
                    ));
                }
                return Ok(VirtualHidHelperMetadata {
                    protocol_version: protocol,
                    vendor_id,
                    product_id,
                    bundle_identifier,
                    signing_identifier,
                    entitlement_present,
                    dry_run,
                });
            }
            ReaderEvent::Response(
                response @ (HelperResponse::SetReport { .. }
                | HelperResponse::GetReport { .. }
                | HelperResponse::Fatal { .. }),
            ) => handle_unsolicited_response(ReaderEvent::Response(response), state)?,
            ReaderEvent::Response(HelperResponse::Applied { .. }) => {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::ProtocolViolation,
                    "helper acknowledged a sequence before ready",
                ));
            }
            ReaderEvent::Error(error) => return Err(error),
            ReaderEvent::Eof => {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::HelperExited,
                    "virtual HID helper exited before ready",
                ));
            }
        }
    }
}

fn classify_startup_failure(
    error: VirtualHidError,
    child: &mut Child,
    dry_run: bool,
) -> VirtualHidError {
    let status = child.try_wait().ok().flatten();
    classify_startup_status(error, status, dry_run)
}

fn classify_startup_status(
    error: VirtualHidError,
    status: Option<std::process::ExitStatus>,
    dry_run: bool,
) -> VirtualHidError {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::process::ExitStatusExt as _;

        if !dry_run
            && error.class() == VirtualHidErrorClass::HelperExited
            && status.is_some_and(|status| status.signal() == Some(9))
        {
            return VirtualHidError::new(
                VirtualHidErrorClass::EntitlementRejected,
                "macOS killed the virtual HID helper before startup; the embedded restricted entitlement is not authorized for this signature",
            );
        }
    }
    let _ = (status, dry_run);
    error
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    mut child: Child,
    mut stdin: ChildStdin,
    responses: &mpsc::Receiver<ReaderEvent>,
    reader: JoinHandle<()>,
    mailbox: &Mailbox,
    state: &Mutex<SharedState>,
    acknowledgement_timeout: Duration,
    shutdown_timeout: Duration,
) {
    let mut sequence = 1_u64;
    let mut process_deadline = Instant::now() + shutdown_timeout;
    loop {
        let item = match mailbox.receive_timeout(Duration::from_millis(25)) {
            MailboxReceive::Item(item) => item,
            MailboxReceive::Timeout => {
                if let Err(error) = service_unsolicited_responses(responses, state) {
                    latch_worker_failure(state, error);
                    break;
                }
                continue;
            }
            MailboxReceive::Closed => break,
        };
        let (request, acknowledgement, shutdown_deadline) = match item {
            WorkItem::Report {
                bytes,
                acknowledgement,
                ..
            } => (
                HelperRequest::InputReport {
                    protocol: HELPER_PROTOCOL_VERSION,
                    sequence,
                    report: bytes,
                },
                acknowledgement,
                None,
            ),
            WorkItem::Shutdown {
                acknowledgement,
                deadline,
            } => (
                HelperRequest::Shutdown {
                    protocol: HELPER_PROTOCOL_VERSION,
                    sequence,
                },
                Some(acknowledgement),
                Some(deadline),
            ),
        };
        let response_timeout = shutdown_deadline.map_or(acknowledgement_timeout, |deadline| {
            deadline.saturating_duration_since(Instant::now())
        });
        let result = write_json_line(&mut stdin, &request)
            .and_then(|()| wait_for_applied(responses, sequence, response_timeout, state));
        if result.is_ok() && shutdown_deadline.is_none() {
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .diagnostics
                .virtual_reports_dispatched += 1;
        }
        if let Some(acknowledgement) = acknowledgement {
            let _ = acknowledgement.send(result.clone());
        }
        if let Err(error) = result {
            latch_worker_failure(state, error);
            break;
        }
        sequence = sequence.wrapping_add(1);
        if let Some(deadline) = shutdown_deadline {
            process_deadline = deadline;
            break;
        }
    }
    drop(stdin);
    let deadline = process_deadline;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    mailbox.close();
    let _ = reader.join();
}

fn latch_worker_failure(state: &Mutex<SharedState>, error: VirtualHidError) {
    let mut shared = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if matches!(
        error.class(),
        VirtualHidErrorClass::ProtocolMismatch | VirtualHidErrorClass::ProtocolViolation
    ) {
        shared.diagnostics.virtual_protocol_failures += 1;
    }
    shared.failure.get_or_insert(error);
}

fn service_unsolicited_responses(
    receiver: &mpsc::Receiver<ReaderEvent>,
    state: &Mutex<SharedState>,
) -> Result<(), VirtualHidError> {
    loop {
        match receiver.try_recv() {
            Ok(event) => handle_unsolicited_response(event, state)?,
            Err(mpsc::TryRecvError::Empty) => return Ok(()),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::HelperExited,
                    "virtual HID helper response channel disconnected",
                ));
            }
        }
    }
}

fn handle_unsolicited_response(
    event: ReaderEvent,
    state: &Mutex<SharedState>,
) -> Result<(), VirtualHidError> {
    match event {
        ReaderEvent::Response(HelperResponse::SetReport {
            protocol, report, ..
        }) => {
            check_response_protocol(protocol)?;
            if report.len() > crate::contract::MAX_RAW_REPORT_LEN {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::ProtocolViolation,
                    "helper sent an oversized set report",
                ));
            }
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .diagnostics
                .virtual_set_reports_received += 1;
            Ok(())
        }
        ReaderEvent::Response(HelperResponse::GetReport {
            protocol, max_size, ..
        }) => {
            check_response_protocol(protocol)?;
            if max_size > crate::contract::MAX_RAW_REPORT_LEN {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::ProtocolViolation,
                    "helper sent an oversized get-report request",
                ));
            }
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .diagnostics
                .virtual_get_reports_received += 1;
            Ok(())
        }
        ReaderEvent::Response(HelperResponse::Fatal {
            protocol,
            class,
            message,
        }) => {
            check_response_protocol(protocol)?;
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .diagnostics
                .virtual_fatal_errors += 1;
            Err(VirtualHidError::new(class, message))
        }
        ReaderEvent::Response(HelperResponse::Applied { .. }) => Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            "helper acknowledged a sequence when none was pending",
        )),
        ReaderEvent::Response(HelperResponse::Ready { .. }) => Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            "helper sent ready more than once",
        )),
        ReaderEvent::Error(error) => Err(error),
        ReaderEvent::Eof => Err(VirtualHidError::new(
            VirtualHidErrorClass::HelperExited,
            "virtual HID helper exited",
        )),
    }
}

fn check_response_protocol(protocol: u16) -> Result<(), VirtualHidError> {
    if protocol == HELPER_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolMismatch,
            format!("helper response uses protocol {protocol}"),
        ))
    }
}

fn wait_for_applied(
    receiver: &mpsc::Receiver<ReaderEvent>,
    sequence: u64,
    timeout: Duration,
    state: &Mutex<SharedState>,
) -> Result<(), VirtualHidError> {
    let deadline = Instant::now() + timeout;
    loop {
        let event = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| {
                VirtualHidError::new(
                    VirtualHidErrorClass::AcknowledgementTimeout,
                    format!("helper did not acknowledge sequence {sequence}"),
                )
            })?;
        match event {
            ReaderEvent::Response(HelperResponse::Applied {
                protocol,
                sequence: applied,
            }) => {
                check_response_protocol(protocol)?;
                if applied == sequence {
                    return Ok(());
                }
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::ProtocolViolation,
                    format!("helper acknowledged sequence {applied}; expected {sequence}"),
                ));
            }
            ReaderEvent::Response(
                response @ (HelperResponse::SetReport { .. }
                | HelperResponse::GetReport { .. }
                | HelperResponse::Fatal { .. }),
            ) => handle_unsolicited_response(ReaderEvent::Response(response), state)?,
            ReaderEvent::Response(HelperResponse::Ready { .. }) => {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::ProtocolViolation,
                    "helper sent ready more than once",
                ));
            }
            ReaderEvent::Error(error) => return Err(error),
            ReaderEvent::Eof => {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::HelperExited,
                    "virtual HID helper exited before acknowledging a report",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_reports_coalesce_and_neutral_supersedes_them() {
        let mailbox = Mailbox::new(2);
        assert!(!mailbox
            .enqueue_report(vec![1; crate::INPUT_REPORT_LEN], false, None)
            .unwrap());
        assert!(mailbox
            .enqueue_report(vec![2; crate::INPUT_REPORT_LEN], false, None)
            .unwrap());
        let (ack, _receiver) = mpsc::channel();
        assert!(!mailbox
            .enqueue_report(crate::NEUTRAL_INPUT_REPORT.to_vec(), true, Some(ack))
            .unwrap());
        let MailboxReceive::Item(WorkItem::Report { bytes, neutral, .. }) =
            mailbox.receive_timeout(Duration::ZERO)
        else {
            panic!("neutral report was not queued");
        };
        assert!(neutral);
        assert_eq!(bytes, crate::NEUTRAL_INPUT_REPORT);
        assert!(matches!(
            mailbox.receive_timeout(Duration::ZERO),
            MailboxReceive::Timeout
        ));
    }

    #[test]
    fn a_full_unserviceable_mailbox_reports_overflow() {
        let mailbox = Mailbox::new(1);
        let (ack, _receiver) = mpsc::channel();
        mailbox
            .enqueue_report(crate::NEUTRAL_INPUT_REPORT.to_vec(), true, Some(ack))
            .unwrap();
        let error = mailbox
            .enqueue_report(vec![1; crate::INPUT_REPORT_LEN], false, None)
            .unwrap_err();
        assert_eq!(error.class(), VirtualHidErrorClass::QueueOverflow);
    }

    #[test]
    fn shutdown_carries_one_end_to_end_deadline() {
        let mailbox = Mailbox::new(1);
        let (acknowledgement, _receiver) = mpsc::channel();
        let deadline = Instant::now() + Duration::from_secs(2);
        mailbox.enqueue_shutdown(acknowledgement, deadline).unwrap();
        let MailboxReceive::Item(WorkItem::Shutdown {
            deadline: queued, ..
        }) = mailbox.receive_timeout(Duration::ZERO)
        else {
            panic!("shutdown was not queued");
        };
        assert_eq!(queued, deadline);
    }

    #[test]
    fn delegate_reports_may_arrive_during_activation_before_ready() {
        let (sender, receiver) = mpsc::channel();
        let state = Mutex::new(SharedState::default());
        sender
            .send(ReaderEvent::Response(HelperResponse::GetReport {
                protocol: HELPER_PROTOCOL_VERSION,
                event_sequence: 1,
                report_type: crate::contract::HidReportType::Feature,
                report_id: 1,
                max_size: 64,
            }))
            .unwrap();
        sender
            .send(ReaderEvent::Response(HelperResponse::Ready {
                protocol: HELPER_PROTOCOL_VERSION,
                vendor_id: crate::DEFAULT_VENDOR_ID,
                product_id: crate::DEFAULT_PRODUCT_ID,
                dry_run: false,
                bundle_identifier: None,
                signing_identifier: None,
                entitlement_present: Some(true),
            }))
            .unwrap();

        let ready = wait_for_ready(
            &receiver,
            Duration::from_secs(1),
            crate::DEFAULT_VENDOR_ID,
            crate::DEFAULT_PRODUCT_ID,
            &state,
        )
        .unwrap();
        assert!(!ready.dry_run);
        assert_eq!(
            state
                .lock()
                .unwrap()
                .diagnostics
                .virtual_get_reports_received,
            1
        );
    }

    #[test]
    fn helper_fatal_is_counted_exactly_once() {
        let state = Mutex::new(SharedState::default());
        let error = handle_unsolicited_response(
            ReaderEvent::Response(HelperResponse::Fatal {
                protocol: HELPER_PROTOCOL_VERSION,
                class: VirtualHidErrorClass::DispatchFailed,
                message: "dispatch failed".to_owned(),
            }),
            &state,
        )
        .unwrap_err();
        latch_worker_failure(&state, error);
        assert_eq!(state.lock().unwrap().diagnostics.virtual_fatal_errors, 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn startup_sigkill_is_a_permanent_entitlement_rejection() {
        use std::os::unix::process::ExitStatusExt as _;

        let error = classify_startup_status(
            VirtualHidError::new(VirtualHidErrorClass::HelperExited, "exited"),
            Some(std::process::ExitStatus::from_raw(9)),
            false,
        );
        assert_eq!(error.class(), VirtualHidErrorClass::EntitlementRejected);
        assert!(error.is_permanent_configuration_failure());
    }
}
