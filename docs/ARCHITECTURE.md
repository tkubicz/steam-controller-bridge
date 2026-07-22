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
   mock / dump / file
```

## Crates

- `gamepad-state` owns platform-neutral state, stable button indices, validation, and sanitization.
- `bridge-protocol` owns integer wire representation, messages, framing, CRC, and stream recovery. It never copies Rust object memory into packets.
- `bridge-output` owns the `GamepadOutput` boundary and hardware-independent backends. `ChangedOnly` is an optional policy wrapper; file output preserves every state by default.
- `gamepad-simulator` owns deterministic and keyboard-driven sources. It depends on output interfaces but not protocol internals.

All crates forbid unsafe code and have no third-party dependencies. The GUI, HID, recording, mapping, and serial layers remain separate future components.

## Lifecycle and safety

Simulator modes send a neutral state before normal exit. Outputs reject non-finite and out-of-range values instead of silently emitting invalid packets. A future integrated bridge must additionally send neutral on input timeout, controller disconnect, repeated decode failure, profile invalidation, and output reconnect.

The future live pipeline will use bounded channels with latest-state semantics for input state while preserving lifecycle events. No concurrency runtime is introduced before a live transport requires it.

