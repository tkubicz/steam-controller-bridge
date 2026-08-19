use std::ffi::OsStr;
use std::io::Write;
use std::process::{Child, Command, Stdio};

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSApplicationActivationOptions,
    NSApplicationActivationPolicy, NSEvent, NSModalPanelWindowLevel, NSRunningApplication,
    NSScreen, NSWindow, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSString, NSURL};

use crate::{ConfirmationChoice, CriticalConfirmation};

pub(super) fn copy_text(value: &str) -> Result<(), String> {
    let mut process = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let write = process
        .stdin
        .take()
        .ok_or_else(|| "pbcopy stdin is unavailable".to_owned())
        .and_then(|mut stdin| {
            stdin
                .write_all(value.as_bytes())
                .map_err(|error| error.to_string())
        });
    let exit = process.wait().map_err(|error| error.to_string());
    write?;
    let exit = exit?;
    if exit.success() {
        Ok(())
    } else {
        Err(format!("pbcopy exited with {exit}"))
    }
}

pub(super) fn open_path(path: &OsStr) -> Result<(), String> {
    run_open(std::iter::once(path))
}

pub(super) fn open_url(url: &str) -> Result<(), String> {
    let url =
        NSURL::URLWithString(&NSString::from_str(url)).ok_or_else(|| "invalid URL".to_owned())?;
    if NSWorkspace::sharedWorkspace().openURL(&url) {
        Ok(())
    } else {
        Err("the default application could not open the URL".to_owned())
    }
}

pub(super) fn reveal_path(path: &OsStr) -> Result<(), String> {
    run_open([OsStr::new("-R"), path])
}

fn run_open(arguments: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Result<(), String> {
    let status = Command::new("/usr/bin/open")
        .args(arguments)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open exited with {status}"))
    }
}

pub(super) fn confirm_critical(confirmation: CriticalConfirmation<'_>) -> ConfirmationChoice {
    let Some(mtm) = MainThreadMarker::new() else {
        return ConfirmationChoice::Cancelled;
    };
    let application = NSApplication::sharedApplication(mtm);
    // An unbundled background process needs a foreground policy before runModal.
    let policy = application.activationPolicy();
    let raised = application.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Critical);
    alert.setMessageText(&NSString::from_str(confirmation.title));
    alert.setInformativeText(&NSString::from_str(confirmation.message));
    alert.addButtonWithTitle(&NSString::from_str(confirmation.confirm_label));
    alert.addButtonWithTitle(&NSString::from_str(confirmation.cancel_label));

    application.activate();
    let window = alert.window();
    window.setLevel(NSModalPanelWindowLevel);
    alert.layout();
    show_on_cursor_screen(&window, mtm);
    window.orderFrontRegardless();
    let response = alert.runModal();
    if raised {
        // Preserve the caller's accessory policy and Dock visibility.
        application.setActivationPolicy(policy);
    }
    if response == NSAlertFirstButtonReturn {
        ConfirmationChoice::Confirmed
    } else {
        ConfirmationChoice::Cancelled
    }
}

fn show_on_cursor_screen(window: &NSWindow, mtm: MainThreadMarker) {
    let cursor = NSEvent::mouseLocation();
    let screens = NSScreen::screens(mtm);
    let Some(screen) = screens
        .iter()
        .find(|screen| contains(screen.frame(), cursor))
        .or_else(|| screens.iter().next())
    else {
        return;
    };
    window.setFrameOrigin(centered_origin(screen.visibleFrame(), window.frame()));
}

fn contains(frame: objc2_foundation::NSRect, point: NSPoint) -> bool {
    point.x >= frame.origin.x
        && point.x < frame.origin.x + frame.size.width
        && point.y >= frame.origin.y
        && point.y < frame.origin.y + frame.size.height
}

fn centered_origin(screen: objc2_foundation::NSRect, window: objc2_foundation::NSRect) -> NSPoint {
    NSPoint::new(
        (screen.size.width - window.size.width)
            .max(0.0)
            .mul_add(0.5, screen.origin.x),
        (screen.size.height - window.size.height)
            .max(0.0)
            .mul_add(0.6, screen.origin.y),
    )
}

#[allow(
    deprecated,
    reason = "required on macOS 13; the replacement API starts at macOS 14"
)]
pub(super) fn activate_child_application(child: &Child) -> bool {
    let Ok(pid) = i32::try_from(child.id()) else {
        return false;
    };
    let Some(application) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
    else {
        return false;
    };
    application.activateWithOptions(
        NSApplicationActivationOptions::ActivateAllWindows
            | NSApplicationActivationOptions::ActivateIgnoringOtherApps,
    )
}

#[cfg(test)]
mod tests {
    use super::{centered_origin, confirm_critical, contains};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    use crate::{ConfirmationChoice, CriticalConfirmation};

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    }

    #[test]
    fn cursor_selects_exactly_one_screen_at_a_shared_edge() {
        let primary = rect(0.0, 0.0, 1920.0, 1080.0);
        let secondary = rect(1920.0, 0.0, 2560.0, 1440.0);

        assert!(contains(primary, NSPoint::new(10.0, 10.0)));
        assert!(!contains(secondary, NSPoint::new(10.0, 10.0)));
        assert!(contains(secondary, NSPoint::new(2000.0, 700.0)));
        assert!(!contains(primary, NSPoint::new(1920.0, 500.0)));
        assert!(contains(secondary, NSPoint::new(1920.0, 500.0)));
    }

    #[test]
    fn confirmation_is_centered_above_midpoint_and_clamped() {
        let origin = centered_origin(
            rect(1920.0, 0.0, 2560.0, 1440.0),
            rect(0.0, 0.0, 400.0, 200.0),
        );
        assert!((origin.x - (1920.0 + (2560.0 - 400.0) / 2.0)).abs() < 0.5);
        assert!(origin.y > (1440.0 - 200.0) / 2.0);
        assert!(origin.y < 1440.0 - 200.0);

        let oversized = centered_origin(rect(0.0, 0.0, 320.0, 240.0), rect(0.0, 0.0, 800.0, 600.0));
        assert!((oversized.x - 0.0).abs() < f64::EPSILON);
        assert!((oversized.y - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn background_confirmation_uses_the_safe_cancel_default() {
        let choice = std::thread::spawn(|| {
            confirm_critical(CriticalConfirmation {
                title: "title",
                message: "message",
                confirm_label: "confirm",
                cancel_label: "cancel",
            })
        })
        .join()
        .unwrap();

        assert_eq!(choice, ConfirmationChoice::Cancelled);
    }
}
