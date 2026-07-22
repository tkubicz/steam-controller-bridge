# Steam Controller Protocol Notes

This document deliberately separates observed facts from future investigation. No Steam Controller was connected during the initial HID probing implementation, so no controller-specific identifiers or packet offsets are treated as confirmed.

## Confirmed

- macOS exposes HID devices as one or more collections, each with a path, VID, PID, usage page, usage, interface number, strings, and bus type where the OS supplies them.
- The host tool can enumerate all current collections, open one explicitly selected collection, receive numbered input reports, detect read failure, and attempt reconnection.
- Raw captures preserve monotonic timestamps, report ID, complete returned bytes, source collection identity, transport, and lifecycle events.
- The Rust `hidapi` backend documents that the first byte returned by input reads contains the report number for devices using numbered reports. The implementation preserves that byte in `data` and also exposes it as `report_id`.

## Inferred

- Collections sharing VID, PID, serial number, manufacturer/product strings, and transport are candidate members of the same physical device. When a serial number is unavailable this grouping can conflate two identical physical devices, so `sc-probe` labels the count as candidate siblings.
- A collection path is the strongest available identity during a connection, but it may change after unplugging. Reconnect therefore falls back to VID/PID, serial, usage, and interface metadata.

## Unknown

- Steam Controller VID/PID values for direct USB, official receiver, and Bluetooth modes in this environment.
- Which collection carries controller input for each transport.
- Report IDs, lengths, field offsets, firmware variants, heartbeat behavior, and feature-report commands.
- The exact command needed to disable lizard mode and the safe restore sequence.
- Whether any reports can be dropped below the HID API; the current backend cannot expose such a counter and records zero.

## Investigation workflow

1. Connect exactly one controller transport and save `sc-probe list` plus `inspect` output.
2. Capture an idle interval and named single-control actions with `sc-probe capture`.
3. Repeat for direct USB, receiver, and Bluetooth without assuming that identifiers or layouts match.
4. Diff report IDs, lengths, and byte changes before assigning field meanings.
5. Document every feature-report byte sequence with its external source and a dry-run path before sending it.

## Sources

- [`hidapi` Rust API documentation](https://docs.rs/hidapi/2.6.6/hidapi/) for enumeration, bus type, input report, and feature report semantics.
- [`libusb/hidapi`](https://github.com/libusb/hidapi) for the native HID abstraction used by the Rust wrapper.

No undocumented Steam Controller feature reports are present in the codebase at this phase.
