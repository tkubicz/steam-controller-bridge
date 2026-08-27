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
the platform-neutral virtual-gamepad selection, but the command-line tools are
not routed to it until the next Track C change. The device-backed acceptance
harness added later in Track C will verify state readback, force-feedback
upload/play/stop/erase, callback magnitudes, and event-node removal. No
macOS-hosted check is counted as that evidence.

Linux capability probing distinguishes a missing `/dev/uinput` device from
denied read/write access. Until the packaging policy lands, development setup
is temporary:

```sh
sudo modprobe uinput
sudo setfacl -m u:$USER:rw /dev/uinput
```

The ACL lasts only until `/dev/uinput` is recreated or the machine reboots.
Access permits the user to create arbitrary virtual input devices, not only
gamepads for this application. Do not run the application as root.
