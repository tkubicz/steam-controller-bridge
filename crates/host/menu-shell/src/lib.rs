//! Platform shell effects used by menu frontends.

use std::ffi::OsStr;
use std::process::Child;

#[cfg(target_os = "macos")]
#[path = "backend/macos.rs"]
mod backend;
#[cfg(not(target_os = "macos"))]
#[path = "backend/unsupported.rs"]
mod backend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationChoice {
    Confirmed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CriticalConfirmation<'a> {
    pub title: &'a str,
    pub message: &'a str,
    pub confirm_label: &'a str,
    pub cancel_label: &'a str,
}

/// Copies text to the host clipboard.
///
/// # Errors
/// Returns an error when the host shell cannot update the clipboard.
pub fn copy_text(value: &str) -> Result<(), String> {
    backend::copy_text(value)
}

/// Opens a URL with the host default handler.
///
/// # Errors
/// Returns an error when the host shell cannot open the URL.
pub fn open_url(url: &str) -> Result<(), String> {
    backend::open_url(url)
}

/// Opens a path with the host default handler.
///
/// # Errors
/// Returns an error when the host shell cannot open the path.
pub fn open_path(path: impl AsRef<OsStr>) -> Result<(), String> {
    backend::open_path(path.as_ref())
}

/// Reveals a path in the host file manager.
///
/// # Errors
/// Returns an error when the host shell cannot reveal the path.
pub fn reveal_path(path: impl AsRef<OsStr>) -> Result<(), String> {
    backend::reveal_path(path.as_ref())
}

/// Presents a critical confirmation with a safe cancel default.
#[must_use]
pub fn confirm_critical(confirmation: CriticalConfirmation<'_>) -> ConfirmationChoice {
    backend::confirm_critical(confirmation)
}

/// Tries to activate the child application's windows.
#[must_use]
pub fn activate_child_application(child: &Child) -> bool {
    backend::activate_child_application(child)
}
