# Serial Bridge-Device Contract

The host's live gamepad output is a protocol-compatible bridge device reached
over a byte-stream serial port. The protocol is public and board-neutral: an
independent implementation does not need Seeed USB identifiers, a Lynxware
manufacturer string, or XIAO hardware. Protocol v1 and its framing remain
unchanged.

On Linux, the official XIAO firmware may instead be opened directly through its
CDC USB interfaces. That native backend is restricted to the exact official
identity and validated CDC/XInput topology, then exposes the same byte stream
to the shared protocol session. Third-party implementations continue using the
serial contract below.

## Discovery and opt-in identity

Zero-configuration discovery considers macOS callout ports (`/dev/cu.*`) and
Linux USB serial endpoints (`/dev/ttyACM<N>` or `/dev/ttyUSB<N>`) whose USB
product string is exactly `Steam Controller Bridge`. The product string is the
implementation's explicit opt-in marker. VID, PID, manufacturer, and board
model are not discovery requirements; the host-specific port name only limits
which endpoints are safe automatic candidates.

Every candidate must still open successfully and complete the protocol-v1
Hello exchange. A marker is not proof of protocol compatibility. With
`--port PATH`, the user selects an endpoint explicitly; this bypasses the USB
product marker but never bypasses Hello negotiation or protocol validation.

The runtime remembers the stable USB serial identity of the selected bridge
device and prefers it after the host assigns a different locator. If multiple
candidates complete Hello and no remembered identity selects exactly one, the
runtime reports an ambiguity. `--port PATH` can select among serial candidates;
a raw-USB ambiguity requires disconnecting the unused bridge.

Implementers who cannot or do not want to publish the discovery marker can
therefore remain fully usable through the explicit-port path.

If access is denied for a protocol-compatible third-party device on Linux,
grant the selected serial endpoint read/write access through a narrowly matched
udev rule or the distribution's serial-access group (commonly `dialout`).
Group changes require a new login session or service restart. Discovery does
not grant broad access to arbitrary USB serial devices. If ModemManager probes
the endpoint, use a narrowly matched `ID_MM_DEVICE_IGNORE` rule for that device
rather than disabling modem detection broadly.

## Session lifecycle and safety

1. The host opens the endpoint at 115200 baud by default and sends
   `Hello { min: 1, max: 1 }`.
2. The device answers `HelloResponse { selected: 1 }` within the handshake
   deadline. Ordinary traffic is not valid before this exchange.
3. When ready, the host flushes only its newest queued gamepad snapshot.
4. A one-second host health interval sends `Ping`; the matching `Pong` must
   arrive before the two-second timeout. Device-originated pings receive an
   immediate pong.
5. While a non-neutral state is current, the host retransmits the complete
   state every 25 ms. These refreshes feed a device-side safety watchdog.
6. Stop, disconnect handling, decode failure, sleep, and orderly shutdown send
   or attempt `Neutral` before releasing hardware.
7. I/O, handshake, or health failure makes the output unavailable and starts
   rediscovery. A device must independently neutralize on expired data,
   disconnected CDC, USB teardown, malformed input, and internal faults.

Ping traffic proves that the bidirectional session is responsive but must not
renew the controller-data watchdog. Rumble is also a bounded lease with an
explicit zero; see [GAMEPAD_PROTOCOL.md](GAMEPAD_PROTOCOL.md).

Sequence numbers cover every frame in each direction and wrap as `u16`. The
host and device own independent transmit sequences. Incoming bytes use the
recovering stream decoder, so corruption is counted and scanning resumes at
the next valid frame.

## DeviceInfo and firmware identity

After Hello, an implementation may report DeviceInfo format 1. Firmware
revision and capability flags describe the running implementation, not the
physical board. The optional target-ID TLV associates that implementation with
a project updater target.

Target IDs are 1-64 byte lowercase ASCII identifiers containing letters,
digits, dots, and hyphens, with an alphanumeric first and last byte. The current
project target is `seeed-xiao-nrf52840`. Target identity is not hardware
detection and grants no update authority by itself: the signed release manifest
must name a target that the app's firmware-target catalog recognizes.

Legacy firmware that reports no target, a malformed target, a duplicate target,
or a different valid target continues normal bridging. The host presents its
firmware neutrally and disables all target-specific revision recommendations,
automatic bootloader requests, and automatic update association.

Unknown DeviceInfo TLVs are ignored. Invalid or truncated target information
fails closed for updater association without invalidating the negotiated serial
session or a separately valid firmware revision. The byte layout is specified
in [GAMEPAD_PROTOCOL.md](GAMEPAD_PROTOCOL.md).

## Optional capabilities

Protocol implementations may omit updater capabilities and still provide
gamepad output. Current DeviceInfo capability bits are:

| Bit | Capability | Required behavior |
| ---: | --- | --- |
| 0 | Enter UF2 bootloader | Accept the correlated command only after Hello, neutralize gamepad and rumble first, acknowledge readiness, then enter the bootloader. |
| 1 | Installation receipts | Commit, read back, and acknowledge the exact correlated receipt; reject unsafe replacement. |

The host invokes these capabilities automatically only when the reported target
matches a signed catalog target. A user may explicitly start the labeled XIAO
install/recovery workflow for unidentified firmware, but the app releases the
serial session and requires manual bootloader entry. It then verifies the XIAO
board ID and UF2 family before writing, and the returned target ID, revision,
and receipt before reporting success.

## Output backends and status

Runtime status describes a generic gamepad output with a backend kind,
readiness, optional endpoint and stable ID, and optional firmware report.
Serial bridge devices, diagnostic dump, file, and mock backends are distinct.
This status shape leaves room for another backend such as virtual HID without
making firmware or XIAO concepts part of the runtime core.

## Validation without custom hardware

The transport-independent session is covered with an in-memory `ByteTransport`
that exercises negotiation, trailing DeviceInfo data, malformed target TLVs,
queue bounds, watchdog refresh cadence, ping/pong, rumble, and corruption
recovery. The existing simulator and replay tools exercise the same output
interface:

```bash
cargo run -p gamepad-simulator -- automated --output serial \
  --port /dev/cu.usbmodemXXXX --baud 115200 --serial-log

cargo run -p sc-replay -- session.jsonl --output serial \
  --port /dev/ttyACM0 --baud 115200
```

The visualizer exposes the same explicit endpoint and baud controls. The
project's first supported implementation is the reference XIAO nRF52840/Sense
firmware in `firmware/xiao-nrf52840`; implementing firmware for another board
does not require changing this host contract.
