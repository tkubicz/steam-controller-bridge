#![cfg(target_os = "linux")]

use steam_controller_device::ControllerEnumerator;

#[test]
#[ignore = "requires a Linux runner with no attached supported controller"]
fn reusable_controller_enumerator_handles_two_empty_scans(
) -> Result<(), steam_controller_device::DeviceError> {
    let mut enumerator = ControllerEnumerator::new()?;

    let initial = enumerator.enumerate()?;
    let refreshed = enumerator.enumerate()?;

    assert_eq!(initial.len(), 0, "runner exposed a supported controller");
    assert_eq!(refreshed.len(), 0, "runner exposed a supported controller");
    Ok(())
}
