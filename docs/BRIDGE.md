# Integrated Host Bridge

`sc-bridge` combines HID lifecycle events, Steam Controller 2 decoding,
normalization and filtering, changed-state output, optional full-pipeline
recording, replay, and periodic metrics.

## Live mode

Choose an enumerated collection explicitly:

```bash
cargo run -p sc-probe -- list
cargo run -p sc-bridge -- --index 0 --output dump
```

Or let the bridge select the first enumerated collection whose metadata names
Valve or Steam:

```bash
cargo run -p sc-bridge -- --controller auto --output serial \
  --port /dev/cu.usbmodemXXXX --record session.jsonl
```

The HID worker uses a bounded 64-event channel. Controller reports use
latest-state behavior when that channel is full; lifecycle events retry until
delivered or shutdown begins. The worker polls with a short timeout and is
always joined, including error paths.

The default safety policy sends neutral after 200 ms without a valid controller
state or after three consecutive decode failures. Disconnect, explicit reset,
and orderly shutdown also clear mapping history and send neutral. Consecutive
unchanged states are not forwarded.

Controller feature initialization remains intentionally disabled. OpenPuck's
documented host configuration channel contains hardware/firmware-dependent
settings, including commands that can produce a repeating actuator buzz when
misapplied. The bridge logs its profile and protocol but does not claim that an
unverified feature write succeeded.

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
processing time. Serial-specific reconnect, framing, and checksum counters are
owned by the serial output session.
