# Testing

The workspace has no hardware-dependent tests. Run the same gates as CI:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-targets
```

The protocol tests cover every message type, fixed axis endpoints, the static CRC vector, partial and combined reads, garbage, truncation, invalid versions, oversized lengths, corruption, and recovery. A deterministic pseudo-random byte-stream test checks that framing never panics.

Recording tests cover typed raw and gamepad round trips, timestamp ordering, unknown events, seeking, deterministic replay, malformed/truncated input, version rejection, and identical simulator-state replay.

Steam Controller protocol tests cover all 30 OpenPuck button bits, every trigger/stick/pad/pressure/motion field, both `0x45` and extended `0x42`, connection/battery/signal reports, typed recording round trips, incorrect sizes, mismatched IDs, unknown IDs, and arbitrary truncated lengths.

The HID device crate unit-tests platform-neutral collection grouping. On macOS, a read-only live diagnostic can verify the native backend without controller-specific assumptions:

```bash
cargo run -p sc-probe -- list
cargo run -p sc-probe -- inspect --index 0
```

Linux CI compiles the hardware-independent API and explicit unsupported-platform implementation; it does not require `hidapi` system libraries or physical hardware.

An end-to-end pre-hardware smoke test is:

```bash
cargo run -p gamepad-simulator -- automated --interval-ms 0 \
  --output recording --file /tmp/sc-session.jsonl
cargo run -p sc-replay -- /tmp/sc-session.jsonl --deterministic \
  --output file --output-file /tmp/sc-session.frames
```

The resulting `.frames` file contains fixed-size, CRC-protected protocol frames suitable for inspection or future firmware parser tests.
