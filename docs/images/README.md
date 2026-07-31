# Screenshots and photos

The README references the files below. They are not committed yet — this note is
the shot list and the redaction checklist for capturing them.

Keep each file under roughly 500 KB. Use PNG for screenshots, JPEG for photos,
and GIF for motion. Do **not** use `<video>`: GitHub only plays video that was
uploaded to a comment or release, not files committed to a repository.

## Shot list

In priority order. The first two carry most of the value.

### 1. `hardware-topology.jpg` — the whole setup, cabled

Steam Controller 2, official Puck, Mac, and XIAO nRF52840, all connected, ideally
with the XIAO's LED showing solid green. This is what makes the project legible
at a glance; the ASCII diagram in the README cannot do the same job.

Shoot from slightly above on a plain surface, in even daylight. Landscape,
roughly 3:2, and wide enough that all four devices and the cables between them
are visible.

### 2. `gamepad-tester.gif` — it actually works

Screen recording of <https://hardwaretester.com/gamepad> while moving both
sticks, pulling both triggers, and pressing the face buttons. Keep
`CONNECTED: Yes` and `MAPPING: standard` in frame the whole time.

For a controller project, motion is far more convincing than any still. Keep it
to 5–10 seconds and loop it. Capture with macOS Screenshot (Shift-Cmd-5),
then convert:

```bash
ffmpeg -i recording.mov -vf "fps=12,scale=800:-1:flags=lanczos" -loop 0 gamepad-tester.gif
```

Check the size afterwards; drop to `fps=10` or `scale=640` if it exceeds ~500 KB.

### 3. `menu-bar.png` — the primary UI

The menu-bar dropdown open at `Running`, showing input transport, controller,
XIAO, battery, and haptics. Capture a window shot with Shift-Cmd-4 then Space so
the menu keeps its shadow and rounded corners.

### 4. `visualizer.png` — the protocol work

`sc-visualizer` with raw, decoded, and mapped state visible side by side, and the
connection/rate diagnostics in frame. This is the best evidence of the decoding
work behind the project.

## Redaction checklist

Check every image before committing. Serials are masked in the applications as of
v0.1.0, but screenshots can still capture identifiers from elsewhere on screen.

- [ ] **macOS Bluetooth settings** — the pairing name `Steam Ctrl (BT) FXA…`
      embeds part of the controller's MAC address. Keep that panel out of frame.
- [ ] **`sc-probe capture` output** — capture and recording files intentionally
      keep full serials so they stay replayable. Never screenshot their contents.
- [ ] **Terminal prompts** — crop or shorten a prompt that shows a home directory
      or host name.
- [ ] **Browser chrome** — bookmarks, other tabs, profile name, and any account
      avatar in the gamepad-tester shot.
- [ ] **Notifications and menu-bar extras** — turn on Do Not Disturb before
      capturing anything full-screen.
- [ ] **Physical labels** — a serial sticker or hand-written marking on the Puck,
      controller, or XIAO in the topology photo.

Confirm the whole set at a glance:

```bash
# Everything should read well under 500 KB.
ls -lh docs/images/
```
