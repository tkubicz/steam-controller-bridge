# S0 Linux VM hardware path

Status: Puck path accepted for B1; XIAO checks deferred.

## Environment

- Apple-silicon macOS host running an Ubuntu 26.04 LTS ARM64 guest in VirtualBuddy.
- Ubuntu kernel `7.0.0-30-generic`.
- VirtualHere macOS server and ARM64 Linux console client.
- Rust `hidapi` 2.6.6 with the `linux-native` backend and libudev.

## Result

VirtualHere attached the official Steam Controller Puck (`28de:1304`) to the
guest through `vhci_hcd`. Linux created `/dev/hidraw2` through `/dev/hidraw6`.
The existing classifier selected interface 2, usage `ff00:0001`, without a
Linux-specific identity exception.

The guest successfully:

- enumerated the Puck with `lsusb` and hidapi;
- read the 372-byte slot descriptors and the 54-byte Puck descriptor;
- received a 54-byte controller input report with report ID `0x42`;
- sent the production 64-byte lizard-mode feature report;
- sent an attributes feature query and read its 63-byte response;
- detached and reattached the Puck, including hidraw disappearance and
  recreation.

The lizard-mode feature write succeeded on its first attempt. The attributes
read returned `EPIPE` once and succeeded after a 20 ms retry. B1 defensively
allows one 20 ms retry for Linux feature writes rather than treating the first
stalled control transfer as a permanent disconnect; the VM run did not observe
a stalled production write. The retry stays at the provider boundary because
the initial lizard-mode write must succeed before the HID worker accepts input.

## Decision

Use VirtualHere as the routine Puck hardware path for Linux development and use
hidapi's `linux-native` backend. Keep the existing VID, PID, usage, and interface
classifier. B1 adds libudev only as a Linux target build dependency; packaging
and runtime dependency declarations remain in F5.

## Known gaps

- The guest exposed the hidraw nodes as `root:root` mode `0600`. B2 owns the
  narrowly matched interactive-session access rule.
- hidapi 2.6.6's `linux-native` backend omits hidraw entries without a
  `HID_UNIQ` property. The tested Puck exposes a USB serial and enumerates;
  direct Bluetooth remains unverified and would be hidden if its Linux HID
  device lacks that property.
- Direct Bluetooth was not tested because the VM has no passed-through
  Bluetooth adapter.
- XIAO CDC runtime mode and UF2 bootloader mode are deferred. B4, B5, U1, U2,
  and milestone M1 cannot use this record as evidence that those paths work.
- This VM run is not bare-metal sleep/wake, Bluetooth-radio, desktop-session,
  packaging, or distribution acceptance.

If the VirtualHere path becomes unreliable, the fallback remains VMware Fusion
USB capture before changing the selected Linux APIs.
