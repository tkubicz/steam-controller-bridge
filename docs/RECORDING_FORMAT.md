# Recording Format v1

Recordings use UTF-8 newline-delimited JSON. Each non-empty line is one complete event and can be parsed independently.

```json
{"version":1,"timestamp_us":123456,"kind":"raw_hid","payload":{"report_id":66,"bytes":"AAH+/w=="}}
```

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
- `mapped_gamepad_state`
- `warning`
- `error`
- `marker`

Unknown kinds are valid v1 events. Readers preserve their JSON payloads and replay ignores them. This permits additive event kinds without increasing the envelope version.

`raw_hid` contains `report_id` as an unsigned byte and `bytes` as standard padded base64. Device identity and transport metadata belong in the corresponding `device_connected` event rather than being repeated for every report.

`decoded_steam_state` is reserved for the typed Steam Controller state introduced with the HID decoder phase. Its payload remains opaque JSON to the core recording reader until that model exists.

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

