use std::ffi::OsStr;
use std::process::Child;

use crate::{ConfirmationChoice, CriticalConfirmation};

pub(super) fn copy_text(_value: &str) -> Result<(), String> {
    unsupported("clipboard")
}

pub(super) fn open_path(_path: &OsStr) -> Result<(), String> {
    unsupported("open")
}

pub(super) fn open_url(_url: &str) -> Result<(), String> {
    unsupported("open URL")
}

pub(super) fn reveal_path(_path: &OsStr) -> Result<(), String> {
    unsupported("reveal")
}

pub(super) fn confirm_critical(_confirmation: CriticalConfirmation<'_>) -> ConfirmationChoice {
    ConfirmationChoice::Cancelled
}

pub(super) fn activate_child_application(_child: &Child) -> bool {
    false
}

fn unsupported<T>(operation: &str) -> Result<T, String> {
    Err(format!(
        "menu shell {operation} is unavailable on {}",
        std::env::consts::OS
    ))
}
