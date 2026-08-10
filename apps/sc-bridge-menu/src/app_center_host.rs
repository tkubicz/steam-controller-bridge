use std::io::{BufReader, Write as _};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use bridge_runtime::FirmwareVersion;

use crate::app_center_protocol::{
    encode, read, AppCenterCommand, AppCenterPage, UpdateRequest, UpdateResponse,
};
use crate::cli::APP_CENTER_COMMAND;

const REQUEST_CAPACITY: usize = 16;

pub struct AppCenterHost {
    child: Option<Child>,
    input: Option<ChildStdin>,
    requests: Receiver<UpdateRequest>,
    sender: SyncSender<UpdateRequest>,
    suspended: bool,
    resume_after: bool,
    last_firmware_version: Option<String>,
    /// A child went away and its exit has not been reported yet. Relaunching
    /// consumes the exit that [`Self::reap`] would otherwise observe, and a
    /// child that owned the bridge suspension must not leave it stopped.
    lost_child: bool,
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
            lost_child: false,
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
                    let navigated = self.send_command(&AppCenterCommand::Navigate {
                        page,
                        firmware_version: firmware_version.clone(),
                    });
                    if navigated.is_ok() {
                        self.last_firmware_version = Some(firmware_version);
                        return Ok(true);
                    }
                    // The child is running but unreachable, so it can no longer
                    // show a window. Replace it instead of reporting a failure
                    // the user cannot act on.
                }
                Ok(Some(_)) => {}
                Err(error) => return Err(format!("cannot inspect app window: {error}")),
            }
            self.discard_child();
        }
        self.spawn(page, &firmware_version)?;
        self.last_firmware_version = Some(firmware_version);
        Ok(false)
    }

    /// Kills the current child and remembers that the menu still owes it a
    /// [`Self::reap`]. Relaunching would otherwise consume the exit that
    /// releases a bridge suspension the lost child still owned.
    fn discard_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            cleanup_child(&mut child);
        }
        self.input = None;
        self.last_firmware_version = None;
        self.lost_child = true;
    }

    fn spawn(&mut self, page: AppCenterPage, firmware_version: &str) -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut command = Command::new(executable);
        command
            .arg(APP_CENTER_COMMAND)
            .arg("--tab")
            .arg(page.argument())
            .arg("--firmware-version")
            .arg(firmware_version)
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
        Ok(())
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

    /// Reports a child that went away since the last call, whether it exited on
    /// its own or had to be replaced by [`Self::launch`].
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
        exited | std::mem::take(&mut self.lost_child)
    }

    /// Ends the session for good. Shutdown owns the bridge from here, so unlike
    /// [`Self::discard_child`] this owes the menu no restart.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            cleanup_child(&mut child);
        }
        self.input = None;
        self.last_firmware_version = None;
        self.lost_child = false;
    }

    pub fn child(&self) -> Option<&Child> {
        self.child.as_ref()
    }
}

fn cleanup_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// The argument the child reads its hero firmware badge from.
fn firmware_argument(version: FirmwareVersion) -> String {
    match version {
        FirmwareVersion::Reported(revision) => revision.to_string(),
        FirmwareVersion::UnsupportedFormat(_) => "newer".to_owned(),
        FirmwareVersion::Unreported | FirmwareVersion::Malformed | FirmwareVersion::Pending => {
            "unknown".to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_child_still_owes_exactly_one_reap() {
        let mut host = AppCenterHost::new();
        host.set_suspended(true);
        host.discard_child();

        assert!(host.reap(), "a discarded child must report its exit");
        assert!(
            host.clear_suspended(),
            "a lost child must release the bridge suspension"
        );
        assert!(!host.reap(), "the same exit must not be reported twice");
    }

    #[test]
    fn shutdown_owes_no_restart() {
        let mut host = AppCenterHost::new();
        host.discard_child();
        host.stop();

        assert!(!host.reap());
    }
}
