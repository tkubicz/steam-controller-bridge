//! Parent-side lifecycle for the profile-wheel overlay process.
//!
//! The overlay is started as soon as a controller is present rather than when
//! the wheel opens, because creating a window and a GL context takes longer
//! than the user expects between letting go of Quick Access and seeing the
//! wheel. It is stopped again when the controller goes away, so an idle machine
//! carries no extra process.

use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use crate::overlay_protocol::{OverlayEnvelope, OverlayMessage, OVERLAY_ARGUMENT};

/// A crash-looping overlay must not be relaunched on every status poll.
const RELAUNCH_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct OverlayHost {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    /// Replayed to a freshly started child so it knows what to draw.
    roster: Option<OverlayMessage>,
    /// Whether the wheel was open when the child last went away.
    open: Option<OverlayMessage>,
    next_launch: Option<Instant>,
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
        self.send(message);
    }

    /// Starts the overlay if it is not already running.
    ///
    /// Safe to call on every status poll: it is a no-op while the child is
    /// alive, and it backs off after a failure so a broken overlay cannot spin.
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
                eprintln!("level=info event=profile_overlay_started");
                // A child that replaced a crashed one has to be told what the
                // world looks like before it can draw anything.
                if let Some(roster) = self.roster.clone() {
                    self.send(roster);
                }
                if let Some(open) = self.open.clone() {
                    self.send(open);
                }
            }
            Err(error) => {
                self.next_launch = Some(Instant::now() + RELAUNCH_BACKOFF);
                eprintln!("level=warn event=profile_overlay_start_failed error={error:?}");
            }
        }
    }

    /// Stops the overlay. Idempotent.
    pub fn stop(&mut self) {
        // Dropping stdin is the child's cue to exit, but the kill makes the
        // teardown deterministic and leaves no process behind on quit.
        self.stdin = None;
        self.open = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("level=info event=profile_overlay_stopped");
        }
    }

    /// Shows the wheel, or moves the highlight while it is up.
    pub fn show(&mut self, selected: usize, page: usize) {
        let message = OverlayMessage::Open { selected, page };
        self.open = Some(message.clone());
        self.send(message);
    }

    pub fn select(&mut self, selected: usize, page: usize) {
        self.open = Some(OverlayMessage::Open { selected, page });
        self.send(OverlayMessage::Select { selected, page });
    }

    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.child.is_some()
    }

    fn send(&mut self, message: OverlayMessage) {
        let Some(stdin) = self.stdin.as_mut() else {
            return;
        };
        let line = match OverlayEnvelope::new(message).to_line() {
            Ok(line) => line,
            Err(error) => {
                eprintln!("level=warn event=overlay_message_unserializable error={error:?}");
                return;
            }
        };
        if let Err(error) = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
        {
            // The pipe broke, so the overlay is gone. Drop it here and let the
            // next `start` bring a replacement up.
            eprintln!("level=warn event=profile_overlay_write_failed error={error:?}");
            self.discard_child();
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
                eprintln!("level=warn event=profile_overlay_exited status={status:?}");
                self.discard_child();
                false
            }
            Err(error) => {
                eprintln!("level=warn event=profile_overlay_wait_failed error={error:?}");
                self.discard_child();
                false
            }
        }
    }

    fn discard_child(&mut self) {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.next_launch = Some(Instant::now() + RELAUNCH_BACKOFF);
    }

    fn spawn(&mut self) -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut child = Command::new(executable)
            .arg(OVERLAY_ARGUMENT)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            // stderr is inherited so the overlay's structured log lines land in
            // the same place as the menu app's.
            .spawn()
            .map_err(|error| error.to_string())?;
        self.stdin = child.stdin.take();
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
        host.select(1, 0);
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
        host.select(3, 1);
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
}
