use std::process::{Command, Stdio};

use platform_capabilities::{CapabilityId, Remedy};

pub(super) fn apply_capability_remedy(id: CapabilityId, remedy: &Remedy) {
    match remedy {
        Remedy::OpenUrl(url) => {
            let pane = match id {
                CapabilityId::InputMonitoring => "InputMonitoring",
                CapabilityId::PostEvent | CapabilityId::Accessibility => "Accessibility",
                _ => "Capability",
            };
            eprintln!("level=info event=privacy_pane_opened pane={pane}");
            if let Err(error) = menu_shell::open_url(url) {
                eprintln!("cannot open {pane} settings: {error}");
            }
        }
        Remedy::RequestFromSystem => {
            eprintln!("cannot apply {id:?} remedy without an interactive provider request");
        }
        Remedy::Instructions { text, command } => {
            eprintln!(
                "cannot display {id:?} instructions in the macOS menu: {text}; command={command:?}"
            );
        }
    }
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
