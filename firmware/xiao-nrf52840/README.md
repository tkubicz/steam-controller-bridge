# XIAO nRF52840 firmware

This firmware turns a non-Sense Seeed Studio XIAO nRF52840 into the physical
USB endpoint for the macOS bridge. It exposes one composite USB device:

- CDC ACM carries protocol-v1 frames from the host.
- An Xbox 360-compatible vendor interface exposes an ABXY gamepad through
  macOS's built-in Xbox GameController driver.

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
UF2 and DFU zip in `build/artifacts/`.

## Flash and recovery

Find the application or bootloader port:

```bash
arduino-cli board list
make flash PORT=/dev/cu.usbmodemXXXX
```

If serial upload cannot reset the board, double-tap RESET. The XIAO bootloader
mounts a USB drive; copy the generated UF2 onto it, or rerun `make flash` using
the bootloader's newly enumerated serial port. A charge-only USB-C cable will
not enumerate either interface.

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

## Runtime and safety

The device starts neutral and ignores state until CDC DTR is asserted and a
protocol-v1 Hello succeeds. USB/CDC disconnect, a new Hello, three consecutive
malformed frames, or 100 ms without an active-state refresh queues a neutral
HID report. A separate two-second hardware watchdog resets the MCU if the main
loop stalls. The host refreshes unchanged active states every 25 ms, leaving
margin inside the firmware's 100 ms data watchdog.

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

Hardware evidence on 2026-07-27:

- serial flashing and CDC reconnect succeeded on a non-Sense XIAO;
- macOS matched `Xbox360Gamepad` and accepted continuously refreshed reports;
- Safari's Gamepad API reported a connected `Controller Extended Gamepad` with
  `mapping: standard`;
- an unchanged active simulator report remained submitted for more than 30
  seconds without firmware watchdog neutralization.

GeForce NOW, Boosteroid, every physical control, explicit fault timing, and the
one-hour soak remain manual release gates.
