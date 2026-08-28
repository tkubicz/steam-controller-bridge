# Linux virtual gamepad

The Linux backend uses `evdevil` 0.5 with default features disabled. It keeps
all project code safe while providing uinput device creation, nonblocking
device events, batched writes with one `SYN_REPORT`, and the complete
force-feedback upload and erase protocol required by Track C.

The virtual controller uses the Xbox-compatible identity `045e:028e`, version
`0114`, on `BUS_USB`, with the name `Steam Controller Bridge Virtual Gamepad`.
It exposes the standard 11 Xbox buttons, two sticks, two triggers, and one
D-pad. Grip and extra buttons are intentionally omitted.

## S2 spike status

Source and API review passed. Live S2 verification on Ubuntu is still required
before the Linux backend can be selected by production code. The device-backed
acceptance harness added later in Track C will verify state readback, force
feedback upload/play/stop/erase, callback magnitudes, and event-node removal.
No macOS-hosted check is counted as that evidence.
