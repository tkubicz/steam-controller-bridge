# Desktop bindings

The macOS bridge can map L4, L5, R4, R5, Quick Access, and the left and right
pad clicks to held keyboard chords or left/right/middle/back/forward mouse
buttons. It can also use the right pad for relative pointer movement and the
left pad for two-axis smooth scrolling; a pad's click binding fires
independently of whether that pad function is enabled. While a pad is held
clicked its motion is frozen (see below), so the click action lands without
disturbing the pointer or scroll position. Stick clicks remain ordinary
Xbox controls; trigger clicks, pressure actions, and gestures are not mapped.
The feature is host-side and requires no protocol-v1 or XIAO firmware change.

## Menu app

Open `Profiles -> Edit Profiles…` to create, duplicate, rename, delete, and edit
profiles. Profile IDs never change when a display name is renamed. Save writes
the validated store atomically; Cancel discards the editor copy. The running
menu app reloads a valid replacement and keeps the previous store if a partial
or invalid file appears.

Selecting a profile applies it without restarting HID or serial. A switch
releases every old held output and ignores physical controls already held until
they are released. The gamepad output is parked at neutral for the length of the
switch. Profile reconfiguration and macOS input-sink lifecycle work run on a
dedicated bounded worker, so a slow window-server operation cannot stall the
thread feeding the XIAO; the neutral report remains the safety boundary while
that ordered work completes. Once authorized, the sink is retained across
profile switches, including an unbound profile, and is dropped only after a
backend failure or during worker shutdown. See
[PROFILE_OVERLAY.md](PROFILE_OVERLAY.md#why-suppression-must-be-exactly-neutral). Profiles can also be switched from the controller itself; see
[PROFILE_OVERLAY.md](PROFILE_OVERLAY.md).

When the profile wheel is enabled, it takes Quick Access over for the length of
a hold. A Quick Access binding still fires on a short press: the press edge is
withheld while the hold is being timed, and replayed as a tap if the button
comes up before the wheel opens. Once the wheel is open the binding is not
delivered at all — including for the press that closes it: a second Quick
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

Fresh stores contain one all-unbound `Default` profile. Both pad functions are
off by default. Pad feedback defaults to Medium and produces one finite tick
on each physical click even when the pointer or scroll function is disabled;
it can be disabled or set to Low (-36 dB), Medium (-30 dB), or High (-24 dB)
independently for each side. Right-pad pointer motion uses the captured lizard
mode's larger anchored stationary envelope: 2,560 counts at pad center, growing
to 3,584 at the rim, and re-parks after a 100 ms window without 384–768 counts
of position-aware net progress. Left-pad scrolling retains its more responsive 192-count center
envelope, growing to 2,048 counts at the edge, and its 150 ms stop window. A
resting finger's bounded wander around its anchor emits nothing; intentional
travel forwards only the excess beyond the current envelope.

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

Left-pad scrolling uses pixel-level smooth scroll events. Swipe velocity adds
up to 3x acceleration, the profile's 25%-300% speed setting scales the result,
and optional momentum decays after touch release. Speed defaults to 100% and
momentum defaults on; neither setting has any effect until left-pad scrolling
itself is enabled.

The version-4 schema
supports 1-32 profiles, trimmed unique names, letters, digits, F1-F24,
navigation/editing keys, punctuation, numpad keys, common media keys, the four
standard modifiers, and five mouse buttons. Modifier-only and raw-keycode
bindings are not representable. Enigo/macOS exposes native F1-F20 and volume
keys but no F21-F24 or play/previous/next media identities; using those portable
entries reports a binding-only degraded error rather than substituting another
key. A physical pad-click rising edge emits exactly one tick at that pad's
configured feedback strength. Holding or releasing it emits no additional
tick, and disabling pad feedback suppresses both click and movement ticks.

Version-1 through version-3 stores are migrated atomically without changing
profile IDs, names, button bindings, or existing pad enablement and feedback;
pad clicks start unbound. Pad click bindings live beside the button bindings as
`left_pad_click` and `right_pad_click` and use the same action shapes:

```json
"bindings": {
  "left_pad_click": { "kind": "key_chord", "key": "F5", "modifiers": ["command"] },
  "right_pad_click": { "kind": "mouse_button", "button": "middle" }
}
```

The pad section is:

```json
"pads": {
  "right_mouse": {
    "enabled": false,
    "feedback": { "enabled": true, "strength": "medium" },
    "speed_percent": 100
  },
  "left_scroll": {
    "enabled": false,
    "feedback": { "enabled": true, "strength": "medium" },
    "speed_percent": 100,
    "momentum": true
  }
}
```

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
duplicate bindings. Startup and reconnect establish a non-emitting baseline.
Stop, disconnect, shutdown, output failure, permission loss, profile change,
and transition-mailbox overflow perform best-effort release of every held
desktop output.

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
