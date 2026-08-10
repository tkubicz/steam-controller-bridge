# Architecture

The input path and bounded reverse-feedback path are:

```text
Steam Controller HID -> protocol decoder -> SteamControllerState
                                             |              |
                                             v              v
                                   mapping and filters  desktop-bindings
                                             |           |          |
                                             v           v          v
keyboard/automated simulator ----------> GamepadState  macOS   finite pad tick
                                                       input         |
                                                                     v
                                                            SC2 pad actuators
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
- `desktop-bindings` owns the versioned profile schema, validation and atomic persistence, bindable-control edge engine, pad motion/feedback policy, shared-output reference counts, and platform-neutral `DesktopInputSink`. Its target-gated macOS adapter is the only layer that exposes Enigo types.
- `macos-power-monitor` is the deliberately narrow IOKit/CFRunLoop ownership boundary for committed sleep and completed wake notifications. It acknowledges sleep only after the runtime callback has closed hardware, and exposes no raw platform pointers.
- `profile-picker` owns the in-game profile wheel as pure logic: hold timing, stick angle to wheel sector with hysteresis, paging, and the set of controls an open wheel takes over. It knows nothing about how the wheel is drawn or how a profile is applied, and it never learns profile names -- only how many there are and which is active.
- `bridge-core` owns the hardware-independent integrated state machine: decode/map timing, changed-state suppression, reconnect counters, repeated-failure and timeout safety, mapper reset, neutral output, and applying host output suppression to the mapped state.
- `bridge-runtime` owns reusable live discovery and orchestration: uniquely active Puck-or-Bluetooth source selection, metadata/Hello-verified XIAO selection, HID/serial ownership, lizard suppression, bounded rumble leasing, typed battery/charge state, meaningful-idle tracking, one-shot Puck-dock detection, automatic power-off, status snapshots, reconnect recovery, and neutral/rumble-zero-before-release cleanup. Its shared status-log tracker converts snapshots into immediate semantic deltas, five-minute full snapshots, and error-context snapshots for both frontends.
- `gamepad-simulator` owns deterministic and keyboard-driven sources. It depends on output interfaces but not protocol internals.
- `sc-replay` is a thin CLI over `recording` and the existing output backends. It contains no format parser of its own.
- `sc-probe` lists and inspects all HID collections, monitors one explicitly selected collection, and records raw lifecycle/report events plus optional decoded states. It does not contain hard-coded device identifiers or feature-report bytes.
- `sc-visualizer` owns the optional `eframe` GUI and the consolidated Lizard Mouse Lab. Its ordinary dashboard uses a dedicated HID polling thread feeding a 64-report `InputMailbox`; the UI drains it into decoder, mapper, background recording, and mock-output diagnostics. While there is room it preserves every report. Only under pressure does it remove redundant analog samples inside a run with the same 32-bit button mask; if no safe sample exists, overflow keeps the newest report as a recovery baseline so the display cannot stick on a stale press. Lifecycle transitions stay ordered, repeated identical backend errors are coalesced, and every report loss is counted. Before the full-window lab takes ownership, the visualizer finishes ordinary recording, neutralizes output, and stops and joins that worker; leaving restarts it with clean decoding state. A process-lifetime HID broker serializes every GUI enumeration/open call on one thread because macOS hidapi binds its global `IOHIDManager` to that thread's Core Foundation run loop; the broker retains no open controller session between owners. Lab capture bypasses the coalescing mailbox and merges controller HID with a passive macOS HID event tap through its lossless 8,192-event queue and timestamp reorder window. Portable analysis, comparison, and dump replay consume additive v1 lab events and older raw recordings; desktop replay is headless-only and pointer-motion-only. `--demo-state neutral|digital|analog|disconnected` still drives the ordinary mapping and rendering path from explicit decoded fixtures and does not claim fabricated raw wire bytes.
- `steam-controller-discovery` owns active-source scan, retained candidate sessions, probing and unique-selection policy. Both `bridge-runtime` and `sc-visualizer` depend on this narrow device-plus-protocol layer, so the diagnostic GUI does not pull in bridge orchestration, desktop bindings, profile picking or power monitoring merely to find a controller.
- `controller-art` holds the traced Steam Controller illustration in unit-square coordinates, shared by the bindings editor and the visualizer. Callers describe each control's state — selected, hovered, pressed, its live position or trigger travel — and the crate decides how to paint it, which is what lets the editor's "selected" and the visualizer's "pressed" share one drawing. It depends only on `eframe` and `ui-theme`: it knows nothing about bindings or report bits, so each consumer owns its own adapter into `Control`. Note that the source-bit names for View and Menu are reversed against the physical buttons (see [MAPPING.md](MAPPING.md)); the crate is named after the physical buttons and consumers cross them over.
- `sc-bridge` is a thin command-line frontend over `bridge-runtime` for live input and `recording` for replay. Its live defaults are automatic discovery plus serial output.
- `sc-bridge-menu` is a macOS-only, windowless `winit`/`tray-icon` frontend over the same runtime. It owns menu rendering, actions, log rotation, and local `.app` packaging, not controller logic. At startup it renders the four status states once, retains their native images through `tray-icon`'s documented `NSStatusItem` access, and swaps those same objects on transitions instead of creating new AppKit/CoreUI cache identities. It also owns the profile store the wheel chooses from, resolving a committed index through the same path the tray submenu uses.
- The bindings editor and the profile overlay are separate processes of that same binary, because `eframe::run_native` owns an event loop and the menu app's is already committed to the status item. The overlay is a pure display driven by newline-delimited JSON on its stdin; it makes no decisions and takes no input, which is what lets its window be non-activating and click-through. See [PROFILE_OVERLAY.md](PROFILE_OVERLAY.md).

All project-authored crates except `macos-power-monitor` forbid unsafe code.
That crate contains the documented IOKit callback and Core Foundation ownership
operations required for system-power notifications; its public API is safe and
typed. Recording and desktop bindings use the shared serialization libraries;
only recording uses base64. The GUI, HID, mapping, and serial layers remain
separate components.

## Lifecycle and safety

Simulator modes send a neutral state before normal exit. Outputs reject non-finite and out-of-range values instead of silently emitting invalid packets. The integrated bridge additionally sends neutral on input timeout, controller disconnect, repeated decode failure, reset, and shutdown. Serial output refreshes unchanged active states while its firmware watchdog is armed. Rumble feedback is latest-only: a 100 ms host lease and the controller's own actuator watchdog prevent stale rumble. Pad feedback consists of finite ticks, coalesced to the latest pending strength per side and discarded on failure or lifecycle reset. Either write path degrades haptics without disabling input.

Automatic shutdown is host policy rather than part of the CDC protocol. A
portable tracker derives meaningful activity from decoded physical controls and
the already dead-zoned mapped state. The HID worker alone may send the fixed
power-off feature report. Automatic shutdown preserves the safety sequence
XIAO neutral -> rumble zero -> stop lizard heartbeat -> power-off burst -> HID
release, then holds the stable source identity in a 2.5-second discovery
cooldown. Immediate Puck placement uses typed `0x43` charge state and a
supervisor-level one-shot latch; it is not represented as a zero-minute idle
timeout.

System sleep is handled by the runtime, because the runtime owns the hardware
handles. For serial output it automatically registers `macos-power-monitor`,
so every frontend inherits the same protection: on committed sleep the runtime
parks the XIAO at neutral, closes the serial port and HID sessions, and only
then lets IOKit acknowledge; on wake it resumes discovery after a two-second
settle so CDC can finish re-enumerating. The public
`BridgeHandle::suspend_for_sleep` and `request_resume_from_wake` methods remain
available for explicit lifecycle integration and tests. Monitor registration
is fail-closed: serial hardware cannot start without it. Serial I/O left in
flight across sleep/wake has panicked macOS's USB CDC-ACM kext, so this safety
boundary is part of runtime ownership rather than optional frontend policy.
Suspension is orthogonal to start/stop: a bridge the user stopped stays stopped
after wake, and a running one resumes on its own.

The HID session uses a bounded 1,024-byte report buffer. Automatic discovery
opens only exact official Puck and direct Bluetooth vendor collections by
stable identity and selects the unique source producing complete state
reports. Inactive candidate sessions remain open and are reconciled against
Valve-VID-filtered metadata scans followed by exact identity checks; scans run
every 500 ms while no candidate can be opened. Once candidates are open,
unchanged inventories back off from two seconds to a ten-second ceiling and
reset to two seconds when membership or open status changes. Explicit
global-index selection uses the same adaptive ceiling for its full-system
lookup while waiting for the collection to open. Unchanged collections are
never repeatedly opened and closed while the controller is asleep.

HIDAPI already receives reports on a native background reader for every open
candidate. Discovery checks those buffered queues with at most four
nonblocking reads per candidate every 500 ms, rather than creating sequential
timed waits. This retains the existing controller-wake bound while letting the
supervisor sleep between checks. Waiting XIAO service similarly checks the
native serial queued-byte count before reading, avoiding an empty one-millisecond
serial poll without changing the write timeout or active-state watchdog cadence.

Keeping discovery sessions open also keeps this project's per-collection
ownership locks continuously. Other bridge, probe-open, and visualizer
processes therefore cannot open any supported candidate until discovery stops,
although metadata enumeration remains available. This is intentional resource
stability behavior, but whether a continuously open macOS IOHIDDevice changes
controller sleep or long-idle battery behavior remains a hardware observation
item. The integrated runtime runs the selected session behind a 64-entry
transition mailbox. Analog-only reports replace the newest queued report only
when the five bindable bits and two pad-touch bits are unchanged; each button or
touch edge remains ordered. Overflow releases desktop outputs, discards stale
transitions, and retains the newest state as a non-emitting recovery baseline.
Lifecycle events remain ordered. A lost source returns to discovery instead of
trusting a stale numeric index.

Serial discovery retains USB metadata, rejects non-XIAO `usbmodem` ports, and
uses the protocol Hello exchange as the final identity check. The MCU USB
serial, rather than the transient callout path, provides reconnect preference.
