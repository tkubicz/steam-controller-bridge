# Virtual HID feasibility and disposable VM runbook

## Evidence matrix

| Layer | Status | Evidence | Notes |
| --- | --- | --- | --- |
| Rust compile | passing | macOS workspace check and Clippy | warnings denied |
| Linux portability | partial | `cargo clippy --target x86_64-unknown-linux-gnu -p virtual-gamepad --all-targets -- -D warnings` | new crate passes; full workspace requires the unavailable `x86_64-linux-gnu-gcc` for an existing TLS dependency |
| Dry-run IPC | passing | `dry_run_ipc` integration test | no entitlement or live device |
| Packaged helper | passing | `build-macos-app.py`, `codesign --verify --deep --strict` | assembly/signature gate only; nested helper alone has the entitlement |
| Normal-host ad-hoc helper launch | blocked as expected | packaged `--self-test` exits 137 | AMFI kills the restricted-entitlement process before `main`; unsigned release `--self-test` passes |
| VM virtual-device creation | passing | `hidutil` shows the userspace Game Pad | lowered-security VM |
| HID reports | passing | `sc-probe` receives changing reports | neutral/standard buttons/hats/axes/triggers observed |
| GameController recognition | passing with fixed contract | `045e:028e` plus the Xbox-style contract registers as a controller | now the only shipped contract |
| System Settings | pending capture | system query recognizes the fixed contract | record the final UI result |
| Browser Gamepad API | passing with fixed contract | offline tester sees the controller | do not tune Safari separately |
| SDL | blocked | local client output | after enumeration |
| Physical controller E2E | blocked | test notes | after simulator |
| Sleep/wake | blocked | test notes | live VM or authorized build |
| Rumble | blocked | raw output-report capture | not implemented |
| Normal-security signed build | blocked | approved provisioning profile | paid account and Apple approval |

`Blocked` means a prerequisite is missing; it is not evidence of incompatibility.
In particular, the exit-137 result proves the normal-security entitlement gate;
it does not exercise `IOHIDUserDevice` or indicate a descriptor problem.

## Historical recognition evidence

These cumulative experiments were tested in the disposable lowered-security VM
on 2026-08-13. They explain the fixed implementation contract; they are no
longer selectable profiles in the code:

| Candidate | Transport | VID:PID | Descriptor/report | Game controller | Offline Gamepad API |
| --- | --- | --- | --- | --- | --- |
| `generic-virtual` | Virtual | `cafe:4001` | generic, 14 bytes | not recognized | not visible |
| `generic-usb` | USB | `cafe:4001` | generic, 14 bytes | not recognized | not visible |
| `xbox360-cafe` | USB | `cafe:4001` | Xbox-style, 20 bytes | not recognized | not visible |
| `xbox360-microsoft` | USB | `045e:028e` | Xbox-style, 20 bytes | recognized | visible |

`xbox360-cafe` and `xbox360-microsoft` have identical transport, descriptor,
and report bytes. Their only intentional difference is VID/PID. This isolates
system recognition to the known Microsoft compatibility identity rather than
the virtual transport, report delivery, or generic descriptor. It proves that
`IOHIDUserDevice` can reach GameController and the browser, but it does not
make the project a Microsoft product; the existing project disclaimer continues
to apply. The virtual backend now exposes only this proven
combination, matching the bridge firmware. A paired VID/PID override remains for
development.

Automated simulator runs omit the Guide/Steam system button by default because
macOS 26 opens the Games app when that button is dispatched. Pass
`--include-guide-button` only when explicitly validating that system action.

## Disposable VM procedure

Use a clean VirtualBuddy macOS VM on Apple silicon. Install no personal
accounts. Copy the packaged artifact locally, shut the guest down, and duplicate
it as a disposable test clone. Never run the following security commands on the
host.

In the clone's Recovery environment:

```bash
csrutil disable
```

Boot the clone, then run:

```bash
sudo nvram boot-args="amfi_get_out_of_my_way=0x1"
sudo reboot
```

After reboot, record `csrutil status`, `nvram boot-args`, and:

```bash
codesign -dvvv --entitlements :- \
  "Steam Controller Bridge.app/Contents/Helpers/Steam Controller Bridge Virtual HID Helper.app"
```

Disconnect guest networking after artifact transfer. Do not enter Apple,
Steam, browser-sync, cloud-gaming, password-manager, or other personal
credentials.

Open System Settings > Game Controllers. In Terminal, set paths to the copied
release binaries, then run the fixed contract:

```bash
HELPER="$PWD/Steam Controller Bridge.app/Contents/Helpers/Steam Controller Bridge Virtual HID Helper.app/Contents/MacOS/sc-virtual-hid-helper"
SIMULATOR="$PWD/gamepad-simulator"

"$SIMULATOR" automated --output virtual-hid --virtual-hid-helper "$HELPER" --cycles 100 --interval-ms 100
```

The command logs `virtual_hid_ready` with `vendor_id=045e product_id=028e` and
runs long enough to inspect the system while reports change. During the run,
use a second Terminal window to capture:

```bash
hidutil list | grep -i -A 8 -B 2 'cafe\|4001\|045e\|028e\|Steam Controller Bridge Virtual Gamepad'
osascript -l JavaScript -e 'ObjC.import("GameController"); const cs=$.GCController.controllers; console.log("count="+cs.count); for (let i=0;i<cs.count;i++) console.log(ObjC.unwrap(cs.objectAtIndex(i).vendorName));'
```

Prove layers in order:

1. `hidutil list` shows `045e:028e`, Generic Desktop/Game Pad, and USB transport.
2. A HID monitor receives neutral, the 11 standard buttons, hats, axes, and
   triggers. The pinned Xbox report intentionally omits the five bridge
   extension buttons.
3. Neutral appears at startup, explicit stop, timeout, and shutdown.
4. Killing the parent removes the device without an orphan helper.
5. Record `GCController.controllers` and System Settings.
6. Only if either system surface recognizes the device, record browser
   `id`/`mapping`/axes/buttons/disconnect and a local SDL client's enumeration.
7. Only after simulator success, test a physical Steam Controller end to end.

For an intentional identity experiment, add both
`--virtual-hid-vendor-id VID` and `--virtual-hid-product-id PID`. This changes
only the identity; the fixed USB transport, descriptor, and report remain the
same. Never interpret a custom identity failure as a regression in the default
contract.

Update the matrix with captured evidence. Delete the lowered-security clone
after testing and retain only the clean baseline.
