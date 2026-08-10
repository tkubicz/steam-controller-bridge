use std::io::{BufReader, Write as _};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use bridge_runtime::FirmwareVersion;

use crate::update_protocol::{encode, read, UpdateRequest, UpdateResponse, UPDATE_CENTER_ARGUMENT};

const REQUEST_CAPACITY: usize = 16;

pub struct UpdateHost {
    child: Option<Child>,
    input: Option<ChildStdin>,
    requests: Receiver<UpdateRequest>,
    sender: SyncSender<UpdateRequest>,
    suspended: bool,
    resume_after: bool,
}

impl UpdateHost {
    pub fn new() -> Self {
        let (sender, requests) = mpsc::sync_channel(REQUEST_CAPACITY);
        Self {
            child: None,
            input: None,
            requests,
            sender,
            suspended: false,
            resume_after: false,
        }
    }

    pub fn launch(&mut self, firmware: FirmwareVersion) -> Result<(), String> {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(None) => return Ok(()),
                Ok(Some(_)) => {}
                Err(error) => return Err(format!("cannot inspect Update Center: {error}")),
            }
        }
        self.child = None;
        self.input = None;
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut command = Command::new(executable);
        command
            .arg(UPDATE_CENTER_ARGUMENT)
            .arg("--firmware-version")
            .arg(firmware_argument(firmware))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let Some(input) = child.stdin.take() else {
            cleanup_child(&mut child);
            return Err("Update Center stdin is unavailable".to_owned());
        };
        let Some(output) = child.stdout.take() else {
            cleanup_child(&mut child);
            return Err("Update Center stdout is unavailable".to_owned());
        };
        let sender = self.sender.clone();
        let reader_thread = thread::Builder::new()
            .name("update-center-ipc".to_owned())
            .spawn(move || {
                let mut reader = BufReader::new(output);
                while let Ok(Some(request)) = read(&mut reader) {
                    if sender.try_send(request).is_err() {
                        break;
                    }
                }
            });
        if let Err(error) = reader_thread {
            cleanup_child(&mut child);
            return Err(error.to_string());
        }
        self.input = Some(input);
        self.child = Some(child);
        Ok(())
    }

    pub fn drain(&self) -> impl Iterator<Item = UpdateRequest> + '_ {
        std::iter::from_fn(|| self.requests.try_recv().ok())
    }

    pub fn respond(&mut self, response: &UpdateResponse) -> Result<(), String> {
        let input = self.input.as_mut().ok_or("Update Center is not running")?;
        let encoded = encode(response)?;
        input
            .write_all(&encoded)
            .map_err(|error| error.to_string())?;
        input.flush().map_err(|error| error.to_string())
    }

    pub fn set_suspended(&mut self, resume_after: bool) {
        self.suspended = true;
        self.resume_after = resume_after;
    }

    pub fn clear_suspended(&mut self) -> bool {
        let restart = self.suspended && self.resume_after;
        self.suspended = false;
        self.resume_after = false;
        restart
    }

    pub fn reap(&mut self) -> bool {
        let exited = self
            .child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(Some(_))));
        if exited {
            self.child = None;
            self.input = None;
        }
        exited
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.input = None;
    }
}

fn cleanup_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn firmware_argument(version: FirmwareVersion) -> String {
    match version {
        FirmwareVersion::Reported(revision) => revision.to_string(),
        FirmwareVersion::UnsupportedFormat(_) => "newer".to_owned(),
        FirmwareVersion::Unreported | FirmwareVersion::Malformed | FirmwareVersion::Pending => {
            "unknown".to_owned()
        }
    }
}
