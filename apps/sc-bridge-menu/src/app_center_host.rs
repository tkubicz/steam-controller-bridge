use std::io::{BufReader, Write as _};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bridge_runtime::FirmwareInfo;

use crate::app_center_protocol::{
    encode, read, AppCenterCommand, AppCenterPage, FirmwareDetails, UpdateRequest, UpdateResponse,
};
use crate::cli::APP_CENTER_COMMAND;
use crate::line_protocol::read_bounded_line;

const REQUEST_CAPACITY: usize = 16;
const COMMAND_CAPACITY: usize = 16;
const DIAGNOSTIC_CAPACITY: usize = 256;
const MAX_DIAGNOSTIC_LINE_BYTES: usize = 16 * 1024;
const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionRequest {
    pub generation: u64,
    pub request: UpdateRequest,
}

struct AppCenterWriter {
    sender: SyncSender<Vec<u8>>,
    thread: JoinHandle<()>,
}

impl AppCenterWriter {
    fn new(mut input: ChildStdin, diagnostics: SyncSender<String>) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(COMMAND_CAPACITY);
        let thread = thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                if let Err(error) = input.write_all(&message).and_then(|()| input.flush()) {
                    let _ = diagnostics.try_send(format!(
                        "level=warn event=app_center_write_failed error={error:?}"
                    ));
                    return;
                }
            }
        });
        Self { sender, thread }
    }
}

pub struct AppCenterHost {
    child: Option<Child>,
    writer: Option<AppCenterWriter>,
    reader_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    requests: Receiver<SessionRequest>,
    request_sender: SyncSender<SessionRequest>,
    diagnostic_receiver: Receiver<String>,
    diagnostic_sender: SyncSender<String>,
    generation: Option<u64>,
    next_generation: u64,
    suspension_owner: Option<u64>,
    last_firmware: Option<FirmwareDetails>,
}

impl AppCenterHost {
    pub fn new() -> Self {
        let (request_sender, requests) = mpsc::sync_channel(REQUEST_CAPACITY);
        let (diagnostic_sender, diagnostic_receiver) = mpsc::sync_channel(DIAGNOSTIC_CAPACITY);
        Self {
            child: None,
            writer: None,
            reader_thread: None,
            stderr_thread: None,
            requests,
            request_sender,
            diagnostic_receiver,
            diagnostic_sender,
            generation: None,
            next_generation: 1,
            suspension_owner: None,
            last_firmware: None,
        }
    }

    pub fn launch(
        &mut self,
        page: AppCenterPage,
        firmware_available: bool,
        firmware: Option<FirmwareInfo>,
    ) -> Result<bool, String> {
        self.reap();
        if self.suspension_owner.is_some() && self.child.is_none() {
            return Err("recovering the bridge after the previous app window exited".to_owned());
        }
        let firmware = FirmwareDetails::from_output(firmware_available, firmware);
        if self.child.is_some() {
            match self.send_command(&AppCenterCommand::Navigate {
                page,
                firmware: firmware.clone(),
            }) {
                Ok(()) => {
                    self.last_firmware = Some(firmware.clone());
                    return Ok(true);
                }
                Err(error) if self.suspension_owner.is_some() => return Err(error),
                Err(_) => {}
            }
        }
        self.spawn(page, &firmware)?;
        self.last_firmware = Some(firmware);
        Ok(false)
    }

    fn spawn(&mut self, page: AppCenterPage, firmware: &FirmwareDetails) -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut command = Command::new(executable);
        command
            .arg(APP_CENTER_COMMAND)
            .arg("--tab")
            .arg(page.argument())
            .arg("--firmware")
            .arg(firmware.to_string());
        self.spawn_command(&mut command)
    }

    fn spawn_command(&mut self, command: &mut Command) -> Result<(), String> {
        if self.child.is_some() || self.suspension_owner.is_some() {
            return Err("an app window session is already active".to_owned());
        }
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        let Some(input) = child.stdin.take() else {
            cleanup_child(&mut child);
            return Err("app window stdin is unavailable".to_owned());
        };
        let Some(output) = child.stdout.take() else {
            cleanup_child(&mut child);
            return Err("app window stdout is unavailable".to_owned());
        };
        let Some(stderr) = child.stderr.take() else {
            cleanup_child(&mut child);
            return Err("app window stderr is unavailable".to_owned());
        };

        let request_sender = self.request_sender.clone();
        let diagnostics = self.diagnostic_sender.clone();
        let reader_thread = thread::Builder::new()
            .name("app-center-ipc-reader".to_owned())
            .spawn(move || {
                let mut reader = BufReader::new(output);
                loop {
                    match read(&mut reader) {
                        Ok(Some(request)) => {
                            if request_sender
                                .try_send(SessionRequest {
                                    generation,
                                    request,
                                })
                                .is_err()
                            {
                                let _ = diagnostics.try_send(
                                    "level=warn event=app_center_request_queue_full".to_owned(),
                                );
                                return;
                            }
                        }
                        Ok(None) => return,
                        Err(error) => {
                            let _ = diagnostics.try_send(format!(
                                "level=warn event=app_center_protocol_error error={error:?}"
                            ));
                            return;
                        }
                    }
                }
            })
            .map_err(|error| {
                cleanup_child(&mut child);
                error.to_string()
            })?;

        let diagnostics = self.diagnostic_sender.clone();
        let stderr_thread = thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            loop {
                match read_bounded_line(&mut reader, MAX_DIAGNOSTIC_LINE_BYTES) {
                    Ok(Some(line)) => {
                        let _ = diagnostics.try_send(String::from_utf8_lossy(&line).into_owned());
                    }
                    Ok(None) => return,
                    Err(error) => {
                        let _ = diagnostics.try_send(format!(
                            "level=warn event=app_center_stderr_invalid error={error:?}"
                        ));
                        return;
                    }
                }
            }
        });

        self.writer = Some(AppCenterWriter::new(input, self.diagnostic_sender.clone()));
        self.reader_thread = Some(reader_thread);
        self.stderr_thread = Some(stderr_thread);
        self.generation = Some(generation);
        self.child = Some(child);
        Ok(())
    }

    pub fn drain(&self) -> Vec<SessionRequest> {
        let generation = self.generation;
        self.requests
            .try_iter()
            .filter(|request| Some(request.generation) == generation)
            .collect()
    }

    pub fn respond(&mut self, generation: u64, response: &UpdateResponse) -> Result<(), String> {
        if self.generation != Some(generation) {
            return Err("app window session is no longer current".to_owned());
        }
        self.send_command(&AppCenterCommand::UpdateResponse(response.clone()))
    }

    pub fn update_firmware(
        &mut self,
        firmware_available: bool,
        firmware: Option<FirmwareInfo>,
    ) -> Result<(), String> {
        if self.child.is_none() {
            return Ok(());
        }
        let firmware = FirmwareDetails::from_output(firmware_available, firmware);
        if self.last_firmware.as_ref() == Some(&firmware) {
            return Ok(());
        }
        self.send_command(&AppCenterCommand::FirmwareVersion {
            firmware: firmware.clone(),
        })?;
        self.last_firmware = Some(firmware);
        Ok(())
    }

    fn send_command(&mut self, command: &AppCenterCommand) -> Result<(), String> {
        let encoded = encode(command)?;
        let result = self
            .writer
            .as_ref()
            .ok_or("app window is not running")?
            .sender
            .try_send(encoded);
        match result {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.discard_child();
                Err("app window command queue is full".to_owned())
            }
            Err(TrySendError::Disconnected(_)) => {
                self.discard_child();
                Err("app window command pipe is unavailable".to_owned())
            }
        }
    }

    pub fn claim_suspension(&mut self, generation: u64) -> Result<(), String> {
        if self.generation != Some(generation) {
            return Err("app window session ended before suspension completed".to_owned());
        }
        match self.suspension_owner {
            None => {
                self.suspension_owner = Some(generation);
                Ok(())
            }
            Some(owner) if owner == generation => Ok(()),
            Some(_) => Err("another app window owns the bridge suspension".to_owned()),
        }
    }

    pub fn release_suspension(&mut self, generation: u64) -> Result<(), String> {
        match self.suspension_owner {
            Some(owner) if owner == generation => {
                self.suspension_owner = None;
                Ok(())
            }
            None => Ok(()),
            Some(_) => Err("app window does not own the bridge suspension".to_owned()),
        }
    }

    #[must_use]
    pub const fn suspension_recovery_needed(&self) -> bool {
        self.suspension_owner.is_some() && self.child.is_none()
    }

    pub fn complete_suspension_recovery(&mut self) {
        if self.child.is_none() {
            self.suspension_owner = None;
        }
    }

    #[must_use]
    pub const fn firmware_session_active(&self) -> bool {
        self.suspension_owner.is_some()
    }

    pub fn reap(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                let _ = self.diagnostic_sender.try_send(format!(
                    "level=info event=app_center_exited status={status:?}"
                ));
                self.discard_child();
                return true;
            }
            Err(error) => {
                let _ = self.diagnostic_sender.try_send(format!(
                    "level=warn event=app_center_wait_failed error={error:?}"
                ));
                self.discard_child();
                return true;
            }
        }
        if self
            .reader_thread
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            let _ = self
                .diagnostic_sender
                .try_send("level=warn event=app_center_reader_ended".to_owned());
            self.discard_child();
            return true;
        }
        false
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if self.suspension_owner.is_some() {
            return Err("firmware installation is still active".to_owned());
        }
        if self.child.is_none() {
            return Ok(());
        }
        self.send_command(&AppCenterCommand::Close)?;
        let deadline = Instant::now() + GRACEFUL_STOP_TIMEOUT;
        while Instant::now() < deadline {
            if self.reap() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.diagnostic_sender.try_send(
            "level=warn event=app_center_detached reason=graceful_stop_timeout".to_owned(),
        );
        self.detach_child();
        Ok(())
    }

    pub fn drain_diagnostics(&self) -> Vec<String> {
        self.diagnostic_receiver.try_iter().collect()
    }

    pub fn child(&self) -> Option<&Child> {
        self.child.as_ref()
    }

    fn discard_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            cleanup_child(&mut child);
        }
        if let Some(writer) = self.writer.take() {
            drop(writer.sender);
            let _ = writer.thread.join();
        }
        if let Some(thread) = self.reader_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
        self.generation = None;
        self.last_firmware = None;
        while self.requests.try_recv().is_ok() {}
    }

    fn detach_child(&mut self) {
        drop(self.child.take());
        if let Some(writer) = self.writer.take() {
            drop(writer.sender);
            drop(writer.thread);
        }
        drop(self.reader_thread.take());
        drop(self.stderr_thread.take());
        self.generation = None;
        self.last_firmware = None;
    }
}

impl Drop for AppCenterHost {
    fn drop(&mut self) {
        self.discard_child();
    }
}

fn cleanup_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_center_protocol::{UpdateOperation, UpdateRequest};

    fn request_line(id: u64) -> String {
        String::from_utf8(
            encode(UpdateRequest {
                id,
                operation: UpdateOperation::SuspendBridge,
            })
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_real_child_request_is_tagged_with_its_generation() {
        let mut host = AppCenterHost::new();
        let line = request_line(9);
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '%s' \"$1\"; read _", "app-center-test", &line]);
        host.spawn_command(&mut command).unwrap();
        let generation = host.generation.unwrap();

        let request = (0..100)
            .find_map(|_| {
                let request = host.drain().into_iter().next();
                if request.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                request
            })
            .expect("child request");
        assert_eq!(request.generation, generation);
        assert_eq!(request.request.id, 9);
        host.stop().unwrap();
    }

    #[test]
    fn an_exited_generation_cannot_deliver_a_stale_request() {
        let mut host = AppCenterHost::new();
        let line = request_line(3);
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '%s' \"$1\"", "app-center-test", &line]);
        host.spawn_command(&mut command).unwrap();
        host.child.as_mut().unwrap().wait().unwrap();
        assert!(host.reap());
        assert!(host.drain().is_empty());
    }

    #[test]
    fn suspension_ownership_survives_child_exit_until_recovery() {
        let mut host = AppCenterHost::new();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "read _"]);
        host.spawn_command(&mut command).unwrap();
        let generation = host.generation.unwrap();
        host.claim_suspension(generation).unwrap();
        host.discard_child();

        assert!(host.suspension_recovery_needed());
        assert!(host.stop().is_err());
        host.complete_suspension_recovery();
        assert!(!host.suspension_recovery_needed());
    }

    #[test]
    fn stop_requests_a_graceful_child_close() {
        let marker =
            std::env::temp_dir().join(format!("app-center-graceful-stop-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let mut host = AppCenterHost::new();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "IFS= read -r line; case \"$line\" in *close*) : > \"$1\";; esac",
            "app-center-test",
        ]);
        command.arg(&marker);
        host.spawn_command(&mut command).unwrap();

        host.stop().unwrap();

        assert!(marker.exists());
        assert!(host.child.is_none());
        let _ = std::fs::remove_file(marker);
    }

    #[test]
    fn a_full_request_queue_retires_the_unreadable_child() {
        let mut host = AppCenterHost::new();
        let lines = (0..=REQUEST_CAPACITY)
            .map(|id| request_line(u64::try_from(id).unwrap()))
            .collect::<String>();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "printf '%s' \"$1\"; read _",
            "app-center-test",
            &lines,
        ]);
        host.spawn_command(&mut command).unwrap();

        let retired = (0..100).any(|_| {
            if host.reap() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
            false
        });

        assert!(retired, "host did not observe the failed reader");
        assert!(host.child.is_none());
        assert!(host.drain().is_empty());
        assert!(host
            .drain_diagnostics()
            .iter()
            .any(|line| line.contains("app_center_request_queue_full")));
    }
}
