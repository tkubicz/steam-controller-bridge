use super::{generic_open_error, SerialError};

pub(super) fn is_callout_port(_path: &str) -> bool {
    true
}

pub(super) fn open_error(_path: &str, error: serialport::Error) -> SerialError {
    generic_open_error(error)
}

#[cfg(test)]
pub(super) const TEST_CALLOUT_PORT: &str = "serial:test";
