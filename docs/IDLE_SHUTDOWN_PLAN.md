# Steam Controller 2 Configurable Automatic Shutdown

## Summary

Add host-controlled automatic shutdown policies for Steam Controller 2 sessions
owned by the bridge. The controller currently powers itself off after roughly
15 minutes when operating from its battery, but remains awake indefinitely while
physically docked on the official Puck and charging. Support two independent
policies:

- a configurable neutral-idle timeout; and
- an optional immediate power-off when the controller is placed on the official
  Puck.

The dock policy is edge-triggered and fires once per placement episode. It is not
implemented as a zero-minute idle timeout: placing the controller on the Puck is
an explicit shutdown signal even if a control is still held or rumble is active.
The bridge must not treat continuous `0x42`/`0x45` report delivery, IMU noise,
lizard suppression, XIAO refreshes, or rumble traffic as user activity.

The preferred shutdown mechanism is the controller's confirmed feature command:

```text
report ID 01, command 9F, length 04, payload 6F 66 66 21 ("off!")
```

Encode it as one zero-padded 64-byte feature report:

```text
01 9F 04 6F 66 66 21 00 ... 00
```

OpenPuck documents this command from a real Puck capture and uses a short burst
because the final Puck-to-controller RF relay is not acknowledged. It also notes
that the controller can emit reports briefly after accepting shutdown. Use
OpenPuck only as a protocol and lifecycle reference; do not copy its AGPL-3.0
implementation. [OpenPuck shutdown command](https://github.com/safijari/openpuck/blob/f0f8c63df16ca5f5fcc94faac47f3bbfd821ff6a/OpenPuck/puck_hid.cpp#L316-L328),
[OpenPuck shutdown burst](https://github.com/safijari/openpuck/blob/f0f8c63df16ca5f5fcc94faac47f3bbfd821ff6a/OpenPuck/haptics.cpp#L52-L71).

Default to 15 minutes to match observed undocked behavior. Support `Never`, 5,
10, 15, and 30 minutes in the menu application, plus an arbitrary whole-minute
CLI override. Default the immediate Puck-dock action to `Leave On` so upgrading
does not introduce a surprising shutdown; users can enable `Power Off` from the
menu or CLI. The idle policy applies to either supported transport while the
live bridge is running. The immediate dock policy applies only to a Puck source
with a fresh `Charging` or `Charged` battery report, because a Bluetooth
controller reporting external power cannot distinguish an official Puck from a
USB charging cable. Replay and non-live diagnostic output remain unaffected.

No XIAO firmware or CDC protocol change is required.

## Prerequisite Hardware Gate

Before enabling automatic shutdown, implement the encoder, narrow device API,
and a manual probe command, then verify the behavior on real hardware.

- Add `sc-probe power-off --index N`. It must validate the exact supported
  controller collection, print an explicit warning that it will power the
  controller off, send the allowlisted command, and report each attempted
  write. Invoking the command is the user's confirmation; do not add an
  interactive prompt that would make scripting or CI parsing unreliable.
- Test the official `28de:1304`, `ff00:0001`, interfaces 2–5 Puck collection
  while the controller is both undocked and physically docked/charging.
- Confirm that a successful shutdown leaves the controller off, charging
  continues, and it does not immediately reconnect merely because it remains
  on the Puck.
- Capture the fresh `0x43` charge-state sequence when placing the active
  controller on the Puck and when removing it. Confirm which observed values
  correspond to `Charging`, `Charged`, and `Discharging`; do not infer placement
  from battery percentage changes.
- Confirm that pressing Steam wakes the controller and produces fresh state
  reports that normal discovery can select.
- Verify the command independently on the supported direct Bluetooth
  `28de:1303`, `ff00:0001`, interface `-1` collection. Do not enable automatic
  Bluetooth shutdown until that transport passes.
- Measure the post-command state-report tail. Use the result to validate the
  planned 2.5-second same-source discovery cooldown.
- Determine resend policy per transport. Start with three nonblocking writes
  approximately one controller poll apart for the Puck, matching OpenPuck's
  no-ACK rationale. A direct Bluetooth disconnect after the first successful
  write must not turn later unavailable-device writes into a false failure.

If power-off while charging causes an immediate hardware wake/reconnect, stop
after this gate and report the hardware limitation rather than adding a loop
that repeatedly shuts the controller down.

## Implementation Changes

### Protocol encoder and restricted device access

- Add `power_off_feature_report()` to `steam-controller-protocol`. It returns
  exactly 64 bytes with `01 9F 04 6F 66 66 21` followed by 57 zero bytes.
- Add a complete golden-vector test, including report ID, command, declared
  length, ASCII payload, and zero padding.
- Add `HidSession::power_off()` in `steam-controller-device`. It may call only
  the fixed encoder; do not expose arbitrary feature-report writes.
- Use the same exact Puck/Bluetooth allowlist as lizard suppression and rumble.
  Reject incorrect VID/PID, transport, usage, interface, and non-macOS targets.
- Give power-off its own `DeviceError` target/backend context so failures are
  distinguishable from lizard and rumble writes.
- Perform all writes in the existing HID worker. The runtime supervisor and
  menu event loop must never issue HID writes directly.

### Portable meaningful-activity tracker

- Add an allocation-free `idle_shutdown` module under `bridge-runtime` with a
  deterministic `IdleActivityTracker` driven by explicit elapsed time.
- Feed it each successfully decoded `ProcessOutcome::State { source, mapped,
  .. }` before discarding the raw and mapped states in `process_report`.
- Define the controller as active whenever any of these conditions is true:
  - any physical button is held;
  - either mapped stick is outside the mapper's dead zone;
  - either mapped trigger is outside its rest dead zone;
  - either trackpad is touched or pressed;
  - either grip touch sensor is active.
- A transition from active to neutral starts a fresh idle interval. Remaining
  active continuously prevents shutdown. Any new meaningful activity resets
  the interval.
- Ignore sequence numbers, raw report differences, report arrival, connection
  and battery reports, IMU timestamps, gyro/acceleration, sub-dead-zone analog
  jitter, lizard refreshes, XIAO state-refresh messages, and all rumble traffic.
- Use the already mapped analog state for stick/trigger dead zones and raw
  decoded touch/button fields for controls not represented by the Xbox output.
  Do not add a second incompatible set of calibration constants.
- Reset the timer on controller selection, reconnect, runtime Start, a timeout
  setting change, and a transition into a known charging state. Pause/clear it
  whenever the runtime is not `Running` or no fresh controller source is
  active.
- Do not reconstruct activity from `BridgeMetrics::output_packets`; that metric
  is intentionally coalesced and omits physical controls that are not mapped to
  the XIAO gamepad.

### Immediate Puck-dock detection

- Add `PuckDockAction::{LeaveOn, PowerOff}`. Its default is `LeaveOn`, independent
  of the idle timeout.
- Recognize a dock event only when all of these are true:
  - the selected source is `ControllerTransport::Puck`;
  - the report is a fresh, valid `0x43` battery report from the active source;
  - its typed charge state is `Charging` or `Charged`; and
  - that stable controller identity has not already handled the current dock
    episode.
- Treat the first fresh charging/charged report after selection as a dock event,
  so enabling the bridge while the controller is already awake on the Puck still
  powers it off. Actual shutdown latency is therefore bounded by battery-report
  delivery plus the short power-off burst, not by the idle timer.
- Never infer docking from Puck enumeration, connection state, percentage
  increase, elapsed time, or the mere presence of a charging-capable Puck. Those
  signals do not prove that the controller is physically docked.
- Keep a supervisor-level handled latch keyed by stable controller identity.
  Preserve it across the intentional worker release, cooldown, and a wake while
  the controller remains on the Puck. This makes the policy fire once per dock
  episode instead of immediately powering the controller off again whenever the
  user deliberately wakes it while it is still docked.
- Clear the handled latch only after a fresh `Discharging` report, replacement
  by another stable controller identity, disabling/re-enabling the dock policy,
  or a new application/runtime lifetime. If the controller is removed while
  powered off, the next awake off-Puck `Discharging` report clears the latch.
- A dock event takes priority over the idle tracker and does not require neutral
  controls. It still uses the same neutral -> rumble zero -> lizard cancellation
  -> power-off -> release safety sequence.
- `Unknown` charge states and Bluetooth charging reports update diagnostics but
  never trigger the Puck-dock policy.

### Runtime scheduling and shutdown lifecycle

- Add `RuntimeConfig::idle_shutdown_timeout: Option<Duration>`, where `None`
  means `Never`; default to `Some(Duration::from_secs(15 * 60))`.
- Add `RuntimeConfig::puck_dock_action: PuckDockAction`, defaulting to
  `PuckDockAction::LeaveOn`.
- Service the tracker from the existing cooperative runtime loop. It must not
  add a polling thread or shorten the current HID wait interval.
- At an idle deadline, proceed only if the selected controller is still connected,
  the latest meaningful state is neutral, and the runtime still owns a ready
  XIAO. Recheck immediately before the first power-off write so activity racing
  the deadline cancels shutdown.
- At a validated dock event, do not wait for neutral inactivity. Recheck source
  identity, Puck transport, fresh charge state, and the handled latch immediately
  before shutdown; then neutralize any currently held XIAO controls as the first
  safety step.
- Preserve this safety order:
  1. send or queue XIAO neutral;
  2. send SC2 rumble zero and cancel its lease;
  3. cancel future lizard-mode refreshes without sending lizard-on;
  4. stage the transport-specific SC2 power-off write/burst;
  5. stop accepting controller reports and release the HID worker;
  6. return to discovery after installing a same-source cooldown.
- Do not block the worker with sleeps between burst writes. Represent the burst
  as scheduled work serviced by the existing loop.
- Add a 2.5-second cooldown keyed by stable controller identity. Discovery may
  enumerate the collection during this period but must not select state reports
  from it. This prevents the controller's shutdown tail from looking like a
  reconnect.
- After the cooldown, ordinary active-source discovery resumes. A later Steam
  button wake is treated as a fresh connection: reset the idle timer, require a
  fresh state, suppress lizard mode, and require a fresh rumble lease before
  forwarding input.
- An automatic-shutdown write failure is nonfatal to gameplay. Immediately
  resume lizard suppression if the source is still connected, mark automatic
  shutdown `Degraded`, and restore input safely. An idle-triggered attempt may
  retry no more than once every 30 seconds while the controller remains neutral.
  A dock-triggered attempt may retry while a fresh charging/charged state remains,
  but user activity after the failed attempt marks that dock episode handled and
  cancels retries so the bridge cannot surprise a user who chooses to keep using
  the controller on the Puck.
- Stop/quit keeps its existing semantics: neutralize and restore desktop lizard
  behavior, but do not automatically power the controller off. Idle shutdown is
  a separate user-configured policy, not a side effect of stopping the bridge.
- macOS system-sleep-triggered power-off is related but remains a separate
  follow-up. This milestone is based only on controller inactivity while the
  bridge is running.

### Charge state, status, and diagnostics

- Preserve `BatteryStatus::charge_state` in the runtime rather than discarding
  it after decoding `0x43`. Add a typed display classification for observed
  states: `Discharging`, `Charging`, `Charged`, and `Unknown(u8)`.
- Charge state resets the idle timer on a transition into charging. It is also
  the sole trigger for the opt-in immediate Puck-dock policy, but it does not
  gate whether the configured idle timeout applies.
- Add `AutomaticShutdownPhase::{Disabled, Monitoring, PoweringOff, Sleeping,
  Degraded}`, `ShutdownTrigger::{IdleTimeout, PuckDock}`, and an
  `AutomaticShutdownStatus` in `BridgeStatus` containing:
  - configured timeout;
  - configured Puck-dock action and whether the current episode is handled;
  - current neutral-idle age;
  - phase;
  - current/past trigger;
  - successful shutdown count;
  - failure count;
  - last successful shutdown age;
  - retry age/deadline when degraded.
- Keep status publication on the existing 250 ms cadence. Activity tracking is
  hot-path state, but menu/log snapshot construction must not return to the
  per-report path.
- Log only transitions and outcomes: timer armed/reset, dock detected, dock
  episode handled/cleared, deadline reached, neutral and rumble-zero completion,
  each power-off attempt, success, failure, cooldown, and post-wake reconnect.
  Do not log every idle tick, battery report, or controller report.
- Include timeout, Puck-dock action/latch, idle age, phase, trigger, charge state,
  counters, and the last actionable failure in rotated logs and
  `Copy Diagnostics`.

### CLI and menu-bar configuration

- Add `--idle-shutdown <never|MINUTES>` to live `sc-bridge`; whole minutes must
  be positive and bounded to a documented safe maximum. Default is `15`.
  Reject it for replay modes where no live SC2 is owned.
- Add `--puck-dock-action <leave|power-off>` to live `sc-bridge`; default is
  `leave`. Reject it for replay modes.
- Add `BridgeHandle::set_idle_shutdown_timeout(Option<Duration>)`. The command
  is idempotent, updates the running tracker without restarting HID/serial, and
  starts a fresh idle interval rather than immediately applying accumulated
  idle time under a newly shorter setting.
- Add an `Idle Shutdown` submenu to the macOS menu with checked choices `Never`,
  `5 minutes`, `10 minutes`, `15 minutes`, and `30 minutes`.
- Add a separate checked `Turn Off When Placed on Puck` item. Changing it must
  call an idempotent live runtime command without restarting HID or serial.
  Enabling it while a fresh charging/charged Puck source is already active must
  treat the next fresh battery report as a dock event.
- Persist both menu choices in a small versioned settings file under
  `~/Library/Application Support/Steam Controller Bridge/`, written atomically.
  Missing or invalid settings fall back to 15 minutes and produce one warning,
  not a fatal startup error.
- Show compact status such as `Auto shutdown: Idle 6:42 / 10:00`, `Auto
  shutdown: On Puck`, `Controller: Sleeping`, or `Auto shutdown: Degraded`.
  Keep full error details available through the existing copy-error/diagnostics
  actions rather than expanding the menu width.

### Documentation

- Update the README, user guide, bridge architecture, testing guide, and menu
  application instructions.
- Explain that SC2's observed autonomous timeout is approximately 15 minutes on
  battery, while charging on the official Puck can keep it awake.
- Explain the default, menu/CLI configuration, meaningful-activity definition,
  immediate Puck-dock option, one-shot dock semantics, how to wake the
  controller, and why continuous reports do not reset the timer.
- Document that `Stop Bridge` deliberately restores lizard mode instead of
  powering off the controller.
- Document the manual probe command and its state-changing nature.
- Record the immutable OpenPuck protocol references and the AGPL boundary.

## Public Interfaces and Compatibility

- `steam_controller_protocol::power_off_feature_report() -> [u8; 64]`
- `HidSession::power_off() -> Result<(), DeviceError>`
- `RuntimeConfig::idle_shutdown_timeout: Option<Duration>`; default 15 minutes
- `PuckDockAction::{LeaveOn, PowerOff}`
- `RuntimeConfig::puck_dock_action: PuckDockAction`; default `LeaveOn`
- `BridgeHandle::set_idle_shutdown_timeout(Option<Duration>)`
- `BridgeHandle::set_puck_dock_action(PuckDockAction)`
- `AutomaticShutdownPhase`, `ShutdownTrigger`, and `AutomaticShutdownStatus`
- `BridgeStatus::automatic_shutdown: AutomaticShutdownStatus`
- `BridgeStatus::battery_charge_state: Option<ControllerChargeState>`
- New CLI option: `--idle-shutdown <never|MINUTES>`
- New CLI option: `--puck-dock-action <leave|power-off>`
- New probe command: `sc-probe power-off --index N`

The CDC protocol, XIAO firmware, Xbox-compatible USB personality, input mapping,
recording format, controller selection flags, and lizard/rumble public behavior
remain compatible.

## Test Plan

### Native and unit tests

- Verify the complete 64-byte shutdown vector, length, ASCII payload, and zero
  padding.
- Verify power-off rejects every unsupported VID/PID, transport, usage,
  interface, and platform combination.
- Test meaningful activity for every button class, both sticks, both triggers,
  both pads, grips, held controls, return to neutral, dead-zone jitter, sequence
  changes, IMU noise, battery reports, lizard reports, and rumble commands.
- Test exact deadline behavior with a fake clock for `Never`, 5, 10, 15, 30,
  and an arbitrary valid CLI duration.
- Test that report delivery at hundreds of hertz does not reset idle time and
  does not increase status/log publication frequency.
- Test timer reset on selection, reconnect, Start, charging transition, activity,
  and runtime configuration changes.
- Test immediate Puck-dock detection on the first fresh `Charging` and `Charged`
  report, including a controller already docked when the bridge starts.
- Test that `Discharging`, `Unknown`, stale battery data, percentage changes,
  Puck enumeration alone, and Bluetooth charging never trigger immediate Puck
  shutdown.
- Test the one-shot dock latch across intentional release, cooldown, and a user
  wake while still docked; verify a later fresh `Discharging` state re-arms it.
- Test enabling the policy while already docked, disabling/re-enabling it, source
  identity replacement, application restart, dock-triggered failure/retry, and
  user activity canceling a failed dock retry.
- Test neutral -> rumble zero -> lizard cancellation -> power-off -> release
  ordering for both idle expiry and immediate dock placement. Confirm a dock
  trigger neutralizes held controls instead of waiting for them to be released.
- Test activity racing the deadline, output-neutral failure, power-off failure,
  bounded retry, lizard suppression resumption, Stop/Quit during a burst, source
  loss during a burst, and XIAO loss before expiry.
- Test the 2.5-second identity cooldown against trailing state reports and
  ensure a real post-cooldown wake is selectable.
- Test CLI parsing and replay rejection, menu checkmarks, persisted-setting
  fallback for both policies, live setting updates, compact labels, diagnostics,
  and clean Quit.
- Run formatting, strict Clippy, all workspace tests/builds, firmware native
  tests as a regression gate, and macOS `.app` packaging.

### Hardware acceptance

- Run `sc-probe power-off` on an undocked Puck-connected controller and verify
  clean shutdown and wake.
- Repeat while physically docked and charging. Confirm the controller stops
  producing input, remains off for at least 30 minutes, and continues charging.
- Enable `Turn Off When Placed on Puck`, begin on battery, then physically place
  the active controller on the Puck. Verify neutralization and shutdown occur on
  the first fresh charging/charged report without waiting for the idle timeout.
- Wake the controller while leaving it on the Puck and confirm the one-shot dock
  latch prevents an immediate second shutdown. Remove it, obtain a fresh
  discharging report, place it back, and confirm exactly one new shutdown.
- Repeat placement while a control is held and while rumble is active; require
  XIAO neutral and rumble zero before power-off.
- Verify the measured shutdown tail cannot cause automatic rediscovery.
- Repeat the diagnostic over direct Bluetooth before enabling that transport's
  automatic policy.
- Configure 5 minutes, leave the controller neutral on the charging Puck, and
  verify XIAO neutralization followed by controller shutdown within a bounded
  tolerance of the deadline.
- Exercise every meaningful activity shortly before expiry and confirm the full
  timeout restarts. Verify stick/IMU noise alone never postpones shutdown.
- Hold each stick/trigger/button beyond the timeout and confirm the controller
  is not shut down while a control remains active.
- Confirm rumble stops before shutdown and cannot keep the timer alive.
- Wake with Steam and verify discovery, lizard suppression, input, battery,
  rumble, and menu status recover without restarting the bridge.
- Verify `Never`, menu setting persistence across app restart, Stop/Quit
  behavior, independent dock-action persistence, Puck unplug/reconnect, XIAO
  loss, and repeated sleep/wake cycles.
- Complete a one-hour charging soak with the 10-minute setting and require one
  shutdown, no automatic reconnect loop, no stuck XIAO input, and no lizard-mode
  leakage.

## Assumptions and Non-Goals

- The official Puck charging state intentionally or effectively suppresses the
  controller's autonomous battery timeout.
- One SC2 remains the supported topology; shutdown is targeted to the selected
  source only.
- Default idle shutdown is 15 minutes and applies on battery and while charging.
- Immediate Puck-dock shutdown is an independent opt-in and defaults to
  `LeaveOn`.
- The immediate dock policy is supported only for a Puck input source with a
  fresh charging/charged report; Bluetooth external power is not treated as
  proof of Puck placement.
- The hardware gate must pass before automatic shutdown is enabled for a
  transport.
- Arbitrary feature reports, user-authored command payloads, charging control,
  battery charge limits, direct USB-C controller input, multiple-controller
  policies, and macOS system-sleep power handling are excluded.
- No firmware rebuild or XIAO flash is required.
