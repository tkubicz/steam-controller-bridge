# Desktop bindings

The macOS bridge can map L4, L5, R4, R5, and Quick Access to held keyboard
chords or left/right/middle/back/forward mouse buttons. It can also use the
right pad for relative pointer movement and the left pad for two-axis smooth
scrolling. Stick clicks remain ordinary Xbox controls; trigger clicks, pad
clicks, pressure actions, and gestures are not mapped. The feature is host-side
and requires no protocol-v1 or XIAO firmware change.

## Menu app

Open `Bindings -> Edit Bindings…` to create, duplicate, rename, delete, and edit
profiles. Profile IDs never change when a display name is renamed. Save writes
the validated store atomically; Cancel discards the editor copy. The running
menu app reloads a valid replacement and keeps the previous store if a partial
or invalid file appears.

Selecting a profile applies it without restarting HID or serial. A switch
releases every old held output and ignores physical controls already held until
they are released. The gamepad output is parked at neutral for the length of the
switch: building or dropping the desktop-input sink is a synchronous
window-server operation on the thread that also feeds the XIAO, and a neutral
report disarms the firmware's controller-data watchdog so the delay cannot fault
the device. Switching to a profile with no bindings drops the sink and switching
away builds one, so both directions take that path. See
[PROFILE_OVERLAY.md](PROFILE_OVERLAY.md#why-suppression-must-be-exactly-neutral). Profiles can also be switched from the controller itself; see
[PROFILE_OVERLAY.md](PROFILE_OVERLAY.md).

When the profile wheel is enabled, it takes Quick Access over for the length of
a hold. A Quick Access binding still fires on a short press: the press edge is
withheld while the hold is being timed, and replayed as a tap if the button
comes up before the wheel opens. Once the wheel is open the binding is not
delivered at all. Nothing changes for L4, L5, R4, R5, or the pads, and nothing
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
off by default. Enabling a pad also enables Medium feedback by default; each
pad's feedback can be disabled or set to Low (-36 dB), Medium (-30 dB), or
High (-24 dB) independently. Pad motion first crosses a recentered 192-count
radial deadzone, preventing stationary coordinate drift from moving the cursor
or scrolling. Feedback uses side-specific `0x82` microticks after 768 counts of
net two-dimensional displacement. Its minimum interval scales from 450 ms for
slow travel to 80 ms for fast travel, so faster movement produces a denser
texture without allowing a stationary finger to tick. Reversing stationary
coordinate noise cancels instead of creating feedback, and rate-limited ticks
are discarded rather than delayed.

Left-pad scrolling uses pixel-level smooth scroll events. Swipe velocity adds
up to 3x acceleration, the profile's 25%-300% speed setting scales the result,
and optional momentum decays after touch release. Speed defaults to 100% and
momentum defaults on; neither setting has any effect until left-pad scrolling
itself is enabled.

The version-3 schema
supports 1-32 profiles, trimmed unique names, letters, digits, F1-F24,
navigation/editing keys, punctuation, numpad keys, common media keys, the four
standard modifiers, and five mouse buttons. Modifier-only and raw-keycode
bindings are not representable. Enigo/macOS exposes native F1-F20 and volume
keys but no F21-F24 or play/previous/next media identities; using those portable
entries reports a binding-only degraded error rather than substituting another
key.

Version-1 and version-2 stores are migrated atomically without changing profile
IDs, names, button bindings, or existing pad enablement and feedback. The pad
section is:

```json
"pads": {
  "right_mouse": {
    "enabled": false,
    "feedback": { "enabled": true, "strength": "medium" }
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
