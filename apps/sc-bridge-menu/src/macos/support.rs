use std::path::{Path, PathBuf};

use bridge_runtime::{OutputSelection, VirtualHidConfig};

use crate::app_state::OutputPreference;

pub(super) fn bundled_virtual_hid_helper_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate menu executable: {error}"))?;
    bundled_virtual_hid_helper_path_from(&executable)
}

pub(super) fn bundled_virtual_hid_helper_path_from(executable: &Path) -> Result<PathBuf, String> {
    let macos = executable
        .parent()
        .ok_or("menu executable has no parent directory")?;
    let contents = macos
        .parent()
        .ok_or("menu executable is not inside an app Contents directory")?;
    if macos.file_name().and_then(|name| name.to_str()) != Some("MacOS") {
        return Err("menu executable is not in Contents/MacOS".to_owned());
    }
    Ok(contents
        .join("Helpers/Steam Controller Bridge Virtual HID Helper.app")
        .join("Contents/MacOS/sc-virtual-hid-helper"))
}

pub(super) fn output_selection(preference: OutputPreference) -> Result<OutputSelection, String> {
    match preference {
        OutputPreference::BridgeDevice => Ok(OutputSelection::BridgeDevice),
        OutputPreference::VirtualHid => bundled_virtual_hid_helper_path()
            .map(VirtualHidConfig::new)
            .map(OutputSelection::VirtualHid),
    }
}
