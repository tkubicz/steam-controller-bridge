# Architecture

The input path and bounded reverse-feedback path are:

```text
Steam Controller HID -> protocol decoder -> SteamControllerState
                                             |
keyboard/automated simulator                  v
          |                         mapping and filters
          |                                  |
          +-------------> GamepadState <-----+
          |
          v
 explicit wire conversion and framing
          |
          v
 mock / dump / file / JSONL recording
                         |
                         v
                 deterministic/timed replay

game/browser -> Xbox OUT -> XIAO -> CDC Rumble -> bridge-runtime
                                                  |
                                                  v
                                      SC2 dual actuator output
```

## Crates

- `gamepad-state` owns platform-neutral state, stable button indices, validation, and sanitization.
- `bridge-protocol` owns integer wire representation, messages, framing, CRC, and stream recovery. It never copies Rust object memory into packets.
- `bridge-output` owns the `GamepadOutput` boundary and hardware-independent backends. `ChangedOnly` is an optional policy wrapper; file output preserves every state by default.
- Its serial submodule separates the `ByteTransport` session state machine from the native port adapter. It owns hello negotiation, bounded latest-state queuing, sequence numbers, ping/pong health, reconnect attempts, neutral shutdown, and transport metrics while reusing `bridge-protocol` framing.
- `recording` owns the versioned JSONL envelope, ordered writer, typed raw/final-state payloads, unknown-event preservation, and deterministic or real-time replay.
- `steam-controller-device` owns HID collection metadata, raw reports, lifecycle events, stable enumeration ordering, and reconnecting sessions. Only its private `platform` module depends on `hidapi`; non-macOS builds expose an explicit unsupported-platform stub.
- `steam-controller-protocol` owns the Steam Controller 2 host-facing `0x45`/`0x42` state layouts, status reports, button masks, motion fields, and structured decode errors. It has no HID or transport dependency and preserves each complete validated report.
- `controller-mapper` owns the validated default mapping profile and allocation-free filter pipeline. Its only inputs are decoded controller state, elapsed time, and explicit configuration; disconnect handling resets its optional smoothing history.
- `bridge-core` owns the hardware-independent integrated state machine: decode/map timing, changed-state suppression, reconnect counters, repeated-failure and timeout safety, mapper reset, and neutral output.
- `bridge-runtime` owns reusable live discovery and orchestration: uniquely active Puck-or-Bluetooth source selection, metadata/Hello-verified XIAO selection, HID/serial ownership, lizard suppression, bounded rumble leasing, battery/status snapshots, reconnect recovery, and neutral/rumble-zero-before-release cleanup.
- `gamepad-simulator` owns deterministic and keyboard-driven sources. It depends on output interfaces but not protocol internals.
- `sc-replay` is a thin CLI over `recording` and the existing output backends. It contains no format parser of its own.
- `sc-probe` lists and inspects all HID collections, monitors one explicitly selected collection, and records raw lifecycle/report events plus optional decoded states. It does not contain hard-coded device identifiers or feature-report bytes.
- `sc-visualizer` owns the optional `eframe` GUI. A dedicated HID polling thread feeds a bounded 64-event channel; the UI drains it into decoder, mapper, recording, and mock-output diagnostics without introducing GUI dependencies into any library or CLI.
- `sc-bridge` is a thin command-line frontend over `bridge-runtime` for live input and `recording` for replay. Its live defaults are automatic discovery plus serial output.
- `sc-bridge-menu` is a macOS-only, windowless `winit`/`tray-icon` frontend over the same runtime. It owns menu rendering, actions, log rotation, and local `.app` packaging, not controller logic.

All crates forbid unsafe code. Only the recording layer uses third-party serialization and base64 libraries. The GUI, HID, mapping, and serial layers remain separate components.

## Lifecycle and safety

Simulator modes send a neutral state before normal exit. Outputs reject non-finite and out-of-range values instead of silently emitting invalid packets. The integrated bridge additionally sends neutral on input timeout, controller disconnect, repeated decode failure, reset, and shutdown. Serial output refreshes unchanged active states while its firmware watchdog is armed. Reverse feedback is also latest-only: a 100 ms host lease and the controller's own actuator watchdog prevent stale rumble, while write failures degrade haptics without disabling input.

The HID session uses a bounded 1,024-byte report buffer. Automatic discovery
opens only exact official Puck and direct Bluetooth vendor collections by
stable identity and selects the unique source producing complete state
reports. Inactive candidate sessions remain open and are reconciled against
Valve-VID-filtered metadata scans followed by exact identity checks; scans run
every 500 ms while no candidate can be opened and every two seconds once the
inventory is stable. Unchanged
collections are never repeatedly opened and closed while the controller is
asleep, while complete state reports are still probed in 500 ms windows. Each
probe window performs one bounded read per candidate. The integrated runtime
then runs the selected session behind a bounded standard-library channel with
latest-state semantics for reports while preserving lifecycle events. A lost
source returns to discovery instead of trusting a stale numeric index.

Serial discovery retains USB metadata, rejects non-XIAO `usbmodem` ports, and
uses the protocol Hello exchange as the final identity check. The MCU USB
serial, rather than the transient callout path, provides reconnect preference.
