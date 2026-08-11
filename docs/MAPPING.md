# Controller Mapping

`controller-mapper` converts a decoded `SteamControllerState` into the
platform-neutral `GamepadState`. The default profile maps the two physical
sticks directly and leaves the right trackpad out of the generic gamepad axes.

## Default controls

| Steam Controller 2 input | Generic output |
| --- | --- |
| Left stick | Left stick |
| Right stick | Right stick |
| A / B / X / Y | South / East / West / North |
| D-pad | Eight-way hat |
| Left / right trigger | Left / right trigger |
| Left / right bumper | Left / right shoulder |
| Physical View (left) / Menu (right) / Steam | Back / Start / Guide |
| Left stick press | Left stick button |
| Right stick press | Right stick button |
| L4 / R4 | Left grip / Right grip |
| L5 / R5 / Quick Access | Extra 1 / Extra 2 / Extra 3 |

Protocol v1 preserves all five extra output bits, but the standard Xbox report
has no corresponding controls and therefore cannot expose them to games. On
macOS, the live bridge independently observes the decoded source bits and may
map L4/L5/R4/R5/Quick Access and the left/right pad clicks to opt-in desktop
keyboard chords or mouse buttons.
It may also use the right pad as a relative pointer and the left pad for smooth
two-axis scrolling. These host-side paths do not change `GamepadState`, serial
frames, or firmware behavior. See [Desktop bindings](DESKTOP_BINDINGS.md).

An alternate diagnostic profile can map a touched right trackpad to the generic
right stick with `RightAxisSource::RightPad`; in that profile, releasing the pad
centers the axis and a pad click acts as the right-stick button. This is not the
gameplay default. In that diagnostic profile a desktop right-pad-click binding
fires alongside the right-stick button - the same host/gamepad concurrency the
grip and Quick Access bindings already have.

The physical buttons follow Xbox conventions: the left View button is Back and
the right Menu button is Start. The Triton source-bit names are counterintuitive
and reversed at this boundary: source `VIEW` is the physical Menu/Start button,
while source `MENU` is the physical View/Back button. The implementation follows
SDL's SC2 driver and OpenPuck's XInput mapping.

## Axis and trigger rules

Stick and pad `i16` values are normalized to `[-1, 1]`. Radial dead zones
preserve direction, smoothly rescale the remaining range, and cap diagonal
magnitude at one. Trigger values are normalized to `[0, 1]`; the default full
scale is `0x8000`, matching OpenPuck's observation that full pulls top out near
that value. `trigger_full_scale` is configurable so captures can calibrate a
specific controller.

The built-in pipeline is:

```text
source mapping
  -> optional axis inversion
  -> radial stick dead zones and trigger dead zone
  -> sensitivity curve
  -> saturation
  -> optional continuous-axis low-pass smoothing
  -> finite-value sanitization and output clamping
```

Buttons and the hat are never smoothed. Call `ControllerMapper::reset` when the
source disconnects or its state becomes uncertain; this clears smoothing
history so reconnecting cannot inherit an old axis or trigger value. Invalid
profiles are rejected at construction rather than silently used.

`AxisDeadZoneFilter` is also exposed for profiles that prefer independent
per-axis dead zones over the mapper's default radial behavior. Every filter
implements `StateFilter` and can be reused independently.
