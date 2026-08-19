#[cfg(all(target_os = "macos", feature = "updater"))]
mod about_pages;
#[cfg(all(target_os = "macos", feature = "updater"))]
mod app_center;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod app_center_host;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod app_center_protocol;
#[cfg(all(target_os = "macos", feature = "editor"))]
mod bindings_editor;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod bindings_recovery;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod cli;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod line_protocol;
#[cfg(target_os = "macos")]
mod macos;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod model;
// The host side only manages the child process, so it is compiled even
// without the `overlay` feature - a featureless build's child simply reports
// that it cannot render and exits, the same degradation the editor gets.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod overlay_host;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod overlay_protocol;
#[cfg(all(target_os = "macos", feature = "overlay"))]
mod profile_overlay;
#[cfg(test)]
mod test_child;
#[cfg(feature = "updater")]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod update_check;
#[cfg(all(target_os = "macos", feature = "updater"))]
mod window_ui;

#[cfg(target_os = "macos")]
fn main() {
    let cli = cli::parse();
    let result = match cli.command {
        None => virtual_gamepad::virtual_hid_enabled(cli.enable_virtual_hid).and_then(macos::run),
        Some(cli::Command::AppCenter(arguments)) => {
            #[cfg(feature = "updater")]
            {
                app_center::run(arguments)
            }
            #[cfg(not(feature = "updater"))]
            {
                let _ = arguments;
                Err("this build has no application information window".to_owned())
            }
        }
        Some(cli::Command::BindingsEditor) => {
            #[cfg(feature = "editor")]
            {
                bindings_editor::run()
            }
            #[cfg(not(feature = "editor"))]
            {
                Err("this build has no bindings editor".to_owned())
            }
        }
        Some(cli::Command::ProfileOverlay) => {
            #[cfg(feature = "overlay")]
            {
                profile_overlay::run()
            }
            #[cfg(not(feature = "overlay"))]
            {
                Err("this build has no profile overlay".to_owned())
            }
        }
    };
    if let Err(error) = result {
        eprintln!("Steam Controller Bridge menu app failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("sc-bridge-menu is available only on macOS");
}
