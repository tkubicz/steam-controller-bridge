# Steam Controller Bridge

Translate a Steam Controller into a conventional USB gamepad through a Seeed Studio XIAO nRF52840 bridge.

> Compatibility: this project targets **Steam Controller 2 (2026)** only. The
> original 2015 Steam Controller and receiver use a different protocol and are
> not supported.

For hardware requirements, firmware flashing, Steam Controller 2 pairing,
macOS permissions, daily startup, verification, and troubleshooting, start with
the [user guide](docs/USER_GUIDE.md).

## Status

The Steam Controller 2 Puck input path, host bridge, and XIAO nRF52840
CDC-to-gamepad firmware are implemented. On macOS, the connected prototype has
been flashed successfully, receives live `0x42` Puck reports, and enumerates in
Safari's Gamepad API with `mapping: standard` through an Xbox/ABXY-compatible
USB personality. Full control-by-control mapping, GeForce NOW/Boosteroid,
disconnect timing, and one-hour soak tests remain release gates.

```text
Simulator -> GamepadState -> protocol frame or JSONL recording -> output/replay
```

The bridge sends framed states over USB CDC to firmware that exposes a physical
Xbox-layout USB gamepad. This avoids depending on Apple's restricted
virtual-HID entitlements.

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

`sc-replay` also supports `--speed`, `--seek-us`, `--step`, `--loop`, and all currently implemented output backends, including negotiated serial output.

## HID probing on macOS

Enumeration never assumes a Steam Controller VID/PID. Start by inspecting the HID collections macOS currently exposes:

```bash
cargo run -p sc-probe -- list
cargo run -p sc-probe -- inspect --index 0
```

Indices use a stable path-sorted snapshot. Connecting or removing hardware can still change the snapshot, so run `list` again immediately before selecting a collection.

Monitor or capture the explicitly selected collection:

```bash
cargo run -p sc-probe -- monitor --index 0 --raw
cargo run -p sc-probe -- capture --index 0 --output reports.jsonl \
  --duration-secs 30 --decoded
```

The session reports disconnects and automatically attempts to reopen the same collection identity every 500 ms. Capture files include connection metadata, transport, source collection identity, report ID, base64 bytes, and the available dropped-report count. `--decoded` additionally records typed Steam Controller 2 state reports.

macOS may reject protected keyboard/gamepad collections with an IOKit `not permitted` error until the terminal or Codex host has Input Monitoring permission. Listing and metadata inspection do not require opening the collection.

## Visualizer

After identifying the desired HID collection with `sc-probe`, open the live
visualizer with the same snapshot index:

```bash
cargo run -p sc-visualizer -- --index 0
```

The visualizer shows raw, decoded, and mapped state; reports connection/rate and
error diagnostics; edits the mapping filters; and records raw, decoded, mapped,
lifecycle, and marker events to JSONL. It supports mock and negotiated serial
output.

See [the wire protocol](docs/GAMEPAD_PROTOCOL.md), [serial transport](docs/SERIAL_TRANSPORT.md), [Steam Controller protocol](docs/STEAM_CONTROLLER_PROTOCOL.md), [mapping](docs/MAPPING.md), [recording format](docs/RECORDING_FORMAT.md), [architecture](docs/ARCHITECTURE.md), [testing](docs/TESTING.md), and [firmware plan](docs/FIRMWARE_PLAN.md).

Firmware setup, native tests, UF2/DFU builds, flashing, recovery, LED states,
and hardware validation are documented in
[`firmware/xiao-nrf52840/README.md`](firmware/xiao-nrf52840/README.md).

## Integrated bridge

```bash
cargo run -p sc-bridge -- --controller auto --output dump
cargo run -p sc-bridge -- --index 0 --output serial \
  --port /dev/cu.usbmodemXXXX --record session.jsonl
cargo run -p sc-bridge -- --input replay --file session.jsonl \
  --deterministic --output mock
```

Live mode uses bounded input buffering, changed-state output, timeout and
decode-failure neutralization, reconnect tracking, Ctrl-C shutdown, and periodic
structured diagnostics. See [the bridge guide](docs/BRIDGE.md).

## Known limitations

- Steam Controller 2 Puck input is live-tested with extended `0x42` reports.
  Direct USB and Bluetooth still need transport-specific regression captures.
- Controller initialization and feature-report transmission are not implemented; probing remains read-only.
- Controller feature initialization is deliberately disabled until a safe SC2 command sequence is confirmed on the user's exact transport and firmware.
- The macOS-compatible output currently uses the Xbox 360 compatibility
  VID/PID (`045e:028e`) so Apple's built-in driver will publish it to
  GameController and browser clients. This is suitable for development
  hardware, but a distributable product needs an owned/licensed USB identity
  and a verified macOS recognition strategy.
- GeForce NOW, Boosteroid, full mapping, failure timing, and soak acceptance
  are not yet complete.
- The keyboard simulator is line-oriented, not a production input UI.
