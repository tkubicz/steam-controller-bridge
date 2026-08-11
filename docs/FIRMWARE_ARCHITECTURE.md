# XIAO nRF52840 Firmware Architecture

Firmware is implemented in `firmware/xiao-nrf52840` for the XIAO nRF52840 and XIAO nRF52840 Sense.
Its native protocol/session tests run without hardware. Serial flashing,
CDC/gamepad enumeration, macOS Xbox-driver binding, Safari Gamepad API
visibility, a 30-second unchanged-state refresh, full control-by-control
mapping, Boosteroid and GeForce NOW, and end-to-end dual rumble have also been
exercised on hardware, along with more than an hour of continuous play. Failure
timing remains a release gate.

The XIAO firmware exposes a USB composite device with a CDC interface for
bridge frames and an Xbox 360-compatible vendor interface for browsers and
streaming clients. The generic HID personality from the original plan was
replaced after hardware proved that macOS enumerated it at the USB layer but
did not publish it to GameController or Safari. Its parser implements protocol
v1 exactly as documented in `GAMEPAD_PROTOCOL.md`.

Required behavior:

- Search and resynchronize on `SC` magic bytes.
- Reject payloads over 256 bytes, unsupported versions, invalid message lengths, invalid hats, reserved axis values, and bad CRCs.
- Complete `Hello` / `HelloResponse` negotiation before applying states.
- Convert `GamepadState` and `Neutral` messages into USB HID reports.
- Track sequence gaps for diagnostics without allowing a gap to leave stale controls active.
- Respond to `Ping` with the same nonce in `Pong`.
- Enter the UF2 bootloader only after negotiated, correlated control, neutral
  input, zero rumble, CDC drain, and a 100 ms response grace period.
- Store verified installation metadata in a dedicated 4 KiB flash page with
  two blank slots, CRC validation, and a commit-last write that never erases.
- Accept only the exact eight-byte Xbox 360 dual-rumble OUT command, scale its
  two 8-bit channels to `u16`, and return protocol message type 8 over CDC.
- Refresh nonzero rumble feedback every 25 ms and prioritize a safety zero
  across USB/CDC/session resets without feeding the input watchdog.
- Reset the HID report to neutral after a host-data watchdog timeout (initial target: 100 ms).
- Reset immediately on CDC disconnect, protocol uncertainty, or parser failure threshold.
- Use an LED for disconnected, negotiating, active, and fault states without affecting timing.

Hardware validation must cover CDC reconnect, HID enumeration, browser Gamepad API behavior, sequence wrap, malformed input recovery, host process termination, cable removal, and watchdog-to-neutral timing.

The host services serial output during idle waits and refreshes unchanged active
states every 25 ms. Ping does not refresh the firmware data watchdog. See the
firmware README for the pinned toolchain, flashing, LED states, and smoke test.

The Xbox OUT path is serviced from the cooperative loop. Interrupt and
`SET_REPORT(Output)` callbacks only stage the completed eight bytes and rearm
work; they never parse packets or write CDC frames directly.

The development personality uses compatibility VID/PID `045e:028e` so Apple's
built-in Xbox driver publishes the device. Shipping requires an owned/licensed
USB identity and a re-qualified macOS recognition path.

OpenPuck is an architectural reference for nRF52840/TinyUSB scheduling and
watchdog practices only. Its direct radio, selectable USB modes, configuration,
and AGPL-licensed implementation are not part of this firmware.
