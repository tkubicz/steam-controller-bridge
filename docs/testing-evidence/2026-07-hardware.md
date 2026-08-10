# July 2026 development-hardware evidence

This is a historical observation record, not proof for later release candidates.
Commands used the development Puck, Steam Controller 2, non-Sense XIAO
nRF52840, and the source revisions current on the dates below.

## 2026-07-27 Puck and XIAO

- The active Puck slot produced valid extended `0x42` reports at about 250 Hz.
- The XIAO enumerated CDC plus an Xbox-layout gamepad and bound to macOS's
  `Xbox360Gamepad` DriverKit class.
- Safari and Boosteroid observed a connected standard-mapped gamepad.
- An unchanged active state refreshed for more than 30 seconds without an
  unintended firmware neutral.
- A 30-second serial run completed ten lizard-suppression refreshes with no
  lizard-write, decode, dropped-report, or serial failures.
- Automatic discovery selected active Puck interface 2 and the XIAO at
  `/dev/cu.usbmodem11201`, reached Running, and reported 94% battery.

## 2026-07-28 haptics and Bluetooth

- Puck index 43 accepted independent left-only and right-only 50% diagnostics,
  each with seven 40 ms writes and a final zero.
- Rebuilt firmware flashed successfully; the bridge renegotiated, stayed
  Running, and shut down cleanly.
- Direct Bluetooth appeared as `28de:1303`, usage `ff00:0001`, interface `-1`,
  and produced complete `0x45` state reports around 67-68 Hz plus `0x43` battery
  reports at 97%.
- With an idle Puck attached, automatic discovery selected only the active
  Bluetooth source and completed XIAO negotiation.
- A seven-second Bluetooth diagnostic completed the initial lizard-off write
  and two refreshes. Independent left/right rumble tests each completed seven
  writes and a final zero without HID error.

Subsequent sessions on the same development setup exercised full control
mapping, Boosteroid and GeForce NOW, strong/weak actuator orientation, desktop
lizard suppression and restoration, and more than one hour of continuous play.

## Unclosed acceptance at the time

Bluetooth reconnect, sleep/wake, refresh-failure timing, pad desktop behavior,
Puck-dock power-off, and clean-state macOS permission flows were not qualified
by these observations. They remain manual gates unless a later dated record
explicitly closes them.
