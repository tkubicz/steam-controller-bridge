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
| 7 | DeviceInfo | implementation-defined bytes, at most 256 |
| 8 | Rumble | low-frequency `u16`, high-frequency `u16` |
| 255 | Error | code `u16`, followed by optional detail bytes |

Unknown message type values are returned as opaque messages so newer messages can be ignored safely. Known messages with invalid lengths are rejected. Frames whose protocol version is not exactly 1 are rejected; version range selection happens through `Hello` before ordinary traffic.

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

`Rumble` is the only protocol-v1 firmware-to-host feedback message. Its
four-byte payload is:

| Offset | Size | Field |
| ---: | ---: | --- |
| 0 | 2 | Xbox low-frequency/strong magnitude, `0..65535` |
| 2 | 2 | Xbox high-frequency/weak magnitude, `0..65535` |

Both values are little-endian. Firmware scales the Xbox driver's 8-bit values
linearly with `value * 257`, sends changes immediately, and refreshes a nonzero
request every 25 ms. A zero command ends that lease. Firmware transmit sequence
numbers are independent wrapping `u16` values; an older host safely ignores
message type 8 as an unknown message.

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
