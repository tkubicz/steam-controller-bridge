# Security policy

## Supported versions

Only the latest published release receives security fixes.

| Version | Supported |
| --- | --- |
| Latest GitHub release | Yes |
| Older releases | No |

## Reporting a vulnerability

Report privately through
[GitHub's security advisory form](https://github.com/tkubicz/steam-controller-bridge/security/advisories/new)
rather than opening a public issue. Expect an initial response within a week.

Useful details: the version or commit, your macOS version, how the controller was
connected, and the smallest reproduction you have. Diagnostics from the menu-bar
app are safe to include - hardware serials are masked. `sc-probe capture` files
are not: they deliberately keep the full serial so recordings stay replayable,
and on Bluetooth that value is the controller's MAC address.

## Scope

This tool takes direct HID-level control of connected hardware, so the following
are the areas most worth scrutiny.

**In scope**

- Any path that writes to a controller outside the four permitted operations:
  the fixed SDL-compatible lizard-off feature report, fixed power-off feature
  report, exact standard dual-rumble output report, and finite SDL Triton
  `0x82` pad tick. Every write is gated on an exact vendor, product, usage,
  interface, and transport match.
- A way to leave an actuator running after the effect stops, the client closes,
  the bridge exits, or either endpoint disconnects.
- A way to leave the controller with desktop keyboard and pointer input
  suppressed after the bridge exits.
- Parsing flaws in the serial protocol, the Steam Controller report decoder, or
  the firmware frame parser - including anything reachable from malformed input
  that panics, hangs, or corrupts state.
- Leaking hardware identifiers through a path documented as masked: status
  output, logs, or **Copy Diagnostics**.
- Anything that lets the firmware accept host frames without a completed
  protocol-v1 Hello handshake.

**Out of scope**

- The Xbox 360 compatibility USB identity (`045e:028e`). This is a documented,
  deliberate choice; see the README.
- The macOS application not being notarized. It is ad-hoc signed by design at
  this stage, and the README and user guide say so.
- Requiring Input Monitoring permission, or requiring that Steam be fully quit.
  Both are inherent to how macOS arbitrates HID access.
- Full serials inside `sc-probe capture` and recording files. That is documented
  behaviour, needed for replay.
- Advisories affecting dependencies that no shipped macOS binary links - the
  Wayland stack behind `eframe`/`winit` and the GTK3 tray backend behind
  `tray-icon`. `deny.toml` scopes the audit to macOS for this reason.
