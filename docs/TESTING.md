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
python3 tools/check-changelog.py --self-test
python3 tools/check-changelog.py
python3 tools/build-macos-app.py --self-test
python3 tools/prepare-local-update.py --self-test
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-targets
make -C firmware/xiao-nrf52840 test
```

On macOS, also build and verify the packaged application:

```bash
./tools/build-macos-app.py
codesign --verify --deep --strict "dist/Steam Controller Bridge.app"
```

CI additionally builds the pinned Arduino firmware artifacts. `cargo deny
check` covers advisories, licenses, bans, and sources for the shipped macOS
dependency graph.

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
  Profiles…` show the recovery alert rather than failing to appear. Confirm Quit
  leaves the file untouched, that Escape picks Quit, and that Reset Profiles
  keeps the original as `bindings-invalid.json` and starts with one empty
  `Default`. The alert is presented by AppKit and is not covered by automation.
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
- For an unreleased candidate, run `tools/prepare-local-update.py`, launch the
  debug menu app with the printed environment, and confirm the Updates page
  names the local development source. Restore Seeed Blink before the first
  installation so factory VID/PID detection, the manual recovery window, the
  revision-3 receipt, and the following automatic reinstall are exercised in
  one sequence. Confirm a non-debug build ignores the local-source variable.
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
