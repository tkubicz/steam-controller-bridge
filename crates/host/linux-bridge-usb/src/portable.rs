use std::io;

use crate::{Discovery, Error};

pub(super) struct UsbTransport {
    unavailable: (),
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "the portable stub preserves the fallible Linux discovery API"
)]
pub(super) fn discover() -> Result<Discovery, Error> {
    Ok(Discovery::default())
}

pub(super) fn open(_stable_id: &str) -> Result<UsbTransport, Error> {
    Err(Error::Unsupported)
}

impl UsbTransport {
    pub(super) fn write_all(&mut self, _bytes: &[u8]) -> io::Result<()> {
        let () = self.unavailable;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Linux USB is unsupported on this platform",
        ))
    }

    pub(super) fn read(&mut self, _bytes: &mut [u8]) -> io::Result<usize> {
        let () = self.unavailable;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Linux USB is unsupported on this platform",
        ))
    }
}
