# Linux device access

`60-steam-controller-bridge.rules` grants the active local session access to
all hidraw collections belonging to these exact supported product identities:

- the official Steam Controller 2 Puck on the USB HID bus (`28de:1304`);
- Steam Controller 2 over the Bluetooth HID bus (`28de:1303`).

It does not grant access to other Valve products or unrelated hidraw devices.
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

Disconnect and reconnect the Puck or Bluetooth controller after reloading the
rules. For a matching `/dev/hidrawN`, verify the tag and access-control list:

```bash
udevadm info --query=property --name=/dev/hidrawN | grep uaccess
getfacl /dev/hidrawN
```

The default policy relies on systemd-logind and an active local session. A
headless service should use a dedicated system group instead of making hidraw
world-writable. An administrator can create the group, add only the service
account, and install a local override with the same exact device matches:

```bash
sudo groupadd --system steam-controller-bridge
sudo usermod --append --groups steam-controller-bridge SERVICE_ACCOUNT
```

```udev
SUBSYSTEM=="hidraw", KERNELS=="0003:28DE:1304.*", GROUP="steam-controller-bridge", MODE="0660"
SUBSYSTEM=="hidraw", KERNELS=="0005:28DE:1303.*", GROUP="steam-controller-bridge", MODE="0660"
```

Save the override as
`/etc/udev/rules.d/61-steam-controller-bridge-headless.rules`, reload the rules,
reconnect the device, and restart the service so its supplementary groups are
refreshed.
