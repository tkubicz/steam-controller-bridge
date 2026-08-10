# Lizard Mouse Capture and Comparison Lab

The Lizard Mouse Lab is a dedicated full-window mode in `sc-visualizer`. It
measures the Steam Controller's original desktop mouse behavior and compares it
with the current production implementation. It is a developer tool and is not
part of the menu-bar application.

Start `sc-visualizer`, then select **Lizard Mouse Lab** at the top of the
dashboard. The landing screen can create a guided capture, open an existing
capture, and select an installed binding profile. Profiles are read from the
normal bindings store without modification; `Default` is preferred, followed
by the first valid installed profile, then the built-in default with a warning.

Entering lab mode finishes an active ordinary recording, sends a neutral state,
disables visualizer output, and stops and joins the ordinary HID worker. The lab
then owns the controller through its lossless measurement path. Leaving the lab
restarts ordinary discovery with a fresh decoder and mapper. Enumeration and
open requests from both workers pass through one process-lifetime HID broker
thread; it retains no controller session while the lab owns the device, but
keeps macOS hidapi attached to one stable Core Foundation run loop.

## Capture

Fully stop the bridge and any process sending a lizard-off heartbeat, but leave
Steam's default controller mouse behavior available. Choose **New guided test**
and select a `NAME.jsonl` destination in the native Save dialog. The lab derives
`NAME-analysis.json` and `NAME-comparison.json` beside it.

The headless equivalent uses the same active-source discovery as the bridge and
visualizer, so no index lookup is normally needed:

```bash
cargo run -p sc-visualizer -- lizard capture --output lizard-usb-1.jsonl --guided
```

If more than one controller is active, disconnect all but one or use
`--index N` as an explicit path-sorted global HID index override.

The illustrated protocol contains center/four-corner stationary holds, separate
slow and fast swipes in eight directions, center/rim precision motion,
three-click trials, and click-drags in eight directions. Each trial has an
untimed instruction, a one-second countdown, a measured interval, a reference
preview, and Accept/Retry controls. Keyboard operation is emphasized so mouse
input does not contaminate a measurement. Do not touch another mouse or
trackpad during measured intervals. Headless free capture runs until Ctrl+C or
accepts `--duration-secs N`.

Retries remain in the JSONL. Additive marker fields identify the protocol,
trial, attempt, and `start`, `end`, `accepted`, or `discarded` phase. Analysis
uses exactly one accepted attempt for each required trial while retaining
compatibility with the earlier 13-stage marker format. Canceling, closing the
window, or leaving an incomplete protocol finalizes and preserves the capture
as invalid.

Capture is macOS-only. Grant Input Monitoring to the terminal or the app hosting
`sc-visualizer` and relaunch it before recording. The preflight view reports the
controller, state and `0x40` activity, event-tap readiness, display setup, and
output path. The tool observes Core Graphics at the HID entry
tap in listen-only mode; it does not suppress, replace, or post those events.
The JSONL includes raw HID, decoded states, decoded `0x40` reports, connection
events, transport, OS/build, display geometry, mouse scaling, tool version, and
passively observed pointer deltas/locations.

The two producer streams feed an 8,192-event bounded queue and are sorted through
a 10 ms timestamp window. Queue overflow, controller disconnect, event-tap
disable, and missing preflight state or `0x40` reports write final invalid
metadata and make the command fail. An absent `0x40` stream usually means Steam
or another three-second heartbeat disabled lizard mode.

Collect at least three guided runs for every transport being qualified. Linux
builds can open, analyze, and compare recordings, but cannot capture or replay
to the desktop.

## Analyze and compare

```bash
cargo run -p sc-visualizer -- lizard analyze lizard-usb-1.jsonl --output analysis.json
cargo run -p sc-visualizer -- lizard compare lizard-usb-1.jsonl --output comparison.json
cargo run -p sc-visualizer -- lizard compare lizard-usb-1.jsonl \
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

The GUI performs analysis and comparison in the background after capture. It
always keeps the JSONL even when report creation fails and offers a retry for
report writes. Opening an existing recording analyzes it in memory and requires
an explicit export action, so sibling files are not overwritten silently. The
results are descriptive rather than pass/fail: they show validity and
contamination warnings, leakage, latency, errors and path ratios, speed and
angular response, and selectable normalized reference/candidate trajectories.

## Replay safety

```bash
cargo run -p sc-visualizer -- lizard replay lizard-usb-1.jsonl \
  --source reference --output dump --speed 1
```

Dump output is the default and does not inject anything. Explicit macOS
`--output desktop` injects relative pointer motion through the production sink.
The wrapper discards every keyboard, modifier, mouse-button, and scroll action,
including actions present in lizard packets or a selected profile.
