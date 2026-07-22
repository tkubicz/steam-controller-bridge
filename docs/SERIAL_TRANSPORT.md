# Serial Transport

The host sends the existing versioned bridge frames over a byte-stream serial
port. No serial-specific framing is added. The default native settings are
115200 baud and a 10 ms read timeout; callers can select another baud rate.

## Session lifecycle

1. The host opens the configured port and sends `Hello { min: 1, max: 1 }`.
2. Firmware must answer `HelloResponse { selected: 1 }` within one second.
3. Only after negotiation does the host flush queued gamepad states.
4. The host sends a `Ping` after one second of health-check time and expects the
   matching `Pong` within two seconds. Firmware-originated pings receive an
   immediate pong.
5. I/O, handshake, or health failure marks the connection unavailable. Native
   output retries by reopening the configured port and repeating negotiation.
6. Normal shutdown sends the dedicated `Neutral` frame when the connection is
   still writable.
7. While a non-neutral state remains current, the serial backend retransmits the
   complete state every 50 ms. All application wait loops service output at
   least every 25 ms. `Neutral` clears the cached active state immediately.

Sequence numbers cover every host-originated frame, including hello, state,
neutral, ping, and pong, and wrap as `u16`. Incoming bytes use the shared
recovering `StreamDecoder`; checksum and framing errors are counted without
creating a second parser.

The refresh interval is independent of the one-second Ping health check. Pings
prove the bidirectional session is responsive but deliberately do not keep the
firmware's 100 ms controller-data watchdog alive.

## Queue behavior

The transport-independent session has a bounded state queue with capacity 8 by
default. When full, it drops the oldest state and retains the newest input. This
prevents stale controller motion from accumulating while a handshake is in
progress. Lifecycle and protocol-control messages are not stored in that queue.

## Commands

```bash
cargo run -p gamepad-simulator -- automated --output serial \
  --port /dev/cu.usbmodemXXXX --baud 115200 --serial-log

cargo run -p sc-replay -- session.jsonl --output serial \
  --port /dev/cu.usbmodemXXXX --baud 115200
```

The visualizer exposes the same path and baud controls when `Serial` is selected
as its output backend. Raw transmit/receive frame logging is opt-in. Actual firmware interoperability remains hardware-bound;
the host state machine is covered with an in-memory `ByteTransport`.
