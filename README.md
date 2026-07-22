# Steam Controller Bridge

Host-side foundations for translating a Steam Controller into a conventional USB gamepad through a future Seeed Studio XIAO nRF52840 bridge.

## Status

The first three pre-hardware phases are implemented: a generic gamepad model, a stable framed protocol, mock/dump/file output backends, keyboard/automated simulation, and versioned recording/replay. HID access, Steam Controller decoding, mapping, serial transport, visualization, and firmware are later phases.

```text
Simulator -> GamepadState -> protocol frame or JSONL recording -> output/replay
```

The eventual path will replace the simulator with Steam Controller HID input and send the same frames over USB CDC to firmware that exposes a physical USB HID gamepad. This avoids depending on Apple's restricted virtual-HID entitlements.

## Build and test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

No physical hardware is required. The recording crate uses the established `serde`, `serde_json`, and `base64` crates; the timing, protocol, output, and simulator paths otherwise use the standard library.

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

## Recording and replay

Record an automated session, then replay it without waiting for recorded timing:

```bash
cargo run -p gamepad-simulator -- automated --interval-ms 0 \
  --output recording --file session.jsonl
cargo run -p sc-replay -- session.jsonl --deterministic --output dump
```

`sc-replay` also supports `--speed`, `--seek-us`, `--step`, `--loop`, and all currently implemented output backends. Serial replay is intentionally deferred until the serial transport phase.

See [the wire protocol](docs/GAMEPAD_PROTOCOL.md), [recording format](docs/RECORDING_FORMAT.md), [architecture](docs/ARCHITECTURE.md), [testing](docs/TESTING.md), and [firmware plan](docs/FIRMWARE_PLAN.md).

## Known limitations

- No Steam Controller HID discovery or report decoding yet.
- No serial transport or handshake driver yet; the protocol messages are defined.
- No live HID recorder or graphical visualizer yet; recording currently accepts generic simulator states and typed raw events through the library API.
- No XIAO firmware is included before hardware validation.
- The keyboard simulator is line-oriented, not a production input UI.
