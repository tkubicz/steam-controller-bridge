#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

#[cfg(target_os = "macos")]
pub(crate) use macos::VirtualDevice;
#[cfg(not(target_os = "macos"))]
pub(crate) use unsupported::VirtualDevice;
