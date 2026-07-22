# Steam Controller 2 Host Protocol

This implementation follows [OpenPuck](https://github.com/safijari/openpuck), as designated for this project. It targets the host-facing HID reports exposed by a Steam Controller 2 puck-compatible collection. It does not apply the original 2015 Steam Controller layout.

## Confirmed from OpenPuck

OpenPuck exposes four puck HID slot interfaces and forwards controller input reports verbatim. Relevant host input report IDs include:

- `0x45`: primary state, 46 bytes including report ID.
- `0x42`: newer-firmware extended state, 54 bytes including report ID. Its first 46 bytes use the same state layout as `0x45`; the eight-byte tail remains preserved and uninterpreted.
- `0x43`: battery/power status, 15 bytes including report ID.
- `0x44`: auxiliary status, 6 bytes including report ID; semantics unresolved.
- `0x79`: connection edge, 2 bytes including report ID. Value `1` is disconnected and `2` connected.
- `0x7b`: periodic 13-byte status; byte 9 is signed signal strength in dBm.

Every multi-byte field is little-endian. HIDAPI returns the report ID as the first byte, and the decoder also receives it as metadata; mismatches are rejected.

## State report layout

| Offset | Size | Meaning |
| ---: | ---: | --- |
| `0x00` | 1 | Report ID, `0x45` or `0x42` |
| `0x01` | 1 | Sequence |
| `0x02` | 4 | Buttons, `u32` |
| `0x06` | 2 | Left trigger, `u16` |
| `0x08` | 2 | Right trigger, `u16` |
| `0x0a` | 2 | Left stick X, `i16` |
| `0x0c` | 2 | Left stick Y, `i16` |
| `0x0e` | 2 | Right stick X, `i16` |
| `0x10` | 2 | Right stick Y, `i16` |
| `0x12` | 2 | Left pad X, `i16` |
| `0x14` | 2 | Left pad Y, `i16` |
| `0x16` | 2 | Left pad pressure, `i16` |
| `0x18` | 2 | Right pad X, `i16` |
| `0x1a` | 2 | Right pad Y, `i16` |
| `0x1c` | 2 | Right pad pressure, `i16` |
| `0x1e` | 4 | IMU timestamp, `u32` |
| `0x22` | 2 | Accelerometer X, `i16` |
| `0x24` | 2 | Accelerometer Y, `i16` |
| `0x26` | 2 | Accelerometer Z, `i16` |
| `0x28` | 2 | Gyro X, `i16` |
| `0x2a` | 2 | Gyro Y, `i16` |
| `0x2c` | 2 | Gyro Z, `i16` |

The decoder clamps the exceptional `i16::MIN` axis representation to `-32767`, matching the bridge protocol's symmetric stick range. It otherwise preserves source signs; mapping owns any axis inversion.

## Button bits

| Bit | Mask | Meaning |
| ---: | ---: | --- |
| 0 | `0x00000001` | A |
| 1 | `0x00000002` | B |
| 2 | `0x00000004` | X |
| 3 | `0x00000008` | Y |
| 4 | `0x00000010` | Quick Access Menu |
| 5 | `0x00000020` | Right stick press |
| 6 | `0x00000040` | View |
| 7 | `0x00000080` | R4 |
| 8 | `0x00000100` | R5 |
| 9 | `0x00000200` | Right shoulder |
| 10 | `0x00000400` | D-pad down |
| 11 | `0x00000800` | D-pad right |
| 12 | `0x00001000` | D-pad left |
| 13 | `0x00002000` | D-pad up |
| 14 | `0x00004000` | Menu |
| 15 | `0x00008000` | Left stick press |
| 16 | `0x00010000` | Steam |
| 17 | `0x00020000` | L4 |
| 18 | `0x00040000` | L5 |
| 19 | `0x00080000` | Left shoulder |
| 20 | `0x00100000` | Right stick touch |
| 21 | `0x00200000` | Right pad touch |
| 22 | `0x00400000` | Right pad click |
| 23 | `0x00800000` | Right trigger click |
| 24 | `0x01000000` | Left stick touch |
| 25 | `0x02000000` | Left pad touch |
| 26 | `0x04000000` | Left pad click |
| 27 | `0x08000000` | Left trigger click |
| 28 | `0x10000000` | Right grip touch |
| 29 | `0x20000000` | Left grip touch |

Bits 30 and 31 are not assigned to physical SC2 controls by OpenPuck.

OpenPuck's published table labels bits 28 and 29 as grip-touch signals, while current `0x42` firmware comments note they may be always-on status bits in that extended variant. The decoder exposes the documented bits and retains the raw report; mapping must keep them out of ordinary button output until local captures confirm their behavior.

## Decoder behavior

- Exact sizes are required for every known report ID.
- Report metadata must match the first byte.
- Unknown IDs and malformed sizes return structured errors.
- No decoding path indexes bytes until the corresponding fixed length is validated.
- Full reports are retained for future discoveries and capture comparison.
- No feature or output reports are sent by the decoder or probe tools.

## Still requiring local captures

- Actual collection paths and VID/PID values for the user's official puck, direct USB, and Bluetooth modes.
- Whether direct USB and Bluetooth expose the same host-facing reports as the puck.
- Real trigger maxima, neutral jitter, pad pressure ranges, IMU scaling, and axis orientation.
- Semantics of report `0x44` and unresolved bytes in `0x43`, `0x7b`, and extended `0x42`.
- Initialization behavior when Steam is absent.

## Sources

- [OpenPuck protocol specification](https://github.com/safijari/openpuck/blob/main/docs/PROTOCOL.md)
- [OpenPuck host HID implementation](https://github.com/safijari/openpuck/blob/main/OpenPuck/puck_hid.cpp)
- [OpenPuck report decoding helpers](https://github.com/safijari/openpuck/blob/main/OpenPuck/triton.h)
- [OpenPuck RF report extraction](https://github.com/safijari/openpuck/blob/main/OpenPuck/rf_link.cpp)
