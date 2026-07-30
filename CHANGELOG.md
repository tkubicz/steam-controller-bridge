# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-07-30

First public release. macOS only, and Steam Controller 2 (2026) only.

### Added

- **Steam Controller 2 input** over the official Proteus Puck (`28de:1304`, USB,
  interfaces 2–5) and over direct Bluetooth (`28de:1303`, interface -1). Zero
  argument discovery selects whichever collection is uniquely producing valid
  `0x42`/`0x45` controller state, and refuses to guess when more than one is.
- **XIAO nRF52840 firmware** exposing a composite USB device: CDC ACM for
  protocol-v1 frames from the host, plus an Xbox 360-compatible vendor interface
  that macOS binds to its built-in `Xbox360Gamepad` driver. Browsers and
  streaming clients see a standard-mapped gamepad.
- **End-to-end dual rumble**, from a client vibration request through Xbox OUT
  traffic and CDC feedback to the correct strong and weak actuators, with a
  100 ms host lease so no path can leave an actuator latched.
- **Lizard-mode suppression** using the single SDL-compatible feature report,
  refreshed every three seconds, so controller input stops producing desktop
  keyboard and pointer events while the bridge runs.
- **macOS menu-bar application** (`sc-bridge-menu`) with bridge, input,
  controller, XIAO, battery, haptics, and error status; Start/Stop, Copy
  Diagnostics, Input Monitoring settings, and rotated logs.
- **Developer tooling**: `sc-probe` for HID enumeration, monitoring, capture, and
  the narrow lizard/rumble diagnostics; `sc-visualizer` for live raw, decoded,
  and mapped state; `gamepad-simulator` and `sc-replay` for hardware-free work.
- Reproducible firmware artifacts (UF2 and DFU zip) and an ad-hoc-signed macOS
  app bundle, published from tagged builds with SHA-256 sums.

### Security

- Only two writes are ever sent to a controller: the fixed lizard-off feature
  report and the exact standard dual-rumble output. Both are gated on an exact
  vendor, product, usage, interface, and transport match. No arbitrary feature or
  output report API is exposed.
- All crates set `unsafe_code = "forbid"`.
- Hardware serials are masked to their last four characters in every status line,
  log, and `Copy Diagnostics` output. On Bluetooth that value is the controller's
  MAC address. Capture and recording files deliberately keep the full serial so
  they stay replayable, and both formats document that before sharing.

### Known limitations

- The firmware enumerates with the Xbox 360 compatibility VID/PID `045e:028e`.
  Apple's driver will not publish a generic HID gamepad to GameController, so
  this is required for macOS recognition. A distributable product needs an owned
  or licensed USB identity.
- Button and axis mapping is fixed; there is no configuration file yet.
- The macOS app is ad-hoc signed, not notarized. Gatekeeper needs a one-time
  right-click → Open.
- Steam coexistence, multiple simultaneous controllers, and other HID consumers
  against the selected collection are unsupported.

[Unreleased]: https://github.com/tkubicz/steam-controller-bridge/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/tkubicz/steam-controller-bridge/releases/tag/v0.1.0
