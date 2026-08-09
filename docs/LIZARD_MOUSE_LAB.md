# Lizard Mouse Capture and Comparison Lab

`sc-lizard-lab` measures the Steam Controller's original desktop mouse behavior
and compares it with the current production implementation. It is a developer
CLI and is not bundled in the menu-bar application.

## Capture

Fully stop the bridge and any process sending a lizard-off heartbeat, but leave
Steam's default controller mouse behavior available. The lab uses the same
active-source discovery as the bridge and visualizer, so no index lookup is
needed:

```bash
cargo run -p sc-lizard-lab -- capture --output lizard-usb-1.jsonl --guided
```

If more than one controller is active, disconnect all but one or use
`--index N` as an explicit path-sorted global HID index override.

The guided sequence marks center/four-corner holds, cardinal and diagonal slow
and fast swipes, center/rim precision motion, clicks, and click-drags. Do not
touch any other mouse or trackpad during it. Free capture either runs until
Ctrl+C or accepts `--duration-secs N`.

Capture is macOS-only. Grant Input Monitoring to the terminal or host app and
relaunch it before recording. The tool observes Core Graphics at the HID entry
tap in listen-only mode; it does not suppress, replace, or post those events.
The JSONL includes raw HID, decoded states, decoded `0x40` reports, connection
events, transport, OS/build, display geometry, mouse scaling, tool version, and
passively observed pointer deltas/locations.

The two producer streams feed an 8,192-event bounded queue and are sorted through
a 10 ms timestamp window. Queue overflow, controller disconnect, event-tap
disable, and missing preflight state or `0x40` reports write final invalid
metadata and make the command fail. An absent `0x40` stream usually means Steam
or another three-second heartbeat disabled lizard mode.

Collect at least three guided runs for every transport being qualified.

## Analyze and compare

```bash
cargo run -p sc-lizard-lab -- analyze lizard-usb-1.jsonl --output analysis.json
cargo run -p sc-lizard-lab -- compare lizard-usb-1.jsonl --output comparison.json
cargo run -p sc-lizard-lab -- compare lizard-usb-1.jsonl \
  --output custom.json --profile bindings.json --profile-name "Desktop"
```

Analysis accepts both lab captures and older v1 visualizer recordings. When
typed lizard events are absent it decodes raw `0x40` packets. Reports include
cadence, touch sessions, response latency, stationary and click leakage, speed
response, direction error, host acceleration ratio, unmatched host movement,
and cursor-edge clipping. Guided captures additionally report every named stage
between its start/end markers, including reference path, input ratio, click
count, and host path. Host-derived fields explicitly remain unavailable for old
recordings.

Comparison clears keyboard/button bindings and left-pad scrolling, enables only
the selected profile's right-pad mouse function, and replays states at their
original timestamps. It compares cumulative reference and bridge trajectories
at state timestamps; it never assumes a one-to-one `0x40`/state relationship.
For guided captures, the headline RMS resets at each stage boundary so movement
while the operator prepares the next stage cannot contaminate the result.

## Replay safety

```bash
cargo run -p sc-lizard-lab -- replay lizard-usb-1.jsonl \
  --source reference --output dump --speed 1
```

Dump output is the default and does not inject anything. Explicit macOS
`--output desktop` injects relative pointer motion through the production sink.
The wrapper discards every keyboard, modifier, mouse-button, and scroll action,
including actions present in lizard packets or a selected profile.
