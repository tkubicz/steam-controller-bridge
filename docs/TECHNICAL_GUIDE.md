# Steam Controller Bridge - Technical Guide

Use a Steam Controller 2 as a standard USB Xbox gamepad on macOS - without Steam
running - in browsers, games, and cloud gaming services. A Seeed Studio XIAO
nRF52840 does the translation in hardware.

> **Requirements:** macOS 13 or later, a **Steam Controller 2 (2026)**, and a
> non-Sense XIAO nRF52840. The original 2015 Steam Controller and its receiver
> use a different protocol and are not supported.

For hardware requirements, firmware flashing, Steam Controller 2 pairing,
macOS permissions, daily startup, verification, and troubleshooting, start with
the [user guide](USER_GUIDE.md).

## Download

Grab the latest [release](https://github.com/tkubicz/steam-controller-bridge/releases):

| File | What it is |
| --- | --- |
| `steam-controller-bridge-xiao-nrf52840.uf2` | Firmware. Double-tap RESET on the XIAO and copy this onto the drive that mounts. |
| `steam-controller-bridge-macos.zip` | The menu-bar application. |
| `steam-controller-bridge-xiao-nrf52840-dfu.zip` | Firmware for serial DFU flashing, if you prefer `make flash`. |
| `steam-controller-bridge-update-manifest.json` | Signed application and firmware release metadata used by the in-app updater. |
| `steam-controller-bridge-update-signatures.json` | Ed25519 signatures for the exact manifest bytes. |

After installing the menu app, use **Check for Updates…** for normal application
and firmware updates. It opens the Updates tab, which downloads nothing beyond
signed metadata until you choose an action. Application replacement is Finder-guided in v1;
firmware flashing is performed by the app and does not require Arduino CLI.

Verify a download against the published sums before flashing:

```bash
shasum -a 256 -c SHA256SUMS.txt
```

The application is ad-hoc signed rather than notarized, so macOS blocks it on
first launch. Right-click the app and choose **Open** once; see
[opening an unnotarized build](USER_GUIDE.md#opening-an-unnotarized-build).

Building from source instead is fully supported - see [build and test](#build-and-test).
Source builds do not trust production updates unless the builder deliberately
embeds the release public keys.

GitHub releases are the only distribution channel today. A Homebrew cask and an
App Store build are intended later. Nothing here is published to crates.io: every
workspace crate sets `publish = false`, and the library crates exist to structure
this application rather than to be depended on externally.

## Why it identifies as an Xbox 360 controller

The firmware enumerates with the Xbox 360 compatibility VID/PID `045e:028e`.
This is deliberate: Apple's built-in driver will not publish a generic HID
gamepad to GameController, so a standards-based HID personality enumerates at the
USB layer but stays invisible to Safari, games, and streaming clients. Borrowing
the identity is what makes the device work at all on macOS.

It is not a claim of ownership or affiliation, and a distributable product would
need an owned or licensed USB identity plus a re-qualified macOS recognition
path. See [THIRD-PARTY-NOTICES.md](../THIRD-PARTY-NOTICES.md).

## Status

Working end to end on macOS. Both the official Puck and direct Bluetooth input
paths feed the host bridge and XIAO nRF52840 CDC-to-gamepad firmware. The
following are verified on hardware:

- the browser Gamepad API in Safari, reporting `mapping: standard`;
- Boosteroid and GeForce NOW, both driven as a standard Xbox-layout gamepad;
- every physical control: buttons, both sticks, both triggers, and the D-pad;
- end-to-end dual rumble, from a client vibration request through to the
  correct strong and weak Steam Controller 2 actuators;
- lizard-mode suppression, with no stray Space keypresses or pointer motion
  while running and normal desktop behavior restored after exit;
- zero-argument discovery of the uniquely active Puck or Bluetooth collection
  and the metadata/Hello-verified XIAO port, including live battery reporting;
- direct Bluetooth `28de:1303` input using 46-byte `0x45` state reports at
  approximately 67-68 Hz and compatible `0x43` battery reports;
- more than an hour of continuous gameplay without degradation.

Bluetooth reconnect/sleep-wake stress and in-game control/rumble confirmation
remain hardware acceptance gates. Distributing this to end users additionally
requires the USB identity and code-signing work described under
[known limitations](#known-limitations).

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

## Contributing and releases

Pull requests use Conventional Commit titles and squash merging. Release Please
turns those merged titles into the workspace version, `CHANGELOG.md`, the
version tag, and the matching GitHub Release notes. Maintainers review and merge
the generated release pull request; they do not maintain a second set of notes
or create version tags manually. See [CONTRIBUTING.md](../CONTRIBUTING.md) for title
formats, multi-entry changes, and the stable-release procedure.

## Daily use

After flashing the XIAO, connecting the Steam Controller 2 through its official
Puck or macOS Bluetooth, granting Input Monitoring permission, and fully
stopping Steam and its IPC helper, start the complete bridge from the
repository root:

```bash
./sc-bridge
```

No HID index or serial path is normally needed. The bridge waits if either
endpoint is absent, identifies the one supported Puck or Bluetooth collection
producing complete `0x42`/`0x45` controller states, verifies the XIAO with a
protocol-v1 Hello handshake, and resumes after either endpoint is reconnected.
It refuses to guess when more than one active controller source or valid XIAO
is present.

Use `./sc-bridge --index N` or `./sc-bridge --port /dev/cu.usbmodem…` only to
resolve an ambiguity or diagnose a specific endpoint. See the
[user guide](USER_GUIDE.md) for the one-time setup.

The live bridge defaults to powering an inactive controller off after 15
minutes of meaningful neutral input. Continuous state reports, IMU noise,
rumble, and XIAO refresh traffic do not postpone it. Configure the timeout with:

```bash
./sc-bridge --idle-shutdown never
./sc-bridge --idle-shutdown 5
```

An independent opt-in can turn the controller off as soon as a fresh Puck
`Charging`/`Charged` report confirms it was placed on the official Puck:

```bash
./sc-bridge --puck-dock-action power-off
```

That action fires once per placement. Waking the controller while it remains on
the Puck does not immediately turn it off again; remove it long enough to emit a
fresh `Discharging` report before the next placement can trigger another
shutdown. The option defaults to `leave`.

Extra L4/L5/R4/R5 and Quick Access controls can also drive opt-in desktop
bindings without changing the XIAO firmware or serial protocol. Profiles can
independently enable the right pad as a relative pointer and the left pad as
accelerated smooth two-axis scrolling with configurable speed and optional
release momentum; both pads are off by default and offer configurable movement
and physical-click feedback. The menu app
requests Input Monitoring directly when macOS has not decided yet. Only after
that grant is detected does it request Post Event and Accessibility access, so
the TCC requests remain ordered even when no controller is attached.
Open `Profiles -> Edit Profiles…` to save a profile. Settings shortcuts remain
available for a previously denied request. CLI use is deliberately paired:

```bash
./sc-bridge --bindings "$HOME/Library/Application Support/Steam Controller Bridge/bindings.json" --profile Default
```

Replay rejects these options so a recording can never inject keyboard or mouse
input. See [desktop bindings](DESKTOP_BINDINGS.md).

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

Supported controller collections are opened with macOS shared HID access
because the tested Puck rejects `kIOHIDOptionsTypeSeizeDevice` with `not
permitted`. Project tools take a per-input ownership lock, preventing two
bridge/probe/visualizer processes from sharing one source. Steam and other
non-project consumers do not honor that lock and must be stopped separately.
After identifying either the active `28de:1304` USB `ff00:0001` interface 2-5
slot or `28de:1303` Bluetooth `ff00:0001` interface -1 collection, test the safe
SDL-compatible lizard-mode heartbeat:

```bash
cargo run -p sc-probe -- suppress-lizard --index 0 --duration-secs 15
```

While the command runs, controller buttons and touchpads should no longer
produce native keyboard/mouse input. The controller watchdog restores desktop
mode after the command exits.

Independently of the XIAO, test the two SC2 actuators directly:

```bash
cargo run -p sc-probe -- rumble --index 0 --low 32768 --high 0
cargo run -p sc-probe -- rumble --index 0 --low 0 --high 32768
```

The diagnostic suppresses lizard mode, refreshes rumble every 40 ms, and always
attempts a zero write before it exits.

The narrowly allowlisted power-off diagnostic changes controller state
immediately:

```bash
cargo run -p sc-probe -- power-off --index 0
```

It accepts only an exact supported Puck or Bluetooth controller collection,
sends the fixed `01 9f 04 6f 66 66 21 …` command, and never exposes arbitrary
feature writes. Press Steam to wake the controller again.

The session reports disconnects and automatically attempts to reopen the same collection identity every 500 ms. Capture files include connection metadata, transport, source collection identity, report ID, base64 bytes, and the available dropped-report count. `--decoded` additionally records typed Steam Controller 2 state reports.

macOS may reject protected keyboard/gamepad collections with an IOKit `not permitted` error until the terminal or Codex host has Input Monitoring permission. Listing and metadata inspection do not require opening the collection.

## Lizard mouse comparison lab

`sc-visualizer` includes a full-window Lizard Mouse Lab. Open the visualizer and
use the mode switch at the top of the dashboard to start a guided capture or
open an existing JSONL recording. Entering the lab finishes an ordinary
visualizer recording, neutralizes and disables mock/serial output, and joins
the ordinary HID worker before the measurement worker takes controller
ownership. Leaving the lab restarts ordinary discovery with a clean decoder and
mapper baseline.

The same capture, analysis, comparison, and safe replay tools are available as
nested headless commands:

```bash
cargo run -p sc-visualizer -- lizard capture --output lizard.jsonl --guided
cargo run -p sc-visualizer -- lizard analyze lizard.jsonl --output analysis.json
cargo run -p sc-visualizer -- lizard compare lizard.jsonl --output comparison.json
cargo run -p sc-visualizer -- lizard replay lizard.jsonl --source reference --output dump
```

Capture auto-detects the unique active controller using the same shared
discovery as the bridge and visualizer; `--index N` remains an explicit
multi-controller/debug override. Capture is macOS-only and needs Input
Monitoring because it installs a passive HID-entry event tap. It uses an
8,192-event bounded queue and a timestamp reorder window; overflow, disconnect,
and tap disable explicitly invalidate the file.
Do not use another mouse or trackpad during measured intervals. Analysis,
comparison, the GUI results view, and textual replay are portable and also
accept older visualizer JSONL files.
Desktop replay must be explicitly requested with `--output desktop`; it injects
pointer motion only and never replays lizard keyboard or mouse-button actions.
See [the lab guide](LIZARD_MOUSE_LAB.md).

## Visualizer

The visualizer finds a supported controller on its own, so it usually needs no
arguments. It keeps looking while none is present, so you can start it first and
plug the controller in afterwards:

```bash
cargo run -p sc-visualizer
```

To pin it to one collection from `sc-probe list` instead of discovering one:

```bash
cargo run -p sc-visualizer -- --index 0
```

The visualizer shows raw, decoded, and mapped state; reports connection/rate and
error diagnostics; edits the mapping filters; and records raw, decoded, mapped,
lifecycle, and marker events to JSONL. It supports mock and negotiated serial
output.

See [the wire protocol](GAMEPAD_PROTOCOL.md), [serial transport](SERIAL_TRANSPORT.md), [Steam Controller protocol](STEAM_CONTROLLER_PROTOCOL.md), [mapping](MAPPING.md), [desktop bindings](DESKTOP_BINDINGS.md), [lizard mouse lab](LIZARD_MOUSE_LAB.md), [recording format](RECORDING_FORMAT.md), [updater operations](UPDATES.md), [architecture](ARCHITECTURE.md), [testing](TESTING.md), and [firmware architecture](FIRMWARE_ARCHITECTURE.md).

Firmware setup, native tests, UF2/DFU builds, flashing, recovery, LED states,
and hardware validation are documented in
[`firmware/xiao-nrf52840/README.md`](../firmware/xiao-nrf52840/README.md).

## Integrated bridge

```bash
./sc-bridge
./sc-bridge --index 43 --port /dev/cu.usbmodem11201
./sc-bridge --input replay --file session.jsonl \
  --deterministic --output mock
```

Live mode defaults to automatic active-source and XIAO discovery plus serial
output. It uses bounded latest-state input, timeout and decode-failure
neutralization, reconnect recovery, Ctrl-C shutdown, and structured status. It
claims the selected Puck or Bluetooth collection from other project tools and
refreshes the narrow lizard-off setting every three seconds. Steam must still
be fully quit. A failed suppression write neutralizes the XIAO and stops the
bridge. See [the bridge guide](BRIDGE.md).

## macOS menu-bar app

Build an ad-hoc-signed, dockless local application:

```bash
./tools/build-macos-app.sh
open "dist/Steam Controller Bridge.app"
```

The menu app embeds the same runtime, starts it automatically, and presents
short, grouped bridge, readiness, hardware, battery, haptics, bindings, and problem
lines. `Idle Shutdown` offers Never/5/10/15/30-minute choices, and `Turn Off
When Placed on Puck` controls the independent immediate-dock action. Both are
applied live and saved under `~/Library/Application Support/Steam Controller
Bridge/`. The menu-bar icon distinguishes Off, On but waiting, Controller ready,
and Action required states. The dynamic `Profiles` submenu selects profiles and
launches the editor; permission shortcuts live in the separate `Permissions`
submenu. `Profile Wheel` enables
an in-game radial switcher: hold Quick Access, point either stick at a profile,
press A to apply or B to cancel. It floats over native-fullscreen and
borderless-windowed games, is off by default because it takes Quick Access over,
and is documented in [PROFILE_OVERLAY.md](PROFILE_OVERLAY.md).
Friendly problem summaries stay bounded; use
`Copy Full Error`, `Copy Diagnostics`, or the rotated log folder for the
complete technical detail. Start/Stop, Input Monitoring settings, and Quit are
also available. `About` and `Check for Updates…` open the corresponding tab in
one foreground Steam Controller Bridge window. Its hero reports the installed
application and connected firmware revisions alongside the project summary,
collapsible changelog, and signed update workflow. Logs write
concise `status_change` records immediately, full `status_snapshot` records at
startup and every five minutes, and an immediate full snapshot when an error or
failure appears. Release metadata is Ed25519-signed and application bundles are
ad-hoc code-signed. Developer ID signing, notarization, a DMG, and Launch at
Login remain future work.

## Known limitations

- Steam Controller 2 Puck input is live-tested with extended `0x42` reports;
  Bluetooth is live-tested with primary `0x45` and battery `0x43` reports.
  Direct USB-C input remains unsupported.
- Only the exact official Proteus Puck `28de:1304` active slots and the direct
  Bluetooth `28de:1303` vendor collection are permitted to receive the
  SDL-compatible lizard-off feature report and exact standard dual-rumble
  output, the finite SDL Triton pad tick, plus the fixed `0x9f` power-off
  command. Arbitrary controller
  initialization, settings, mappings, custom
  haptics, and feature/output writes remain intentionally unavailable.
- Automatic-shutdown protocol, scheduling, and recovery are covered by native
  tests, but real Puck/Bluetooth power-off, charge-state transitions, and
  stay-asleep behavior remain an explicit hardware acceptance gate. Use the
  documented `sc-probe power-off --index N` procedure before relying on it.
- The profile wheel cannot appear over a game that captures the display
  exclusively; such a game draws above every window level. Native-fullscreen and
  borderless-windowed games, including those under Game Porting Toolkit or
  Whisky, are supported. The wheel also does not suppress the Quick Access hold
  itself, so the game sees `Extra3` held for the hold duration before it opens.
- Steam coexistence, multiple simultaneous SC2 controllers, and running another
  HID consumer against the selected slot are unsupported.
- Automatic discovery deliberately reports an ambiguity instead of choosing
  when multiple Puck/Bluetooth sources produce controller states or multiple
  XIAOs complete the firmware handshake. Use an explicit override after
  identifying the intended endpoint.
- macOS access is intentionally shared at the native HID layer. The project
  lock excludes other project tools only; Steam's persistent
  `com.valvesoftware.steam.ipctool` LaunchAgent must be booted out before play.
- Standard Xbox button and axis mapping remains fixed. Desktop bindings cover
  L4/L5/R4/R5/Quick Access and the two pad clicks plus fixed right-pad pointer
  and left-pad scroll roles; pressure actions, gestures, trigger clicks, and
  stick clicks are not configurable in this milestone. macOS has native function-key
  identities only through F20, and Enigo does not expose macOS
  play/previous/next media events, so those portable entries degrade bindings
  with an explicit error instead of emitting a different key.
- The macOS application is ad-hoc signed rather than notarized, and there is no
  DMG or Launch at Login.
- The output uses the Xbox 360 compatibility USB identity described
  [above](#why-it-identifies-as-an-xbox-360-controller).
- Automatic lizard restoration timing and lizard-suppression failure timing
  have not been formally qualified.
- The keyboard simulator is line-oriented, not a production input UI.
