use bridge_output::{SerialDeviceInfo, SerialError};

#[test]
#[ignore = "requires a Linux runner with no attached bridge serial device"]
fn native_serial_discovery_handles_an_empty_bridge_scan() -> Result<(), SerialError> {
    let bridge_devices = bridge_output::available_serial_devices()?
        .into_iter()
        .filter(SerialDeviceInfo::is_bridge_device)
        .count();
    assert_eq!(bridge_devices, 0, "runner exposed a bridge serial device");
    Ok(())
}
