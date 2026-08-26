use bridge_output::{BridgeEndpoint, BridgeTransportError};

#[test]
#[ignore = "requires a Linux runner with no attached bridge device"]
fn native_endpoint_discovery_handles_an_empty_bridge_scan() -> Result<(), BridgeTransportError> {
    let bridge_devices = bridge_output::discover_bridge_endpoints()?
        .endpoints
        .into_iter()
        .filter(BridgeEndpoint::is_bridge_device)
        .count();
    assert_eq!(bridge_devices, 0, "runner exposed a bridge device");
    Ok(())
}
