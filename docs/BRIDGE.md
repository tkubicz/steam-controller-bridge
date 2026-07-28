# Integrated Host Bridge

This bridge targets Steam Controller 2 (2026) through the official Proteus
Puck. See the [end-user guide](USER_GUIDE.md) for pairing, firmware, macOS
permissions, and troubleshooting.

## Zero-configuration live mode

The normal workflow is:

```bash
./sc-bridge
```

Live mode defaults to automatic active-Puck discovery, automatic XIAO
discovery, negotiated serial output, and lizard-mode suppression. The process
may start before either endpoint is present. It scans at 500 ms intervals and
reports state transitions only when they change.

Puck discovery considers only official `28de:1304`, usage `ff00:0001`,
interface 2–5 collections. It opens candidates by stable HID identity and
observes them without sending feature reports. A slot becomes active only
after a complete valid `0x42` or `0x45` state report. Zero active slots waits;
multiple active slots fail safely and require `--index N`.

XIAO discovery enumerates serial metadata rather than matching filenames. An
automatic candidate must be a macOS `/dev/cu.*` callout port with exact
`Lynxware / Steam Controller Bridge`, `045e:028e` metadata and must complete
the protocol-v1 Hello handshake. This rejects the Puck's own `usbmodem` port.
The runtime remembers the MCU-derived USB serial number so it can prefer the
same board after its device path changes. Multiple valid XIAOs require an
explicit `--port PATH`.

Overrides remain available:

```bash
./sc-bridge --controller auto --port auto
./sc-bridge --index 43
./sc-bridge --port /dev/cu.usbmodem11201
./sc-bridge --output dump
```

Replay keeps textual dump output as its default:

```bash
./sc-bridge --input replay --file session.jsonl --deterministic
```

## Shared runtime and status

`bridge-runtime` owns discovery, HID/serial lifecycle, lizard suppression,
decode/map/output orchestration, recording, battery state, metrics, and safety
cleanup. `BridgeRuntime::spawn(RuntimeConfig)` returns a `BridgeHandle` with
idempotent Start, Stop, and Shutdown commands plus a thread-safe latest
`BridgeStatus`.

Status distinguishes `Stopped`, `Discovering`, `Waiting`, `Starting`,
`Running`, `Stopping`, and `Error`. It includes selected Puck identity,
controller-state connectivity, XIAO path/serial/Hello state, lizard refresh
state, a best-effort battery percentage from `0x43`, bridge/output metrics, and
the latest actionable error. Battery values above 100 are ignored and battery
state is cleared when the controller disconnects or the slot changes.

The macOS `sc-bridge-menu` binary embeds this same runtime. It does not launch a
CLI subprocess.

## Lifecycle and safety

Input reports use one replaceable latest-state slot. New snapshots overwrite
stale motion if output temporarily stalls; lifecycle events remain ordered.
The bridge sends neutral after 200 ms without a valid controller state or after
three consecutive decode failures.

If the selected slot stops producing valid states for one second, the XIAO has
already been neutralized by the 200 ms deadline. The runtime then stops the
lizard heartbeat, releases the Puck, and returns to active-slot discovery. If
the XIAO disappears, firmware CDC/data watchdogs neutralize it, host input
acceptance and lizard refreshes stop, and both endpoints are rediscovered.

Every owned transition follows this ordering:

1. send or attempt XIAO neutral;
2. stop and join the Puck/lizard worker;
3. release output and HID devices.

Live mode sends the fixed SDL-compatible lizard-off report only after one slot
is selected and refreshes it every three seconds. An initial or refresh write
failure is fail-closed: input stops, neutralization is attempted, the HID
worker ends, and status becomes an actionable error. No lizard-on command is
sent; the controller watchdog restores desktop mode after heartbeats stop.

Native HID access remains shared because the tested Puck rejects macOS seize
access. A project-level ownership lock excludes another bridge/probe/visualizer,
but Steam and its persistent IPC helper must be stopped manually:

```bash
launchctl bootout user/$(id -u)/com.valvesoftware.steam.ipctool
```

Discovery never kills a process or modifies `launchd`.

## Menu-bar app and logs

Build and ad-hoc sign the menu-only local `.app`:

```bash
./tools/build-macos-app.sh
open "dist/Steam Controller Bridge.app"
```

The tray is created on the main thread after the `winit` event loop starts. It
polls status at 250 ms and updates menu items only when the snapshot revision
changes. The app starts the runtime automatically and exposes Start, Stop,
Copy Diagnostics, Input Monitoring settings, log folder, and Quit.

Structured status transitions are written with bounded rotation under
`~/Library/Logs/Steam Controller Bridge/`. The bundle identifier is
`com.lynxware.steam-controller-bridge`, `LSUIElement` removes ordinary Dock
presence, and the source-build package is ad-hoc signed. Developer ID signing,
notarization, DMG distribution, and Launch at Login are intentionally deferred.
