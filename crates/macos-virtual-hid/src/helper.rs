use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use crate::contract::{
    read_json_line, write_json_line, HelperRequest, HelperResponse, HELPER_PROTOCOL_VERSION,
};
use crate::platform;
use crate::{
    VirtualHidError, VirtualHidErrorClass, VirtualHidHelperMetadata, GAMEPAD_REPORT_DESCRIPTOR,
    NEUTRAL_INPUT_REPORT,
};

/// Runs the helper using its command-line arguments and inherited stdio.
///
/// # Errors
///
/// Returns a structured error for invalid arguments, protocol failures, or a
/// virtual-device lifecycle failure.
pub fn run_from_environment() -> Result<(), VirtualHidError> {
    let mut dry_run = false;
    let mut self_test = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--dry-run" if !dry_run => dry_run = true,
            "--self-test" if !self_test => self_test = true,
            _ => {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::InvalidConfiguration,
                    format!("unsupported helper argument: {argument}"),
                ));
            }
        }
    }
    if self_test {
        crate::contract::validate_input_report(&NEUTRAL_INPUT_REPORT)?;
        if GAMEPAD_REPORT_DESCRIPTOR.is_empty() {
            return Err(VirtualHidError::new(
                VirtualHidErrorClass::InvalidConfiguration,
                "virtual HID descriptor is empty",
            ));
        }
        eprintln!("level=info event=self_test_complete contract=xbox360");
        return Ok(());
    }
    run_session(BufReader::new(std::io::stdin()), std::io::stdout(), dry_run)
}

/// Runs one strict helper protocol session over the supplied streams.
///
/// # Errors
///
/// Returns a structured error for malformed or out-of-order traffic, writer
/// failure, or a virtual-device lifecycle failure.
pub fn run_session<R, W>(mut input: R, output: W, dry_run: bool) -> Result<(), VirtualHidError>
where
    R: BufRead,
    W: Write + Send + 'static,
{
    let (response_sender, response_receiver) = mpsc::sync_channel::<HelperResponse>(64);
    // Delegate callbacks must never block behind diagnostic traffic. Fatal
    // callback failures use a separate unbounded lane so they cannot be
    // silently discarded when the bounded event queue is full.
    let (fatal_sender, fatal_receiver) = mpsc::channel::<HelperResponse>();
    let finished = Arc::new(AtomicBool::new(false));
    let writer_finished = Arc::clone(&finished);
    let writer = thread::Builder::new()
        .name("virtual-hid-helper-writer".to_owned())
        .spawn(move || {
            write_responses(
                output,
                &response_receiver,
                &fatal_receiver,
                &writer_finished,
            )
        })
        .map_err(|error| {
            VirtualHidError::new(VirtualHidErrorClass::SpawnFailed, error.to_string())
        })?;

    let result = run_protocol(&mut input, &response_sender, &fatal_sender, dry_run);
    if let Err(error) = &result {
        let _ = fatal_sender.send(HelperResponse::Fatal {
            protocol: HELPER_PROTOCOL_VERSION,
            class: error.class(),
            message: error.message().to_owned(),
        });
    }
    finished.store(true, Ordering::Release);
    drop(response_sender);
    drop(fatal_sender);
    let writer_result = writer.join().map_err(|_| {
        VirtualHidError::new(
            VirtualHidErrorClass::HelperExited,
            "virtual HID helper writer panicked",
        )
    })?;
    result.and(writer_result)
}

fn run_protocol(
    input: &mut impl BufRead,
    responses: &mpsc::SyncSender<HelperResponse>,
    fatal_responses: &mpsc::Sender<HelperResponse>,
    dry_run: bool,
) -> Result<(), VirtualHidError> {
    let Some(first) = read_json_line::<HelperRequest>(input)? else {
        return Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            "helper stdin ended before create",
        ));
    };
    check_protocol(first.protocol())?;
    let HelperRequest::Create {
        vendor_id,
        product_id,
        ..
    } = first
    else {
        return Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            "create must be the first helper command",
        ));
    };

    let mut device = if dry_run {
        None
    } else {
        Some(platform::VirtualDevice::create(
            vendor_id,
            product_id,
            responses.clone(),
            fatal_responses.clone(),
        )?)
    };
    let metadata = device.as_ref().map_or_else(
        VirtualHidHelperMetadata::default,
        platform::VirtualDevice::metadata,
    );
    responses
        .send(HelperResponse::Ready {
            protocol: HELPER_PROTOCOL_VERSION,
            vendor_id,
            product_id,
            dry_run,
            bundle_identifier: metadata.bundle_identifier,
            signing_identifier: metadata.signing_identifier,
            entitlement_present: metadata.entitlement_present,
        })
        .map_err(|_| {
            VirtualHidError::new(
                VirtualHidErrorClass::HelperExited,
                "helper response writer stopped before ready",
            )
        })?;

    let mut expected_sequence = 1_u64;
    while let Some(request) = read_json_line::<HelperRequest>(input)? {
        check_protocol(request.protocol())?;
        match request {
            HelperRequest::Create { .. } => {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::ProtocolViolation,
                    "create may only be sent once",
                ));
            }
            HelperRequest::InputReport {
                sequence, report, ..
            } => {
                check_sequence(sequence, expected_sequence)?;
                crate::contract::validate_input_report(&report)?;
                if let Some(device) = device.as_mut() {
                    device.dispatch(&report)?;
                }
                responses
                    .send(HelperResponse::Applied {
                        protocol: HELPER_PROTOCOL_VERSION,
                        sequence,
                    })
                    .map_err(response_writer_stopped)?;
                expected_sequence = expected_sequence.checked_add(1).ok_or_else(|| {
                    VirtualHidError::new(
                        VirtualHidErrorClass::ProtocolViolation,
                        "helper sequence space was exhausted",
                    )
                })?;
            }
            HelperRequest::Shutdown { sequence, .. } => {
                check_sequence(sequence, expected_sequence)?;
                if let Some(device) = device.as_mut() {
                    device.dispatch(&NEUTRAL_INPUT_REPORT)?;
                    device.shutdown()?;
                }
                responses
                    .send(HelperResponse::Applied {
                        protocol: HELPER_PROTOCOL_VERSION,
                        sequence,
                    })
                    .map_err(response_writer_stopped)?;
                return Ok(());
            }
        }
    }

    if let Some(device) = device.as_mut() {
        let _ = device.dispatch(&NEUTRAL_INPUT_REPORT);
        let _ = device.shutdown();
    }
    Ok(())
}

/// How long the writer waits on an idle channel before re-checking whether the
/// protocol has finished.
const WRITER_IDLE_POLL: Duration = Duration::from_millis(50);

fn write_responses(
    mut output: impl Write,
    responses: &mpsc::Receiver<HelperResponse>,
    fatal_responses: &mpsc::Receiver<HelperResponse>,
    finished: &AtomicBool,
) -> Result<(), VirtualHidError> {
    loop {
        match fatal_responses.try_recv() {
            Ok(response) => {
                write_json_line(&mut output, &response)?;
                return Ok(());
            }
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {}
        }
        match responses.recv_timeout(WRITER_IDLE_POLL) {
            Ok(response) => write_json_line(&mut output, &response)?,
            // A cancellation timeout deliberately leaks the registered IOKit
            // callback owners, and with them their clones of this channel's
            // sender, so a hangup is not guaranteed. The finished flag is the
            // second exit, so the helper still terminates on its own after the
            // queue has been drained.
            Err(mpsc::RecvTimeoutError::Disconnected | mpsc::RecvTimeoutError::Timeout)
                if finished.load(Ordering::Acquire) =>
            {
                // The producer publishes terminal failure before setting
                // `finished`. Re-check its separate lane after waking so the
                // disconnect cannot race past the fatal response.
                if let Ok(response) = fatal_responses.try_recv() {
                    write_json_line(&mut output, &response)?;
                }
                return Ok(());
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(VirtualHidError::new(
                    VirtualHidErrorClass::HelperExited,
                    "helper response channel disconnected before protocol completion",
                ));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn check_protocol(protocol: u16) -> Result<(), VirtualHidError> {
    if protocol == HELPER_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolMismatch,
            format!(
                "helper protocol {protocol} is unsupported; expected {HELPER_PROTOCOL_VERSION}"
            ),
        ))
    }
}

fn check_sequence(actual: u64, expected: u64) -> Result<(), VirtualHidError> {
    if actual == expected && actual != 0 {
        Ok(())
    } else {
        Err(VirtualHidError::new(
            VirtualHidErrorClass::ProtocolViolation,
            format!("expected sequence {expected}, received {actual}"),
        ))
    }
}

fn response_writer_stopped(_: mpsc::SendError<HelperResponse>) -> VirtualHidError {
    VirtualHidError::new(
        VirtualHidErrorClass::HelperExited,
        "helper response writer stopped",
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn dry_run_create_report_shutdown_exchange() {
        let input = concat!(
            "{\"type\":\"create\",\"protocol\":4,\"vendor_id\":1118,\"product_id\":654}\n",
            "{\"type\":\"input_report\",\"protocol\":4,\"sequence\":1,\"report\":[0,20,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}\n",
            "{\"type\":\"shutdown\",\"protocol\":4,\"sequence\":2}\n"
        );
        let output = SharedWriter::default();
        let captured = output.clone();
        run_session(BufReader::new(input.as_bytes()), output, true).unwrap();
        let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        let responses = text
            .lines()
            .map(|line| serde_json::from_str::<HelperResponse>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(
            responses[0],
            HelperResponse::Ready { dry_run: true, .. }
        ));
        assert!(matches!(
            responses[1],
            HelperResponse::Applied { sequence: 1, .. }
        ));
        assert!(matches!(
            responses[2],
            HelperResponse::Applied { sequence: 2, .. }
        ));
    }

    #[test]
    fn wrong_sequence_emits_structured_fatal() {
        let input = concat!(
            "{\"type\":\"create\",\"protocol\":4,\"vendor_id\":1118,\"product_id\":654}\n",
            "{\"type\":\"shutdown\",\"protocol\":4,\"sequence\":2}\n"
        );
        let output = SharedWriter::default();
        let captured = output.clone();
        assert!(run_session(BufReader::new(input.as_bytes()), output, true).is_err());
        let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("\"type\":\"fatal\""));
        assert!(text.contains("\"class\":\"protocol_violation\""));
    }

    #[test]
    fn fatal_response_bypasses_a_full_delegate_queue() {
        let (response_sender, response_receiver) = mpsc::sync_channel(1);
        response_sender
            .send(HelperResponse::GetReport {
                protocol: HELPER_PROTOCOL_VERSION,
                event_sequence: 1,
                report_type: crate::contract::HidReportType::Feature,
                report_id: 1,
                max_size: 64,
            })
            .unwrap();
        let (fatal_sender, fatal_receiver) = mpsc::channel();
        fatal_sender
            .send(HelperResponse::Fatal {
                protocol: HELPER_PROTOCOL_VERSION,
                class: VirtualHidErrorClass::ProtocolViolation,
                message: "callback rejected an invalid report".to_owned(),
            })
            .unwrap();

        let output = SharedWriter::default();
        let captured = output.clone();
        write_responses(
            output,
            &response_receiver,
            &fatal_receiver,
            &AtomicBool::new(false),
        )
        .unwrap();
        let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("\"type\":\"fatal\""));
        assert!(!text.contains("\"type\":\"get_report\""));
    }

    #[test]
    fn zero_duplicate_skipped_and_decreasing_sequences_are_rejected() {
        for (actual, expected) in [(0, 1), (1, 2), (3, 2), (7, 8)] {
            assert!(
                check_sequence(actual, expected).is_err(),
                "{actual} / {expected}"
            );
        }
        assert!(check_sequence(1, 1).is_ok());
    }
}
