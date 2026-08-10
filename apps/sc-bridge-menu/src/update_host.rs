use std::io::{BufReader, Write as _};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use bridge_runtime::FirmwareVersion;

use crate::cli::APP_CENTER_COMMAND;
use crate::update_protocol::{
    encode, read, AppCenterCommand, AppCenterPage, UpdateRequest, UpdateResponse,
};

const REQUEST_CAPACITY: usize = 16;

pub struct AppCenterHost {
    child: Option<Child>,
    input: Option<ChildStdin>,
    requests: Receiver<UpdateRequest>,
    sender: SyncSender<UpdateRequest>,
    suspended: bool,
    resume_after: bool,
    last_firmware_version: Option<String>,
}

impl AppCenterHost {
    pub fn new() -> Self {
        let (sender, requests) = mpsc::sync_channel(REQUEST_CAPACITY);
        Self {
            child: None,
            input: None,
            requests,
            sender,
            suspended: false,
            resume_after: false,
            last_firmware_version: None,
        }
    }

    /// Opens the shared information window, or navigates the existing child.
    /// Returns `true` when an existing child should be brought to the front.
    pub fn launch(
        &mut self,
        page: AppCenterPage,
        firmware: FirmwareVersion,
    ) -> Result<bool, String> {
        let firmware_version = firmware_argument(firmware);
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(None) => {
                    self.send_command(&AppCenterCommand::Navigate {
                        page,
                        firmware_version: firmware_version.clone(),
                    })?;
                    self.last_firmware_version = Some(firmware_version);
                    return Ok(true);
                }
                Ok(Some(_)) => {}
                Err(error) => return Err(format!("cannot inspect app window: {error}")),
            }
        }
        self.child = None;
        self.input = None;
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut command = Command::new(executable);
        command
            .arg(APP_CENTER_COMMAND)
            .arg("--tab")
            .arg(page_argument(page))
            .arg("--firmware-version")
            .arg(&firmware_version)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let Some(input) = child.stdin.take() else {
            cleanup_child(&mut child);
            return Err("app window stdin is unavailable".to_owned());
        };
        let Some(output) = child.stdout.take() else {
            cleanup_child(&mut child);
            return Err("app window stdout is unavailable".to_owned());
        };
        let sender = self.sender.clone();
        let reader_thread = thread::Builder::new()
            .name("app-center-ipc".to_owned())
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
        self.last_firmware_version = Some(firmware_version);
        Ok(false)
    }

    pub fn drain(&self) -> impl Iterator<Item = UpdateRequest> + '_ {
        std::iter::from_fn(|| self.requests.try_recv().ok())
    }

    pub fn respond(&mut self, response: &UpdateResponse) -> Result<(), String> {
        self.send_command(&AppCenterCommand::UpdateResponse(response.clone()))
    }

    pub fn update_firmware(&mut self, firmware: FirmwareVersion) -> Result<(), String> {
        if self.child.is_none() {
            return Ok(());
        }
        let firmware_version = firmware_argument(firmware);
        if self.last_firmware_version.as_deref() == Some(&firmware_version) {
            return Ok(());
        }
        self.send_command(&AppCenterCommand::FirmwareVersion {
            firmware_version: firmware_version.clone(),
        })?;
        self.last_firmware_version = Some(firmware_version);
        Ok(())
    }

    fn send_command(&mut self, command: &AppCenterCommand) -> Result<(), String> {
        let input = self.input.as_mut().ok_or("app window is not running")?;
        let encoded = encode(command)?;
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
            self.last_firmware_version = None;
        }
        exited
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.input = None;
        self.last_firmware_version = None;
    }

    pub fn child(&self) -> Option<&Child> {
        self.child.as_ref()
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

const fn page_argument(page: AppCenterPage) -> &'static str {
    match page {
        AppCenterPage::About => "about",
        AppCenterPage::Changelog => "changelog",
        AppCenterPage::Updates => "updates",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tab_has_a_stable_launch_argument() {
        assert_eq!(page_argument(AppCenterPage::About), "about");
        assert_eq!(page_argument(AppCenterPage::Changelog), "changelog");
        assert_eq!(page_argument(AppCenterPage::Updates), "updates");
    }
}
