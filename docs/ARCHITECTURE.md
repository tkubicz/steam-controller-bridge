# Architecture

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
                         framed serial protocol
                                   |
                                   v
                          XIAO Xbox gamepad USB

game/browser -> Xbox rumble OUT -> XIAO -> CDC feedback -> bridge runtime
                                                        -> SC2 actuators
```

## Ownership

| Component | Responsibility |
| --- | --- |
| `gamepad-state` | Platform-neutral gamepad state and validation. |
| `bridge-protocol` | Integer wire format, framing, CRC, and stream recovery. |
| `bridge-output` | Output backends and the negotiated serial session. |
| `recording` | Versioned JSONL recording and deterministic or timed replay. |
| `steam-controller-device` | HID metadata, sessions, lifecycle events, and the exact write allowlist. |
| `steam-controller-protocol` | Steam Controller 2 report layouts and typed decode errors. |
| `controller-mapper` | Validated mapping profiles and stateful filters. |
| `desktop-bindings` | Profile storage, edge processing, desktop output, and pad feedback policy. |
| `steam-controller-discovery` | Candidate probing and unique active-source selection. |
| `profile-picker` | Profile-wheel hold, sector, paging, and input-suppression logic. |
| `bridge-core` | Hardware-independent decode/map/output lifecycle. |
| `bridge-runtime` | Live hardware ownership, discovery, safety cleanup, power policy, and status. |
| `macos-power-monitor` | Typed macOS sleep/wake notifications and acknowledgement ownership. |
| `release-updater` | Signed metadata, rollback policy, bounded downloads, staging, and firmware flashing. |
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

## Platform boundary

Live HID and desktop integration are macOS-specific. Shared crates compile on
other targets and return explicit unsupported-platform errors instead of
pretending live access exists. All project-authored crates forbid unsafe code
except `macos-power-monitor`, whose documented IOKit/Core Foundation ownership
is isolated behind a safe typed API.

The XIAO firmware provides CDC transport plus the Xbox-compatible USB gamepad.
Its framing contract is documented in [GAMEPAD_PROTOCOL.md](GAMEPAD_PROTOCOL.md);
firmware scheduling and watchdog ownership are documented in
[FIRMWARE_ARCHITECTURE.md](FIRMWARE_ARCHITECTURE.md).
