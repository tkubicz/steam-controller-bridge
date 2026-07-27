# Steam Controller 2 Bridge User Guide

This guide covers the current source-build workflow on macOS. The bridge reads
a **Steam Controller 2 (2026)** and exposes a conventional physical USB gamepad
through a Seeed Studio XIAO nRF52840.

The discontinued Steam Controller from 2015, its Micro-USB connection, and its
original wireless receiver use a different protocol and are not supported.

## Current readiness

The following paths have been verified on development hardware:

- the official Puck enumerates as `28de:1304`, and its active slot produces
  extended `0x42` state reports at about 250 Hz;
- the non-Sense XIAO flashes through its serial bootloader and exposes CDC plus
  an Xbox-layout gamepad;
- macOS binds the gamepad to its built-in `Xbox360Gamepad` driver;
- Safari's Gamepad API sees a connected controller with `mapping: standard`;
- Boosteroid detects the connected XIAO as a valid gamepad;
- an unchanged active simulator state remains held for more than 30 seconds
  without an unintended watchdog neutral;
- the live bridge sent the initial lizard-off command plus ten three-second
  refreshes over a 30-second Puck-to-XIAO run with no HID, decode, suppression,
  or serial failure.

Every physical Steam Controller 2 control, explicit watchdog/disconnect and
manual Space/pointer suppression, lizard restoration timing, GeForce NOW,
reconnect stress, and the one-hour soak still require acceptance testing. Do
not treat the development smoke test as a release qualification.

The bridge sends exactly one whitelisted controller setting on the official
Puck: SDL's lizard-mode-off command. It does not expose arbitrary feature
writes, initialization, mapping clears, settings, or haptics.

## What you need

- A Mac with at least two available USB data connections or a powered USB hub.
- A Steam Controller 2 (2026).
- Preferably its official Steam Controller Puck. Direct USB-C and Bluetooth can
  be probed, but have not yet been confirmed to expose the same raw reports.
- One non-Sense Seeed Studio XIAO nRF52840.
- A USB-C data cable for the XIAO and another data connection for the controller
  or Puck. Charge-only cables will not work.
- The project source checkout.
- Xcode command-line tools, Rust, Homebrew, and Arduino CLI for the current
  source/firmware build workflow.

The intended connection is:

```text
Steam Controller 2 → official Puck → Mac host bridge
                                      ↓ USB CDC
                                  XIAO nRF52840
                                      ↓ Xbox-layout USB gamepad
                              browser or streaming service
```

## 1. Install build tools

Install Apple's command-line tools if they are not already present:

```bash
xcode-select --install
```

Install Rust using [rustup](https://rustup.rs/), then install Arduino CLI. The
official Arduino documentation supports Homebrew installation:

```bash
brew install arduino-cli
```

From the project root, build the host applications:

```bash
cargo build --release --workspace
```

The commands used below will then be available in `target/release/`.

## 2. Build and flash the XIAO

Install the pinned Seeed nRF52 core, run portable firmware tests, and create the
firmware artifacts:

```bash
make -C firmware/xiao-nrf52840 setup
make -C firmware/xiao-nrf52840 test
make -C firmware/xiao-nrf52840 artifacts
```

The release files are:

```text
firmware/xiao-nrf52840/build/artifacts/
├── steam-controller-bridge-xiao-nrf52840.uf2
└── steam-controller-bridge-xiao-nrf52840-dfu.zip
```

Connect the XIAO with a data-capable USB-C cable and list its port:

```bash
arduino-cli board list
```

Flash through the serial bootloader, substituting the actual port:

```bash
make -C firmware/xiao-nrf52840 flash PORT=/dev/cu.usbmodemXXXX
```

If serial flashing cannot reset the XIAO, double-tap RESET. A bootloader volume
will mount in Finder; drag the generated `.uf2` file onto it. The board should
reboot as a composite device named `Steam Controller Bridge` with both CDC and
an Xbox-layout gamepad interface.

The development firmware uses Xbox 360 compatibility VID/PID `045e:028e` so
macOS's built-in driver publishes the device to GameController, Safari, and
streaming clients. The USB strings remain `Lynxware / Steam Controller Bridge`;
it is not a PlayStation `Wireless Controller`. See the release-identity note at
the end of this guide before distributing hardware.

XIAO LED states:

- slow blue: CDC host disconnected;
- fast blue: connected and waiting for protocol negotiation;
- solid green: bridge negotiated;
- blinking red: parser threshold or state-watchdog fault.

## 3. Connect the Steam Controller 2

Valve documents three connection modes and identifies them by the controller
LED: Puck is white, wired USB-C is green, and Bluetooth is blue. See Valve's
[Steam Controller 2 feature and troubleshooting guide](https://help.steampowered.com/en/faqs/view/33E8-5EDF-24E6-4CFB).

### Recommended: official Puck

The Puck is the preferred bridge input because it is the transport this
project's report decoder is designed around.

1. Connect the official Puck to the Mac over USB-C.
2. Start Steam and ensure no game is running.
3. A controller and its packaged Puck should already be paired. For manual
   pairing, follow Steam's prompt after placing the controller on the connected
   Puck.
4. With the controller off, hold `A + R1 + Steam` for the right Puck slot or
   `A + L1 + Steam` for the left slot. A solid white LED means connected.
5. After pairing succeeds, choose **Steam → Quit Steam**. Do not merely close
   its window. Steam must remain quit while probing or running the bridge.
6. Steam's `ipcserver` is a persistent `launchd` job and can survive after the
   application exits. Stop it for the current login session:

   ```bash
   launchctl bootout user/$(id -u)/com.valvesoftware.steam.ipctool
   ```

   Killing `ipcserver` is insufficient because its `KeepAlive` setting makes
   `launchd` restart it. Launching Steam later normally registers it again.

The official Puck exposes several controller-slot collections. Do not assume
the first HID index is the active slot; use the probe procedure in the next
section.

### Experimental: direct USB-C

With the controller off, connect it directly to the Mac. If it is already on in
another mode, hold Steam while connecting the cable. A solid green LED means
wired mode. This transport is usable only if `sc-probe` observes compatible
`0x42` or `0x45` reports.

### Experimental: Bluetooth

With the controller off, hold `B + R1 + Steam` through the second chime until
the LED shows rapid double blue pulses. Pair the device named similarly to
`Steam Ctrl (BT) FXA...` in macOS Bluetooth settings. A solid blue LED means
connected. Bluetooth is usable only if the probe sees compatible state reports.

## 4. Grant macOS input permission

Listing HID metadata normally works without special permission, but opening a
controller collection may fail with `not permitted`.

Open **System Settings → Privacy & Security → Input Monitoring**, enable the
terminal application used to run the bridge, then fully quit and reopen that
terminal. Apple documents this setting in
[Control access to Input Monitoring on Mac](https://support.apple.com/en-au/guide/mac-help/mchl4cedafb6/mac).

On Apple-silicon laptops, macOS may also ask whether to allow a newly connected
USB accessory. Approve both the Puck/controller and the XIAO while the Mac is
unlocked.

## 5. Identify and verify the controller input

List every HID collection:

```bash
target/release/sc-probe list
```

Fully quit Steam, `sc-visualizer`, other `sc-probe` processes, and other
controller tools before opening a collection. Project tools take a per-slot
ownership lock, which rejects another project process. Native macOS HID access
remains shared because this Puck rejects an exclusive seize request. Steam and
other non-project tools do not honor the project lock, so the `ipcserver`
bootout above is mandatory.

The XIAO output is named `Steam Controller Bridge`; **do not select it as the
input**. Look for a Valve/Steam Controller 2 or Puck collection, then inspect
candidate indices:

```bash
target/release/sc-probe inspect --index N
target/release/sc-probe monitor --index N --raw --duration-secs 10
```

Press buttons and move controls while monitoring. A compatible collection
produces changing reports whose first byte is normally `0x42` or `0x45`. Status
reports such as `0x43`, `0x44`, `0x79`, and `0x7b` may also appear. If a Puck
exposes several slots, repeat the monitor command for each plausible index.

Once raw state reports are confirmed, check typed decoding:

```bash
target/release/sc-probe monitor --index N --duration-secs 10
```

If no candidate produces state reports, see Troubleshooting; the bridge cannot
operate from metadata alone.

Test lizard suppression on the verified index:

```bash
target/release/sc-probe suppress-lizard --index N --duration-secs 15
```

While it runs, focus a text field and press controller `A`, then move both
touchpads. No Space key or pointer motion should occur. Stop the command and
wait up to about 10 seconds; the controller watchdog should restore its normal
desktop keyboard/mouse behavior. Do not run the monitor or visualizer at the
same time as this command.

## 6. Verify the XIAO output independently

Before involving the controller, test the host-to-XIAO path. Find the XIAO CDC
port with `arduino-cli board list`, then use the keyboard simulator:

```bash
target/release/gamepad-simulator keyboard --output serial \
  --port /dev/cu.usbmodemXXXX
```

The XIAO should change from fast blue to solid green. Open
[Hardware Tester](https://hardwaretester.com/gamepad) in Safari, focus the page,
and send these commands one at a time:

| Command | Expected standard-gamepad control |
| --- | --- |
| `space` | A, bottom face button, API button 0 |
| `1` | B, right face button, API button 1 |
| `2` | X, left face button, API button 2 |
| `3` | Y, top face button, API button 3 |
| `4` / `5` | left/right shoulder |
| `6` / `7` | Back/Start |
| `8` | Guide |
| `w/a/s/d` | left stick |
| `up/left/down/right` | right stick |
| `q` / `e` | left/right trigger |
| `i/j/k/l` | D-pad |
| `r` | neutral |

The tester may label the device `Steam Controller Bridge Extended Gamepad` or
`Controller Extended Gamepad`; the important fields are `CONNECTED: Yes` and
`MAPPING: standard`. This isolates firmware, USB, and browser behavior from
Steam Controller input problems. `sc-probe monitor` is not a reliable output
observer after Apple's Xbox driver owns the interface.

## 7. Run the complete bridge

Use the verified source index and XIAO CDC port:

```bash
target/release/sc-bridge --index N --output serial \
  --port /dev/cu.usbmodemXXXX --record session.jsonl
```

Explicit `--index` is recommended. `--controller auto` considers only official
Puck `ff00:0001` controller slots, but a multi-slot Puck can still expose
several candidates and the first one may not be active.

Keep the terminal and XIAO connected while playing. Press `Ctrl-C` for orderly
shutdown; the bridge sends a final neutral state before releasing the Puck.
The controller's desktop lizard mode returns automatically after its watchdog
expires. The XIAO also neutralizes if CDC disconnects or active-state refreshes
stop for 100 ms.

Successful startup logs `lizard_suppressed=true`. Periodic metrics should show
`lizard_suppressed=true`, increasing `lizard_refreshes`, zero
`lizard_failures`, and a `lizard_refresh_age_ms` below 3000. The diagnostic
option `--lizard-mode leave` retains the old native keyboard/mouse behavior and
is not suitable for gameplay.

For a first end-to-end test:

1. Confirm the XIAO LED is solid green.
2. Confirm `sc-bridge` logs `lizard_suppressed=true`, input reports, and no
   continuing decode or lizard failures.
3. Open the browser or streaming client only after the bridge is running.
4. Exercise every button, both sticks, both triggers, and the D-pad.
5. Keep a text field focused while pressing `A` and using touchpads; verify no
   Space key or pointer movement occurs.
6. Hold a stick for at least 30 seconds to verify refresh behavior.
7. Stop `sc-bridge` and verify the physical USB gamepad becomes neutral within
   125 ms and desktop lizard mode returns within about 10 seconds.

## Daily startup

After initial setup and pairing:

1. Connect the flashed XIAO.
2. Connect the official Puck.
3. Power on Steam Controller 2 in Puck mode and confirm a solid white LED.
4. Fully quit Steam and boot out `com.valvesoftware.steam.ipctool`.
5. Close all controller probes/visualizers.
6. If device ordering may have changed, rerun `sc-probe list` and monitor the
   candidate index briefly.
7. Close the monitor, then start `sc-bridge` with the input index and XIAO
   serial port.
8. Confirm `lizard_suppressed=true`, then start the browser or streaming
   service.

HID indices and `/dev/cu.usbmodem…` names can change after reconnecting devices,
so do not permanently assume old values.

## Troubleshooting

### `sc-probe` reports `not permitted`

Grant Input Monitoring permission to the actual host application—Terminal,
iTerm, or the application launching the command—then quit and reopen it.

### The controller is listed but no `0x42`/`0x45` reports arrive

- Confirm this is Steam Controller 2 (2026), not the 2015 controller.
- Confirm the controller LED is solid white for Puck, green for USB, or blue for
  Bluetooth.
- For Puck mode, use Steam only for pairing, then fully quit it before opening a
  collection.
- Probe every Puck slot collection.
- Capture evidence for implementation work:

  ```bash
  target/release/sc-probe capture --index N --output reports.jsonl \
    --duration-secs 30 --decoded
  ```

Do not send guessed feature reports. Only the fixed lizard-off setting used by
`sc-probe suppress-lizard` and `sc-bridge` is permitted.

### Ownership or HID open fails

Fully quit Steam rather than closing its window. Stop `sc-visualizer`, every
other `sc-probe monitor`/`capture`/`suppress-lizard` process, and any other
controller translator. An `already owned by another
steam-controller-bridge tool` error comes from the per-slot project lock; a
native HID-open error may instead indicate missing Input Monitoring permission.
Confirm Steam's persistent helper is absent:

```bash
pgrep -ifl ipcserver
launchctl print user/$(id -u)/com.valvesoftware.steam.ipctool
```

If it is present, use the `launchctl bootout` command above. Then run
`sc-probe list` again because indices may have changed.

### `A` still types Space or a touchpad moves the pointer

- Confirm the bridge startup log says `lizard_suppressed=true`.
- Confirm periodic metrics show increasing `lizard_refreshes`, zero
  `lizard_failures`, and refresh age below 3000 ms.
- Verify the selected collection is `28de:1304`, usage `ff00:0001`, interface
  2–5, and is the slot producing the active `0x42`/`0x45` stream.
- Fully quit Steam and close other controller tools.
- Confirm only one Steam Controller 2 is active.

If a refresh fails, the bridge intentionally neutralizes the XIAO and exits
rather than continuing with duplicate keyboard/gamepad input.

### No XIAO serial port appears

- Use a known data-capable cable and approve the USB accessory in macOS.
- Double-tap RESET and check `arduino-cli board list` again.
- If the bootloader volume appears, reflash the UF2.
- A slow-blue XIAO means no CDC host has opened the firmware session.

### XIAO stays fast blue

The host opened CDC but negotiation did not complete. Confirm the selected port
belongs to `Steam Controller Bridge`, close serial monitors using that port,
and rerun with `--serial-log`.

### XIAO blinks red or controls become neutral

The firmware detected malformed protocol traffic or missed the 100 ms active
state deadline. Ensure only one process owns the CDC port and that it is a
current build containing the 25 ms state refresh implementation.

### macOS lists `Wireless Controller`

`Wireless Controller` is the conventional PlayStation identity and is not the
current firmware. During development, a short-lived DualShock-compatible test
was used to prove that macOS could publish a physical gamepad; System Settings
can retain that disconnected row.

The connected firmware should instead appear in `sc-probe list` as
`Steam Controller Bridge` with VID/PID `045e:028e`. Safari should report a
standard-mapped extended gamepad. If `Wireless Controller` is the only live
device, reflash the current firmware, unplug/replug the XIAO, reload the tester,
and press a face button.

### Controls appear twice or the wrong gamepad is used

Use the official Puck path and default lizard suppression. Direct USB/Bluetooth
may expose another standard gamepad in addition to the XIAO output. Fully quit
Steam, close other controller translators, confirm the lizard diagnostics, and
verify the `Steam Controller Bridge` HID device independently with the
simulator.

### Mapping is wrong

Record a session with `--record`, note the controller transport and firmware,
and compare raw, decoded, and mapped state in `sc-visualizer`. Determine
whether the error first appears in the raw Puck report, decoded Steam state,
mapped generic state, or Xbox USB report before changing a mapping.

The intended face mapping is Steam Controller A/B/X/Y to standard gamepad
South/East/West/North and then Xbox A/B/X/Y. Grip and extra buttons are retained
in the host protocol, but the Xbox 360 USB report has no corresponding controls,
so those inputs are not exposed to games in the current compatibility
personality.

## What remains for a polished end-user release

The bridge can be completed and hardware-qualified from this source workflow,
but a broadly distributable product should additionally provide:

- verified captures for the official Puck and a documented decision on direct
  USB/Bluetooth support;
- hardware qualification of the narrow lizard-mode suppression lifecycle;
- automated hardware acceptance and long-duration testing;
- signed/notarized macOS binaries and downloadable firmware artifacts;
- an owned or licensed USB VID/PID plus a macOS recognition path that does not
  depend on another vendor's Xbox 360 compatibility identity;
- stable source/slot and XIAO-port discovery instead of manual indices/paths;
- a background service or packaged UI with startup, reconnect, and diagnostics;
- versioned releases and a supported upgrade/recovery procedure.
