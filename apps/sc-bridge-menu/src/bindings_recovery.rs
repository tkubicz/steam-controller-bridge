//! Recovery prompt for a profile store that cannot be read.
//!
//! Neither caller has a console: the menu app has no window and the editor is
//! spawned with its output discarded.

use std::path::Path;

use desktop_bindings::{load_or_create_store, reset_store, BindingStore};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSApplicationActivationPolicy,
    NSEvent, NSModalPanelWindowLevel, NSScreen, NSWindow,
};
use objc2_foundation::{NSPoint, NSString};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryChoice {
    Reset,
    Quit,
}

/// Loads the profile store, offering recovery if it cannot be read.
///
/// `Ok(None)` means the user chose to quit; the caller should exit successfully.
///
/// # Errors
/// Returns an error only when recovery itself fails, which leaves the original
/// file untouched.
pub(crate) fn load_store_or_recover(path: &Path) -> Result<Option<BindingStore>, String> {
    recover_with(path, ask)
}

/// Split from [`load_store_or_recover`] so tests can answer without a human.
fn recover_with(
    path: &Path,
    ask: impl FnOnce(&Path, &str) -> RecoveryChoice,
) -> Result<Option<BindingStore>, String> {
    let error = match load_or_create_store(path) {
        Ok(store) => return Ok(Some(store)),
        Err(error) => error,
    };
    eprintln!(
        "level=error event=binding_store_unreadable path={:?} message={error:?} action=prompt",
        path.display()
    );
    match ask(path, &error) {
        RecoveryChoice::Quit => {
            eprintln!("level=warn event=binding_store_unreadable_choice choice=quit action=exit");
            Ok(None)
        }
        RecoveryChoice::Reset => {
            let kept = reset_store(path)?;
            eprintln!(
                "level=warn event=binding_store_reset kept={:?} action=defaults",
                kept.display()
            );
            Ok(Some(BindingStore::default()))
        }
    }
}

/// Quit is the second button so that Escape picks it: the destructive choice
/// must not be the accidental one.
fn ask(path: &Path, error: &str) -> RecoveryChoice {
    let Some(mtm) = MainThreadMarker::new() else {
        // Cannot ask, so do not move a file the user never agreed to move.
        return RecoveryChoice::Quit;
    };
    let application = NSApplication::sharedApplication(mtm);
    // An unbundled process is a background-only application until a policy is
    // set, and cannot show a window at all: `runModal` would block forever on an
    // invisible alert. Both callers ask before winit/eframe sets one.
    let policy = application.activationPolicy();
    let raised = application.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Critical);
    alert.setMessageText(&NSString::from_str(
        "Steam Controller Bridge cannot read your profiles",
    ));
    alert.setInformativeText(&NSString::from_str(&informative_text(path, error)));
    alert.addButtonWithTitle(&NSString::from_str("Reset Profiles"));
    alert.addButtonWithTitle(&NSString::from_str("Quit"));

    // `activate` is best effort, and macOS 14 made the "ignore other apps"
    // override a no-op, so focus cannot be taken. The window level is what
    // actually puts the alert in front.
    application.activate();
    let window = alert.window();
    window.setLevel(NSModalPanelWindowLevel);
    // Lay out first so the frame used for centring is final.
    alert.layout();
    show_on_the_users_screen(&window, mtm);
    window.orderFrontRegardless();
    let response = alert.runModal();
    if raised {
        // Restore, so the menu app keeps its own policy and no Dock icon.
        application.setActivationPolicy(policy);
    }
    if response == NSAlertFirstButtonReturn {
        RecoveryChoice::Reset
    } else {
        RecoveryChoice::Quit
    }
}

/// With no key window `NSScreen::mainScreen` is the menu-bar screen by fallback,
/// which strands the alert on a display the user may not be facing. The cursor
/// is the only signal left; see `profile_overlay::window::target_screen`.
fn show_on_the_users_screen(window: &NSWindow, mtm: MainThreadMarker) {
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

/// Centred horizontally, above the middle vertically, matching macOS alerts.
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

fn informative_text(path: &Path, error: &str) -> String {
    format!(
        "{error}\n\n\
         The file is:\n{}\n\n\
         Reset Profiles keeps your file next to it, renamed, and starts with a \
         single empty Default profile. Quit changes nothing so you can back the \
         file up or repair it by hand.",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::{centered_origin, contains, informative_text, recover_with, RecoveryChoice};
    use desktop_bindings::{save_store, BindingStore};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use std::fs;
    use std::path::Path;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    }

    #[test]
    fn the_alert_lands_on_the_display_holding_the_cursor() {
        let primary = rect(0.0, 0.0, 1920.0, 1080.0);
        let secondary = rect(1920.0, 0.0, 2560.0, 1440.0);

        assert!(contains(primary, NSPoint::new(10.0, 10.0)));
        assert!(!contains(secondary, NSPoint::new(10.0, 10.0)));
        assert!(contains(secondary, NSPoint::new(2000.0, 700.0)));
        // The shared edge belongs to exactly one screen.
        assert!(!contains(primary, NSPoint::new(1920.0, 500.0)));
        assert!(contains(secondary, NSPoint::new(1920.0, 500.0)));
    }

    #[test]
    fn the_alert_is_centred_above_the_middle_of_its_screen() {
        let origin = centered_origin(
            rect(1920.0, 0.0, 2560.0, 1440.0),
            rect(0.0, 0.0, 400.0, 200.0),
        );
        assert!((origin.x - (1920.0 + (2560.0 - 400.0) / 2.0)).abs() < 0.5);
        assert!(origin.y > (1440.0 - 200.0) / 2.0);
        assert!(origin.y < 1440.0 - 200.0);

        // Oversized: pinned to the origin, not pushed off-screen.
        let oversized = centered_origin(rect(0.0, 0.0, 320.0, 240.0), rect(0.0, 0.0, 800.0, 600.0));
        assert!((oversized.x - 0.0).abs() < f64::EPSILON);
        assert!((oversized.y - 0.0).abs() < f64::EPSILON);
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("sc-bridge-menu-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn a_readable_store_is_returned_without_asking_anything() {
        let directory = scratch("recover-ok");
        let path = directory.join("bindings.json");
        save_store(&path, &BindingStore::default()).unwrap();

        let store = recover_with(&path, |_, _| panic!("a readable store must not prompt"))
            .unwrap()
            .expect("a readable store loads");

        assert_eq!(store, BindingStore::default());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn choosing_quit_changes_nothing_on_disk() {
        let directory = scratch("recover-quit");
        let path = directory.join("bindings.json");
        fs::write(&path, b"not a binding store").unwrap();

        let outcome = recover_with(&path, |_, _| RecoveryChoice::Quit).unwrap();

        assert!(outcome.is_none());
        assert_eq!(fs::read(&path).unwrap(), b"not a binding store");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn choosing_reset_starts_fresh_and_keeps_the_original_next_to_it() {
        let directory = scratch("recover-reset");
        let path = directory.join("bindings.json");
        fs::write(&path, b"not a binding store").unwrap();

        let store = recover_with(&path, |_, _| RecoveryChoice::Reset)
            .unwrap()
            .expect("reset yields a usable store");

        assert_eq!(store, BindingStore::default());
        assert_eq!(
            fs::read(directory.join("bindings-invalid.json")).unwrap(),
            b"not a binding store"
        );
        assert_eq!(
            desktop_bindings::load_store(&path).unwrap(),
            BindingStore::default()
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn the_alert_body_names_the_file_the_error_and_both_outcomes() {
        let text = informative_text(
            Path::new("/Users/someone/Library/Application Support/x/bindings.json"),
            "invalid bindings JSON: unknown field `id` at line 84 column 18",
        );
        assert!(text.contains("unknown field `id` at line 84 column 18"));
        assert!(text.contains("/Users/someone/Library/Application Support/x/bindings.json"));
        assert!(text.contains("renamed"));
        assert!(text.contains("Quit changes nothing"));
    }
}
