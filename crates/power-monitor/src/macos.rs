use std::ffi::c_void;
use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::sync::{mpsc, Arc, Barrier};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use objc2_core_foundation::{kCFRunLoopCommonModes, CFRetained, CFRunLoop};
use objc2_io_kit::{
    io_connect_t, io_object_t, io_service_t,
    kIOMessageCanSystemSleep as K_IO_MESSAGE_CAN_SYSTEM_SLEEP,
    kIOMessageSystemHasPoweredOn as K_IO_MESSAGE_SYSTEM_HAS_POWERED_ON,
    kIOMessageSystemWillSleep as K_IO_MESSAGE_SYSTEM_WILL_SLEEP, kIOReturnSuccess,
    IOAllowPowerChange, IODeregisterForSystemPower, IONotificationPort, IONotificationPortRef,
    IORegisterForSystemPower, IOServiceClose,
};

use crate::{PowerEvent, PowerMonitorError};

const SLEEP_TEARDOWN_DEADLINE: Duration = Duration::from_secs(25);

/// Owns the `IOKit` notification port and the run-loop thread that services it.
pub struct PowerMonitor {
    run_loop: Option<SendableRunLoop>,
    thread: Option<JoinHandle<()>>,
}

impl PowerMonitor {
    /// Registers a callback for committed sleep and completed wake events.
    ///
    /// The callback runs on the monitor thread. A `WillSleep` callback may
    /// block while hardware is closed: `IOKit` does not receive its required
    /// acknowledgement until the callback returns.
    ///
    /// # Errors
    ///
    /// Returns an error if the monitor thread cannot start or `IOKit` refuses
    /// to register the system-power notification source.
    pub fn new(
        handler: impl FnMut(PowerEvent) + Send + 'static,
    ) -> Result<Self, PowerMonitorError> {
        let (sender, receiver) = mpsc::channel();
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = Arc::clone(&barrier);
        let thread = std::thread::Builder::new()
            .name("power-monitor".to_owned())
            .spawn(move || run_loop_thread(Box::new(handler), &sender, &worker_barrier))
            .map_err(|error| PowerMonitorError::new(error.to_string()))?;

        let run_loop = match receiver.recv() {
            Ok(Ok(run_loop)) => run_loop,
            Ok(Err(error)) => {
                let _ = thread.join();
                return Err(error);
            }
            Err(error) => {
                let _ = thread.join();
                return Err(PowerMonitorError::new(format!(
                    "power-monitor initialization ended early: {error}"
                )));
            }
        };
        barrier.wait();
        Ok(Self {
            run_loop: Some(run_loop),
            thread: Some(thread),
        })
    }

    #[must_use]
    pub const fn is_live(&self) -> bool {
        true
    }
}

impl Drop for PowerMonitor {
    fn drop(&mut self) {
        let run_loop = self.run_loop.take();
        if let Some(thread) = self.thread.take() {
            if let Some(run_loop) = &run_loop {
                // `CFRunLoopStop` only stops a loop that is already running. A
                // drop can land in the instant between the startup barrier and
                // the monitor thread entering its run loop, where a single stop
                // would be lost and the join below would wait forever - so keep
                // stopping until the thread has actually exited.
                while !thread.is_finished() {
                    run_loop.0.stop();
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            let _ = thread.join();
        }
    }
}

/// A retained `CFRunLoop` pointer that may be stopped by the owner thread.
struct SendableRunLoop(CFRetained<CFRunLoop>);
// SAFETY: Core Foundation documents `CFRunLoopStop` as callable from another
// thread. No other operation is exposed through this wrapper.
unsafe impl Send for SendableRunLoop {}

struct ThreadState {
    handler: Box<dyn FnMut(PowerEvent) + Send>,
    root_port: io_connect_t,
}

fn run_loop_thread(
    handler: Box<dyn FnMut(PowerEvent) + Send>,
    sender: &mpsc::Sender<Result<SendableRunLoop, PowerMonitorError>>,
    barrier: &Barrier,
) {
    let Some(run_loop) = CFRunLoop::current() else {
        let _ = sender.send(Err(PowerMonitorError::new(
            "Core Foundation returned no run loop for the monitor thread",
        )));
        return;
    };
    let state = Box::new(ThreadState {
        handler,
        root_port: 0,
    });
    let state = Box::into_raw(state);
    let mut notification_port = ptr::null_mut();
    let mut notifier = 0;
    // SAFETY: `state` remains allocated until after the run loop stops;
    // `notification_port` and `notifier` are valid out-pointers. The callback
    // is not serviced until its source is added to the run loop below.
    let root_port = unsafe {
        IORegisterForSystemPower(
            state.cast(),
            &raw mut notification_port,
            Some(system_power_event),
            &raw mut notifier,
        )
    };
    if root_port == 0 {
        // SAFETY: Registration failed, so IOKit cannot reference `state`.
        unsafe {
            drop(Box::from_raw(state));
        }
        let _ = sender.send(Err(PowerMonitorError::new(
            "IORegisterForSystemPower returned no root power port",
        )));
        return;
    }
    // SAFETY: `state` is exclusively owned by this run-loop thread and the
    // callback it serially invokes.
    unsafe { (*state).root_port = root_port };

    // SAFETY: Registration initialized this notification-port pointer. The
    // returned source receives an additional retain from the bindings and is
    // removed and dropped before the port is destroyed.
    let Some(source) = (unsafe { IONotificationPort::run_loop_source(notification_port) }) else {
        cleanup_registration(notification_port, notifier, root_port, state);
        let _ = sender.send(Err(PowerMonitorError::new(
            "system-power notification port returned no run-loop source",
        )));
        return;
    };
    // SAFETY: This imported Core Foundation constant is immutable for the
    // lifetime of the process.
    let Some(mode) = (unsafe { kCFRunLoopCommonModes }) else {
        drop(source);
        cleanup_registration(notification_port, notifier, root_port, state);
        let _ = sender.send(Err(PowerMonitorError::new(
            "Core Foundation returned no common run-loop mode",
        )));
        return;
    };

    if sender.send(Ok(SendableRunLoop(run_loop.clone()))).is_err() {
        // The owner went away during initialization; continue through normal
        // cleanup without ever exposing the callback source.
        drop(source);
        cleanup_registration(notification_port, notifier, root_port, state);
        return;
    }
    barrier.wait();

    run_loop.add_source(Some(&source), Some(mode));
    CFRunLoop::run();
    run_loop.remove_source(Some(&source), Some(mode));
    drop(source);
    cleanup_registration(notification_port, notifier, root_port, state);
}

fn cleanup_registration(
    notification_port: IONotificationPortRef,
    mut notifier: io_object_t,
    root_port: io_connect_t,
    state: *mut ThreadState,
) {
    // SAFETY: These are the successfully registered objects. Their source was
    // either never added to a run loop or has already been removed, so no
    // callback can race this cleanup.
    unsafe {
        let _ = IODeregisterForSystemPower(&raw mut notifier);
    }
    let _ = IOServiceClose(root_port);
    // SAFETY: Deregistration and connection close have made the notification
    // port and callback context unreachable from IOKit.
    unsafe {
        IONotificationPort::destroy(notification_port);
        drop(Box::from_raw(state));
    }
}

unsafe extern "C-unwind" fn system_power_event(
    context: *mut c_void,
    _service: io_service_t,
    message_type: u32,
    message_argument: *mut c_void,
) {
    // SAFETY: IOKit invokes this only while `run_loop_thread` owns the boxed
    // `ThreadState`, and the notification source serializes callbacks.
    let state = unsafe { &mut *context.cast::<ThreadState>() };
    match message_type {
        K_IO_MESSAGE_CAN_SYSTEM_SLEEP => allow_power_change(state.root_port, message_argument),
        K_IO_MESSAGE_SYSTEM_WILL_SLEEP => {
            invoke_handler(
                state,
                PowerEvent::WillSleep {
                    deadline: Instant::now() + SLEEP_TEARDOWN_DEADLINE,
                },
                "will_sleep",
            );
            allow_power_change(state.root_port, message_argument);
        }
        K_IO_MESSAGE_SYSTEM_HAS_POWERED_ON => {
            invoke_handler(state, PowerEvent::DidWake, "did_wake");
        }
        _ => {}
    }
}

fn invoke_handler(state: &mut ThreadState, event: PowerEvent, phase: &str) {
    if panic::catch_unwind(AssertUnwindSafe(|| (state.handler)(event))).is_err() {
        eprintln!("level=error event=system_power_callback_panicked phase={phase}");
    }
}

fn allow_power_change(root_port: io_connect_t, message_argument: *mut c_void) {
    let result = IOAllowPowerChange(root_port, message_argument as isize);
    if result != kIOReturnSuccess {
        eprintln!(
            "level=warn event=system_power_ack_failed result=0x{:08x}",
            result.cast_unsigned()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_and_teardown_are_owned_by_the_safe_wrapper() {
        let monitor = PowerMonitor::new(|_| {}).expect("register system-power notifications");
        assert!(monitor.is_live());
        drop(monitor);
    }
}
