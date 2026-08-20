# Changelog

Release Please generates new entries from Conventional Commit squash merges.
Published history may be corrected directly when this file and the matching
GitHub Release are updated together. Versions follow [Semantic
Versioning](https://semver.org/spec/v2.0.0.html).

## [1.10.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.9.0...v1.10.0) (2026-08-20)


### Features

* **caps:** map Linux HID and serial requirements from active features ([#73](https://github.com/tkubicz/steam-controller-bridge/issues/73)) ([7c68a8f](https://github.com/tkubicz/steam-controller-bridge/commit/7c68a8fee8f48ea04a610f15faaa7fa80221ed0e))
* configurable trackpad motion and user-defined pad regions ([1c01758](https://github.com/tkubicz/steam-controller-bridge/commit/1c01758ff143e45253e3b8e4dc0c06b1c6e7c20b))
* **device:** enable the hidapi linux-native backend ([#67](https://github.com/tkubicz/steam-controller-bridge/issues/67)) ([66d5dc3](https://github.com/tkubicz/steam-controller-bridge/commit/66d5dc3785875f3fceb52a1213e2371098690528))
* **device:** report conflicting Linux kernel HID ownership ([#69](https://github.com/tkubicz/steam-controller-bridge/issues/69)) ([2915c3b](https://github.com/tkubicz/steam-controller-bridge/commit/2915c3bb9d769be8ddd903438151db68de04668d))
* **menu:** offer recovery when the profile store cannot be read ([1c01758](https://github.com/tkubicz/steam-controller-bridge/commit/1c01758ff143e45253e3b8e4dc0c06b1c6e7c20b))
* **packaging:** add narrowly matched Linux controller udev rules ([#68](https://github.com/tkubicz/steam-controller-bridge/issues/68)) ([d0e70e2](https://github.com/tkubicz/steam-controller-bridge/commit/d0e70e2d2de1d1c06793aa6dc606fd39b6c73238))
* **packaging:** add narrowly matched official bridge-device serial rules ([#72](https://github.com/tkubicz/steam-controller-bridge/issues/72)) ([ecfdb99](https://github.com/tkubicz/steam-controller-bridge/commit/ecfdb99b3c740a80db3fb6eed29afd946ccd8ec7))
* **serial:** add Linux bridge-port discovery and diagnostics ([#70](https://github.com/tkubicz/steam-controller-bridge/issues/70)) ([d7ebeb1](https://github.com/tkubicz/steam-controller-bridge/commit/d7ebeb19832eae143f70bfe9d644eee906eab320))


### Bug Fixes

* proper color scheme when macOS light mode is enabled ([#41](https://github.com/tkubicz/steam-controller-bridge/issues/41)) ([a68a2f8](https://github.com/tkubicz/steam-controller-bridge/commit/a68a2f83527a94f63f5a3e2561791a28415ac0d8))
* **updater:** gate local update sources explicitly ([#55](https://github.com/tkubicz/steam-controller-bridge/issues/55)) ([7578549](https://github.com/tkubicz/steam-controller-bridge/commit/757854924d40fe3a97cb30baad7d7d3b0df66ae2))

## [1.9.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.8.0...v1.9.0) (2026-08-13)


### Features

* add an experimental macOS virtual gamepad output ([ffc6fd1](https://github.com/tkubicz/steam-controller-bridge/commit/ffc6fd17701665dbb61193e9d00d92f0b3d64e37))

## [1.8.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.7.0...v1.8.0) (2026-08-12)


### Features

* **menu:** improve update controls, progress feedback, and preview states ([ab6e94e](https://github.com/tkubicz/steam-controller-bridge/commit/ab6e94e61ddc821d7afbf3a74b8e59ff1c9e7e98))
* **updater:** add manual update checks and resilient release downloads ([ab6e94e](https://github.com/tkubicz/steam-controller-bridge/commit/ab6e94e61ddc821d7afbf3a74b8e59ff1c9e7e98))


### Bug Fixes

* **updater:** verify successful firmware flashes after UF2 disconnects ([ab6e94e](https://github.com/tkubicz/steam-controller-bridge/commit/ab6e94e61ddc821d7afbf3a74b8e59ff1c9e7e98))

## [1.7.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.6.0...v1.7.0) (2026-08-12)


### Features

* **firmware:** automate UF2 updates with verified installation receipts ([b6f78cd](https://github.com/tkubicz/steam-controller-bridge/commit/b6f78cd0fc7872fc5264d42e13363a8ae9834b0d))
* **menu:** clarify bridge hardware and firmware status ([c6789e3](https://github.com/tkubicz/steam-controller-bridge/commit/c6789e38a82d7e605fef2a30f320f8b3d4940094))
* support protocol-compatible bridge devices ([c6789e3](https://github.com/tkubicz/steam-controller-bridge/commit/c6789e38a82d7e605fef2a30f320f8b3d4940094))


### Bug Fixes

* **app-center:** keep catalog and firmware status accurate after failures ([b6f78cd](https://github.com/tkubicz/steam-controller-bridge/commit/b6f78cd0fc7872fc5264d42e13363a8ae9834b0d))
* **updater:** stop reporting verified firmware flashes as failed ([c6789e3](https://github.com/tkubicz/steam-controller-bridge/commit/c6789e38a82d7e605fef2a30f320f8b3d4940094))

## [1.6.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.5.0...v1.6.0) (2026-08-11)


### Features

* **updater:** add one App Center for application and XIAO firmware updates ([9fa5e63](https://github.com/tkubicz/steam-controller-bridge/commit/9fa5e63a77619d108220d99749bdd2562dfa73c0))


### Bug Fixes

* **runtime:** preserve bridge run intent across firmware updates ([9fa5e63](https://github.com/tkubicz/steam-controller-bridge/commit/9fa5e63a77619d108220d99749bdd2562dfa73c0))
* **updater:** keep retries, downloads, and firmware flashing lifecycle-safe ([9fa5e63](https://github.com/tkubicz/steam-controller-bridge/commit/9fa5e63a77619d108220d99749bdd2562dfa73c0))

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

* log status changes as deltas with periodic snapshots ([bfacf56](https://github.com/tkubicz/steam-controller-bridge/commit/bfacf56bdacc08ae051fa518523b56f4a4e78977))

## [1.1.0](https://github.com/tkubicz/steam-controller-bridge/compare/v1.0.0...v1.1.0) (2026-08-04)


### Features

* add configurable controller idle shutdown ([05f399c](https://github.com/tkubicz/steam-controller-bridge/commit/05f399ca55dcc44f6e13cf16b9f50f01cdb96b62))


### Bug Fixes

* **menu:** reuse native status images to bound memory usage ([05f399c](https://github.com/tkubicz/steam-controller-bridge/commit/05f399ca55dcc44f6e13cf16b9f50f01cdb96b62))

## [1.0.0](https://github.com/tkubicz/steam-controller-bridge/releases/tag/v1.0.0) (2026-08-01)


### Features

* **bridge:** translate Steam Controller 2 input through the Proteus Puck into a standard gamepad ([f6d5c08](https://github.com/tkubicz/steam-controller-bridge/commit/f6d5c088169750f8cb6bb7c42365599f00268a4b))
* **controller:** add direct Bluetooth input with automatic transport discovery ([#2](https://github.com/tkubicz/steam-controller-bridge/issues/2)) ([426e2f9](https://github.com/tkubicz/steam-controller-bridge/commit/426e2f9a904c129c100c3c82e34b1bd9bc594966))
* **firmware:** add XIAO nRF52840 CDC/gamepad firmware and pinned build tooling ([bde2a1f](https://github.com/tkubicz/steam-controller-bridge/commit/bde2a1fb9959354b2fb4368f44243fcf682f0685))
* **haptics:** add end-to-end dual-actuator rumble with a bounded host lease ([91838ab](https://github.com/tkubicz/steam-controller-bridge/commit/91838ab1816a95a0cf67abc84ae4734d85f794d6))
* **menu:** add native macOS menu-bar controls and diagnostics ([4d5891c](https://github.com/tkubicz/steam-controller-bridge/commit/4d5891c9ebfd0da093f402b602b4730b86c2b030))
* **recording:** add deterministic capture and replay ([244c8a7](https://github.com/tkubicz/steam-controller-bridge/commit/244c8a714997e9d41a1619cf52dc66d470672fbd))
* **tools:** add controller probing and report decoding ([84d4bf4](https://github.com/tkubicz/steam-controller-bridge/commit/84d4bf46857d694fe71922fc860f59a23296212d))
* **visualizer:** add live controller diagnostics ([bb54cde](https://github.com/tkubicz/steam-controller-bridge/commit/bb54cdeb4767402381ce9dcbb08de92288bfd793))
* **release:** publish firmware and macOS application artifacts with checksums ([#3](https://github.com/tkubicz/steam-controller-bridge/issues/3)) ([6f0542b](https://github.com/tkubicz/steam-controller-bridge/commit/6f0542be5176ad33bd17290e41182c4b913aa260))


### Bug Fixes

* stabilize packaging, hardware selection, and documentation before the public release ([#1](https://github.com/tkubicz/steam-controller-bridge/issues/1)) ([abb30e1](https://github.com/tkubicz/steam-controller-bridge/commit/abb30e1a70a9af8283b553aa51d834d3ed843332))


### Performance Improvements

* **runtime:** reduce idle CPU use and keep status publication off the controller hot path ([#3](https://github.com/tkubicz/steam-controller-bridge/issues/3)) ([6f0542b](https://github.com/tkubicz/steam-controller-bridge/commit/6f0542be5176ad33bd17290e41182c4b913aa260))
