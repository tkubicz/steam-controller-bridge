#[cfg(target_os = "macos")]
mod macos;
// Only `macos` renders the model, so a non-macOS binary build would see it as
// dead code. `test` keeps its unit tests running on every CI platform.
#[cfg(any(target_os = "macos", test))]
mod model;

#[cfg(target_os = "macos")]
fn main() {
    if let Err(error) = macos::run() {
        eprintln!("Steam Controller Bridge menu app failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("sc-bridge-menu is available only on macOS");
}
