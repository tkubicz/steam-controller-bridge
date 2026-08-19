//! Recovery prompt for a profile store that cannot be read.
//!
//! Neither caller has a console: the menu app has no window and the editor is
//! spawned with its output discarded.

use std::path::Path;

use desktop_bindings::{load_or_create_store, reset_store, BindingStore};
use menu_shell::{confirm_critical, ConfirmationChoice, CriticalConfirmation};

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
    let message = informative_text(path, error);
    let choice = confirm_critical(CriticalConfirmation {
        title: "Steam Controller Bridge cannot read your profiles",
        message: &message,
        confirm_label: "Reset Profiles",
        cancel_label: "Quit",
    });
    if choice == ConfirmationChoice::Confirmed {
        RecoveryChoice::Reset
    } else {
        RecoveryChoice::Quit
    }
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
