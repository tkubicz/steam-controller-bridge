use std::path::PathBuf;

use bridge_output::GamepadOutput;
use gamepad_state::{Button, GamepadState};
use virtual_gamepad::{VirtualHidConfig, VirtualHidOutput, HELPER_PROTOCOL_VERSION};

fn helper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_sc-virtual-hid-helper"))
}

#[test]
fn parent_and_helper_complete_the_dry_run_lifecycle() {
    let config = VirtualHidConfig::dry_run(helper_path());
    let mut output = VirtualHidOutput::open(config).unwrap();
    let metadata = output.helper_metadata();
    assert!(metadata.dry_run);
    assert_eq!((metadata.vendor_id, metadata.product_id), (0x045e, 0x028e));
    assert_eq!(metadata.protocol_version, HELPER_PROTOCOL_VERSION);

    let mut state = GamepadState::neutral();
    state.buttons.set(Button::South, true);
    output.send_state(&state).unwrap();
    output.send_neutral().unwrap();
    assert!(output.diagnostics().virtual_reports_dispatched >= 1);

    // Drop performs the sequenced shutdown handshake, waits for its applied
    // response, closes the pipes, and reaps the child.
    drop(output);
}

#[test]
fn missing_helper_is_a_permanent_configuration_failure() {
    let error = VirtualHidOutput::open(VirtualHidConfig::dry_run(PathBuf::from(
        "/definitely/not/a/virtual-hid-helper",
    )))
    .err()
    .expect("missing helper must fail");
    assert!(error.is_permanent_configuration_failure());
}

#[test]
fn custom_identity_round_trips_without_changing_the_contract() {
    let config = VirtualHidConfig::dry_run(helper_path()).with_identity(0xcafe, 0x4001);
    let mut output = VirtualHidOutput::open(config).unwrap();
    let metadata = output.helper_metadata();
    assert_eq!((metadata.vendor_id, metadata.product_id), (0xcafe, 0x4001));
    output.send_state(&GamepadState::neutral()).unwrap();
    output.send_neutral().unwrap();
}
