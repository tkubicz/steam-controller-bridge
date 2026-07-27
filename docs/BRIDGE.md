# Integrated Host Bridge

This bridge targets Steam Controller 2 (2026) only. See the
[end-user guide](USER_GUIDE.md) for hardware setup, pairing, permissions,
firmware flashing, source collection selection, and troubleshooting.

`sc-bridge` combines HID lifecycle events, Steam Controller 2 decoding,
normalization and filtering, changed-state output, optional full-pipeline
recording, replay, and periodic metrics.

## Live mode

Choose an enumerated collection explicitly:

```bash
cargo run -p sc-probe -- list
cargo run -p sc-bridge -- --index 0 --output dump
```

Or let the bridge select the first supported official Puck controller slot
(`28de:1304`, usage `ff00:0001`, interface 2–5):

```bash
cargo run -p sc-bridge -- --controller auto --output serial \
  --port /dev/cu.usbmodemXXXX --record session.jsonl
```

The HID worker stores controller input in a single replaceable latest-report
slot. If output processing stalls, new snapshots overwrite stale motion instead
of accumulating behind it. A notification channel wakes the main loop, while
connection lifecycle events retry until delivered or shutdown begins. The
worker polls with a short timeout and is always joined, including error paths.

Live mode defaults to `--lizard-mode suppress`. Before publishing `Connected`
or accepting a controller state, the HID worker sends the fixed 64-byte
SDL-compatible feature report `01 87 03 09 00 00 00…` and then refreshes it
every three seconds. The write is allowed only on an official Proteus Puck
controller-slot collection. `--lizard-mode leave` disables this behavior for
diagnostics and is not a supported gameplay configuration.

The selected slot is protected by a nonblocking per-slot ownership lock shared
by all project tools. A second `sc-bridge`, `sc-probe`, or `sc-visualizer`
process fails with an actionable ownership error.

Native macOS HID access is intentionally shared. On the tested Puck composite,
HIDAPI's exclusive `kIOHIDOptionsTypeSeizeDevice` open fails with
`0xE00002E2 not permitted` even after the visible Steam application exits.
Shared access is therefore required for a working bridge. The project lock
cannot exclude Steam or another non-project consumer, so supported use requires
fully quitting Steam and booting out its persistent IPC LaunchAgent:

```bash
launchctl bootout user/$(id -u)/com.valvesoftware.steam.ipctool
```

Launching Steam again normally restores the service. Do not run Steam or other
third-party controller tools while the bridge owns the Puck.

The default safety policy sends neutral after 200 ms without a valid controller
state or after three consecutive decode failures. Disconnect, explicit reset,
and orderly shutdown also clear mapping history and send neutral. Consecutive
unchanged states are not forwarded.

An initial or periodic lizard-off write failure is fail-closed: input forwarding
stops, the XIAO is neutralized, and the bridge exits. On orderly shutdown the
XIAO is neutralized before the HID worker stops. No lizard-on report is sent;
the controller watchdog restores its desktop keyboard/mouse mode after the
three-second heartbeat ceases.

All other controller feature initialization remains disabled. In particular,
the bridge cannot send digital-mapping clears, arbitrary settings, haptics, or
actuator commands.

## Replay mode

```bash
cargo run -p sc-bridge -- --input replay --file session.jsonl \
  --deterministic --output file --output-file replay.frames
```

Replay accepts the same dump, file, mock, and serial outputs as live mode and
sends neutral when it finishes.

## Metrics and logging

Diagnostics are emitted as stable `key=value` records. Metrics include input
and dropped reports, report rate, decode failures, state changes, sent and
skipped outputs, HID reconnects, and average decode, mapping, and total host
processing time. Lizard diagnostics include active state, successful refreshes,
failures, and the age of the last successful refresh. Serial-specific reconnect,
framing, and checksum counters are owned by the serial output session.

Expected lifecycle logs are:

- `bridge_started ... lizard_mode=Suppress`, followed by
  `hid_connected ... lizard_suppressed=true` only after the initial write;
- periodic `metrics` with increasing `lizard_refreshes`, zero
  `lizard_failures`, and `lizard_refresh_age_ms` below 3000;
- `hid_disconnected action=neutral`, then another suppressed `hid_connected`
  after the same slot reopens;
- a lizard-write error followed by output neutralization and process exit,
  rather than continued controller-state forwarding;
- `bridge_stopped ... action=neutral` on Ctrl-C, duration expiry, or normal
  completion.
