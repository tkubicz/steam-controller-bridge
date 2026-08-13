# Experimental virtual gamepad

Steam Controller Bridge has an opt-in macOS virtual-gamepad backend implemented
entirely in Rust with the public IOKit `IOHIDUserDevice` API. It does not use
Swift or CoreHID today. The implementation sits behind the same `GamepadOutput`
boundary as the proven serial bridge-device backend, so a future provider can
replace the helper without changing controller decoding or mapping.

The serial bridge device remains the default for new and migrated installations.
Normal menu and CLI launches keep virtual HID hidden and refuse to open it. Opt
in for that process with `--enable-virtual-hid` or
`SC_BRIDGE_ENABLE_VIRTUAL_HID=1`. In the packaged menu, this reveals
**Gamepad Output > Virtual Gamepad — Experimental** directly below Start/Stop.
The application never silently falls back after an enabled process selects a
backend. Switching sends neutral to the old output, releases it, and only then
opens the new backend. Stop, quit, sleep, wake, and updater suspension use the
same order. When stopped, the virtual controller is disconnected from macOS.

## Entitlement limitation

Creating an `IOHIDUserDevice` requires Apple's restricted
`com.apple.developer.hid.virtual.device` entitlement. An entitlement plist and
an ad-hoc signature do not grant authorization. On the current development Mac,
AMFI kills the ad-hoc helper at process launch once that restricted entitlement
is embedded (exit 137), before even `--self-test` can run. This is an expected
authorization failure, not a packaging failure. The unsigned build can run
`--self-test` and the entitlement-free `--dry-run` protocol; the signed bundle
can still be inspected and verified. Live use requires either the disposable
lowered-security VM or an Apple-approved helper App ID and matching provisioning
profile.

Only the nested Rust helper app receives the entitlement. The menu app does
not. The helper communicates with its parent over bounded, versioned JSON-lines
stdio and is released when stdin closes. The first milestone is input-only:
host set/get reports are counted and diagnosed, but virtual rumble is not
claimed. The bridge device remains the proven dual-rumble backend.

## Development commands

```bash
cargo build -p macos-virtual-hid --bin sc-virtual-hid-helper
cargo test -p macos-virtual-hid --test dry_run_ipc

cargo run -p gamepad-simulator -- automated \
  --output virtual-hid \
  --virtual-hid-helper ./target/debug/sc-virtual-hid-helper

cargo run -p sc-bridge -- \
  --enable-virtual-hid \
  --output virtual-hid \
  --virtual-hid-helper ./target/debug/sc-virtual-hid-helper
```

The equivalent environment opt-in is:

```bash
SC_BRIDGE_ENABLE_VIRTUAL_HID=1 cargo run -p sc-bridge -- \
  --output virtual-hid \
  --virtual-hid-helper ./target/debug/sc-virtual-hid-helper
```

The virtual backend has one fixed, tested contract: USB transport, the pinned
Xbox-style descriptor, and its 20-byte input report. Its default identity is
`045e:028e`, matching the Xbox 360 compatibility identity the bridge firmware
already enumerates with. The disposable-VM matrix showed that this complete
combination is the only tested one that reaches GameController and the offline
browser tester.
The HID properties include a stable serial, physical-device unique ID, and
location ID. This gives macOS stable identity material when the device is
recreated after Start, app relaunch, wake, or update. The disposable VM must
still confirm that System Settings reconnects its existing remembered profile
instead of creating another one.

The simulator, replay tool, and CLI bridge retain a narrow development hatch to
override only the identity. Both values must be supplied together:

```text
--virtual-hid-vendor-id 0xcafe --virtual-hid-product-id 0x4001
```

The override does not change the transport, descriptor, report bytes, or
mapping. The packaged menu intentionally exposes none of these knobs. The
bridge device remains the app-wide default output and there is no silent
fallback between backends.

Run the final commands live only in the disposable VM described below.
`sc-bridge` requires the runtime opt-in in both live and replay modes. The
standalone simulator and replay development tools accept the same output/helper
pair without the product-surface gate. Development commands require an explicit
path. The packaged menu resolves only its exact nested helper path and never
searches `PATH`.

The same packaged menu artifact can be launched experimentally without a
separate build:

```bash
open "Steam Controller Bridge.app" --args --enable-virtual-hid
```

Alternatively, launch its executable directly with
`SC_BRIDGE_ENABLE_VIRTUAL_HID=1`. The opt-in only exposes the backend; it does
not bypass macOS entitlement enforcement.

For system recognition, check `GCController.controllers` and System Settings
before treating the offline [Gamepad API tester](../tools/gamepad-api-tester.html)
as meaningful. Safari is downstream of the system GameController layer.

Automated simulator runs omit the Guide/Steam button because macOS 26 uses it
as a system shortcut that opens the Games app. Use `--include-guide-button`
only for an intentional system-button test. Keyboard command `8` remains an
explicit way to send it.

The macOS builder accepts `SC_BRIDGE_CODESIGN_IDENTITY` (default `-`),
`SC_BRIDGE_VIRTUAL_HID_PROVISIONING_PROFILE`, and
`SC_BRIDGE_VIRTUAL_HID_HELPER_IDENTIFIER`. It signs the nested helper first
with `packaging/macos/VirtualHidHelper.entitlements`, signs the outer app without
`--deep`, and uses `--deep --strict` only for final verification.

See [the IPC contract](VIRTUAL_HID_IPC.md) and [the feasibility matrix and VM
runbook](VIRTUAL_HID_FEASIBILITY.md).
