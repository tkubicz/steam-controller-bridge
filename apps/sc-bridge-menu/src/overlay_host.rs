//! Parent-side lifecycle for the profile-wheel overlay process.
//!
//! There is one overlay process per wheel and none at rest: `start` is called
//! when a hold reaches [`profile_picker::PickerEvent::Preparing`] — halfway
//! through, so the window and GL context are ready by the time the wheel is
//! wanted — and `stop` kills the process on every close. A window on the
//! game's Space is not free to the compositor, and the wheel is up for a few
//! seconds at a time, so it does not get to exist the rest of the time.

use std::io::{BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::cli::PROFILE_OVERLAY_COMMAND;
use crate::line_protocol::read_bounded_line;
use crate::overlay_protocol::{OverlayEnvelope, OverlayMessage};

const MAX_DIAGNOSTIC_LINE_BYTES: usize = 16 * 1024;

/// An overlay binary that cannot even start must not be re-executed on every
/// hold of a determined user.
const RELAUNCH_BACKOFF: Duration = Duration::from_secs(5);

/// Lines waiting for the child to read them. The wheel produces messages at
/// human rate, so this is only reachable with a child wedged before its stdin
/// loop — the case the writer thread exists to keep off the main thread.
const WRITER_QUEUE_CAPACITY: usize = 64;

/// Overlay diagnostics are sparse, but the child is still a separate process
/// and must not be able to grow the menu app's memory without bound.
const DIAGNOSTIC_QUEUE_CAPACITY: usize = 256;

/// Owns the pipe to the child so the main thread never blocks on it.
///
/// A pipe write blocks once the kernel buffer fills, and the child only starts
/// reading after its window and GL context exist. A child wedged in that setup
/// would otherwise freeze the whole menu app inside `write_all`.
struct OverlayWriter {
    sender: SyncSender<String>,
    thread: JoinHandle<()>,
}

impl OverlayWriter {
    fn new(mut stdin: std::process::ChildStdin, diagnostics: SyncSender<String>) -> Self {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<String>(WRITER_QUEUE_CAPACITY);
        let thread = std::thread::spawn(move || {
            while let Ok(line) = receiver.recv() {
                if let Err(error) = stdin
                    .write_all(line.as_bytes())
                    .and_then(|()| stdin.flush())
                {
                    let _ = diagnostics.try_send(format!(
                        "level=warn event=profile_overlay_write_failed error={error:?}"
                    ));
                    return;
                }
            }
            // The sender is gone: the host dropped this child. Returning drops
            // stdin, which is the child's cue to exit if it is somehow still
            // alive after the kill.
        });
        Self { sender, thread }
    }
}

pub struct OverlayHost {
    child: Option<Child>,
    writer: Option<OverlayWriter>,
    stderr_thread: Option<JoinHandle<()>>,
    /// Replayed to a freshly started child so it knows what to draw.
    roster: Option<OverlayMessage>,
    /// Whether the wheel was open when the child last went away.
    open: Option<OverlayMessage>,
    next_launch: Option<Instant>,
    diagnostic_sender: SyncSender<String>,
    diagnostic_receiver: Receiver<String>,
}

impl Default for OverlayHost {
    fn default() -> Self {
        let (diagnostic_sender, diagnostic_receiver) =
            std::sync::mpsc::sync_channel(DIAGNOSTIC_QUEUE_CAPACITY);
        Self {
            child: None,
            writer: None,
            stderr_thread: None,
            roster: None,
            open: None,
            next_launch: None,
            diagnostic_sender,
            diagnostic_receiver,
        }
    }
}

impl OverlayHost {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Remembers the profiles to draw and forwards them if a child is running.
    pub fn set_roster(&mut self, names: Vec<String>, active: Option<usize>, sectors: usize) {
        let message = OverlayMessage::Roster {
            names,
            active,
            sectors_per_page: sectors,
        };
        if self.roster.as_ref() == Some(&message) {
            return;
        }
        self.roster = Some(message.clone());
        let had_child = self.child.is_some();
        if !self.send(message) && had_child && self.open.is_some() {
            self.start();
        }
    }

    /// Starts the overlay if it is not already running.
    ///
    /// Called at `Preparing` and again as a safety net at `Opened`, so a child
    /// that crashed mid-hold is replaced rather than mourned. Only a failure of
    /// `spawn` itself backs off: a binary that cannot start will not start a
    /// moment later either, while a child that merely died deserves its retry.
    pub fn start(&mut self) {
        if self.reap_if_exited() {
            return;
        }
        if self.next_launch.is_some_and(|at| Instant::now() < at) {
            return;
        }
        match self.spawn() {
            Ok(()) => {
                self.next_launch = None;
                self.record_diagnostic("level=info event=profile_overlay_started");
                // A child that replaced a crashed one has to be told what the
                // world looks like before it can draw anything.
                if let Some(roster) = self.roster.clone() {
                    if !self.send(roster) {
                        return;
                    }
                }
                if let Some(open) = self.open.clone() {
                    let _ = self.send(open);
                }
            }
            Err(error) => {
                self.next_launch = Some(Instant::now() + RELAUNCH_BACKOFF);
                self.record_diagnostic(format!(
                    "level=warn event=profile_overlay_start_failed error={error:?}"
                ));
            }
        }
    }

    /// Stops the overlay. Idempotent.
    pub fn stop(&mut self) {
        self.open = None;
        if self.kill_child() {
            self.record_diagnostic("level=info event=profile_overlay_stopped");
        }
    }

    /// Shows the wheel, or moves the highlight while it is up.
    pub fn show(&mut self, selected: usize, page: usize) {
        let message = OverlayMessage::Open { selected, page };
        self.open = Some(message.clone());
        let had_child = self.child.is_some();
        if !self.send(message) && had_child {
            self.start();
        }
    }

    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.child.is_some()
    }

    /// Removes all diagnostics currently waiting to be written by the menu
    /// app's single log writer.
    pub fn drain_diagnostics(&self) -> Vec<String> {
        self.diagnostic_receiver.try_iter().collect()
    }

    fn record_diagnostic(&self, line: impl Into<String>) {
        let _ = self.diagnostic_sender.try_send(line.into());
    }

    fn send(&mut self, message: OverlayMessage) -> bool {
        let Some(writer) = self.writer.as_ref() else {
            return false;
        };
        let line = match OverlayEnvelope::new(message).to_line() {
            Ok(line) => line,
            Err(error) => {
                self.record_diagnostic(format!(
                    "level=warn event=overlay_message_unserializable error={error:?}"
                ));
                return false;
            }
        };
        match writer.sender.try_send(line) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                // A live child with a full queue is wedged. Retaining it would
                // make every later `start` return early and permanently lose
                // the cached latest state, so force a fresh process.
                self.record_diagnostic("level=warn event=profile_overlay_queue_full");
                self.discard_child();
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                // The writer hit a broken pipe, so the overlay is gone. Drop it
                // here; the caller restarts it when a wheel is still open.
                self.discard_child();
                false
            }
        }
    }

    /// Drops a child that has already exited. Returns whether one is still alive.
    fn reap_if_exited(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                self.record_diagnostic(format!(
                    "level=warn event=profile_overlay_exited status={status:?}"
                ));
                self.discard_child();
                false
            }
            Err(error) => {
                self.record_diagnostic(format!(
                    "level=warn event=profile_overlay_wait_failed error={error:?}"
                ));
                self.discard_child();
                false
            }
        }
    }

    fn discard_child(&mut self) {
        self.kill_child();
    }

    /// Kills and reaps the child, if any. Returns whether one existed.
    ///
    /// The kill is what makes the teardown deterministic: it unblocks a writer
    /// stuck in a full pipe (the write fails once the read end is gone), and it
    /// leaves no process behind on quit. The writer thread is joined so a
    /// spawn/kill cycle per hold cannot accumulate threads.
    fn kill_child(&mut self) -> bool {
        let child = self.child.take();
        let existed = child.is_some();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(writer) = self.writer.take() {
            drop(writer.sender);
            let _ = writer.thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
        existed
    }

    fn spawn(&mut self) -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut command = Command::new(executable);
        command.arg(PROFILE_OVERLAY_COMMAND);
        self.spawn_command(&mut command)
    }

    fn spawn_command(&mut self, command: &mut Command) -> Result<(), String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            // Read stderr back into the parent so StatusLogger remains the only
            // process writing and rotating the application log.
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        self.writer = child
            .stdin
            .take()
            .map(|stdin| OverlayWriter::new(stdin, self.diagnostic_sender.clone()));
        self.stderr_thread = child.stderr.take().map(|stderr| {
            let diagnostics = self.diagnostic_sender.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                loop {
                    match read_bounded_line(&mut reader, MAX_DIAGNOSTIC_LINE_BYTES) {
                        Ok(Some(line)) => {
                            let _ =
                                diagnostics.try_send(String::from_utf8_lossy(&line).into_owned());
                        }
                        Ok(None) | Err(_) => return,
                    }
                }
            })
        });
        self.child = Some(child);
        Ok(())
    }
}

impl Drop for OverlayHost {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_with_no_child_swallows_every_command() {
        // The menu app calls these on a schedule, so they must be safe before
        // the overlay is started and after it has been stopped.
        let mut host = OverlayHost::new();
        assert!(!host.is_running());
        host.set_roster(vec!["Default".to_owned()], Some(0), 8);
        host.show(0, 0);
        host.show(1, 0);
        host.stop();
        assert!(!host.is_running());
    }

    #[test]
    fn an_unchanged_roster_is_not_resent() {
        let mut host = OverlayHost::new();
        host.set_roster(vec!["Default".to_owned()], Some(0), 8);
        let first = host.roster.clone();
        host.set_roster(vec!["Default".to_owned()], Some(0), 8);
        assert_eq!(host.roster, first);
        host.set_roster(vec!["Default".to_owned()], Some(1), 8);
        assert_ne!(host.roster, first);
    }

    #[test]
    fn the_wheel_state_is_remembered_so_a_replacement_child_can_be_reseeded() {
        let mut host = OverlayHost::new();
        host.show(2, 1);
        assert_eq!(
            host.open,
            Some(OverlayMessage::Open {
                selected: 2,
                page: 1
            })
        );
        host.show(3, 1);
        assert_eq!(
            host.open,
            Some(OverlayMessage::Open {
                selected: 3,
                page: 1
            })
        );
        // Stopping is how the wheel closes now, and it must not leave state
        // behind that would make a later child open a wheel nobody asked for.
        host.stop();
        assert!(host.open.is_none());
    }

    #[test]
    fn a_failed_start_backs_off_instead_of_retrying_immediately() {
        let mut host = OverlayHost::new();
        host.next_launch = Some(Instant::now() + RELAUNCH_BACKOFF);
        host.start();
        assert!(!host.is_running(), "the backoff must suppress the launch");
    }

    #[test]
    fn discarding_a_dead_child_does_not_eat_the_next_launch() {
        // Regression: a crashed child used to arm the failure backoff, so the
        // `Opened` safety net two seconds later refused to spawn a replacement
        // and the wheel stayed blank for that hold and the next.
        let mut host = OverlayHost::new();
        host.discard_child();
        assert!(
            host.next_launch.is_none(),
            "only a failed spawn may back the host off"
        );
    }

    #[test]
    fn a_saturated_writer_is_discarded_instead_of_stranding_cached_state() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel(1);
        sender.send("already queued".to_owned()).unwrap();
        let mut host = OverlayHost::new();
        host.writer = Some(OverlayWriter {
            sender,
            thread: std::thread::spawn(|| {}),
        });

        assert!(!host.send(OverlayMessage::Open {
            selected: 1,
            page: 0,
        }));
        assert!(host.writer.is_none());
        assert!(!host.is_running());
    }

    #[test]
    fn repeated_child_cycles_reap_processes_and_join_writers() {
        let mut host = OverlayHost::new();
        for index in 0..32 {
            let mut command = Command::new("/bin/cat");
            host.spawn_command(&mut command).unwrap();
            host.set_roster(vec![format!("Profile {index}")], Some(0), 8);
            host.show(0, 0);
            host.stop();
            assert!(!host.is_running());
            assert!(host.writer.is_none());
        }
    }

    #[test]
    fn child_stderr_and_host_lifecycle_events_are_collected() {
        let mut host = OverlayHost::new();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo 'level=info event=overlay_test_child' >&2"]);
        host.spawn_command(&mut command).unwrap();
        assert!(host.child.as_mut().unwrap().wait().unwrap().success());
        assert!(!host.reap_if_exited());

        let diagnostics = host.drain_diagnostics();
        assert!(diagnostics
            .iter()
            .any(|line| line == "level=info event=overlay_test_child"));
        assert!(diagnostics
            .iter()
            .any(|line| line.contains("event=profile_overlay_exited")));
        assert!(host.stderr_thread.is_none());
    }
}
