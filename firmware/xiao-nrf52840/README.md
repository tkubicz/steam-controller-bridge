# XIAO nRF52840 firmware

This firmware turns a non-Sense Seeed Studio XIAO nRF52840 into the physical
USB endpoint for the macOS bridge. It exposes one composite USB device:

- CDC ACM carries protocol-v1 frames from the host.
- Generic HID exposes a 16-button gamepad with a hat, four signed axes, and two
  unsigned triggers.

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

The application retains the BSP's prototype VID/PID. Its product string is
`Steam Controller Bridge`; the nRF52 core derives the USB serial number from
the MCU identifier.

## Runtime and safety

The device starts neutral and ignores state until CDC DTR is asserted and a
protocol-v1 Hello succeeds. USB/CDC disconnect, a new Hello, three consecutive
malformed frames, or 100 ms without an active-state refresh queues a neutral
HID report. A separate two-second hardware watchdog resets the MCU if the main
loop stalls. The host refreshes unchanged active states every 50 ms.

The active-low RGB LED indicates:

- slow blue: CDC disconnected;
- fast blue: connected but negotiating;
- solid green: negotiated and active;
- blinking red: parser threshold or data-watchdog fault.

## Hardware smoke test

1. Flash the board and locate its CDC port with `arduino-cli board list`.
2. Run `cargo run -p sc-probe -- list` and identify the `Steam Controller
   Bridge` gamepad collection.
3. Monitor that collection with `cargo run -p sc-probe -- monitor --index N
   --raw`.
4. In another terminal run:

   ```bash
   cargo run -p gamepad-simulator -- automated --output serial \
     --port /dev/cu.usbmodemXXXX --serial-log
   ```

Verify every input category, hold an unchanged active input for at least 30
seconds, terminate the sender, and confirm a neutral HID report within 125 ms.
Then repeat after a cable reconnect and test Chrome/Safari Gamepad APIs plus a
target streaming service. The one-hour soak and browser/service checks remain
manual hardware release gates.
