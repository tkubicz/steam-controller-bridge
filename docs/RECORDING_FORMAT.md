# Recording Format v1

Recordings use UTF-8 newline-delimited JSON. Each non-empty line is one complete event and can be parsed independently.

```json
{"version":1,"timestamp_us":123456,"kind":"raw_hid","payload":{"report_id":66,"bytes":"AAH+/w=="}}
```

> Connection metadata retains the unmasked device serial so that recordings stay
> replayable against the collection they came from. On Bluetooth that serial is the
> controller's MAC address; review a recording before sharing it publicly.

## Envelope

| Field | Type | Meaning |
| --- | --- | --- |
| `version` | unsigned integer | Format version; currently exactly 1 |
| `timestamp_us` | unsigned integer | Microseconds from the recording's monotonic start |
| `kind` | string | Event discriminator |
| `payload` | JSON value | Kind-specific data |

Timestamps must be nondecreasing. Equal timestamps are valid. Wall-clock time is deliberately excluded, so clock changes cannot reorder input. Writers flush after each event in the current implementation to favor recoverable diagnostics over throughput.

## Event kinds

Known kinds are:

- `device_connected`
- `device_disconnected`
- `raw_hid`
- `decoded_steam_state`
- `decoded_lizard_mouse`
- `host_pointer`
- `capture_metadata`
- `mapped_gamepad_state`
- `warning`
- `error`
- `marker`

Unknown kinds are valid v1 events. Readers preserve their JSON payloads and replay ignores them. This permits additive event kinds without increasing the envelope version.

`raw_hid` contains `report_id` as an unsigned byte and `bytes` as standard padded base64. Live captures also include `source_device_id`, `transport`, and `dropped_reports`; older or synthetic v1 events may omit these additive fields. Full collection metadata is stored in each corresponding `device_connected` event.

`decoded_steam_state` contains the typed Steam Controller 2 state produced from a validated `0x45` or `0x42` report. It includes buttons, triggers, both sticks, both pads and pressures, IMU timestamp, acceleration, gyro, and the complete original report bytes. The recording library can decode it back into `SteamControllerState` without depending on HID access.

`decoded_lizard_mouse` contains the validated six-byte `0x40` report as typed
button bits, signed X/Y deltas, signed vertical/horizontal wheel deltas, and the
complete original bytes.

`host_pointer` is a passive macOS HID-event-tap observation. It records the
mouse/drag/button/scroll event kind, Core Graphics delta fields, global cursor
location, and point-scroll deltas. It is absent from older recordings and from
captures made without the required Input Monitoring access.

`capture_metadata` records the lab/tool version, OS version and build, selected
path-sorted controller index and collection identity, transport, capture mode,
display geometry/scales, mouse scaling, and optional final validity. A capture
that overflowed its queue, lost the controller, or lost its event tap finishes
with `valid: false` and an `invalid_reason` rather than silently omitting data.

`mapped_gamepad_state` contains:

```json
{
  "buttons": 1,
  "hat": 8,
  "left_x": 0.0,
  "left_y": 0.0,
  "right_x": 0.0,
  "right_y": 0.0,
  "left_trigger": 0.0,
  "right_trigger": 0.0
}
```

Gamepad values must satisfy the same finite-value and range rules as `gamepad-state`. Invalid hats, non-finite numbers, and out-of-range axes are rejected during typed decoding.

## Compatibility and failure behavior

Readers reject unknown envelope versions, decreasing timestamps, malformed JSON, invalid typed payloads, and truncated JSON lines with a structured error. A final valid JSON object is accepted even if the file lacks a trailing newline. This distinguishes harmless interrupted flushing after a complete event from a genuinely truncated object.

Real-time replay uses deltas between recorded timestamps and divides them by the requested positive finite speed. Deterministic replay ignores timing entirely. Seeking selects the first event whose timestamp is greater than or equal to the requested timestamp.
