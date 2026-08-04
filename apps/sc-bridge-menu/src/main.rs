#[cfg(target_os = "macos")]
mod bindings_editor;
#[cfg(target_os = "macos")]
mod macos;
// Only `macos` renders the model, so a non-macOS build would see it as dead code.
// Its unit tests therefore run on macOS only.
#[cfg(target_os = "macos")]
mod model;

#[cfg(target_os = "macos")]
fn main() {
    let editor = std::env::args().any(|argument| argument == "--bindings-editor");
    let result = if editor {
        bindings_editor::run()
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
