use std::io::{BufReader, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::app_center_protocol::{
    encode, read, AppCenterCommand, UpdateOperation, UpdateRequest, UpdateResponse, UpdateResult,
};

const HOST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub(super) struct HostClient {
    output: Arc<Mutex<std::io::Stdout>>,
    responses: Arc<Mutex<mpsc::Receiver<UpdateResponse>>>,
    request_gate: Arc<Mutex<()>>,
    next_request_id: Arc<AtomicU64>,
}

impl HostClient {
    pub(super) fn new(ctx: egui::Context) -> (Self, mpsc::Receiver<AppCenterCommand>) {
        const COMMAND_CAPACITY: usize = 16;
        let (command_sender, commands) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (response_sender, responses) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("app-center-host-reader".to_owned())
            .spawn(move || {
                let mut input = BufReader::new(std::io::stdin());
                while let Ok(Some(command)) = read(&mut input) {
                    let sent = match command {
                        AppCenterCommand::UpdateResponse(response) => {
                            response_sender.send(response).is_ok()
                        }
                        command => match command_sender.try_send(command) {
                            Ok(()) | Err(mpsc::TrySendError::Full(_)) => true,
                            Err(mpsc::TrySendError::Disconnected(_)) => false,
                        },
                    };
                    if !sent {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("app center host reader thread must start");
        (
            Self {
                output: Arc::new(Mutex::new(std::io::stdout())),
                responses: Arc::new(Mutex::new(responses)),
                request_gate: Arc::new(Mutex::new(())),
                next_request_id: Arc::new(AtomicU64::new(1)),
            },
            commands,
        )
    }

    pub(super) fn request(&self, operation: UpdateOperation) -> Result<(), String> {
        let _request = self
            .request_gate
            .lock()
            .map_err(|_| "app window IPC failed")?;
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let encoded = encode(UpdateRequest { id, operation })?;
        {
            let mut output = self.output.lock().map_err(|_| "app window IPC failed")?;
            output
                .write_all(&encoded)
                .and_then(|()| output.flush())
                .map_err(|error| error.to_string())?;
        }
        let deadline = Instant::now() + HOST_RESPONSE_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("app window host response timed out".to_owned());
            }
            let response = self
                .responses
                .lock()
                .map_err(|_| "app window IPC failed")?
                .recv_timeout(remaining)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => {
                        "app window host response timed out".to_owned()
                    }
                    mpsc::RecvTimeoutError::Disconnected => {
                        "app window host response is unavailable".to_owned()
                    }
                })?;
            if response.id != id {
                continue;
            }
            return validate_update_result(operation, response.result);
        }
    }
}

pub(super) fn validate_update_result(
    operation: UpdateOperation,
    result: UpdateResult,
) -> Result<(), String> {
    match (operation, result) {
        (_, UpdateResult::Error { message }) => Err(message),
        (UpdateOperation::SuspendBridge, UpdateResult::Suspended)
        | (UpdateOperation::ResumeBridge, UpdateResult::Resumed)
        | (UpdateOperation::QuitForReplacement, UpdateResult::Quitting) => Ok(()),
        (_, result) => Err(format!(
            "app window host returned an unexpected response: {result:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_must_match_the_requested_operation() {
        assert!(
            validate_update_result(UpdateOperation::SuspendBridge, UpdateResult::Suspended).is_ok()
        );
        assert!(
            validate_update_result(UpdateOperation::SuspendBridge, UpdateResult::Resumed).is_err()
        );
        assert!(validate_update_result(
            UpdateOperation::QuitForReplacement,
            UpdateResult::Error {
                message: "refused".to_owned()
            }
        )
        .is_err());
    }
}
