use super::*;

pub(super) fn copy_text(value: &str) -> Result<(), String> {
    let mut process = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    process
        .stdin
        .take()
        .ok_or("pbcopy stdin is unavailable")?
        .write_all(value.as_bytes())
        .map_err(|error| error.to_string())?;
    let exit = process.wait().map_err(|error| error.to_string())?;
    if exit.success() {
        Ok(())
    } else {
        Err(format!("pbcopy exited with {exit}"))
    }
}

/// The System Settings panes a user has to visit when macOS has already
/// recorded a refusal, since nothing can re-prompt after that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrivacyPane {
    InputMonitoring,
    Accessibility,
}

pub(super) const fn privacy_pane_url(pane: PrivacyPane) -> &'static str {
    match pane {
        PrivacyPane::InputMonitoring => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
        PrivacyPane::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
    }
}

pub(super) fn open_privacy_pane(pane: PrivacyPane) {
    eprintln!("level=info event=privacy_pane_opened pane={pane:?}");
    if let Err(error) = open_path(privacy_pane_url(pane)) {
        eprintln!("cannot open {pane:?} settings: {error}");
    }
}

#[allow(deprecated)] // Required on macOS 13; the replacement API starts at macOS 14.
pub(super) fn activate_child_application(child: &std::process::Child) -> bool {
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

/// Spawns the editor process. The caller keeps the child so it can be reaped
/// once the user closes the window; dropping the handle would leave a zombie
/// per launch until the menu app itself exits.
pub(super) fn launch_bindings_editor() -> Result<std::process::Child, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Command::new(executable)
        .arg(crate::cli::BINDINGS_EDITOR_COMMAND)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())
}

pub(crate) fn open_path(path: impl AsRef<std::ffi::OsStr>) -> Result<(), String> {
    run_open(std::iter::once(path.as_ref()))
}

pub(crate) fn reveal_path(path: impl AsRef<std::ffi::OsStr>) -> Result<(), String> {
    run_open([std::ffi::OsStr::new("-R"), path.as_ref()])
}

fn run_open(
    arguments: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
) -> Result<(), String> {
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
