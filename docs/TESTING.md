# Testing

## Firmware

Run the portable parser/session tests without hardware:

```bash
make -C firmware/xiao-nrf52840 test
```

With Arduino CLI and the pinned Seeed core installed, compile the actual sketch:

```bash
make -C firmware/xiao-nrf52840 build
```

Physical acceptance additionally covers CDC/gamepad enumeration, every report
field, a 30-second unchanged hold, host termination neutralization within 125
ms, malformed recovery, reconnect/sequence wrap, Chrome and Safari Gamepad API
behavior, one streaming service, and a one-hour soak. Detailed steps are in the
firmware README; these results must not be inferred from native or CI builds.

The workspace has no hardware-dependent tests. Run the same gates as CI:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-targets
./tools/build-macos-app.sh
```

The protocol tests cover every message type, fixed axis endpoints, the static CRC vector, partial and combined reads, garbage, truncation, invalid versions, oversized lengths, corruption, and recovery. A deterministic pseudo-random byte-stream test checks that framing never panics.

Recording tests cover typed raw and gamepad round trips, timestamp ordering, unknown events, seeking, deterministic replay, malformed/truncated input, version rejection, and identical simulator-state replay.

Steam Controller protocol tests cover all 30 OpenPuck button bits, every trigger/stick/pad/pressure/motion field, both `0x45` and extended `0x42`, connection/battery/signal reports, typed recording round trips, incorrect sizes, mismatched IDs, unknown IDs, and arbitrary truncated lengths.

The same crate commits the complete 64-byte SDL-compatible lizard-off golden
vector. Device tests cover the exact `28de:1304`/`ff00:0001`/interface 2–5
allowlist plus immediate, three-second, disconnect, and reconnect heartbeat
scheduling. Bridge tests cover safe-default option parsing, initial
suppression, periodic refresh, leave mode, and fail-closed write behavior. The
macOS device tests also verify that a second project process cannot acquire the
same per-slot ownership lock and that ownership is released when the first
session ends.

The tested Puck cannot be opened with HIDAPI's macOS exclusive seize option:
it returns `0xE00002E2 not permitted`, including after Steam's persistent IPC
LaunchAgent is removed. Hardware validation therefore uses shared native HID
access plus the project-level per-slot lock. Contention with Steam or another
non-project HID consumer remains a manual unsupported-use check.

Mapper tests cover every documented button and hat mapping, neutral state,
stick/pad/trigger normalization, pad release, the default physical-right-stick
mapping and explicit alternate right-pad profile, independent and radial dead
zones, inversion, sensitivity, saturation, finite clamping, smoothing
convergence and reset, and immediate discrete controls while smoothing is
active.

The HID device crate unit-tests platform-neutral collection grouping. On macOS,
enumeration and metadata inspection remain read-only:

```bash
cargo run -p sc-probe -- list
cargo run -p sc-probe -- inspect --index 0
```

After identifying the active official Puck slot and fully quitting Steam, the
whitelisted hardware test is:

```bash
cargo run -p sc-probe -- suppress-lizard --index N --duration-secs 15
```

While it runs, controller `A` must not emit Space and touchpads must not move the
pointer. After it stops, desktop behavior must return within about 10 seconds.
Never run this alongside the visualizer, monitor, Steam, or another bridge.

Linux CI compiles the hardware-independent API and explicit unsupported-platform implementation; it does not require `hidapi` system libraries or physical hardware.

The optional GUI remains part of the workspace build and strict Clippy gates.
On macOS, use `cargo run -p sc-visualizer -- --index N` after `sc-probe list`
to verify live report rate, decoded controls, mapped output, recording controls,
and disconnect-to-neutral behavior with hardware.

Serial tests use an in-memory `ByteTransport` and cover hello success,
latest-only queued state flush and sequence ownership, version rejection,
handshake timeout, bounded overflow, ping/pong timeout, firmware-originated
ping response, and corrupted-frame accounting. Bridge tests additionally cover
replacement of a stale raw HID report before decoding and deferral of the input
timeout while newer HID input is waiting. Physical-port negotiation and
refreshed state delivery have been exercised with a flashed XIAO.

Runtime and CLI tests cover zero-argument defaults, explicit controller/port
overrides, exact XIAO metadata filtering, callout-versus-tty filtering,
battery-range handling, latest-report replacement, and replay's unchanged dump
default. Menu-model tests cover status strings, battery unknown/percentage,
error visibility, and Start/Stop enablement. macOS tests build the tray
frontend, diagnostics renderer, and template icon.

The `macos-app` CI job builds the current-architecture release binary, creates
an `LSUIElement` `.app`, ad-hoc signs and verifies it, archives it, and uploads
the bundle artifact. This proves source packaging, not Developer ID trust or
notarization.

The 2026-07-27 development-hardware smoke test additionally confirmed:

- the active Puck slot produced valid extended `0x42` reports at about 250 Hz;
- the XIAO enumerated CDC plus an Xbox-layout gamepad and bound to macOS's
  `Xbox360Gamepad` DriverKit class;
- Safari reported a connected standard-mapped gamepad;
- Boosteroid detected the XIAO as a valid gamepad;
- an unchanged active state refreshed for more than 30 seconds without an
  unintended firmware neutral;
- the Puck accepted the fixed lizard-off feature report, and a 30-second
  end-to-end serial run completed ten suppression refreshes with zero
  lizard-write, decode, dropped-report, or serial failures.
- zero-argument discovery selected active Puck interface 2 and the
  `/dev/cu.usbmodem11201` XIAO by exact metadata plus Hello, reached `Running`,
  enabled suppression, and surfaced a valid 94% battery report.

The hardware observation that `A` emits no Space, touchpads do not move the
pointer, and desktop mode returns within about 10 seconds is still pending.
Refresh failure handling, full mapping, GeForce NOW, fault timing, reconnect,
and soak gates also remain unproven.

Bridge-core tests cover changed-state suppression, timeout neutralization,
disconnect/reset/shutdown neutralization, repeated decode failures, and HID
reconnect accounting. An end-to-end hardware-independent bridge replay smoke
test can reuse a simulator recording:

```bash
cargo run -p gamepad-simulator -- automated --interval-ms 0 \
  --output recording --file /tmp/bridge-input.jsonl
cargo run -p sc-bridge -- --input replay --file /tmp/bridge-input.jsonl \
  --deterministic --output file --output-file /tmp/bridge-output.frames
```

An end-to-end pre-hardware smoke test is:

```bash
cargo run -p gamepad-simulator -- automated --interval-ms 0 \
  --output recording --file /tmp/sc-session.jsonl
cargo run -p sc-replay -- /tmp/sc-session.jsonl --deterministic \
  --output file --output-file /tmp/sc-session.frames
```

The resulting `.frames` file contains fixed-size, CRC-protected protocol frames suitable for inspection and firmware parser testing.
