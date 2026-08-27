#[allow(
    clippy::wildcard_imports,
    reason = "controller lifecycle helpers share the supervisor's safety-critical dependencies"
)]
use super::*;

mod haptics;
mod hid_worker;
mod session;

#[allow(
    clippy::wildcard_imports,
    reason = "controller submodules form one private supervisor implementation boundary"
)]
pub(crate) use haptics::*;
#[allow(
    clippy::wildcard_imports,
    reason = "controller submodules form one private supervisor implementation boundary"
)]
pub(crate) use hid_worker::*;
#[allow(
    clippy::wildcard_imports,
    reason = "controller submodules form one private supervisor implementation boundary"
)]
pub(crate) use session::*;
