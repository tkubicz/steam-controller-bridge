#[cfg(all(target_os = "macos", feature = "editor"))]
mod bindings_editor;
#[cfg(target_os = "macos")]
mod macos;
// Only `macos` renders the model, so a non-macOS build would see it as dead code.
// Its unit tests therefore run on macOS only.
#[cfg(target_os = "macos")]
mod model;
// The host side only manages the child process, so it is compiled even
// without the `overlay` feature — a featureless build's child simply reports
// that it cannot render and exits, the same degradation the editor gets.
#[cfg(target_os = "macos")]
mod overlay_host;
#[cfg(target_os = "macos")]
mod overlay_protocol;
#[cfg(all(target_os = "macos", feature = "overlay"))]
mod profile_overlay;

#[cfg(target_os = "macos")]
fn main() {
    let mut editor = false;
    let mut overlay = false;
    for argument in std::env::args() {
        editor |= argument == "--bindings-editor";
        overlay |= argument == overlay_protocol::OVERLAY_ARGUMENT;
    }
    let result = if editor {
        #[cfg(feature = "editor")]
        {
            bindings_editor::run()
        }
        #[cfg(not(feature = "editor"))]
        {
            Err("this build has no bindings editor".to_owned())
        }
    } else if overlay {
        #[cfg(feature = "overlay")]
        {
            profile_overlay::run()
        }
        #[cfg(not(feature = "overlay"))]
        {
            Err("this build has no profile overlay".to_owned())
        }
    } else {
        macos::run()
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
