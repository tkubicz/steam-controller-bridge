# Controller Mapping

`controller-mapper` converts a decoded `SteamControllerState` into the
platform-neutral `GamepadState`. The default profile is deliberately simple:
the right trackpad is an absolute right stick and no trackball inertia or action
layers are applied.

## Default controls

| Steam Controller 2 input | Generic output |
| --- | --- |
| Left stick | Left stick |
| Touched right trackpad | Right stick |
| A / B / X / Y | South / East / West / North |
| D-pad | Eight-way hat |
| Left / right trigger | Left / right trigger |
| Left / right bumper | Left / right shoulder |
| View / Menu / Steam | Back / Start / Guide |
| Left stick press | Left stick button |
| Right stick press or right pad click | Right stick button |
| L4 / R4 | Left grip / Right grip |
| L5 / R5 / Quick Access | Extra 1 / Extra 2 / Extra 3 |

Releasing the right trackpad immediately centers the generic right stick. A
profile can select the physical right stick instead with
`RightAxisSource::RightStick`.

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
