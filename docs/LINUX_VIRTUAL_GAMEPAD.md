# Linux virtual gamepad

The Linux backend uses `evdevil` 0.5 with default features disabled. It keeps
all project code safe while providing uinput device creation, nonblocking
device events, batched writes with one `SYN_REPORT`, and the complete
force-feedback upload and erase protocol required by Track C.

The virtual controller uses the Xbox-compatible identity `045e:028e`, version
`0114`, on `BUS_USB`, with the name `Steam Controller Bridge Virtual Gamepad`.
It exposes the standard 11 Xbox buttons, two sticks, two triggers, and one
D-pad. Grip and extra buttons are intentionally omitted.

The backend advertises 16 `FF_RUMBLE` slots. It services upload, update, erase,
play, and stop requests without blocking; applies replay delay and duration;
and combines overlapping effects by taking the strongest magnitude on each
motor. Zero-length effects continue until stopped.

## S2 spike status

Source and API review passed. Live S2 verification on Ubuntu is still required
before the Linux CLI can make virtual output its default. The runtime exposes
the platform-neutral virtual-gamepad selection. `sc-bridge`, `sc-replay`, and
`gamepad-simulator` select it with `--output virtual-gamepad`; the `virtual-hid`
spelling remains an alias. Linux does not require the macOS experimental opt-in
or helper path. Live `sc-bridge` still defaults to serial, and replay still
defaults to diagnostic output. The device-backed acceptance harness added later
in Track C will verify state readback, force-feedback upload/play/stop/erase,
callback magnitudes, and event-node removal. No macOS-hosted check is counted as
that evidence.

```sh
cargo run -p gamepad-simulator -- automated --output virtual-gamepad
cargo run -p sc-replay -- recording.jsonl --output virtual-gamepad
cargo run -p sc-bridge -- --output virtual-gamepad
```

Simulator and replay waits service force feedback at least every 25 ms and log
each observed aggregate as `event=output_rumble` with its low- and
high-frequency magnitudes.

Linux capability probing distinguishes a missing `/dev/uinput` device from
denied read/write access. The checked-in Linux policy loads `uinput` during boot
and grants `/dev/uinput` access to the active local session. Follow the
[Linux device-access instructions](../packaging/linux/README.md) to install the
udev and modules-load files for development, apply them without rebooting, or
configure the dedicated-group headless alternative.

Access permits the user to create arbitrary virtual input devices, including
keyboards and pointers. It cannot be restricted to this application or to
gamepads alone. Do not run the application as root.
