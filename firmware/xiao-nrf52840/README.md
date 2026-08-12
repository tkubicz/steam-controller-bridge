# XIAO nRF52840 firmware

This firmware turns a Seeed Studio XIAO nRF52840 or XIAO nRF52840 Sense into the physical
USB endpoint for the macOS bridge. It exposes one composite USB device:

- CDC ACM carries protocol-v1 frames from the host.
- An Xbox 360-compatible vendor interface exposes an ABXY gamepad through
  macOS's built-in Xbox GameController driver and accepts standard dual-rumble
  output.

The Steam Controller radio and feature protocol remain on the host. OpenPuck
informed the nonblocking TinyUSB and watchdog structure, but no OpenPuck source
or USB descriptors are included; OpenPuck is AGPL-3.0 while this project is MIT.

## Toolchain

The build pins the Seeed nRF52 Arduino core to `1.1.13` and uses the board FQBN
`Seeeduino:nrf52:xiaonRF52840`. Install `arduino-cli`, then run:

```bash
cd firmware/xiao-nrf52840
make setup
make test
make build
make artifacts
```

`make setup` installs the board core using Seeed's package index without
replacing an existing Arduino CLI configuration. `make test` is host-native and does not require Arduino or hardware.
Build products are written below `build/`, and `make artifacts` collects the
UF2 and developer DFU zip in `build/artifacts/`. The release publishes only the
UF2; the DFU zip remains available for `make flash` development workflows.

The pinned core gives the standard and Sense variants the same MCU memory
layout, pin map, and UF2 family. Their bootloaders use different USB product and
board IDs; the App Center recognizes and validates both.

## Flash and recovery

Find the application or bootloader port:

```bash
arduino-cli board list
make flash PORT=/dev/cu.usbmodemXXXX
```

App Center is the normal update path. Firmware revision 3 reports updater
target `seeed-xiao-nrf52840` and acknowledges a
correlated bootloader request, drains CDC, waits 100 ms, and enters the Adafruit
UF2 bootloader automatically. Targetless revision 2, factory firmware, and
unidentified application firmware require the explicit XIAO recovery action.
For that migration or later recovery, quickly press the tiny reset button beside
the USB-C connector twice. The XIAO bootloader mounts a USB drive; copy the
generated UF2 onto it, or rerun `make flash` using the bootloader's newly
enumerated serial port. A charge-only USB-C cable will not enumerate either
interface.

The application identifies itself as `Lynxware / Steam Controller Bridge`, and
the nRF52 core derives its serial number from the MCU identifier.

The current macOS prototype uses the Xbox 360 compatibility VID/PID
`045e:028e`, a vendor-class top-level device, and an `ff/5d/01` gamepad
interface. This is necessary because the original standards-based generic HID
prototype enumerated at the USB layer but was not published to macOS
GameController/browser clients. The compatibility identity is for development
hardware; a distributable product must use an owned or licensed USB identity
and re-qualify macOS recognition.

The 20-byte input report follows the conventional Xbox 360 layout: D-pad;
Start, Back, stick clicks, shoulders, Guide; A/B/X/Y; 8-bit triggers; and four
signed 16-bit stick axes. The protocol's South/East/West/North buttons map to
A/B/X/Y respectively. Protocol-only grip and extra buttons have no Xbox 360
counterpart and are ignored by the USB report.

The only accepted Xbox output is the exact eight-byte dual-rumble packet:

```text
00 08 00 LOW HIGH 00 00 00
```

It is accepted from both interrupt OUT and interface
`SET_REPORT(Output)`. USB callbacks stage bytes only; the cooperative loop
validates the full packet, scales each channel with `value * 257`, and sends
protocol-v1 `Rumble` feedback over CDC. Other lengths, headers, reserved bytes,
and Xbox output commands are ignored.

## Runtime and safety

The device starts neutral and ignores state until CDC DTR is asserted and a
protocol-v1 Hello succeeds. USB/CDC disconnect, a new Hello, three consecutive
malformed frames, or 100 ms without an active-state refresh queues a neutral
HID report. A separate two-second hardware watchdog resets the MCU if the main
loop stalls. The host refreshes unchanged active states every 25 ms, leaving
margin inside the firmware's 100 ms data watchdog.

The firmware tracks the last HID report successfully queued on the USB endpoint
and suppresses any later report - forced safety neutrals included - whose
content matches it. Serial refreshes feed the 100 ms lease without generating
USB input, and a host-side CDC teardown (for example macOS closing the port on
system-sleep entry) produces no gamepad activity after neutral has already been
queued, so it can no longer be mistaken for user input that aborts sleep.
Endpoint acceptance does not prove that the host polled the report; the cache
is invalidated on every USB mount, so a freshly enumerated host always gets a
new baseline neutral. This behavior lives in firmware: XIAOs flashed before it
must be reflashed to pick it up.

Nonzero rumble changes are returned immediately and refreshed every 25 ms as a
host-side lease. HID mount, CDC DTR loss, USB unmount, a new Hello, parser/session
reset, watchdog expiry, and reboot clear rumble and prioritize a zero feedback
command after negotiation. Rumble never refreshes the 100 ms controller-input
watchdog and does not compete with a pending neutral input report.

## Firmware revision

`src/firmware_version.h` holds `kFirmwareRevision`, a hand-maintained
monotonic counter independent of release numbering. After every successful
Hello negotiation the firmware queues one protocol `DeviceInfo` frame carrying
the revision, capability flags, installation receipt state, and the XIAO target
ID TLV. A revision 1
three-byte report remains valid, so mixed pairings stay compatible.

Revision 2 includes a dedicated 4 KiB flash page with two initially blank
receipt slots. The page is part of every UF2, so even a same-version reinstall
restores the blank marker. Firmware programs a blank slot without erasing the
page, validates field bounds and CRC, and writes the commit word last. The
second slot recovers an interrupted first write. `make artifacts` inspects the
actual ELF, Intel HEX, and UF2 to prove page alignment, isolation, blank slots,
and exactly one marker.

Bump `kFirmwareRevision` in the same commit as any behavior-affecting firmware
change. Revision and compatibility policy belongs to this target's descriptor
in `crates/release-updater/src/targets.rs`; the board-neutral protocol and
runtime do not apply a global minimum revision.

The active-low RGB LED indicates:

- slow blue: CDC disconnected;
- fast blue: connected but negotiating;
- solid green: negotiated and active;
- blinking red: parser threshold or data-watchdog fault.

## Hardware smoke test

1. Flash the board and locate its CDC port with `arduino-cli board list`.
2. Run `cargo run -p sc-probe -- list`. A successful macOS bind normally
   exposes `Steam Controller Bridge` gamepad and pointer collections in
   addition to the CDC port.
3. Run:

   ```bash
   cargo run -p gamepad-simulator -- automated --output serial \
     --port /dev/cu.usbmodemXXXX --serial-log
   ```

Verify every input category, hold an unchanged active input for at least 30
seconds, terminate the sender, and confirm a neutral HID report within 125 ms.
Then repeat after a cable reconnect and test Chrome/Safari Gamepad APIs plus a
target streaming service. Raw `sc-probe monitor` is not a reliable observer for
the output once Apple's Xbox DriverKit extension owns the interface; use a
Gamepad API tester or target client instead.

For rumble acceptance, first flash this updated firmware, start `./sc-bridge`,
and use GamepadTester's one-second and infinite vibration actions. Verify
unequal strong/weak values and that stopping the effect, closing the browser,
unplugging either endpoint, or stopping the bridge cannot leave an actuator
running. Test the Puck path independently with:

```bash
cargo run -p sc-probe -- rumble --index N --low 32768 --high 0
cargo run -p sc-probe -- rumble --index N --low 0 --high 32768
cargo run -p sc-probe -- rumble --index N --low 65535 --high 65535 \
  --duration-ms 1000
```

Hardware evidence on 2026-07-27:

- serial flashing and CDC reconnect succeeded on XIAO nRF52840 hardware;
- macOS matched `Xbox360Gamepad` and accepted continuously refreshed reports;
- Safari's Gamepad API reported a connected `Controller Extended Gamepad` with
  `mapping: standard`;
- an unchanged active simulator report remained submitted for more than 30
  seconds without firmware watchdog neutralization.
- on 2026-07-28, the rumble-enabled build flashed successfully, completed a
  fresh Hello handshake, reached Running with live Puck input, and shut down
  cleanly;
- Boosteroid, GeForce NOW, every physical control, and end-to-end dual rumble
  with correct strong/weak actuator sides are confirmed on hardware.

Sustained play of more than an hour completed without degradation. Explicit
fault timing remains a manual release gate.
