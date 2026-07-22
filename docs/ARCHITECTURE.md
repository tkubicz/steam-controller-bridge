# Architecture

The implemented foundation follows a one-way pipeline:

```text
keyboard/automated simulator
          |
          v
    GamepadState
          |
          v
 explicit wire conversion and framing
          |
          v
 mock / dump / file / JSONL recording
                         |
                         v
                 deterministic/timed replay
```

## Crates

- `gamepad-state` owns platform-neutral state, stable button indices, validation, and sanitization.
- `bridge-protocol` owns integer wire representation, messages, framing, CRC, and stream recovery. It never copies Rust object memory into packets.
- `bridge-output` owns the `GamepadOutput` boundary and hardware-independent backends. `ChangedOnly` is an optional policy wrapper; file output preserves every state by default.
- `recording` owns the versioned JSONL envelope, ordered writer, typed raw/final-state payloads, unknown-event preservation, and deterministic or real-time replay.
- `steam-controller-device` owns HID collection metadata, raw reports, lifecycle events, stable enumeration ordering, and reconnecting sessions. Only its private `platform` module depends on `hidapi`; non-macOS builds expose an explicit unsupported-platform stub.
- `gamepad-simulator` owns deterministic and keyboard-driven sources. It depends on output interfaces but not protocol internals.
- `sc-replay` is a thin CLI over `recording` and the existing output backends. It contains no format parser of its own.
- `sc-probe` lists and inspects all HID collections, monitors one explicitly selected collection, and records raw lifecycle/report events. It does not contain Steam-specific identifiers or feature-report bytes.

All crates forbid unsafe code. Only the recording layer uses third-party serialization and base64 libraries. The GUI, HID, mapping, and serial layers remain separate future components.

## Lifecycle and safety

Simulator modes send a neutral state before normal exit. Outputs reject non-finite and out-of-range values instead of silently emitting invalid packets. A future integrated bridge must additionally send neutral on input timeout, controller disconnect, repeated decode failure, profile invalidation, and output reconnect.

The HID session currently polls synchronously with a bounded 1,024-byte report buffer and 100 ms CLI timeout. A read failure immediately emits a lifecycle disconnect and clears the handle; later polls refresh enumeration every 500 ms and match the selected path or physical/collection metadata. The future integrated bridge will place this session behind a bounded channel with latest-state semantics while preserving lifecycle events. No concurrency runtime is introduced before the integrated transport requires it.
