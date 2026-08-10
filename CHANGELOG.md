# Changelog

Release Please generates this file from Conventional Commit squash merges. Do
not edit release entries by hand; correct the originating pull-request metadata
and let the release pull request regenerate them. Versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.5.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.4.0...v1.5.0) (2026-08-10)


### Features

* bindable left and right pad clicks with one-tick click feedback ([0a9eb50](https://github.com/tkubicz/steam-controller-bridge/commit/0a9eb5009333221d230b81a030655fe38bb0b3e2))
* capture-matched pad pointer motion with press-freeze, drag intent, and pointer speed ([0a9eb50](https://github.com/tkubicz/steam-controller-bridge/commit/0a9eb5009333221d230b81a030655fe38bb0b3e2))
* **cli:** add validated clap interfaces to controller tools ([bdccf33](https://github.com/tkubicz/steam-controller-bridge/commit/bdccf335cb3262dc4ef631b257d1e3fef1d5a978))
* Common visual identity ([#21](https://github.com/tkubicz/steam-controller-bridge/issues/21)) ([e712087](https://github.com/tkubicz/steam-controller-bridge/commit/e712087bb7d9f4cb18224dece323072072ce2b1c))
* **menu:** add a dedicated About window with browsable release notes ([b1bafbf](https://github.com/tkubicz/steam-controller-bridge/commit/b1bafbf20f20726c1f1ab59e4a62b9eb8bd3f345))
* **menu:** reorganize bridge controls, profiles, shutdown, and troubleshooting ([e49648e](https://github.com/tkubicz/steam-controller-bridge/commit/e49648e5ba9410cd2ef1c79bd9f6e478773e68ab))
* **menu:** report XIAO firmware revisions and update recommendations ([4418454](https://github.com/tkubicz/steam-controller-bridge/commit/4418454f98c6fcebedc15ad02b39459e9f053610))
* **updater:** add signed application and XIAO firmware updates ([e49648e](https://github.com/tkubicz/steam-controller-bridge/commit/e49648e5ba9410cd2ef1c79bd9f6e478773e68ab))
* use common, higher quality font for every GUI window ([b1bafbf](https://github.com/tkubicz/steam-controller-bridge/commit/b1bafbf20f20726c1f1ab59e4a62b9eb8bd3f345))
* **visualizer:** add responsive controller diagnostics with automatic discovery ([bdccf33](https://github.com/tkubicz/steam-controller-bridge/commit/bdccf335cb3262dc4ef631b257d1e3fef1d5a978))
* **visualizer:** guided Lizard Mouse Lab for capture, analysis, and comparison ([0a9eb50](https://github.com/tkubicz/steam-controller-bridge/commit/0a9eb5009333221d230b81a030655fe38bb0b3e2))


### Bug Fixes

* **firmware:** suppress duplicate HID reports during macOS sleep ([4418454](https://github.com/tkubicz/steam-controller-bridge/commit/4418454f98c6fcebedc15ad02b39459e9f053610))
* **menu:** rename binding menus to profiles and clarify menu grouping ([b1bafbf](https://github.com/tkubicz/steam-controller-bridge/commit/b1bafbf20f20726c1f1ab59e4a62b9eb8bd3f345))
* **menu:** show profile overlay above screen-saver-level windows ([4418454](https://github.com/tkubicz/steam-controller-bridge/commit/4418454f98c6fcebedc15ad02b39459e9f053610))
* **menu:** stop settings saves from clobbering each other and leaving stale temp files ([2cf931a](https://github.com/tkubicz/steam-controller-bridge/commit/2cf931abbed6b03798420466bfc593229ba7d314))
* **visualizer:** align dead-zone guides and touch-gated pad input with the mapper ([bdccf33](https://github.com/tkubicz/steam-controller-bridge/commit/bdccf335cb3262dc4ef631b257d1e3fef1d5a978))
* **visualizer:** neutralize stale output across disconnects, timeouts, and failures ([bdccf33](https://github.com/tkubicz/steam-controller-bridge/commit/bdccf335cb3262dc4ef631b257d1e3fef1d5a978))


### Performance Improvements

* **serial:** yield while waiting for firmware handshake ([4418454](https://github.com/tkubicz/steam-controller-bridge/commit/4418454f98c6fcebedc15ad02b39459e9f053610))
* **visualizer:** preserve 250 Hz input fidelity and record off the UI thread ([bdccf33](https://github.com/tkubicz/steam-controller-bridge/commit/bdccf335cb3262dc4ef631b257d1e3fef1d5a978))

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


### Bug Fixes

* **menu:** keep the tray menu open when a submenu is first opened ([54f52d1](https://github.com/tkubicz/steam-controller-bridge/commit/54f52d13751fde76a08421c4175b0fb4108aa285))
* **menu:** request Input Monitoring and Accessibility from the menu ([54f52d1](https://github.com/tkubicz/steam-controller-bridge/commit/54f52d13751fde76a08421c4175b0fb4108aa285))


### Performance Improvements

* **app:** shrink the macOS app with link-time optimisation and stripped symbols ([54f52d1](https://github.com/tkubicz/steam-controller-bridge/commit/54f52d13751fde76a08421c4175b0fb4108aa285))

## [1.2.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.1.0...v1.2.0) (2026-08-04)


### Features

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
