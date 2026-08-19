use std::collections::VecDeque;
use std::fs;
use std::io::{BufReader, BufWriter};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bridge_output::{GamepadOutput, OutputDiagnostics, OutputError, OutputFeedback};
use gamepad_state::GamepadState;

use crate::backend::Backend;
use crate::contract::{
    read_json_line, write_json_line, HelperRequest, HelperResponse, HELPER_PROTOCOL_VERSION,
    INPUT_REPORT_LEN,
};
use crate::{VirtualHidConfig, VirtualHidError, VirtualHidErrorClass, VirtualHidHelperMetadata};

const RESPONSE_QUEUE_CAPACITY: usize = 64;
/// Slack a caller adds to the deadline it handed the worker, so the worker's
/// own precise classification wins the race against the caller giving up.
const ACKNOWLEDGEMENT_GRACE: Duration = Duration::from_millis(100);

enum ReaderEvent {
    Response(HelperResponse),
    Error(VirtualHidError),
    Eof,
}

/// One caller waiting on a queued item, and the single absolute deadline both
/// it and the worker measure against. Sharing the deadline keeps the worker
/// from waiting past the moment its caller has already given up.
struct Acknowledgement {
    sender: mpsc::Sender<Result<(), VirtualHidError>>,
    deadline: Instant,
}

enum WorkItem {
    Report {
        bytes: [u8; INPUT_REPORT_LEN],
        neutral: bool,
        acknowledgement: Option<Acknowledgement>,
    },
    Shutdown {
        acknowledgement: Acknowledgement,
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
        bytes: [u8; INPUT_REPORT_LEN],
        neutral: bool,
        acknowledgement: Option<Acknowledgement>,
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

    fn enqueue_shutdown(&self, acknowledgement: Acknowledgement) -> Result<(), VirtualHidError> {
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
        state
            .queue
            .push_back(WorkItem::Shutdown { acknowledgement });
        self.available.notify_one();
        Ok(())
    }

    /// Waits for one item, reporting `Closed` only for a genuinely closed
    /// mailbox. The predicate is re-checked after every wake, so a spurious
    /// condvar wakeup cannot be mistaken for a closed queue and tear the
    /// helper down underneath a live session.
    fn receive_timeout(&self, timeout: Duration) -> MailboxReceive {
        let deadline = Instant::now() + timeout;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(item) = state.queue.pop_front() {
                return MailboxReceive::Item(item);
            }
            if state.closed {
                return MailboxReceive::Closed;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return MailboxReceive::Timeout;
            }
            state = self
                .available
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
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
                | WorkItem::Shutdown { acknowledgement } => {
                    let _ = acknowledgement.sender.send(Err(error));
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
    last_delegate_sequence: u64,
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
        // serde_json emits one write per JSON token, and a pipe turns each of
        // those into a syscall. Buffering collapses a report to the single
        // write the explicit flush in `write_json_line` already implies.
        let stdin = child.child_mut().stdin.take().ok_or_else(|| {
            VirtualHidError::new(
                VirtualHidErrorClass::SpawnFailed,
                "virtual HID helper stdin was not captured",
            )
        })?;
        let mut stdin = BufWriter::new(stdin);
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
            join_reader(reader_receiver, reader);
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
                join_reader(reader_receiver, reader);
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
                    reader_receiver,
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

    fn backend_send_state(&mut self, state: &GamepadState) -> Result<(), VirtualHidError> {
        if let Some(error) = self.failure() {
            return Err(error);
        }
        let report = crate::encode_input_report(state)?;
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
                Err(error)
            }
        }
    }

    fn backend_send_neutral(&mut self) -> Result<(), VirtualHidError> {
        if let Some(error) = self.failure() {
            return Err(error);
        }
        let (sender, receiver) = mpsc::channel();
        let deadline = Instant::now() + self.config.acknowledgement_timeout;
        self.mailbox.enqueue_report(
            crate::NEUTRAL_INPUT_REPORT,
            true,
            Some(Acknowledgement { sender, deadline }),
        )?;
        let wait = deadline.saturating_duration_since(Instant::now()) + ACKNOWLEDGEMENT_GRACE;
        if let Ok(result) = receiver.recv_timeout(wait) {
            result
        } else {
            let error = VirtualHidError::new(
                VirtualHidErrorClass::AcknowledgementTimeout,
                "virtual HID neutral report acknowledgement timed out",
            );
            self.latch_failure(error.clone());
            Err(error)
        }
    }

    fn backend_service(&mut self) -> Result<(), VirtualHidError> {
        self.failure().map_or(Ok(()), Err)
    }

    fn backend_diagnostics(&self) -> OutputDiagnostics {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .diagnostics
    }

    fn backend_shutdown(&mut self) -> Result<(), VirtualHidError> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        let deadline = Instant::now() + self.config.shutdown_timeout;
        let (sender, receiver) = mpsc::channel();
        let result = match self
            .mailbox
            .enqueue_shutdown(Acknowledgement { sender, deadline })
        {
            Ok(()) => {
                let wait =
                    deadline.saturating_duration_since(Instant::now()) + ACKNOWLEDGEMENT_GRACE;
                receiver.recv_timeout(wait).unwrap_or_else(|_| {
                    Err(VirtualHidError::new(
                        VirtualHidErrorClass::AcknowledgementTimeout,
                        "virtual HID shutdown acknowledgement timed out",
                    ))
                })
            }
            Err(error) => Err(error),
        };
        self.mailbox.close();
        if worker.join().is_err() && result.is_ok() {
            return Err(VirtualHidError::new(
                VirtualHidErrorClass::HelperExited,
                "virtual HID worker panicked during shutdown",
            ));
        }
        result
    }
}

impl Backend for VirtualHidOutput {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), VirtualHidError> {
        self.backend_send_state(state)
    }

    fn send_neutral(&mut self) -> Result<(), VirtualHidError> {
        self.backend_send_neutral()
    }

    fn service(&mut self) -> Result<(), VirtualHidError> {
        self.backend_service()
    }

    fn take_feedback(&mut self) -> Option<OutputFeedback> {
        None
    }

    fn diagnostics(&self) -> OutputDiagnostics {
        self.backend_diagnostics()
    }

    fn shutdown(&mut self) -> Result<(), VirtualHidError> {
        self.backend_shutdown()
    }
}

impl GamepadOutput for VirtualHidOutput {
    fn send_state(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        self.backend_send_state(state).map_err(OutputError::from)
    }

    fn send_neutral(&mut self) -> Result<(), OutputError> {
        self.backend_send_neutral().map_err(OutputError::from)
    }

    fn service(&mut self) -> Result<(), OutputError> {
        self.backend_service().map_err(OutputError::from)
    }

    fn diagnostics(&self) -> OutputDiagnostics {
        self.backend_diagnostics()
    }
}

impl Drop for VirtualHidOutput {
    fn drop(&mut self) {
        let _ = self.backend_shutdown();
    }
}

/// Hangs up the response channel before joining, so a reader thread parked in
/// a blocking send on a full queue cannot deadlock the teardown.
fn join_reader(responses: mpsc::Receiver<ReaderEvent>, reader: JoinHandle<()>) {
    drop(responses);
    let _ = reader.join();
}

fn validate_helper_path(path: &std::path::Path) -> Result<(), VirtualHidError> {
    let metadata = fs::metadata(path).map_err(|error| {
        VirtualHidError::new(
            VirtualHidErrorClass::MissingHelper,
            format!("virtual HID helper is unavailable: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(VirtualHidError::new(
            VirtualHidErrorClass::MissingHelper,
            "virtual HID helper is not a regular file",
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(VirtualHidError::new(
            VirtualHidErrorClass::InvalidConfiguration,
            "virtual HID helper is not executable",
        ));
    }
    Ok(())
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
    let status = if error.class() == VirtualHidErrorClass::HelperExited {
        wait_for_startup_exit(child, Duration::from_millis(100))
    } else {
        child.try_wait().ok().flatten()
    };
    classify_startup_status(error, status, dry_run)
}

fn wait_for_startup_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) | Err(_) => return None,
        }
    }
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
                VirtualHidErrorClass::StartupKilled,
                "the virtual HID helper was killed by SIGKILL before startup; on normal macOS this usually means AMFI rejected its restricted entitlement, but the signal alone cannot prove the cause",
            );
        }
    }
    let _ = (status, dry_run);
    error
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    mut child: Child,
    mut stdin: BufWriter<ChildStdin>,
    responses: mpsc::Receiver<ReaderEvent>,
    reader: JoinHandle<()>,
    mailbox: &Mailbox,
    state: &Mutex<SharedState>,
    acknowledgement_timeout: Duration,
    shutdown_timeout: Duration,
) {
    let mut sequence = 1_u64;
    let mut process_deadline = None;
    loop {
        let item = match mailbox.receive_timeout(Duration::from_millis(25)) {
            MailboxReceive::Item(item) => item,
            MailboxReceive::Timeout => {
                if let Err(error) = service_unsolicited_responses(&responses, state) {
                    latch_worker_failure(state, error);
                    break;
                }
                continue;
            }
            MailboxReceive::Closed => break,
        };
        let (request, acknowledgement, is_shutdown) = match item {
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
                false,
            ),
            WorkItem::Shutdown { acknowledgement } => (
                HelperRequest::Shutdown {
                    protocol: HELPER_PROTOCOL_VERSION,
                    sequence,
                },
                Some(acknowledgement),
                true,
            ),
        };
        // An acknowledged item carries its caller's absolute deadline, so both
        // sides give up at the same instant and the worker's precise error
        // class is what the caller observes.
        let deadline = acknowledgement.as_ref().map(|item| item.deadline);
        let response_timeout = deadline.map_or(acknowledgement_timeout, |deadline| {
            deadline.saturating_duration_since(Instant::now())
        });
        let result = write_json_line(&mut stdin, &request)
            .and_then(|()| wait_for_applied(&responses, sequence, response_timeout, state));
        if result.is_ok() && !is_shutdown {
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .diagnostics
                .virtual_reports_dispatched += 1;
        }
        if let Some(acknowledgement) = acknowledgement {
            let _ = acknowledgement.sender.send(result.clone());
        }
        if let Err(error) = result {
            latch_worker_failure(state, error);
            break;
        }
        sequence = sequence.wrapping_add(1);
        if is_shutdown {
            process_deadline = deadline;
            break;
        }
    }
    drop(stdin);
    // Every other exit reaches here without a shutdown handshake, so the helper
    // gets its own full grace period to observe the closed stdin, release the
    // device, and exit before it is killed.
    let deadline = process_deadline.unwrap_or_else(|| Instant::now() + shutdown_timeout);
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
    join_reader(responses, reader);
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
            protocol,
            event_sequence,
            report,
            ..
        }) => {
            check_response_protocol(protocol)?;
            if report.len() > crate::contract::MAX_RAW_REPORT_LEN {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::ProtocolViolation,
                    "helper sent an oversized set report",
                ));
            }
            let mut state = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            check_delegate_sequence(&mut state, event_sequence)?;
            state.diagnostics.virtual_set_reports_received += 1;
            Ok(())
        }
        ReaderEvent::Response(HelperResponse::GetReport {
            protocol,
            event_sequence,
            max_size,
            ..
        }) => {
            check_response_protocol(protocol)?;
            if max_size > crate::contract::MAX_RAW_REPORT_LEN {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::ProtocolViolation,
                    "helper sent an oversized get-report request",
                ));
            }
            let mut state = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            check_delegate_sequence(&mut state, event_sequence)?;
            state.diagnostics.virtual_get_reports_received += 1;
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

/// Tracks the helper's delegate event sequence.
///
/// A gap is not a protocol failure. Delegate diagnostics travel over a bounded
/// queue that the helper deliberately drops from rather than block an
/// `IOKit` callback, so a burst of host reports legitimately skips numbers.
/// Those are counted, because losing a diagnostic must never disable the
/// gamepad. A repeated or decreasing sequence cannot be explained by a drop,
/// so that remains fatal.
fn check_delegate_sequence(state: &mut SharedState, actual: u64) -> Result<(), VirtualHidError> {
    let expected = state.last_delegate_sequence.checked_add(1).ok_or_else(|| {
        VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            "helper delegate sequence space was exhausted",
        )
    })?;
    if actual < expected {
        return Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            format!("helper delegate sequence went backwards: expected at least {expected}, received {actual}"),
        ));
    }
    state.diagnostics.virtual_delegate_reports_dropped += actual - expected;
    state.last_delegate_sequence = actual;
    Ok(())
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

    fn pending() -> (Acknowledgement, mpsc::Receiver<Result<(), VirtualHidError>>) {
        let (sender, receiver) = mpsc::channel();
        (
            Acknowledgement {
                sender,
                deadline: Instant::now() + Duration::from_secs(2),
            },
            receiver,
        )
    }

    #[test]
    fn ordinary_reports_coalesce_and_neutral_supersedes_them() {
        let mailbox = Mailbox::new(2);
        assert!(!mailbox
            .enqueue_report([1; crate::INPUT_REPORT_LEN], false, None)
            .unwrap());
        assert!(mailbox
            .enqueue_report([2; crate::INPUT_REPORT_LEN], false, None)
            .unwrap());
        let (ack, _receiver) = pending();
        assert!(!mailbox
            .enqueue_report(crate::NEUTRAL_INPUT_REPORT, true, Some(ack))
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
        let (ack, _receiver) = pending();
        mailbox
            .enqueue_report(crate::NEUTRAL_INPUT_REPORT, true, Some(ack))
            .unwrap();
        let error = mailbox
            .enqueue_report([1; crate::INPUT_REPORT_LEN], false, None)
            .unwrap_err();
        assert_eq!(error.class(), VirtualHidErrorClass::QueueOverflow);
    }

    #[test]
    fn acknowledged_work_carries_one_end_to_end_deadline() {
        let mailbox = Mailbox::new(2);
        let (shutdown, _shutdown_receiver) = pending();
        let deadline = shutdown.deadline;
        mailbox.enqueue_shutdown(shutdown).unwrap();
        let MailboxReceive::Item(WorkItem::Shutdown { acknowledgement }) =
            mailbox.receive_timeout(Duration::ZERO)
        else {
            panic!("shutdown was not queued");
        };
        assert_eq!(acknowledgement.deadline, deadline);

        let (neutral, _neutral_receiver) = pending();
        let deadline = neutral.deadline;
        mailbox
            .enqueue_report(crate::NEUTRAL_INPUT_REPORT, true, Some(neutral))
            .unwrap();
        let MailboxReceive::Item(WorkItem::Report {
            acknowledgement: Some(acknowledgement),
            ..
        }) = mailbox.receive_timeout(Duration::ZERO)
        else {
            panic!("neutral was not queued");
        };
        assert_eq!(acknowledgement.deadline, deadline);
    }

    #[test]
    fn a_closed_mailbox_is_distinguishable_from_an_idle_one() {
        let mailbox = Mailbox::new(1);
        assert!(matches!(
            mailbox.receive_timeout(Duration::from_millis(5)),
            MailboxReceive::Timeout
        ));
        mailbox.close();
        assert!(matches!(
            mailbox.receive_timeout(Duration::from_millis(5)),
            MailboxReceive::Closed
        ));
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

    #[test]
    fn a_dropped_delegate_diagnostic_is_counted_but_a_replayed_one_is_fatal() {
        let state = Mutex::new(SharedState::default());
        let deliver = |sequence| {
            handle_unsolicited_response(
                ReaderEvent::Response(HelperResponse::GetReport {
                    protocol: HELPER_PROTOCOL_VERSION,
                    event_sequence: sequence,
                    report_type: crate::contract::HidReportType::Feature,
                    report_id: 1,
                    max_size: 64,
                }),
                &state,
            )
        };
        deliver(1).unwrap();
        deliver(2).unwrap();
        // The helper drops delegate diagnostics under load by design, so the
        // gap is recorded rather than allowed to disable the gamepad.
        deliver(5).unwrap();
        {
            let state = state.lock().unwrap();
            assert_eq!(state.diagnostics.virtual_delegate_reports_dropped, 2);
            assert_eq!(state.diagnostics.virtual_get_reports_received, 3);
        }
        // A sequence that goes backwards cannot be a drop.
        let error = deliver(3).unwrap_err();
        assert_eq!(error.class(), VirtualHidErrorClass::ProtocolViolation);
    }

    #[cfg(unix)]
    #[test]
    fn configured_helper_must_be_executable() {
        use std::os::unix::fs::PermissionsExt as _;

        let helper = tempfile::NamedTempFile::new().unwrap();
        let mut permissions = helper.as_file().metadata().unwrap().permissions();
        permissions.set_mode(0o600);
        helper.as_file().set_permissions(permissions).unwrap();
        let error = validate_helper_path(helper.path()).unwrap_err();
        assert_eq!(error.class(), VirtualHidErrorClass::InvalidConfiguration);

        let mut permissions = helper.as_file().metadata().unwrap().permissions();
        permissions.set_mode(0o700);
        helper.as_file().set_permissions(permissions).unwrap();
        validate_helper_path(helper.path()).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn startup_sigkill_is_a_permanent_but_truthfully_classified_failure() {
        use std::os::unix::process::ExitStatusExt as _;

        let error = classify_startup_status(
            VirtualHidError::new(VirtualHidErrorClass::HelperExited, "exited"),
            Some(std::process::ExitStatus::from_raw(9)),
            false,
        );
        assert_eq!(error.class(), VirtualHidErrorClass::StartupKilled);
        assert!(error.is_permanent_configuration_failure());
    }
}
