# Serial Transport

The host sends the existing versioned bridge frames over a byte-stream serial
port. No serial-specific framing is added. The default native settings are
115200 baud and a 1 ms read timeout; callers can select another baud rate.

Zero-configuration live mode enumerates `SerialPortInfo`, keeps only macOS
callout ports with exact XIAO metadata (`Lynxware / Steam Controller Bridge`,
`045e:028e`), and then performs the Hello exchange below before selection. It
never chooses a port merely because its filename contains `usbmodem`, which
prevents confusing the Puck's own CDC interface with the XIAO.

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
   complete state every 25 ms. All application wait loops service output at
   least every 25 ms. `Neutral` clears the cached active state immediately.

Sequence numbers cover every host-originated frame, including hello, state,
neutral, ping, and pong, and wrap as `u16`. Firmware owns an independent
wrapping transmit sequence for Hello responses, Pong, and rumble feedback.
Incoming bytes use the shared
recovering `StreamDecoder`; checksum and framing errors are counted without
creating a second parser.

The refresh interval is independent of the one-second Ping health check. Pings
prove the bidirectional session is responsive but deliberately do not keep the
firmware's 100 ms controller-data watchdog alive.

## Reverse rumble lease

Updated firmware returns `Rumble { low_frequency, high_frequency }` frames
after validating an exact Xbox 360 output report. The serial backend retains
only the newest feedback command, so rapid effects cannot create an unbounded
queue. The runtime applies changes to the selected Puck or Bluetooth controller
immediately and treats every nonzero frame as a 100 ms lease. Firmware
refreshes nonzero values every 25 ms; the runtime refreshes the selected SC2
actuator report every 40 ms. If feedback frames stop, the runtime sends zero
and stops refreshing. Ping and controller input do not renew this lease.

Old firmware remains input-compatible but never produces rumble feedback. Old
hosts ignore message type 8 through the protocol's unknown-message rule.

The live runtime remembers the selected XIAO's MCU-derived USB serial number.
After reconnect it prefers that identity even when macOS assigns a different
`/dev/cu.usbmodem…` path. On an initial run, two Hello-valid XIAOs are an
ambiguity and require `--port PATH`.

## Queue behavior

The transport-independent session has a bounded state queue with capacity 8 by
default. When the connection becomes ready, it sends only the newest pending
snapshot and discards every older state; when full, it also drops the oldest
state first. This prevents stale controller motion from replaying after a
handshake or temporary output stall. Lifecycle and protocol-control messages
are not stored in that queue.

## Commands

```bash
cargo run -p gamepad-simulator -- automated --output serial \
  --port /dev/cu.usbmodemXXXX --baud 115200 --serial-log

cargo run -p sc-replay -- session.jsonl --output serial \
  --port /dev/cu.usbmodemXXXX --baud 115200
```

The visualizer exposes the same path and baud controls when `Serial` is selected
as its output backend. Raw transmit/receive frame logging is opt-in. Native
firmware interoperability has been exercised on a flashed XIAO; the host state
machine remains covered independently with an in-memory `ByteTransport`.
