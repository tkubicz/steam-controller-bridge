# Linux device access

`60-steam-controller-bridge.rules` grants the active local session access to
the exact supported controller and official bridge identities:

- the official Steam Controller 2 Puck on the USB HID bus (`28de:1304`);
- Steam Controller 2 over the Bluetooth HID bus (`28de:1303`);
- the raw USB device and optional CDC serial fallback exposed by official XIAO
  bridge firmware, using the exact runtime identity in the firmware target
  catalog.

It does not grant access to other Valve products, unrelated hidraw devices, or
generic devices that reuse its Xbox-compatible USB identity. The XIAO serial
fallback rule also tells ModemManager to ignore the bridge. Factory
firmware and UF2 bootloader identities are intentionally excluded from this
runtime-device rule.
The raw-USB ACL grants the active local user read/write access to the whole
matching USB device node. The bridge immediately narrows its own descriptor to
the CDC interfaces with `USBDEVFS_DROP_PRIVILEGES`, but that cannot restrict a
different process running as the same user from opening another descriptor.
This is an inherent boundary of unprivileged usbfs access, not isolation for
the Xbox interface across the whole user session.
The Puck match is based on the Linux S0 capture. Direct Bluetooth uses the exact
identity supported by the device classifier, but still needs live Linux
hardware acceptance. Cargo does not install the rule. Until Linux packaging is
available, install it for development with:

```bash
sudo install -Dm0644 \
  packaging/linux/60-steam-controller-bridge.rules \
  /etc/udev/rules.d/60-steam-controller-bridge.rules
sudo udevadm control --reload-rules
```

Disconnect and reconnect the controller and XIAO after reloading the rules. For
a matching `/dev/hidrawN`, `/dev/bus/usb/BBB/DDD`, or `/dev/ttyACMN`, verify
the tag and access-control list:

```bash
udevadm info --query=property --name=/dev/hidrawN | grep uaccess
getfacl /dev/hidrawN
udevadm info --query=property --name=/dev/bus/usb/BBB/DDD | grep uaccess
getfacl /dev/bus/usb/BBB/DDD
udevadm info --query=property --name=/dev/ttyACMN | grep -E 'uaccess|ID_MM_DEVICE_IGNORE'
getfacl /dev/ttyACMN
```

The default policy relies on systemd-logind and an active local session. A
headless service should use a dedicated system group instead of making hidraw
and USB device nodes world-writable. An administrator can create the group,
add only the service account, and replace the packaged rule with the same exact
device matches:

```bash
sudo groupadd --system steam-controller-bridge
sudo usermod --append --groups steam-controller-bridge SERVICE_ACCOUNT
```

```udev
SUBSYSTEM=="hidraw", KERNELS=="0003:28DE:1304.*", GROUP="steam-controller-bridge", MODE="0660"
SUBSYSTEM=="hidraw", KERNELS=="0005:28DE:1303.*", GROUP="steam-controller-bridge", MODE="0660"
SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", ATTR{idVendor}=="045e", ATTR{idProduct}=="028e", ATTR{manufacturer}=="Lynxware", ATTR{product}=="Steam Controller Bridge", GROUP="steam-controller-bridge", MODE="0660"
SUBSYSTEM=="tty", ATTRS{idVendor}=="045e", ATTRS{idProduct}=="028e", ATTRS{manufacturer}=="Lynxware", ATTRS{product}=="Steam Controller Bridge", GROUP="steam-controller-bridge", MODE="0660", ENV{ID_MM_DEVICE_IGNORE}="1"
```

Save the group-only policy as
`/etc/udev/rules.d/60-steam-controller-bridge.rules`. The identical filename in
`/etc` replaces a packaged copy under `/usr/lib`; adding a later rule under a
different name would leave the original `uaccess` tag in place. Reload the
rules, reconnect the device, and restart the service so its supplementary
groups are refreshed.

If you previously installed the headless policy as
`/etc/udev/rules.d/61-steam-controller-bridge-headless.rules`, remove that old
file before reloading the rules. Leaving both files installed preserves the
packaged `uaccess` policy.
