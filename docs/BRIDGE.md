# Integrated Host Bridge

This bridge targets Steam Controller 2 (2026) through either the official
Proteus Puck or direct macOS Bluetooth. See the
[end-user guide](USER_GUIDE.md) for pairing, firmware, macOS permissions, and
troubleshooting.

## Zero-configuration live mode

The normal workflow is:

```bash
./sc-bridge
```

Live mode defaults to automatic active-source discovery, automatic XIAO
discovery, negotiated serial output, and lizard-mode suppression. The process
may start before either endpoint is present. Missing hardware is scanned every
500 ms. Once controller candidates are open, unchanged metadata scans back off
from two seconds to a ten-second ceiling while already-open HID report queues
are still checked every 500 ms. State transitions are reported only when they
change.

Controller discovery considers only official `28de:1304` USB, usage
`ff00:0001`, interface 2-5 Puck collections and the `28de:1303` Bluetooth,
usage `ff00:0001`, interface -1 collection. It opens candidates by stable HID
identity and observes them without sending feature reports. A source becomes
active only after a complete valid `0x42` or `0x45` state report. An idle Puck
therefore does not block an active Bluetooth controller. Zero active sources
waits; multiple active sources fail safely, list their global `sc-probe`
indices and identities, and require `--index N`.

An explicit `--index N` is a global `sc-probe list` index, so resolving it
requires the full HID inventory. The runtime caches the selected stable
identity and backs the global lookup off from two seconds to a ten-second
ceiling while it is waiting to open the collection; open retries continue every
500 ms. It does not rebuild the full-system HID metadata on every retry.

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

Automatic shutdown is available only with live serial output to a ready XIAO:

```bash
./sc-bridge --idle-shutdown never
./sc-bridge --idle-shutdown 10
./sc-bridge --puck-dock-action power-off
```

The idle timeout defaults to 15 minutes and accepts whole minutes from 1 to
1440, or `never`. The Puck-dock action is independent and defaults to `leave`.
Replay and dump/file/mock diagnostic outputs never power off hardware; passing
either flag alongside one of them is rejected at startup rather than silently
ignored, so a shutdown request is never lost without notice.

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
`Running`, `Stopping`, and `Error`. It includes selected input identity and
`ControllerTransport::{Puck, Bluetooth}`, controller-state connectivity, XIAO
path/serial/Hello state, lizard refresh state, haptics idle/active/degraded
state and counters, desktop binding profile/permission/held-output/failure
state, a best-effort battery percentage from `0x43`,
typed `Discharging`/`Charging`/`Charged` charge state, automatic-shutdown
phase/trigger/counters,
bridge/output metrics, and the latest actionable error. Battery values above
100 are ignored and battery state is cleared when the controller disconnects
or the source changes.

The macOS `sc-bridge-menu` binary embeds this same runtime. It does not launch a
CLI subprocess.

## Lifecycle and safety

Input reports use a bounded 64-entry transition-preserving mailbox. New analog
snapshots overwrite only the newest queued state with the same desktop-binding
mask; bindable button changes remain ordered. Overflow releases all injected
desktop inputs, clears stale transitions, and retains the newest snapshot as a
new non-emitting baseline; lifecycle events remain ordered.
The bridge sends neutral after 200 ms without a valid controller state or after
three consecutive decode failures.

If the selected source stops producing valid states for one second, the XIAO has
already been neutralized by the 200 ms deadline. The runtime then stops the
lizard heartbeat, releases the input, and returns to active-source discovery.
If the XIAO disappears, firmware CDC/data watchdogs neutralize it, host input
acceptance and lizard refreshes stop, and both endpoints are rediscovered.

Every owned transition follows this ordering:

1. send or attempt XIAO neutral;
2. send SC2 rumble zero;
3. stop the lizard heartbeat;
4. release output and HID devices.

Configured automatic shutdown inserts the fixed controller power-off command
between steps 3 and 4. The runtime first requires a ready XIAO and neutralizes
it, cancels the rumble lease and writes zero, stops future lizard refreshes,
then schedules a three-write `0x9f "off!"` burst in the HID worker before
releasing ownership. A 2.5-second stable-identity cooldown ignores trailing
post-shutdown reports. A failed power-off write is nonfatal to gameplay: lizard
suppression is restored, input resumes, status becomes `Degraded`, and retries
are bounded to once every 30 seconds.

Meaningful idle activity is any physical button, mapped stick/trigger outside
its existing dead zone, trackpad touch/click, or grip touch. Report arrival,
sequence changes, IMU data, sub-dead-zone jitter, lizard heartbeats, rumble, and
XIAO state refreshes do not count. Neutral time begins on the active-to-neutral
transition and resets on reconnect, Start, a setting change, or entry into a
known charging state.

The immediate Puck action is edge-triggered from a fresh valid `0x43` report on
the selected Puck source with charge state `Charging` or `Charged`. It does not
infer placement from enumeration or battery percentage, and Bluetooth external
power never counts as Puck placement. A supervisor-level latch makes it fire
once per placement even across worker release, cooldown, and a deliberate wake
while still docked. A later fresh `Discharging` report re-arms it.

Live mode sends the fixed SDL-compatible lizard-off report only after one source
is selected and refreshes it every three seconds. An initial or refresh write
failure is fail-closed: input stops, neutralization is attempted, the HID
worker ends, and status becomes an actionable error. No lizard-on command is
sent; the controller watchdog restores desktop mode after heartbeats stop.

Rumble travels in the opposite direction. The XIAO validates the Xbox OUT
packet and renews a CDC lease every 25 ms. The HID worker applies changed
values immediately, refreshes nonzero actuator output at 40 ms, and expires the
lease at 100 ms. Only the newest command is retained. A write failure marks
haptics `Degraded`, leaves input running, and retries no more often than every
500 ms while fresh lease frames continue. Reconnect requires a new
post-reconnect lease.

Native HID access remains shared because the tested Puck rejects macOS seize
access. A project-level ownership lock keyed by stable device identity excludes
other project processes. In automatic discovery, the bridge keeps every
supported candidate session and its ownership lock open continuously while it
waits for one to become active, not only during each 500 ms probe. Therefore
`sc-probe` commands that open a collection and `sc-visualizer` cannot use any
candidate slot until the bridge is stopped; `sc-probe list` remains available
because enumeration does not open a collection. This persistent ownership
avoids repeated macOS HID reader allocation. Its effect on controller sleep and
battery during a long idle has not yet been measured, so long-idle hardware
tests should monitor both; stop the bridge when prolonged idle battery behavior
matters. Steam and its persistent IPC helper must also be stopped manually:

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
changes. The menu uses short grouped lines rather than exposing device
identities, paths, or raw errors in its main status. A bounded friendly problem
summary is paired with `Copy Full Error`; `Copy Diagnostics` and the log folder
retain the complete context.

The template icon reflects the whole usable path: an x badge means Off,
ellipsis means On but waiting for hardware, a check means the controller
source and XIAO are both ready, and an exclamation mark means Action required.
The app starts the runtime automatically and exposes Start, Stop, a dynamic
Profiles submenu/editor, Profile Wheel settings, permission shortcuts, About,
updates, the log folder, and Quit. About, Changelog, and Updates are tabs in one
foreground Steam Controller Bridge child window; the hero reports both the
application version and connected firmware revision. Bounded line-delimited
JSON carries tab navigation, firmware status changes, and updater lifecycle
requests. Only an acknowledged firmware action or final app replacement
shutdown releases runtime resources. Binding failures have
their own ready/permission/degraded status and never stop gamepad output.

Structured status transitions are written with bounded rotation under
`~/Library/Logs/Steam Controller Bridge/`. Meaningful transitions use concise
`status_change` records; full `status_snapshot` records are written on startup,
every five minutes even when the revision is unchanged, and immediately for a
new error or failure-counter increase. The CLI uses the same records on stderr.
The bundle identifier is `com.lynxware.steam-controller-bridge`, `LSUIElement`
removes ordinary Dock presence, and the source-build package is ad-hoc signed.
Developer ID signing, notarization, DMG distribution, and Launch at Login are
intentionally deferred.
