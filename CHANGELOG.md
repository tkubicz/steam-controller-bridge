# Changelog

Release Please generates this file from Conventional Commit squash merges. Do
not edit release entries by hand; correct the originating pull-request metadata
and let the release pull request regenerate them. Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.4.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.3.0...v1.4.0) (2026-08-07)


### Features

* Adds support for Steam Controller pads - mouse movement and scrolling ([#19](https://github.com/tkubicz/steam-controller-bridge/issues/19)) ([5801b0e](https://github.com/tkubicz/steam-controller-bridge/commit/5801b0ee126ee869d80eed0f3914e2b5178aebd2))
* **menu:** add controller-driven in-game profile wheel ([d2e4bef](https://github.com/tkubicz/steam-controller-bridge/commit/d2e4bef886273ec7bb114ab8de46e115f9718624))


### Bug Fixes

* proper version for macos application ([#17](https://github.com/tkubicz/steam-controller-bridge/issues/17)) ([f5b9730](https://github.com/tkubicz/steam-controller-bridge/commit/f5b9730e829ef5f0490cdac6a1fdd25ab491897f))
* **runtime:** keep controller supervision responsive during desktop input cleanup ([d2e4bef](https://github.com/tkubicz/steam-controller-bridge/commit/d2e4bef886273ec7bb114ab8de46e115f9718624))
* **runtime:** release bridge hardware safely across macOS sleep and wake ([d2e4bef](https://github.com/tkubicz/steam-controller-bridge/commit/d2e4bef886273ec7bb114ab8de46e115f9718624))


### Performance Improvements

* **runtime:** eliminate idle desktop-binding polling ([d2e4bef](https://github.com/tkubicz/steam-controller-bridge/commit/d2e4bef886273ec7bb114ab8de46e115f9718624))

## [1.3.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.2.0...v1.3.0) (2026-08-05)


### Features

* **bindings:** map grip paddles and Quick Access to keyboard and mouse actions ([54f52d1](https://github.com/tkubicz/steam-controller-bridge/commit/54f52d13751fde76a08421c4175b0fb4108aa285))
* **ci:** fix release using github app ([#16](https://github.com/tkubicz/steam-controller-bridge/issues/16)) ([6334279](https://github.com/tkubicz/steam-controller-bridge/commit/6334279dc5ed1fb4f131b804166859427937f36b))


### Bug Fixes

* **ci:** commit message release gate ([#15](https://github.com/tkubicz/steam-controller-bridge/issues/15)) ([e9732b0](https://github.com/tkubicz/steam-controller-bridge/commit/e9732b0b5a0e618488b436f89d09df50b8365c5c))
* **menu:** keep the tray menu open when a submenu is first opened ([54f52d1](https://github.com/tkubicz/steam-controller-bridge/commit/54f52d13751fde76a08421c4175b0fb4108aa285))
* **menu:** request Input Monitoring and Accessibility from the menu ([54f52d1](https://github.com/tkubicz/steam-controller-bridge/commit/54f52d13751fde76a08421c4175b0fb4108aa285))


### Performance Improvements

* **app:** shrink the macOS app with link-time optimisation and stripped symbols ([54f52d1](https://github.com/tkubicz/steam-controller-bridge/commit/54f52d13751fde76a08421c4175b0fb4108aa285))

## [1.2.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.1.0...v1.2.0) (2026-08-04)


### Features

* log status changes as deltas with periodic snapshots ([bfacf56](https://github.com/tkubicz/steam-controller-bridge/commit/bfacf56bdacc08ae051fa518523b56f4a4e78977))
* log status changes as deltas with periodic snapshots ([#10](https://github.com/tkubicz/steam-controller-bridge/issues/10)) ([bfacf56](https://github.com/tkubicz/steam-controller-bridge/commit/bfacf56bdacc08ae051fa518523b56f4a4e78977))

## [1.1.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.0.0...v1.1.0) (2026-08-04)


### Features

* add configurable controller idle shutdown ([05f399c](https://github.com/tkubicz/steam-controller-bridge/commit/05f399ca55dcc44f6e13cf16b9f50f01cdb96b62))


### Bug Fixes

* **menu:** reuse native status images to bound memory usage ([05f399c](https://github.com/tkubicz/steam-controller-bridge/commit/05f399ca55dcc44f6e13cf16b9f50f01cdb96b62))

## [1.0.0](https://github.com/tkubicz/steam-controller-bridge/releases/tag/v1.0.0) (2026-07-30)


### Features

* First public release for macOS and Steam Controller 2 (2026)
* **controller:** support Steam Controller 2 input over the official Proteus Puck (`28de:1304`, USB interfaces 2–5) and direct Bluetooth (`28de:1303`, interface -1), with zero-argument discovery that refuses ambiguous collections
* **firmware:** expose the XIAO nRF52840 as a composite CDC ACM and Xbox 360-compatible USB device that macOS, browsers, and streaming clients recognize as a standard-mapped gamepad
* **haptics:** route end-to-end dual rumble to the correct strong and weak actuators with a 100 ms host lease that prevents latched output
* **controller:** suppress lizard mode with the SDL-compatible feature report refreshed every three seconds
* **menu:** provide a macOS menu-bar application with bridge, device, battery, haptics, diagnostics, permission, and rotated-log controls
* **tools:** add `sc-probe`, `sc-visualizer`, `gamepad-simulator`, and `sc-replay` for hardware diagnostics, visualization, simulation, and replay
* **release:** publish reproducible firmware artifacts and an ad-hoc-signed macOS application bundle with SHA-256 sums

### Security

* Restrict controller writes to the fixed lizard-off feature report and exact standard dual-rumble output, gated by vendor, product, usage, interface, and transport
* Set `unsafe_code = "forbid"` across all crates
* Mask hardware serials in status, logs, and diagnostics while retaining full serials only in explicitly share-sensitive capture and recording files

### Known Limitations

* Firmware uses the Xbox 360 compatibility VID/PID `045e:028e`; a distributable product requires an owned or licensed USB identity
* Button and axis mapping is fixed with no configuration file
* The macOS application is ad-hoc signed rather than notarized and requires a one-time right-click → Open
* GitHub Releases is the only distribution channel; there is no Homebrew cask, App Store build, or crates.io publication
* Steam coexistence, multiple simultaneous controllers, and other HID consumers against the selected collection are unsupported
