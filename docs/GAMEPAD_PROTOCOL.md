# Gamepad Bridge Protocol v1

All multi-byte integers use little-endian byte order. Encoding is field-by-field and makes no assumptions about Rust, C, or firmware structure layout.

## Frame

| Offset | Size | Field |
| ---: | ---: | --- |
| 0 | 2 | Magic bytes `53 43` (`SC`) |
| 2 | 1 | Protocol version (`01`) |
| 3 | 1 | Message type |
| 4 | 2 | Payload length |
| 6 | 2 | Sequence number, wrapping `u16` |
| 8 | N | Payload, at most 256 bytes |
| 8+N | 2 | CRC-16, little-endian |

The CRC is CRC-16/CCITT-FALSE with polynomial `0x1021`, initial value `0xffff`, no input or output reflection, and final XOR `0x0000`. It covers the complete header and payload, but not the checksum itself. The standard ASCII check string `123456789` produces `0x29b1`.

## Message types

| Value | Name | Payload |
| ---: | --- | --- |
| 1 | Hello | minimum version `u8`, maximum version `u8` |
| 2 | HelloResponse | selected version `u8` |
| 3 | GamepadState | 18-byte payload below |
| 4 | Neutral | empty |
| 5 | Ping | nonce `u32` |
| 6 | Pong | nonce `u32` |
| 7 | DeviceInfo | device-info payload below, at most 256 bytes |
| 8 | Rumble | low-frequency `u16`, high-frequency `u16` |
| 9 | EnterUf2Bootloader | request ID `u32` |
| 10 | Uf2BootloaderReady | matching request ID `u32` |
| 11 | RecordInstallReceipt | receipt payload below |
| 12 | InstallReceiptRecorded | matching receipt payload below |
| 255 | Error | control error code `u16`, matching request ID `u32` |

Unknown message type values are returned as opaque messages so newer messages can be ignored safely. Known messages with invalid lengths are rejected. Frames whose protocol version is not exactly 1 are rejected; version range selection happens through `Hello` before ordinary traffic.

## Device info payload

Firmware may send one `DeviceInfo` frame to the host after every successful
Hello negotiation, retried until the transport accepts it. `DeviceInfo` is
deliberately extensible: firmware that predates it remains bridge-compatible,
and hosts must ignore trailing fields that they do not understand.

| Offset | Size | Field |
| ---: | ---: | --- |
| 0 | 1 | Device-info format, currently `1` |
| 1 | 2 | Firmware revision, `u16` little-endian, hand-maintained monotonic |
| 3 | 4 | Capability flags: bit 0 automatic UF2 entry, bit 1 installation receipts |
| 7 | 1 | Receipt state: 0 unsupported, 1 pending, 2 recorded, 3 invalid |
| 8 | 8 | Recorded UTC Unix seconds, present when state is recorded |
| 16 | 16 | Recorded installation ID, present when state is recorded |
| 32 | 1 | Source: 1 App Center, 2 first observed, present when state is recorded |

Exactly three bytes is the legacy revision 1 report. Lengths four through seven
are malformed. Extended reports require at least eight bytes, or 33 bytes for a
recorded receipt. Zero or more TLVs follow that base body:

| Field | Size | Meaning |
| --- | ---: | --- |
| Tag | 1 | Extension type |
| Length | 1 | Value length in bytes |
| Value | Length | Tag-specific value |

Tag `1` is the firmware target ID. Its value is 1-64 lowercase ASCII bytes
using `a-z`, `0-9`, dot, and hyphen, with an alphanumeric first and last byte.
It identifies an implementation for updater-catalog matching; it is not a
hardware-model claim or update authority. The project XIAO firmware reports
`seeed-xiao-nrf52840`.

Unknown complete TLVs are ignored. A duplicate target tag, invalid target
value, truncated TLV header, or truncated value makes target identity malformed
and disables update association. It does not invalidate a separately valid
revision or the bridge session. No target TLV means legacy/unreported identity.
Old format-1 hosts tolerate the appended bytes by continuing to parse only the
base or receipt body.

An unknown format byte is classified as firmware newer than the host and is
never treated as outdated. A current-format payload shorter than three bytes is
malformed instead of being mistaken for a future format. Firmware that never
sends `DeviceInfo` predates version reporting and is reported as such after a
short host-side grace period.

## Firmware update control

Update commands are optional protocol capabilities and are accepted only after
Hello negotiation. Request IDs are
little-endian `u32` values. A bootloader request forces neutral input and zero
rumble before `Uf2BootloaderReady` is queued. The sketch drains CDC, waits 100
ms, and calls the Adafruit UF2 entry routine. Repeating the same request ID is
idempotent; a different ID after transition begins receives a busy error.

Both receipt messages use the same 29-byte payload:

| Offset | Size | Field |
| ---: | ---: | --- |
| 0 | 4 | Request ID `u32` |
| 4 | 8 | UTC Unix seconds `u64` |
| 12 | 16 | Random installation ID |
| 28 | 1 | Source: 1 App Center, 2 first observed |

Firmware acknowledges only after the receipt is committed and read back. It
rejects replacement metadata until another UF2 flash restores blank slots.

A host must associate automatic update control with a matching, locally known
firmware target before issuing these commands. Targetless, malformed, or
different-target implementations remain valid bridge devices but receive no
automatic bootloader request or target-specific revision policy.

Control errors have an exact six-byte payload. The first two bytes contain the
error code and the final four bytes contain the request ID. Current error codes
are 1 for a busy UF2 transition, 2 for a rejected receipt, and 3 for a receipt
readback mismatch. A host correlates the request ID before applying the error.

## Gamepad state payload

| Offset | Size | Field | Encoding |
| ---: | ---: | --- | --- |
| 0 | 4 | buttons | stable bit mask |
| 4 | 1 | hat | 0-7 clockwise from north; 8 centered |
| 5 | 1 | flags | reserved; sender writes 0 |
| 6 | 2 | left X | signed axis |
| 8 | 2 | left Y | signed axis |
| 10 | 2 | right X | signed axis |
| 12 | 2 | right Y | signed axis |
| 14 | 2 | left trigger | unsigned axis |
| 16 | 2 | right trigger | unsigned axis |

Stick values map `-1.0`, `0.0`, and `1.0` to `-32767`, `0`, and `32767`. `-32768` is reserved and rejected. Trigger values map `0.0` and `1.0` to `0` and `65535`.

Button bits 0 through 15 are, in order: South, East, West, North, LeftShoulder, RightShoulder, LeftStick, RightStick, Back, Start, Guide, LeftGrip, RightGrip, Extra1, Extra2, Extra3. Bits 16 through 31 are reserved.

`Neutral` is a semantic command to clear firmware state. A `GamepadState` containing all-neutral fields has the same controller outcome but remains useful as a fully explicit state sample.

## Rumble feedback

`Rumble` is the gamepad feedback message in protocol v1. Its
four-byte payload is:

| Offset | Size | Field |
| ---: | ---: | --- |
| 0 | 2 | Xbox low-frequency/strong magnitude, `0..65535` |
| 2 | 2 | Xbox high-frequency/weak magnitude, `0..65535` |

Both values are little-endian. Firmware scales the Xbox driver's 8-bit values
linearly with `value * 257`, sends changes immediately, and refreshes a nonzero
request every 25 ms. A zero command ends that lease. From revision 4 those
refreshes cannot outlive their source: a nonzero request also expires 250 ms
after the last nonzero Xbox OUT packet, and nonzero output is quarantined
after each HID mount, CDC connection, and Hello until a zero arrives or the
source stays quiet for 250 ms. Firmware transmit sequence numbers are
independent wrapping `u16` values; an older host safely ignores message type 8
as an unknown message.

## Stream recovery

The decoder accepts partial reads and multiple frames per read. It discards garbage before the next magic sequence. Oversized lengths and checksum failures emit an error and resume searching one byte later, allowing a valid frame after malformed input to be recovered. A trailing `53` byte is retained because it may begin the next magic sequence. Truncated frames remain buffered until more bytes arrive.

## Static test vectors

The bytes below include the checksum in transmitted little-endian order.

```text
Neutral, sequence 0:
53 43 01 04 00 00 00 00 e7 fb

CRC check only:
ASCII "123456789" -> CRC value 29b1 (wire bytes b1 29)
```

The neutral vector is asserted by both Rust and native firmware tests.
