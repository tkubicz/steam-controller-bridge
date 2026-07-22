# Steam Controller Bridge

Host-side foundations for translating a Steam Controller into a conventional USB gamepad through a future Seeed Studio XIAO nRF52840 bridge.

## Status

The pre-hardware foundation is implemented: a generic gamepad model, a stable framed protocol, mock/dump/file output backends, and keyboard/automated simulation. HID access, Steam Controller decoding, serial transport, recording, visualization, and firmware are later phases.

```text
Simulator -> GamepadState -> bridge-protocol frame -> dump/file output
```

The eventual path will replace the simulator with Steam Controller HID input and send the same frames over USB CDC to firmware that exposes a physical USB HID gamepad. This avoids depending on Apple's restricted virtual-HID entitlements.

## Build and test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

No physical hardware or external Rust dependencies are required for this phase.

## Simulator

Generate one automated cycle as readable state changes:

```bash
cargo run -p gamepad-simulator -- automated --interval-ms 0 --output dump
```

Write binary protocol frames for later firmware inspection:

```bash
cargo run -p gamepad-simulator -- automated --output file --file gamepad.frames
```

Use the line-oriented keyboard simulator:

```bash
cargo run -p gamepad-simulator -- keyboard --output json
```

Enter `w/a/s/d`, `up/left/down/right`, `q/e`, `i/j/k/l`, `space`, `1` through `9`, `r`, or `exit`, followed by Enter. Each line produces one state, which keeps the tool dependency-free and portable; raw-terminal input can be added in a later UI phase.

See [the wire protocol](docs/GAMEPAD_PROTOCOL.md), [architecture](docs/ARCHITECTURE.md), and [firmware plan](docs/FIRMWARE_PLAN.md).

## Known limitations

- No Steam Controller HID discovery or report decoding yet.
- No serial transport or handshake driver yet; the protocol messages are defined.
- No recording/replay or graphical visualizer yet.
- No XIAO firmware is included before hardware validation.
- The keyboard simulator is line-oriented, not a production input UI.

