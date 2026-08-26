# Testing

Virtual-gamepad automated proof and the separate entitlement-blocked VM/live
gates are tracked in [the feasibility matrix](VIRTUAL_HID_FEASIBILITY.md).

Automated gates prove portable logic, lifecycle ordering, packaging structure,
and signed-update policy. They do not prove physical controls, permissions,
USB re-enumeration, browser behavior, or visual acceptance.

## CI-equivalent gates

```bash
python3 tools/check-workspace-versions.py --self-test
python3 tools/check-workspace-versions.py
python3 tools/check-linux-udev-rules.py --self-test
python3 tools/check-linux-udev-rules.py
python3 tools/check-changelog.py --self-test
python3 tools/check-changelog.py
python3 tools/build-macos-app.py --self-test
python3 tools/prepare-local-update.py --self-test
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo clippy -p release-updater -p sc-bridge-menu --all-targets --all-features -- -D warnings
cargo test -p release-updater -p sc-bridge-menu --all-features
cargo build --workspace --all-targets
make -C firmware/xiao-nrf52840 test
```

On a Linux runner with no attached supported controller or bridge device, also
run the explicitly ignored native smoke targets:

```bash
cargo test -p steam-controller-device --test linux_no_device -- --ignored
cargo test -p bridge-output --test linux_no_device -- --ignored
```

The default pass checks the distributed configuration, where local filesystem
updates are compiled out. The focused all-feature pass checks the two packages
whose code changes when `local-update-source` is enabled. Neither pass replaces
the other because `cfg(not(feature = "local-update-source"))` code exists only
in the default configuration.

On macOS, also build and verify the packaged application:

```bash
cargo check -p sc-bridge-menu --features local-update-source
./tools/build-macos-app.py
codesign --verify --deep --strict "dist/Steam Controller Bridge.app"
```

The focused check uses the exact feature exposed by the menu package, proving
that it forwards into the macOS-only `release-updater` dependency. A
workspace-wide `--all-features` command alone cannot prove that edge because it
enables the updater crate's feature directly.

CI additionally builds the pinned Arduino firmware artifacts. `cargo deny
check` covers advisories, licenses, bans, and sources for the shipped macOS
dependency graph; CI separately audits the `platform-capabilities` graph for
the x86-64 Linux target used by the hosted runner. That graph includes the
Linux controller HID, serial, and access-check dependencies while remaining
scoped away from the unshipped Linux GUI stack.

Ubuntu CI runs `udevadm verify` on every checked-in Linux device rule. The
cross-platform policy check separately rejects missing, duplicate, broad, or
world-writable controller HID and bridge serial access entries, and requires
the exact bridge rule to opt out of ModemManager probing. Installation and
headless fallback policy are documented with the
[Linux packaging inputs](../packaging/linux/README.md).

The Ubuntu hosted runner also executes explicitly no-device native smoke tests:
the filtered HID enumerator is constructed, refreshed, and dropped across two
empty scans, and bridge serial discovery completes without a matching
endpoint. The ordinary workspace suite supplies the scripted connect, report,
disconnect, retry, and shutdown lifecycle coverage. These checks do not claim
physical Puck or XIAO access.

## Automated coverage map

| Boundary | What tests establish |
| --- | --- |
| Steam protocol and device | Complete report decoding, typed failures, exact collection allowlists, fixed write vectors, and masked diagnostics. |
| Mapper and bridge core | Control mapping, filter reset, timeout/disconnect neutralization, changed-state policy, and reconnect accounting. |
| Serial and wire protocol | Negotiation, framing/CRC recovery, bounded queues, ping/pong health, sequence handling, and unchanged-state refresh. |
| Runtime | Discovery, transition retention, neutral-before-release, sleep/update suspension, automatic shutdown, feedback leasing, and command acknowledgement. |
| Desktop profiles | Schema migration, validation, atomic persistence, unreadable-store recovery, edge/reference counts, pad motion modes, region geometry and hysteresis, click/touch latching, feedback cadence, and sink-failure cleanup. |
| Recording and replay | Typed round trips, ordering, unknown events, seeking, malformed input, and deterministic replay. |
| Updater | Signature-before-parse, rollback prevention, exact artifact verification, retry/cache semantics, concurrent temporaries, UF2 validation, automatic entry, cancellation, and receipt verification. |
| Menu and child processes | Settings migration, status rendering, bounded IPC, request correlation, stale-generation rejection, process reaping, diagnostics, and feature gates. |
| Firmware native tests | Parser/session recovery, handshake gating, watchdog behavior, rumble feedback, and malformed-frame handling without hardware. |

Test names and fixtures are the source of truth for individual cases. This file
records maintained contracts rather than duplicating every assertion.

## Focused performance and lifecycle gates

```bash
cargo test --release -p bridge-runtime \
  tests::desktop_worker_input_latency_stays_within_budget -- --exact --nocapture
cargo test --release -p sc-visualizer \
  mailbox::tests::nominal_device_rate_with_sixty_hz_drains_loses_no_reports -- --exact
cargo test --release -p sc-visualizer \
  recording_sink::tests::background_sink_preserves_an_nominal_second_of_three_event_reports -- --exact
```

The runtime latency p95 must remain below its 10 ms tick. The visualizer gates
require no nominal-rate loss and a readable flush of every accepted recording
event. Menu child-host tests repeatedly spawn, kill, reap, and join process and
pipe resources.

## Manual release acceptance

Run these on the exact packaged candidate; automated results are not substitutes.

- Verify clean-state Input Monitoring and Accessibility prompts, grant
  detection, Stop/Quit cleanup, and log/diagnostic redaction.
- Corrupt `bindings.json` by hand and confirm both the menu app and `Edit
  Profiles…` show the recovery alert rather than failing to appear. Check it from
  a `cargo run` as well as the packaged app: an unbundled process is a
  background-only application until a policy is set, and such a process cannot
  put a window on screen at all. On a multi-display desk, confirm the alert
  appears on the display holding the cursor and above other windows. Confirm Quit
  leaves the file untouched, that Escape picks Quit, and that Reset Profiles keeps
  the original as `bindings-invalid.json` and starts with one empty `Default`.
  Confirm the app leaves no Dock icon behind afterwards. The alert is presented by
  AppKit and is not covered by automation.
- Test Puck and direct Bluetooth discovery, every control and axis direction,
  battery/charge reports, reconnect, lizard restoration, and ownership
  contention. Direct USB-C input remains unsupported.
- Verify left/right actuator orientation, unequal magnitudes, continuous and
  rapid effects, and zero after effect stop, process exit, or either disconnect.
- Verify pad pointer/scroll behavior on either pad, click edges, feedback
  strength and side, stationary noise, profile changes, and permission
  revocation.
- Verify pad regions: clicks landing in the intended region near seams and near
  the rim, a finger resting on a seam producing no chatter, a slide during a held
  click keeping the pressed region's action, touch actions handing over between
  regions and releasing on lift, and every held action released on profile
  switch, Stop, and disconnect.
- Exercise idle and fresh-Puck-dock power-off, charging behavior, wake, one-shot
  latching, and injected failure recovery.
- Install revision 3 once through manual reset-button recovery. Save its
  displayed timestamp and installation ID, then reinstall revision 3 without
  pressing the reset button. Confirm automatic UF2 entry, the same revision,
  and a changed timestamp and installation ID. Power-cycle without flashing
  and confirm the receipt does not change. Also verify wrong/multiple-board
  refusal, close/Quit interlocks, and interruption before writing, during UF2
  writing, and before receipt commit.
- On Linux, repeat the automatic App Center update while the official bridge is
  using direct raw USB. Confirm `xpad` retains the Xbox interface, the UF2
  transition succeeds, the application reconnects under the same stable serial
  even when its bus address changes, and the receipt is committed and re-read.
- For an unreleased candidate, run `tools/prepare-local-update.py`, launch the
  local-source build with the printed command, and confirm the Updates page
  names the local development source. Restore Seeed Blink before the first
  installation so factory VID/PID detection, the manual recovery window, the
  revision-3 receipt, and the following automatic reinstall are exercised in
  one sequence. Then launch with the same environment but without
  `--features local-update-source` and confirm the app stays on the stable
  GitHub source.
- Flash legacy targetless revision 2 and confirm it bridges normally without an
  automatic update prompt or bootloader command; the explicit XIAO recovery
  action must release serial and require manual UF2 entry.
- Validate automatic discovery with an implementation using the exact
  `Steam Controller Bridge` product marker but different VID, PID, and
  manufacturer metadata. Validate `--port` with a Hello-compatible
  implementation that does not publish the marker.
- Sleep and wake with the XIAO attached; confirm hardware closes before sleep,
  CDC re-enumerates, a previously running bridge resumes, and a user-stopped
  bridge stays stopped.
- Check the App Center and visualizer at minimum and normal sizes, the profile
  overlay above native fullscreen and borderless games, and menu status icons.
- Verify Safari/Chrome standard mapping plus the supported streaming services
  on the release hardware identity.

Hardware command sequences and cautions are maintained in
[USER_GUIDE.md](USER_GUIDE.md), [BRIDGE.md](BRIDGE.md), the
[firmware README](../firmware/xiao-nrf52840/README.md), and
[LIZARD_MOUSE_LAB.md](LIZARD_MOUSE_LAB.md).
