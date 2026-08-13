# Virtual HID helper protocol

The Rust parent and Rust helper share the types in
`crates/macos-virtual-hid/src/contract.rs`. Checked-in golden messages live in
`crates/macos-virtual-hid/fixtures`. Protocol version 3 uses compact UTF-8 JSON,
one object per line, over inherited stdin/stdout. Stderr is human diagnostics.

Lines are limited to 65,536 bytes including the newline. Unknown fields,
malformed or unterminated JSON, a wrong protocol, an invalid 20-byte input
report, or an out-of-order sequence is fatal. Raw host set/get report
diagnostics are limited to 4,096 bytes. Both sides use bounded queues.

The parent sends `create` first, then monotonically sequenced `input_report`
messages, and finally `shutdown`:

```json
{"type":"create","protocol":3,"vendor_id":1118,"product_id":654}
{"type":"input_report","protocol":3,"sequence":1,"report":[0,20,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}
{"type":"shutdown","protocol":3,"sequence":2}
```

The helper replies with `ready` only after the live provider creates the device
and dispatches initial neutral. Every sequenced command receives the matching
`applied`. Delegate traffic is forwarded as `set_report` or `get_report`;
terminal failures use `fatal` with a stable error class. macOS may invoke a
registered delegate during activation, before `ready` reaches stdout. The
parent records those messages but does not mark the output ready until `ready`
arrives. `ready` echoes the active VID/PID, and the parent rejects a mismatch.

There is one virtual-gamepad contract: USB transport, the pinned 212-byte
Xbox-style HID descriptor, and its 20-byte XInput report. Bytes 0 and 1 are the
fixed type/size header `00 14`; bytes 2-3 contain D-pad and button bits; bytes
4-5 contain unsigned triggers; bytes 6-13 contain four little-endian signed
16-bit axes; bytes 14-19 are reserved zeroes. The exact neutral report is:

```text
00 14 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
```

The default identity is `045e:028e`, matching the compatibility identity used
by the XIAO firmware and the only identity proven to register through macOS
GameController in the disposable VM. Callers may override `vendor_id` and
`product_id` together for development, but cannot replace the descriptor,
transport, report encoder, or validation contract.

Ordinary pending states may coalesce. Neutral removes queued ordinary states
and waits for its sequence acknowledgement. Queue overflow, acknowledgement
timeout, malformed traffic, helper exit, or dispatch failure marks the output
lost; stale input is never silently retained.
