# Desktop bindings

The macOS bridge can map L4, L5, R4, R5, and Quick Access to held keyboard
chords or left/right/middle/back/forward mouse buttons.

Each trackpad is configured independently and owns no fixed behavior. A pad has
a **motion mode** - none, relative pointer, or two-axis smooth scrolling - and a
list of **regions**, areas of its surface that carry their own click and touch
actions. Either pad can take either motion mode, and region actions fire whether
or not that pad drives motion. While a pad is held clicked its motion is frozen
(see below), so the click action lands without disturbing the pointer or scroll
position. Stick clicks remain ordinary Xbox controls; trigger clicks and
pressure actions are not mapped. The feature is host-side and requires no
protocol-v1 or bridge-device firmware change.

## Regions and triggers

A region is a bearing sweep crossed with an extent band: a start bearing, a
sweep, and an inner/outer extent. Zero degrees points at twelve o'clock and
bearings increase clockwise, the same convention `profile_picker::sector_for`
uses for the profile wheel. Four integers therefore express a whole pad, a half,
a quadrant, a corner, a centre square, or an edge frame; the editor ships presets
for the common compass layouts.

**Extent is measured to the pad's square edge, not as a circle.** The pads are
rounded squares and each axis reaches full scale independently, so extent is the
Chebyshev distance `max(|x|, |y|)` - the same metric the motion filter's
`position_aware_threshold` already uses to grow its dead zones toward the rim.
100% is therefore the pad boundary in every direction, corners included; a centre
region is a square; and an edge band is a frame rather than a ring. A Euclidean
radius would describe the disc inscribed in the pad instead, putting the corners
at about 141% and leaving an "edge" band that bulges into the middle along the
diagonals while missing the edge midpoints entirely.

Regions are an **ordered list, resolved first-match-wins**. Overlap is legal and
is how a centre region listed ahead of a whole-pad region shadows it, so a layout
the user draws is never rejected for overlapping itself. A pad supports up to 16
regions and starts with none.

Each region binds two triggers, which differ deliberately:

- **Click** resolves its region once, at the press, and holds that action until
  the pad is physically released. Sliding during a held click cannot swap the
  action. The region is read from the pressure-freeze anchor rather than the
  click bit, because pressure crosses its threshold tens of milliseconds earlier
  and so precedes most of the fingertip roll.
- **Touch** follows the finger: crossing into a different region releases the old
  action and presses the new one, and lifting off releases. Boundaries carry a
  4%/6-degree hysteresis margin around the region currently occupied, so a finger
  resting on a seam holds one action instead of alternating between two.

Region tracking freezes while the pad is effectively pressed and through the
release guard, because a flattening or un-flattening fingertip's reported
centroid wanders far enough to cross seams on its own.

Gesture support - swipe, rotate, tap - is not implemented. When it lands it
becomes further `PadTrigger` variants and further `PadEvent` arms in
`desktop-bindings`, dispatched through the same reference-counted press/release
path as everything above, rather than a second binding mechanism.

## Menu app

Open `Profiles -> Edit Profiles…` to create, duplicate, rename, delete, and edit
profiles. Profile IDs never change when a display name is renamed. Save writes
the validated store atomically; Cancel discards the editor copy. The running
menu app reloads a valid replacement and keeps the previous store if a partial
or invalid file appears.

Selecting a pad opens its motion mode, speed, momentum, and feedback, then its
regions: a layout preset that replaces the list, a map of the current regions
drawn on that pad's own shape and cant, and the ordered list itself. Selecting a
region opens its name, its four shape numbers, and its click and touch actions,
which use the same action picker as the buttons. The list order is the resolution
order, so a region can be raised above the ones it overlaps.

Selecting a profile applies it without restarting HID or serial. A switch
releases every old held output and ignores physical controls already held until
they are released. The gamepad output is parked at neutral for the length of the
switch. Profile reconfiguration and macOS input-sink lifecycle work run on a
dedicated bounded worker, so a slow window-server operation cannot stall the
thread feeding gamepad output; the neutral report remains the safety boundary while
that ordered work completes. Once authorized, the sink is retained across
profile switches, including an unbound profile, and is dropped only after a
backend failure or during worker shutdown. See
[PROFILE_OVERLAY.md](PROFILE_OVERLAY.md#why-suppression-must-be-exactly-neutral). Profiles can also be switched from the controller itself; see
[PROFILE_OVERLAY.md](PROFILE_OVERLAY.md).

When the profile wheel is enabled, it takes Quick Access over for the length of
a hold. A Quick Access binding still fires on a short press: the press edge is
withheld while the hold is being timed, and replayed as a tap if the button
comes up before the wheel opens. Once the wheel is open the binding is not
delivered at all - including for the press that closes it: a second Quick
Access press cancels the wheel and stays masked until released, so cancelling
can never fire the binding. Nothing changes for L4, L5, R4, R5, or the pads, and nothing
changes at all while the wheel is disabled. At launch, the menu app queries Input Monitoring directly
and uses `IOHIDRequestAccess` when macOS has not decided yet. Only after that
grant is detected does it request Post Event and Accessibility access, before a
profile needs any bindings. This works even when no controller is attached and
adds the app to the Privacy & Security lists without using the manual `+` file
picker. After the user grants access, the bridge detects it and enables bindings
without a restart. `Request Permissions…` repeats the ordered checks, and `Open
Accessibility Settings` remains available for a previously denied request.

Profiles live at:

```text
~/Library/Application Support/Steam Controller Bridge/bindings.json
```

Fresh stores contain one all-unbound `Default` profile. Both pads default to no
motion mode and no regions. Pad feedback defaults to Medium and produces one
finite tick on each physical click even when the pad has no motion mode; it can
be disabled or set to Low (-36 dB), Medium (-30 dB), or High (-24 dB)
independently for each side. A touch crossing into a region that binds a touch
action also ticks, rate-limited to the movement texture's fastest 80 ms interval
so boundary traffic cannot flood the actuator.

Motion thresholds follow the mode, not the side. Pointer motion uses the
captured lizard mode's larger anchored stationary envelope: 2,560 counts at pad
center, growing to 3,584 at the rim, and re-parks after a 100 ms window without
384-768 counts of position-aware net progress. Scrolling retains its more
responsive 192-count center envelope, growing to 2,048 counts at the edge, and
its 150 ms stop window. A resting finger's bounded wander around its anchor emits
nothing; intentional travel forwards only the excess beyond the current envelope.

A pad press freezes that pad's motion. The supplied raw capture shows the
reported centroid wandering hundreds to thousands of counts as a fingertip
flattens and rolls. The freeze keys on analog pressure as well as the click bit:
the 1,600-count pressure crossing led the click bit by roughly 29-153 ms in the
recorded right-pad presses, so it catches most of the approach roll. A light
press that never actuates the switch also freezes, with hysteresis on the way
out. The physical press edge establishes a fresh drag anchor; deliberately
travelling more than 2,800 counts from there enters a drag. A paused drag uses
the smaller stop-progress envelope when it resumes instead of requiring the
full contact or drag threshold again. Physical release immediately cancels drag motion and
scroll momentum, suppresses residual pressure until it drops below the exit
threshold, and guards motion for 250 ms so the un-flattening tail stays silent.

Pointer movement follows the measured lizard-mode linear transfer of 128 raw
counts per pointer pixel, multiplied only by the per-profile 25%-300% Pointer
speed setting. The lab capture records both raw `0x40` reference output and
passive host cursor events; final feel remains a physical acceptance check.
Feedback tracks accepted finger travel and uses side-specific
`0x82` microticks after 768 counts of
net two-dimensional displacement. Its minimum interval scales from 450 ms for
slow travel to 80 ms for fast travel, so faster movement produces a denser
texture without allowing a stationary finger to tick. Parked or press-frozen
coordinate noise is discarded before feedback accounting, and rate-limited
ticks are discarded rather than delayed.

Scrolling uses pixel-level smooth scroll events. Swipe velocity adds up to 3x
acceleration, the profile's 25%-300% speed setting scales the result, and
optional momentum decays after touch release. Speed defaults to 100% and momentum
defaults on; both are kept while a pad's motion mode is `none`, so turning a mode
back on restores the tuning rather than the defaults.

The version-5 schema
supports 1-32 profiles, trimmed unique names, letters, digits, F1-F24,
navigation/editing keys, punctuation, numpad keys, common media keys, the four
standard modifiers, and five mouse buttons. Modifier-only and raw-keycode
bindings are not representable. Enigo/macOS exposes native F1-F20 and volume
keys but no F21-F24 or play/previous/next media identities; using those portable
entries reports a binding-only degraded error rather than substituting another
key. A physical pad-click rising edge emits exactly one tick at that pad's
configured feedback strength. Holding or releasing it emits no additional
tick, and disabling pad feedback suppresses click, touch, and movement ticks.

Each pad is one object with its motion mode, its shared speed and momentum
settings, its feedback, and its ordered region list:

```json
"pads": {
  "left": {
    "motion": "scroll",
    "speed_percent": 100,
    "momentum": true,
    "feedback": { "enabled": true, "strength": "medium" },
    "regions": []
  },
  "right": {
    "motion": "pointer",
    "speed_percent": 100,
    "momentum": true,
    "feedback": { "enabled": true, "strength": "medium" },
    "regions": [
      {
        "id": "region-1",
        "name": "Left",
        "shape": {
          "start_degrees": 248,
          "sweep_degrees": 45,
          "inner_percent": 0,
          "outer_percent": 100
        },
        "click": { "kind": "key_chord", "key": "ArrowLeft", "modifiers": [] },
        "touch": null
      }
    ]
  }
}
```

Validation rejects a version other than the current one, a pad speed outside
25%-300%, more than 16 regions on a pad, a region ID that is not an ASCII slug,
an untrimmed, empty, or over-32-character region name, a duplicate region ID or
name within a pad, a sweep outside 1-360 degrees, a start bearing of 360 or more,
and an extent band whose inner edge is not below its outer edge.

## Migration

Version 1 through version 4 stores are read through a frozen mirror of the old
schema and converted in place; the rewrite is atomic and changes no profile ID,
name, or button binding. Because the store denies unknown fields, a document that
predates regions cannot deserialize into the current shape at all, so this is a
real conversion rather than a version-number bump.

| Version 4 | Version 5 |
| --- | --- |
| `pads.right_mouse.enabled` | `pads.right.motion` becomes `pointer`, else `none` |
| `pads.left_scroll.enabled` | `pads.left.motion` becomes `scroll`, else `none` |
| `speed_percent`, `momentum`, `feedback` | carried onto the matching pad verbatim |
| `bindings.left_pad_click` | one whole-pad region on the left pad, bound on click |
| `bindings.right_pad_click` | the same on the right pad |
| an unbound pad click | no regions on that pad |

## CLI

Desktop injection is opt-in and requires both options:

```bash
./sc-bridge \
  --bindings "$HOME/Library/Application Support/Steam Controller Bridge/bindings.json" \
  --profile "Default"
```

`--profile` selects the display name case-insensitively. Replay rejects either
option before opening a recording, so replay cannot inject desktop input.

## Edge and failure behavior

Physical press emits output-down once and physical release emits output-up.
Keys, modifiers, and mouse buttons are reference-counted across overlapping or
duplicate bindings, which is also what lets a pad's click and touch actions, or
two pads' actions, hold the same key at once without one release cancelling the
other. Startup and reconnect establish a non-emitting baseline. Stop, disconnect,
shutdown, output failure, permission loss, profile change, and
transition-mailbox overflow perform best-effort release of every held desktop
output, including both region latches on each pad. A pad touched or clicked
across a profile switch or a sink failure stays inert until it is physically
released.

A pad holds at most one click action and at most one touch action at a time. A
touch hand-off releases the region it is leaving before it presses the one it is
entering, so the two are never held together.

Desktop-output failures increment separate status counters and disable only the
binding sink. Standard Xbox output, rumble, lizard suppression, recording,
automatic shutdown, and reconnect continue independently. Logs record profile,
state, held-output count in snapshots, bounded last error, and failure changes;
they never log each injected key.

Pad movement establishes a fresh baseline on touch, reconnect, profile change,
or recovery, so it cannot jump the pointer. Pad feedback uses finite SDL Triton
`0x82` tick commands and never sends an artificial stop. Failed tick writes use
their own 500 ms backoff: pointer/scroll output and ordinary game rumble remain
operational.
