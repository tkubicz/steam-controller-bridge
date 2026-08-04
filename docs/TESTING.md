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
behavior, one streaming service, and an extended play session. Detailed steps are
in the firmware README; these results must not be inferred from native or CI
builds.

The workspace has no hardware-dependent tests. Run the same gates as CI:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-targets
./tools/build-macos-app.sh
```

The protocol tests cover every message type, fixed axis endpoints, the static CRC vector, partial and combined reads, garbage, truncation, invalid versions, oversized lengths, corruption, and recovery. A deterministic pseudo-random byte-stream test checks that framing never panics.

Rumble coverage includes the protocol type-8 little-endian vector, exact
eight-byte Xbox command validation and `u8 * 257` scaling, the complete
ten-byte SC2 output vector, independent/zero/full-scale channels, latest-value
coalescing, 25 ms firmware feedback, 40 ms actuator refresh, 100 ms lease
expiry, 500 ms degraded retry, reconnect clearing, and safety-zero priority.

Automatic-shutdown coverage includes the complete 64-byte `0x9f`/`off!`
feature vector, exact device allowlist, charge-state typing, meaningful
activity without report-arrival resets, timeout changes, Puck-only fresh-dock
detection, one-shot placement latching, scheduled three-write bursts,
disconnect-after-success handling, failure recovery, runtime command updates,
and neutral-before-power-off ordering. These portable tests do not prove that
real hardware accepts or remains asleep after the command.

Recording tests cover typed raw and gamepad round trips, timestamp ordering, unknown events, seeking, deterministic replay, malformed/truncated input, version rejection, and identical simulator-state replay.

Steam Controller protocol tests cover all 30 OpenPuck button bits, every
trigger/stick/pad/pressure/motion field, both `0x45` and extended `0x42`,
connection/battery/signal reports, anonymized observed Bluetooth `0x45`/`0x43`
goldens, typed recording round trips, incorrect sizes, mismatched IDs, unknown
IDs, and arbitrary truncated lengths.

The same crate commits the complete 64-byte SDL-compatible lizard-off golden
vector. Device tests cover exact classification and rejection boundaries for
the `28de:1304` USB/`ff00:0001`/interface 2–5 Puck and `28de:1303`
Bluetooth/`ff00:0001`/interface -1 collection, plus immediate, three-second,
disconnect, and reconnect heartbeat scheduling. Bridge tests cover
safe-default option parsing, unique active-source selection, initial
suppression, periodic refresh, leave mode, and fail-closed write behavior. The
macOS device tests also verify that a second project process cannot acquire the
same per-input ownership lock and that ownership is released when the first
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

After identifying an active exact supported Puck or Bluetooth collection and
fully quitting Steam, the whitelisted hardware test is:

```bash
cargo run -p sc-probe -- suppress-lizard --index N --duration-secs 15
```

While it runs, controller `A` must not emit Space and touchpads must not move the
pointer. After it stops, desktop behavior must return within about 10 seconds.
Never run this alongside the visualizer, monitor, Steam, or another bridge.

Test the controller actuators without the XIAO:

```bash
cargo run -p sc-probe -- rumble --index N --low 32768 --high 0
cargo run -p sc-probe -- rumble --index N --low 0 --high 32768
cargo run -p sc-probe -- rumble --index N --low 65535 --high 65535 \
  --duration-ms 1000
```

Then flash the updated firmware, start `./sc-bridge`, and verify GamepadTester's
one-second and infinite vibration actions. Hardware acceptance covers unequal
strong/weak magnitudes, Boosteroid, GeForce NOW, rapid and continuous effects,
and zero within 100 ms after effect stop, browser exit, bridge termination, or
controller/XIAO disconnect. These physical results must not be inferred from unit
tests or a successful firmware build.

Test the fixed controller power-off command separately before enabling an
automatic policy on development hardware:

```bash
cargo run -p sc-probe -- power-off --index N
```

Run the command for the exact active Puck collection with the controller both
undocked and docked/charging, then for the exact supported Bluetooth collection.
Record the `0x43` charge-state sequence for placement and removal. Acceptance
requires that charging continues while powered off, the controller does not
immediately reconnect while still docked, pressing Steam wakes it, and the
post-command state tail fits inside the 2.5-second discovery cooldown. A
Bluetooth disconnect after at least one successful write is a successful
outcome. Do not infer any of these results from unit tests.

After that gate, exercise live automatic shutdown with:

```bash
./sc-bridge --idle-shutdown 5
./sc-bridge --idle-shutdown never --puck-dock-action power-off
```

For the idle path, hold every meaningful control beyond the deadline, then
release to neutral and verify exactly one shutdown after the full interval.
Verify IMU motion, state-report traffic, rumble, and sub-dead-zone noise do not
reset the timer. For the Puck path, verify immediate shutdown from a fresh
charging report, no Bluetooth/USB-cable false positive, no repeat when waking
on the same placement, re-arming only after `Discharging`, and safe recovery
from an injected write failure. Stop/Quit must neutralize and restore lizard
mode without powering the controller off.

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
default. Menu-model tests cover Puck/Bluetooth source rendering, battery
unknown/percentage, haptics state, error visibility, and Start/Stop enablement.
macOS tests build the tray frontend, diagnostics renderer, and template icon.

The `macos-app` CI job builds the current-architecture release binary, creates
an `LSUIElement` `.app`, ad-hoc signs and verifies it, archives it, and uploads
the bundle artifact. This proves source packaging, not Developer ID trust or
notarization.

The Release Please workflow separately validates the exact tagged source and
builds both firmware formats and the macOS application into a draft release.
Only after tests, dependency policy, checksums, artifact upload, and generated
release-note comparison succeed does it publish the draft. A failed release run
therefore leaves an inspectable draft instead of an incomplete public release.

The menu app renders and retains one native image for each of its four status
states at startup. A native memory stress run cycles them 1,000 times and
compares `leaks` memory graphs before and after. Acceptance requires the 352 KiB
CoreUI/ColorSync allocation count, decoded 48 KiB image count, and
definite leaked-byte count to remain unchanged; ordinary Rust heap tests cannot
substitute for this AppKit check.

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
- on 2026-07-28, active Puck index 43 accepted separate left-only and right-only
  50% diagnostics, each with seven 40 ms writes and a successful final zero.
  The updated firmware then flashed successfully; a bounded live bridge
  re-negotiated, stayed Running, and shut down cleanly.

Subsequent hardware acceptance confirmed full control-by-control mapping,
Boosteroid and GeForce NOW as a standard gamepad, end-to-end dual rumble with
correct strong/weak actuator orientation, and that `A` emits no Space while
touchpads do not move the pointer, with desktop mode returning after exit.
Continuous play of more than an hour completed without degradation.

On 2026-07-28, direct Bluetooth was additionally observed as `28de:1303`,
transport Bluetooth, usage `ff00:0001`, interface `-1`. Its vendor collection
produced complete 46-byte `0x45` states at approximately 67–68 Hz, including
normal sequence wrap, and compatible `0x43` battery reports reporting 97%.
With the idle four-slot Puck still attached, the rebuilt zero-argument bridge
selected only the active Bluetooth source, completed Hello with the XIAO at
`/dev/cu.usbmodem11201`, reached `Running`, surfaced the battery, and shut down
cleanly. A separate seven-second diagnostic completed the initial Bluetooth
lizard-off write and two three-second refreshes with no failures. Independent
left-only and right-only 50% rumble diagnostics each completed seven output
writes and an explicit final zero without a HID error.

Bluetooth full-control mapping, lizard suppression, rumble, reconnect,
sleep/wake, refresh failure handling, and fault timing remain hardware
acceptance work until explicitly recorded here.

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
