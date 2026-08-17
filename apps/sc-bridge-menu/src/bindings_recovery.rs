//! What to do when the profile store on disk cannot be read.
//!
//! A binding store is a plain JSON file the user is invited to hand-edit, so a
//! typo in it is an ordinary mistake rather than an exceptional one. Failing
//! startup with a line on stderr hides that mistake completely: a menu-bar app
//! has no console, and the editor is launched with its output discarded. So the
//! failure is presented instead, with the only two answers that respect the
//! user's data - keep the file and let them fix it, or set it aside and start
//! fresh.
//!
//! Nothing here decides on the user's behalf. In particular the broken file is
//! never deleted, only renamed, so "start fresh" stays undoable.

use std::path::Path;

use desktop_bindings::{load_or_create_store, reset_store, BindingStore};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication};
use objc2_foundation::NSString;

/// What the user chose when told their profile store cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryChoice {
    /// Set the unreadable file aside and continue with a default store.
    Reset,
    /// Leave the file exactly as it is and quit, so it can be backed up or
    /// repaired by hand.
    Quit,
}

/// Loads the profile store, offering recovery if it cannot be read.
///
/// `Ok(None)` means the user chose to quit and nothing was changed; the caller
/// should exit successfully rather than reporting a failure they already saw.
///
/// # Errors
/// Returns an error only when recovery itself fails, which leaves the original
/// file untouched.
pub(crate) fn load_store_or_recover(path: &Path) -> Result<Option<BindingStore>, String> {
    recover_with(path, ask)
}

/// The recovery flow with the question left open, so everything except the
/// alert itself is exercised by tests rather than only by a human clicking.
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

/// Presents the failure and returns what the user chose.
///
/// Quit is the second button, which makes it the one Escape picks: doing
/// nothing to an unreadable file is always the safe answer, and the destructive
/// choice should never be the accidental one.
fn ask(path: &Path, error: &str) -> RecoveryChoice {
    let Some(mtm) = MainThreadMarker::new() else {
        // Without the main thread there is no way to ask, and guessing "reset"
        // would move a file the user never agreed to move.
        return RecoveryChoice::Quit;
    };
    // The alert runs its own modal loop, which needs the shared application to
    // exist. It already does when the menu app asks; the editor asks before
    // eframe has started, and this is idempotent either way.
    let application = NSApplication::sharedApplication(mtm);

    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Critical);
    alert.setMessageText(&NSString::from_str(
        "Steam Controller Bridge cannot read your profiles",
    ));
    alert.setInformativeText(&NSString::from_str(&informative_text(path, error)));
    alert.addButtonWithTitle(&NSString::from_str("Reset Profiles"));
    alert.addButtonWithTitle(&NSString::from_str("Quit"));

    // An accessory app puts up windows behind whatever is frontmost, and an
    // alert nobody sees is the bug this module exists to fix.
    application.activate();
    if alert.runModal() == NSAlertFirstButtonReturn {
        RecoveryChoice::Reset
    } else {
        RecoveryChoice::Quit
    }
}

/// The body of the alert: what happened, where, and what each button does.
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
    use super::{informative_text, recover_with, RecoveryChoice};
    use desktop_bindings::{save_store, BindingStore};
    use std::fs;
    use std::path::Path;

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

        // The caller exits successfully, and the user still has their file to
        // back up or repair.
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
        // The fresh file on disk is the one the app will use from now on.
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
        // Everything the user needs to act is in the alert itself, because a
        // menu-bar app gives them nowhere else to look.
        assert!(text.contains("unknown field `id` at line 84 column 18"));
        assert!(text.contains("/Users/someone/Library/Application Support/x/bindings.json"));
        assert!(text.contains("renamed"));
        assert!(text.contains("Quit changes nothing"));
    }
}
