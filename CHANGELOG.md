# Changelog

Release Please generates this file from Conventional Commit squash merges. Do
not edit release entries by hand; correct the originating pull-request metadata
and let the release pull request regenerate them. Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.2.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.1.0...v1.2.0) (2026-08-04)


### Features

* log status changes as deltas with periodic snapshots ([bfacf56](https://github.com/tkubicz/steam-controller-bridge/commit/bfacf56bdacc08ae051fa518523b56f4a4e78977))
* log status changes as deltas with periodic snapshots ([#10](https://github.com/tkubicz/steam-controller-bridge/issues/10)) ([bfacf56](https://github.com/tkubicz/steam-controller-bridge/commit/bfacf56bdacc08ae051fa518523b56f4a4e78977))

## [1.1.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.0.0...v1.1.0) (2026-08-04)


### Features

* add configurable controller idle shutdown ([05f399c](https://github.com/tkubicz/steam-controller-bridge/commit/05f399ca55dcc44f6e13cf16b9f50f01cdb96b62))


### Bug Fixes

* **menu:** reuse native status images to bound memory usage ([05f399c](https://github.com/tkubicz/steam-controller-bridge/commit/05f399ca55dcc44f6e13cf16b9f50f01cdb96b62))

## [1.0.0] - 2026-07-30

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
  Diagnostics, Input Monitoring settings, and rotated logs. The bundle carries
  an application icon, so it is identifiable in Finder and Spotlight rather than
  showing the blank placeholder.
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
- GitHub releases are the only distribution channel. A Homebrew cask and an App
  Store build are intended later, and no crate is published to crates.io.
- Steam coexistence, multiple simultaneous controllers, and other HID consumers
  against the selected collection are unsupported.

[1.0.0]: https://github.com/tkubicz/steam-controller-bridge/releases/tag/v1.0.0
