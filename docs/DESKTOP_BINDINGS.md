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
they are released. At launch, the menu app queries Input Monitoring directly
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
pad's feedback can be disabled or set to Low, Medium, or High independently.
The version-2 schema
supports 1-32 profiles, trimmed unique names, letters, digits, F1-F24,
navigation/editing keys, punctuation, numpad keys, common media keys, the four
standard modifiers, and five mouse buttons. Modifier-only and raw-keycode
bindings are not representable. Enigo/macOS exposes native F1-F20 and volume
keys but no F21-F24 or play/previous/next media identities; using those portable
entries reports a binding-only degraded error rather than substituting another
key.

Version-1 stores are migrated atomically without changing profile IDs, names,
or button bindings. The new profile section is:

```json
"pads": {
  "right_mouse": {
    "enabled": false,
    "feedback": { "enabled": true, "strength": "medium" }
  },
  "left_scroll": {
    "enabled": false,
    "feedback": { "enabled": true, "strength": "medium" }
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
