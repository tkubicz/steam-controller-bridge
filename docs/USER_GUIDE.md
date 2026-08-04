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
- direct Bluetooth enumerates as `28de:1303`, transport `Bluetooth`, usage
  `ff00:0001`, interface `-1`, and produces 46-byte `0x45` state reports at
  approximately 67–68 Hz plus compatible `0x43` battery reports;
- the non-Sense XIAO flashes through its serial bootloader and exposes CDC plus
  an Xbox-layout gamepad;
- macOS binds the gamepad to its built-in `Xbox360Gamepad` driver;
- Safari's Gamepad API sees a connected controller with `mapping: standard`;
- Boosteroid and GeForce NOW both drive the bridge as a standard gamepad;
- every physical control is confirmed: buttons, both sticks, both triggers, and
  the D-pad;
- end-to-end dual rumble works from a client vibration request through to the
  correct strong and weak actuators;
- lizard suppression produces no Space keypresses or pointer motion while
  running, and normal desktop behavior returns afterwards;
- an unchanged active simulator state remains held for more than 30 seconds
  without an unintended watchdog neutral;
- the live bridge sent the initial lizard-off command plus ten three-second
  refreshes over a 30-second Puck-to-XIAO run with no HID, decode, suppression,
  or serial failure.

Unless a bullet explicitly identifies Bluetooth, the end-to-end gameplay
validation above used the Puck path. Bluetooth input, battery, suppression
heartbeats, and independent rumble writes are verified; full in-game mapping and
reconnect/sleep-wake remain below.

Explicit watchdog/disconnect timing, lizard restoration timing, and reconnect
stress still require acceptance testing.

On either exact supported input collection, the bridge exposes only three fixed
operations: SDL's lizard-mode-off setting, standard dual rumble, and the
`0x9f`/`off!` controller power-off command. It does not expose arbitrary
feature/output writes, initialization, mapping clears, settings, trigger
rumble, tones, or scripted haptics.

## What you need

- A Mac with one USB data connection for Bluetooth input, or two connections/a
  powered hub when using the Puck.
- A Steam Controller 2 (2026).
- Either its official Steam Controller Puck or a Mac with Bluetooth available.
  The Puck is recommended when minimum latency or maximum reliability matters.
- One non-Sense Seeed Studio XIAO nRF52840.
- A USB-C data cable for the XIAO and, in Puck mode, another data connection for
  the Puck. Charge-only cables will not work.
- The project source checkout.
- Xcode command-line tools, Rust, Homebrew, and Arduino CLI for the current
  source/firmware build workflow.

The intended connection is:

```text
Steam Controller 2 → official Puck ─┐
                                    ├→ Mac host bridge → USB CDC → XIAO
Steam Controller 2 → Bluetooth ─────┘                         ↓
                                                 Xbox-layout USB gamepad
                                                            ↓
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

Rumble requires firmware containing protocol-v1 message type 8. An older XIAO
remains input-compatible but cannot return Xbox vibration requests to the host,
so reflash the board after updating the project.

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

The Puck is preferred for its approximately 250 Hz report rate and established
reliability. Bluetooth is supported but was observed at approximately 67–68 Hz.

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

The official Puck exposes several controller-slot collections. Normal bridge
startup discovers which slot is active from complete state reports. The probe
procedure below is optional diagnostics rather than a daily setup step.

### Unsupported: direct USB-C

With the controller off, connect it directly to the Mac. If it is already on in
another mode, hold Steam while connecting the cable. A solid green LED means
wired mode. The bridge does not currently classify or open direct USB-C input.

### Supported: Bluetooth

1. Power the controller off.
2. Hold `B + R1 + Steam` through the second chime and until the LED shows rapid
   double blue pulses.
3. Open macOS **System Settings → Bluetooth** and pair the device named
   similarly to `Steam Ctrl (BT) FXA...`.
4. A solid blue LED means the Bluetooth connection is active.

The bridge automatically selects its exact `28de:1303`, `ff00:0001`, interface
`-1` vendor collection once valid controller state arrives. An attached but
idle Puck is harmless and does not prevent Bluetooth selection.

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

## 5. Optionally verify the controller input

List every HID collection:

```bash
target/release/sc-probe list
```

Fully quit Steam, `sc-visualizer`, other `sc-probe` processes, and other
controller tools before opening a collection. Project tools take a per-input
ownership lock, which rejects another project process. Native macOS HID access
remains shared because the tested Puck rejects an exclusive seize request.
Steam and other non-project tools do not honor the project lock, so the
`ipcserver` bootout above is mandatory for either input transport.

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
The supported Bluetooth collection reports `28de:1303`, usage `ff00:0001`,
interface `-1`.

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

Test each SC2 actuator independently without the XIAO:

```bash
target/release/sc-probe rumble --index N --low 32768 --high 0
target/release/sc-probe rumble --index N --low 0 --high 32768
target/release/sc-probe rumble --index N --low 65535 --high 65535 \
  --duration-ms 1000
```

`low` is the Xbox low-frequency/strong channel mapped to the SC2 left
actuator; `high` is the high-frequency/weak channel mapped to the right
actuator. Values are `0..65535`. The command suppresses lizard mode, refreshes
rumble every 40 ms, and attempts an explicit zero on duration expiry, Ctrl-C,
or error.

Test the restricted controller power-off command only after selecting the
active exact Puck or Bluetooth collection:

```bash
target/release/sc-probe power-off --index N
```

The command prints an explicit warning, sends a short nonblocking burst of the
fixed `0x9f`/`off!` feature report, and reports every attempted write. It cannot
send arbitrary feature reports. Run it with the controller both off and on the
Puck before relying on automatic shutdown: confirm that charging continues,
the controller stays off, and pressing Steam wakes it normally. Do not run it
alongside the bridge, Steam, another probe, or the visualizer.

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

From the project root, use the normal zero-configuration command:

```bash
./sc-bridge
```

The launcher uses Cargo's release profile so source changes are rebuilt when
needed. Live mode automatically:

- filters controller candidates to the exact Puck identity `28de:1304`, USB,
  usage `ff00:0001`, interfaces 2–5, or the exact Bluetooth identity
  `28de:1303`, Bluetooth, usage `ff00:0001`, interface `-1`;
- observes candidates without feature writes until exactly one emits a complete
  valid `0x42` or `0x45` controller-state report;
- filters macOS callout ports by the XIAO's `Lynxware / Steam Controller
  Bridge`, `045e:028e` USB metadata, rejecting the Puck's own CDC port;
- completes the protocol-v1 Hello handshake before selecting the XIAO;
- remembers the XIAO MCU serial number across a changed `/dev/cu.usbmodem…`
  path; and
- waits and rescans every 500 ms when hardware is absent. Once supported
  collections are already open, report queues are still checked every 500 ms
  while unchanged metadata scans back off from two to at most ten seconds.

If more than one supported source produces controller states, the bridge
refuses the ambiguity, lists transport/product/serial/index, and asks for
`--index N`. An idle connected Puck does not conflict with an active Bluetooth
controller. If more than one XIAO completes Hello, use `--port PATH`. Explicit
forms are:

```bash
./sc-bridge --controller auto --port auto
./sc-bridge --index N
./sc-bridge --port /dev/cu.usbmodemXXXX
./sc-bridge --index N --port /dev/cu.usbmodemXXXX --record session.jsonl
```

While automatic discovery is waiting, it keeps each supported controller
collection open so macOS does not repeatedly allocate HID readers. Stop the
bridge before using `sc-probe monitor`, `sc-probe rumble`,
`sc-probe suppress-lizard`, or `sc-visualizer`; `sc-probe list` is safe because
it only enumerates metadata. The long-idle effect of keeping these collections
open on controller sleep and battery has not yet been measured. Until that
hardware observation is complete, stop the bridge for prolonged periods when
idle battery life matters.

Keep the terminal and XIAO connected while playing. Press `Ctrl-C` for orderly
shutdown; the bridge sends a final neutral state before releasing the selected
controller input. The controller's desktop lizard mode returns automatically
after its watchdog expires. The XIAO also neutralizes if CDC disconnects or
active-state refreshes stop for 100 ms.

Normal logs progress through `Discovering`, `Waiting`, `Starting`, and
`Running`. Running status must show the selected XIAO path, a connected
controller, `lizard_suppressed=true`, and haptics `Idle` until an effect is
requested. `Active` means a fresh XIAO rumble lease is being refreshed;
`Degraded` means actuator writes failed while controller input remains usable.
The diagnostic option
`--lizard-mode leave` retains the old native keyboard/mouse behavior and is not
suitable for gameplay.

### Automatic controller shutdown

The live serial bridge defaults to turning the controller off after 15 minutes
without meaningful controller input. A held button, stick outside its mapped
dead zone, trigger, pad touch/press, or grip touch keeps the controller awake.
Continuous HID reports, IMU movement, sub-dead-zone jitter, rumble, lizard
heartbeats, and XIAO refresh traffic do not count as activity.

Configure the idle policy in whole minutes, or disable it:

```bash
./sc-bridge --idle-shutdown 5
./sc-bridge --idle-shutdown 30
./sc-bridge --idle-shutdown never
```

The independent Puck-placement policy is opt-in:

```bash
./sc-bridge --puck-dock-action power-off
```

It fires only after a fresh `Charging` or `Charged` battery report from the
selected official Puck source. It does not fire for Bluetooth charging, an
attached empty Puck, or battery percentage changes. The event has priority over
the idle timer and safely neutralizes a held gamepad state before power-off.

Placement power-off is one-shot. Waking the controller while it remains on the
Puck does not immediately turn it off again. Remove it until a fresh
`Discharging` report is received, then placing it back creates a new episode.
Disabling and re-enabling the option also begins a new episode. Stop and Quit
never power the controller off as a side effect; they retain the normal neutral
and lizard-restoration behavior.

On a successful automatic shutdown the menu/log status briefly reports
`Controller sleeping`, discovery ignores the dying report tail for 2.5 seconds,
and then waits for a normal Steam-button wake. A failed command does not stop
gameplay: status becomes `Degraded`, lizard suppression and input resume, and a
neutral idle attempt is rate-limited to once per 30 seconds. User activity after
a failed placement attempt cancels that placement's retries.

For a first end-to-end test:

1. Confirm the XIAO LED is solid green.
2. Confirm `sc-bridge` reaches `Running`, reports `lizard_suppressed=true`, and
   shows no continuing decode or suppression failures.
3. Open the browser or streaming client only after the bridge is running.
4. Exercise every button, both sticks, both triggers, and the D-pad.
5. Keep a text field focused while pressing `A` and using touchpads; verify no
   Space key or pointer movement occurs.
6. Hold a stick for at least 30 seconds to verify refresh behavior.
7. Use GamepadTester's one-second vibration action, then its infinite action.
   Verify both unequal channels and that Stop ends rumble promptly.
8. Stop `sc-bridge` and verify the physical USB gamepad becomes neutral within
   125 ms and desktop lizard mode returns within about 10 seconds.

## Daily startup

After initial setup and pairing:

1. Connect the flashed XIAO.
2. Either connect the official Puck and power on in Puck mode (solid white), or
   power on the paired Bluetooth controller (solid blue).
3. Fully quit Steam and boot out `com.valvesoftware.steam.ipctool`.
4. Close all controller probes/visualizers.
5. Run `./sc-bridge`.
6. Wait for `Running` with `lizard_suppressed=true`, then start the streaming
   service.

The command can be started before either device is connected. It remains in a
waiting state, accepts hardware in either order, and rediscovers changed HID
indices and serial paths after reconnects.

## Menu-bar application

Build the current-architecture local application:

```bash
./tools/build-macos-app.sh
```

Move `dist/Steam Controller Bridge.app` to `/Applications` if desired, then
open it from Finder or:

```bash
open "dist/Steam Controller Bridge.app"
```

It has no ordinary window or Dock icon. The controller icon itself shows the
current state:

- controller with an x badge: bridge is Off;
- controller with ellipsis: bridge is On and waiting for hardware;
- controller with a check: the controller and XIAO are connected and ready;
- controller with an exclamation mark: action is required.

All four are macOS template icons and automatically follow the light or dark
menu-bar appearance.

The menu opens compact, grouped status and actions:

- bridge power and readiness;
- input transport, controller, and XIAO state;
- battery percentage, or `Unknown` until a valid `0x43` report arrives;
- haptics `Idle`, `Active`, or `Degraded`;
- automatic shutdown state and current neutral-idle time;
- an `Idle Shutdown` submenu with `Never`, 5, 10, 15, and 30 minutes;
- `Turn Off When Placed on Puck`, disabled by default;
- a short, friendly problem summary;
- `Copy Full Error` for the complete technical error;
- `Start Bridge` and `Stop Bridge`;
- `Copy Diagnostics`;
- `Open Input Monitoring Settings`;
- `Open Log Folder`; and
- `Quit`.

The bridge starts automatically when the app launches. Stop and Quit
neutralize the XIAO before releasing the selected input and ending lizard
suppression.
Logs rotate at a bounded size under:

```text
~/Library/Logs/Steam Controller Bridge/
```

Menu choices are applied to the running bridge without restarting it and saved
atomically in:

```text
~/Library/Application Support/Steam Controller Bridge/settings.json
```

Missing, malformed, unsupported-version, or out-of-range settings fall back to
15 minutes plus `Leave On` and produce one startup warning.

Grant Input Monitoring to **Steam Controller Bridge** itself when using the
app. Permission granted to Terminal does not automatically cover the app.

### Opening an unnotarized build

The app is ad-hoc signed, not notarized, whether it came from a release download
or a local `./tools/build-macos-app.sh`. A downloaded copy also carries macOS's
quarantine flag, so double-clicking it reports that the app is damaged or cannot
be opened. This is Gatekeeper refusing an unknown developer, not a broken build.

Open it once the long way, and macOS remembers the choice:

1. Move `Steam Controller Bridge.app` to `/Applications`.
2. Right-click (or Control-click) it and choose **Open**.
3. Confirm **Open** in the dialog that appears.

If the dialog offers no Open button, approve the app under **System Settings →
Privacy & Security**, where it appears shortly after the blocked launch. Removing
the quarantine flag directly also works:

```bash
xattr -d com.apple.quarantine "/Applications/Steam Controller Bridge.app"
```

Verify a download before opening it, using the `SHA256SUMS.txt` published with
each release:

```bash
shasum -a 256 -c SHA256SUMS.txt
```

## Troubleshooting

### The controller did not turn off automatically

- Confirm the bridge uses serial output and the XIAO is ready. Replay, dump,
  file, and mock modes never power off hardware.
- Inspect `Auto shutdown` in the menu or copied diagnostics. `Off` means the
  timeout is `Never` and Puck placement is disabled.
- For idle shutdown, confirm no button, pad/grip touch, stick, or trigger is
  continuously active. The displayed neutral-idle timer should advance.
- For immediate shutdown, confirm the input says `Puck` and charge state becomes
  `Charging` or `Charged`. Bluetooth external power is intentionally ignored.
- If the placement episode is already handled, remove the awake controller
  until a `Discharging` report arrives before docking it again.
- Use `sc-probe power-off --index N` with the bridge stopped to validate the
  hardware command independently. A successful build is not proof that the
  Puck relay or Bluetooth controller accepted it.
- `Degraded` keeps gameplay available. Copy diagnostics for the full backend
  error and retry state.

### `sc-probe` reports `not permitted`

Grant Input Monitoring permission to the actual host application—Terminal,
iTerm, or the application launching the command—then quit and reopen it.

### The controller is listed but no `0x42`/`0x45` reports arrive

- Confirm this is Steam Controller 2 (2026), not the 2015 controller.
- Confirm the controller LED is solid white for Puck, green for USB, or blue for
  Bluetooth.
- For Puck mode, use Steam only for pairing, then fully quit it before opening a
  collection.
- In Puck mode, probe every Puck slot collection. In Bluetooth mode, inspect
  the `28de:1303`, `ff00:0001`, interface `-1` collection.
- Capture evidence for implementation work:

  ```bash
  target/release/sc-probe capture --index N --output reports.jsonl \
    --duration-secs 30 --decoded
  ```

  Capture files record the full device serial so that reports stay replayable. On
  Bluetooth that serial is the controller's MAC address, so review a capture before
  attaching it to a public issue. Status output, logs, and `Copy Diagnostics` show
  only the last four characters and are safe to paste as-is.

Do not send guessed feature/output reports. Only the fixed lizard-off setting
and exact standard rumble output used by `sc-probe` and `sc-bridge` are
permitted.

### Ownership or HID open fails

Fully quit Steam rather than closing its window. Stop `sc-visualizer`, every
other `sc-probe monitor`/`capture`/`suppress-lizard`/`rumble` process, and any
other controller translator. An `already owned by another
steam-controller-bridge tool` error comes from the per-input project lock; a
native HID-open error may instead indicate missing Input Monitoring permission.
Confirm Steam's persistent helper is absent:

```bash
pgrep -ifl ipcserver
launchctl print user/$(id -u)/com.valvesoftware.steam.ipctool
```

If it is present, use the `launchctl bootout` command above. Then run
`sc-probe list` again because indices may have changed.

### Automatic discovery reports multiple active sources or XIAOs

The bridge intentionally does not prefer Puck or Bluetooth and does not pick
the first device. Quit the bridge, run `target/release/sc-probe list`, identify
the intended active `ff00:0001` collection from the listed transport, product,
serial, and index, then restart with `./sc-bridge --index N`. For XIAO
ambiguity, disconnect the unused board or restart with
`./sc-bridge --port /dev/cu.usbmodem…`.

### The bridge stays in `Waiting`

Read the status detail. `Waiting for a Steam Controller 2 Puck or Bluetooth
connection` means no exact supported collection is enumerated. `waiting for
valid controller state` means collections opened but no complete state stream
was observed; wake the controller and verify its solid white Puck LED or solid
blue Bluetooth LED. `Waiting for XIAO ... CDC` means no exact XIAO metadata
match. A Hello-handshake message means the port exists but firmware negotiation
failed; close other serial tools and reflash current firmware if needed.

### `A` still types Space or a touchpad moves the pointer

- Confirm Running status says `lizard_suppressed=true`.
- In the menu app, use `Copy Diagnostics` or `Open Log Folder` and confirm
  increasing `lizard_refreshes`, zero failures, and a recent refresh age.
- Verify the selected collection is either `28de:1304`, USB, usage
  `ff00:0001`, interface 2–5, or `28de:1303`, Bluetooth, usage `ff00:0001`,
  interface -1, and is producing the active `0x42`/`0x45` stream.
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

Use either supported input path and default lizard suppression. Fully quit
Steam, close other controller translators, confirm the selected input
transport and lizard diagnostics, and verify the `Steam Controller Bridge` HID
device independently with the simulator.

### GamepadTester or a game requests vibration, but the SC2 does not rumble

- Rebuild and reflash the current XIAO firmware; old firmware has no reverse
  feedback path.
- First run the left-only and right-only `sc-probe rumble` commands above. If
  they fail, verify the active exact Puck or Bluetooth `ff00:0001` collection
  and close Steam and other controller tools.
- Start `./sc-bridge`, request vibration again, and inspect the CLI or copied
  diagnostics. `commands_received` must increase. If it does not, macOS did not
  deliver an Xbox OUT packet to the XIAO or the CDC feedback frame did not
  arrive.
- `Degraded` with increasing failures means the selected controller output
  write failed.
  Controller input intentionally continues; the worker retries at most every
  500 ms while the XIAO keeps the 100 ms lease fresh.
- Confirm the browser tab is focused and has received a controller input. Test
  one-second vibration before infinite vibration.

Stopping the effect, closing the browser, disconnecting either endpoint, or
stopping the bridge must stop rumble. Do not continue using a build that can
leave an actuator latched.

### Mapping is wrong

Record a session with `--record`, note the controller transport and firmware,
and compare raw, decoded, and mapped state in `sc-visualizer`. Determine
whether the error first appears in the raw controller report, decoded Steam
state, mapped generic state, or Xbox USB report before changing a mapping.

The intended face mapping is Steam Controller A/B/X/Y to standard gamepad
South/East/West/North and then Xbox A/B/X/Y. Grip and extra buttons are retained
in the host protocol, but the Xbox 360 USB report has no corresponding controls,
so those inputs are not exposed to games in the current compatibility
personality.
## What remains for a polished end-user release

The bridge can be completed and hardware-qualified from this source workflow,
but a broadly distributable product should additionally provide:

- completed Bluetooth reconnect, sleep/wake, and lizard restoration acceptance;
  direct USB-C remains a separate future transport;
- hardware qualification of the narrow lizard-mode suppression lifecycle;
- automated hardware acceptance and long-duration testing;
- signed/notarized macOS binaries and downloadable firmware artifacts;
- an owned or licensed USB VID/PID plus a macOS recognition path that does not
  depend on another vendor's Xbox 360 compatibility identity;
- hardware qualification of dual-rumble expiry and disconnect timing, beyond
  the confirmed delivery path;
- Developer ID signing, notarization, and a supported upgrade/recovery
  procedure beyond the current versioned GitHub downloads.
