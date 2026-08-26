# Architecture

The optional Rust `IOHIDUserDevice` output, helper isolation, and backend-switch
lifecycle are documented in [Experimental virtual HID](VIRTUAL_HID.md).

Steam Controller Bridge separates controller input, mapping, output, desktop
automation, and UI so each safety boundary has one owner.

```text
Steam Controller HID -> decode -> SteamControllerState
                                   |              |
                                   v              v
                                mapping     desktop profiles
                                   |          |        |
                                   v          v        v
simulator/replay ------------> GamepadState  macOS  finite pad tick
                                   |
                                   v
                           GamepadOutput
                                   |
                   +---------------+---------------+
                   |               |               |
            bridge device      dump/file         mock
                   |
                   v
              gamepad-facing HID

game/browser -> gamepad feedback -> bridge device -> protocol feedback
                                                 -> bridge runtime -> SC2 actuators
```

## Ownership

| Component | Responsibility |
| --- | --- |
| `gamepad-state` | Platform-neutral gamepad state and validation. |
| `bridge-protocol` | Integer wire format, framing, CRC, and stream recovery. |
| `bridge-output` | Output backends and the negotiated transport-neutral bridge session. |
| `linux-bridge-usb` | Exact official-bridge USB discovery, descriptor validation, and CDC interface transport. |
| `recording` | Versioned JSONL recording and deterministic or timed replay. |
| `steam-controller-device` | HID metadata, sessions, lifecycle events, and the exact write allowlist. |
| `steam-controller-protocol` | Steam Controller 2 report layouts and typed decode errors. |
| `controller-mapper` | Validated mapping profiles and stateful filters. |
| `desktop-bindings` | Profile storage, edge processing, desktop output, and pad feedback policy. |
| `steam-controller-discovery` | Candidate probing and unique active-source selection. |
| `profile-picker` | Profile-wheel hold, sector, paging, and input-suppression logic. |
| `bridge-core` | Hardware-independent decode/map/output lifecycle. |
| `bridge-runtime` | Live hardware ownership, discovery, safety cleanup, power policy, and status. |
| `menu-shell` | Platform clipboard, open/reveal, confirmation, and child-activation effects. |
| `power-monitor` | Typed sleep/wake notifications and acknowledgement ownership. |
| `release-updater` | Signed metadata, rollback policy, bounded downloads, staging, firmware-target catalog, and target-specific installation. |
| `controller-art`, `ui-theme` | Shared visual primitives without controller or profile policy. |
| CLI and GUI applications | Argument parsing, presentation, user actions, and composition of the crates above. |

The simulator, replay, probe, and bridge CLIs stay thin: they select an input
and output and delegate formats and lifecycle to library crates. The visualizer
owns diagnostic presentation and the Lizard Mouse Lab, but uses the same device,
decoder, mapper, and discovery layers as the runtime.

## Process boundaries

The menu app is a windowless `winit`/`tray-icon` process. The bindings editor,
profile overlay, and App Center are child modes of the same binary because each
`eframe` window owns its event loop.

The profile overlay is display-only. The parent makes every selection decision
and sends bounded, versioned JSON lines. Closing the wheel terminates and reaps
the child; owned pipe threads keep a wedged child off the menu event loop.

The App Center uses bidirectional bounded JSON lines. Safety requests carry a
request ID and child generation. The parent accepts requests only from the
current generation, validates the exact response type, and keeps suspension
ownership attached to that generation until synchronous recovery succeeds.
Presentation writes use a bounded non-blocking queue so they cannot delay bridge
recovery.

## Safety invariants

- Outputs reject invalid values; they do not serialize Rust object memory.
- Stop, timeout, disconnect, decode failure, and shutdown neutralize output
  before releasing hardware.
- Rumble uses a bounded host lease and an explicit safety zero. Pad haptics are
  finite ticks and failure degrades feedback without disabling input.
- Controller writes are limited to the exact supported Puck or Bluetooth
  collection and the fixed lizard, rumble, pad-tick, and power-off operations.
- Discovery preserves button and touch transitions under pressure and treats
  overflow as a lifecycle reset, never as permission to emit stale state.
- System sleep and application update are independent suspension owners. User
  start/stop intent is preserved while either owner keeps hardware closed.
- Firmware flashing begins only after acknowledged hardware release. The App
  Center cannot close and the tray cannot quit until resume or crash recovery
  completes.
- Release metadata is verified before parsing, cached atomically, and checked
  for rollback. Artifacts require the signed size and SHA-256 before use.
- A reported firmware target is only an implementation identity. Automatic
  recommendations, bootloader control, and successful reconnect verification
  require an exact match to a target in the signed updater path. Targetless,
  malformed, and different-target devices continue bridging without automatic
  update association.

## Platform boundary

Live controller HID is implemented on macOS and Linux. Desktop integration and
the shipped application remain macOS-specific; unsupported providers return
explicit errors instead of pretending live access exists. Unsafe platform code
is isolated behind safe typed APIs in `power-monitor` and the single
`USBDEVFS_DROP_PRIVILEGES` wrapper in `linux-bridge-usb`; other authored crates
retain the workspace prohibition.

Linux capability policy derives controller HID and bridge-device requirements
independently from active features. Its bridge probe checks an eligible serial
node or briefly opens the exact raw USB node to validate access and topology;
it claims no interface and retains no handle. An absent device is left to
normal discovery rather than reported as a permission failure.

The portable-core allowlist is enforced by
[`tools/check-portable-core.py`](../tools/check-portable-core.py). Platform
selection is permitted only in a facade root or its backend modules; ordinary
files added anywhere else in an allowlisted crate are checked automatically for
target configuration, native dependencies and APIs, and recognizable platform
data-path literals.

The Linux HID backend choice, VM evidence, and deferred hardware paths are
recorded in [the S0 Linux hardware-path decision](decisions/s0-linux-vm-hardware-path.md).

The core host depends on the public bridge-device protocol, not a board model.
Zero-configuration serial discovery uses the product marker
`Steam Controller Bridge`; Linux raw-USB discovery separately requires the
exact official identity and descriptor topology. Both paths require a
protocol-v1 Hello handshake. An explicit port bypasses serial marker and path
policy, but not Hello. The contract is documented in
[SERIAL_TRANSPORT.md](SERIAL_TRANSPORT.md) and
[GAMEPAD_PROTOCOL.md](GAMEPAD_PROTOCOL.md).

Seeed Studio XIAO nRF52840/Sense is the first project-supported firmware target
and the sole current entry in the updater catalog. Its CDC-to-Xbox reference
implementation, scheduling, and watchdog ownership remain documented in
[FIRMWARE_ARCHITECTURE.md](FIRMWARE_ARCHITECTURE.md). Another compatible serial
implementation can bridge without becoming an installable project target.
