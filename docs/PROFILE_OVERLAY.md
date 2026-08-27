# Profile Overlay

The profile overlay is a radial wheel that lets a binding profile be switched
from the controller, without leaving the game. It is off by default and is
enabled from the menu bar under **Profile Wheel**.

## The gesture

| Step | Control |
| --- | --- |
| Open | Hold **Quick Access** for the configured hold (2 or 3 seconds) |
| Choose | Point either stick at a sector |
| Apply | **A** |
| Cancel | **B**, or press Quick Access again |
| Page | **L1** / **R1**, only when there are more profiles than one wheel holds |

The wheel stays open after Quick Access is released, so the selection can be
made unhurried. A sector stays selected once the stick recentres: flick, let go,
then reach for A.

A **short** press of Quick Access is not the gesture, and still does whatever
the active profile binds Quick Access to. The press is withheld while the hold
is being timed and delivered as a tap if the button comes up early, so a binding
never fires just because the user started a hold.

The wheel refuses to open with fewer than two profiles, and in that case Quick
Access behaves exactly as it does with the feature switched off.

## What the game sees

While the wheel is open the game sees a **fully neutral** gamepad: every stick
centred, every button released, every trigger at rest. Suppression is applied to
the mapped state in `bridge-core`, so the unchanged-output dedupe and the
metrics see exactly what the game sees.

It is all-or-nothing, and that is a protocol requirement rather than a
simplification - see [Why suppression must be exactly neutral](#why-suppression-must-be-exactly-neutral).
It is also the right semantics: someone in a menu is not playing, so a trigger
they happen to be holding should not keep firing.

Desktop bindings on controls the wheel does not consume - the grips and the
pads - stay live while it is open. That is deliberate: suppressing them would
recreate on close exactly the fresh-edge leak the button latch below exists to
prevent, for controls the wheel never reads in the first place.

Two consequences are deliberate and worth knowing:

- The hold itself is **not** suppressed. The game sees `Extra3` held for the
  full hold duration before the wheel opens and clears it. Suppressing from the
  press edge would mean delaying every Quick Access press by the hold.
- Suppression begins on the report **after** the wheel opens, about 4 ms later,
  because the mapped state for a report is sent before the picker has seen it.

### The button that closed the wheel is held back until released

The wheel closes on a **press**, not a release, so whatever closed it - A, B, or
a second Quick Access - is still physically down on the next report. Simply
lifting suppression there sent that press straight into the game: a press the
user aimed at the overlay, arriving as an in-game action a few milliseconds
later.

So on closing, the controls the wheel consumes that are still held become
`OutputSuppression::Buttons` and stay withheld until the user lets go. The
sticks and everything else come back immediately - play resumes at once; only
the button that is still down is withheld, and only until it is not.

This is the one place a **partial** mask is correct rather than dangerous. The
rule in `OutputSuppression` is about *pinned* states: once the wheel is closed
the state tracks the controller again, so it either keeps changing or settles at
exactly neutral when the user lets go. Neither leaves the watchdog armed against
a silent host.

A forced close - a controller that disappeared - latches nothing, since no
further report would ever arrive to clear it. Closing the wheel by
reconfiguring or disabling it is the opposite case: reports keep arriving, so
the still-held controls (and a hold in flight) are latched and drain as usual.

The latch also covers the bindings side: the trigger stays masked from
desktop bindings while it is latched, so dismissing the wheel with a second
Quick Access press cannot fire the very binding the wheel exists to protect.

## Why suppression must be exactly neutral

The firmware arms its 100 ms controller-data watchdog only for reports that are
**not** exactly neutral (`BridgeSession::apply_gamepad`), and the host stops
refreshing unchanged state once it has sent a neutral one (`flush_states` in
`bridge-output`). The two rules fit together: an active state is refreshed every
25 ms and keeps the watchdog fed, and a neutral state needs no heartbeat because
the watchdog is disarmed.

A **partially** suppressed state breaks that fit. It is pinned, so the host has
nothing new to send and falls back to the 25 ms refresh, but it is still
non-neutral, so the watchdog stays armed - leaving only a 4× margin. Anything
that stalls the host thread past 100 ms then faults the device, which flashes
red until traffic resumes. Nothing is logged, because no write ever failed.

The same hazard applies to any slow work on the thread that feeds gamepad output.
macOS input-sink construction and destruction are window-server operations
whose duration this side does not control, so `bridge-runtime` owns them on a
dedicated worker. `neutralize_before_desktop_work` still parks the device at
neutral before an ordered profile operation, which makes even a blocked worker
irrelevant to the firmware watchdog. An authorized sink is retained through
ordinary and unbound profile switches; backend failure and shutdown are the
places that destroy it.

If you ever add work to the supervisor loop that can block, park the output
first. On the reference XIAO firmware, the symptom of getting this wrong is a
red-flashing device and a clean log.

## Layout

Sector 0 sits at twelve o'clock and sectors run clockwise, in
`profile_picker::sector_for` and in the overlay's drawing alike. A wheel holds
`PickerConfig::sectors_per_page` profiles, eight by default, which is a
45-degree arc each. A roster larger than one wheel is split into pages; the last
page may be short, and its arcs widen to fill the circle.

The stick must be pushed past `engage_dead_zone` (0.55) to start steering and
must fall back below `track_dead_zone` (0.35) to stop. Whichever stick is pushed
further wins, so resting a thumb on the other one cannot fight it.

## Where the parts live

```text
HID report
  |
  v
bridge-core: decode, map, send to gamepad output
  ^                    |
  |                    v
suppression   profile-picker: hold timing, angle to sector, commit
  |                    |
  |                    +--> desktop-bindings, with the trigger masked out
  |                    |
  |                    v
  +------------ bridge-runtime ---- PickerEvent ----> sc-bridge-menu
                                                          |
                                          JSON lines on stdin
                                                          v
                                              overlay process (eframe)
```

- **`crates/app/profile-picker`** is pure: hold timing, wheel geometry, paging, and
  the suppression set. It has no platform dependency and carries the bulk of the
  tests. It also reports `Preparing` halfway through a hold, so a host with
  something expensive to build can start before the wheel is wanted.
- **`crates/bridge/bridge-runtime`** drives it once per controller report, applies the
  suppression to `bridge-core`, masks the trigger out of the snapshot handed to
  `desktop-bindings`, and streams `PickerEvent`s to the frontend through the
  sink given to `BridgeRuntime::spawn_with_picker`.
- **`apps/sc-bridge-menu`** owns the profile store. The runtime is told only how
  many profiles there are and which is active (`PickerRoster`); it never learns
  their names, because it has no use for them. Every roster carries an opaque
  revision that selection and commit events echo, so a queued `Commit { index }`
  can only resolve against the exact ID snapshot the wheel used. The resolved
  profile is applied through `select_binding_profile`, the same path the tray
  submenu uses.

Events reach the frontend through a bounded, coalescing mailbox plus one
`EventLoopProxy` wake-up per pending batch rather than the 250 ms status poll,
so a stick flick shows up in the next frame without an unbounded UI backlog.

## The overlay process

The overlay is a second process of the same binary, started with the hidden
`profile-overlay` subcommand. It is separate because `eframe::run_native` owns
an event loop and the menu app's is already committed to the status item.

The child never writes back. Every decision is the runtime's, which is what lets
the window be non-activating, click-through, and completely input-free.

### One process per wheel, and nothing at rest

There is no overlay process and no overlay window except while a wheel is on its
way or on screen. A window on the game's Space is not free to the compositor,
and the wheel is used for a few seconds at a time, so it does not get to exist
the rest of the time.

The lifecycle is driven entirely by picker events:

| Event | Parent does |
| --- | --- |
| `Preparing` - halfway through the hold | spawn the overlay |
| `Opened` | spawn if it somehow is not running, then show the wheel |
| `Selection` | move the highlight |
| `Commit` / `Dismissed` / `TriggerTapped` | kill the process, which takes the window with it |

Spawning halfway through the hold is what makes this affordable. Measured on the
release build, the overlay reaches a window on screen about 215 ms after `exec`
warm, and about 660 ms on the first launch after a build, when the binary is not
in the page cache. Half a two-second hold is 1000 ms, so even the cold case has
margin, and the three-second option has far more.

Being late is survivable rather than fatal: an `Open` written before the child
is reading simply waits in the pipe, and the wheel appears as soon as the child
comes up. Half a hold is also far longer than any ordinary press, so a Quick
Access tap never starts a process at all.

Killing rather than hiding is deliberate, and it is what the two rules below
exist to protect.

### Within a process, the window is ordered in once and never ordered out

A window macOS has ordered out stops receiving redraws, and a redraw is the
overlay's only route to the main thread: the stdin reader can set state and call
`request_repaint`, but only a frame can act on it. Hiding the window by ordering
it out therefore made the wheel a one-shot - it opened the first time and could
never be told to come back, because no frame ever ran again.

So the window is ordered in exactly once, at zero alpha as soon as it has been
placed on a display, and is never ordered out; the process is killed instead.
`Presentation` in `profile_overlay.rs` owns that rule and is unit tested for it.
A second order-in in those tests would mean the window had been ordered out in
between, which is the state the wheel cannot recover from.

### Placement and ordering go through AppKit, not winit

Both are done against the real `NSWindow`, alongside the level and collection
behaviour. Neither is a stylistic preference; each replaces a winit path that
produced a specific bug.

**Ordering.** `ViewportCommand::Visible(true)` reaches winit's
`makeKeyAndOrderFront`, which asks to become the **key** window. A background
accessory app cannot do that over another application's fullscreen Space, so the
call is ignored and the wheel never appears over a fullscreen game - which is
most of what this feature exists for. `orderFrontRegardless` is the call meant
for showing a window from an app that is not active, and it is what the
`CanJoinAllSpaces | FullScreenAuxiliary` behaviour is waiting for. No viewport
commands are sent from the render loop at all now; anything that asked winit to
change visibility would `orderOut` and strand the wheel again.

**Placement.** winit positions windows relative to the **primary** display, so
an overlay could only ever land there, while its size came from whichever
display winit associated with the window - one display's position with another's
size. `NSScreen::frame` and `NSWindow::setFrame_display` share a coordinate
space and a unit, so placement needs no conversion and cannot reproduce that
mismatch.

### Choosing the display

`NSScreen::mainScreen` is documented as the screen holding the window with
keyboard focus - but *also* as falling back to the menu-bar screen when the
calling app has no key window, and this overlay never has one. So an answer of
"the primary" is ambiguous: it cannot be told apart from that fallback, and
trusting it is what would pin the wheel to the primary display forever.

`choose_screen` therefore uses whichever signal can discriminate:

| `mainScreen` | Cursor | Chosen |
| --- | --- | --- |
| non-primary | anything | `mainScreen` - only a real key window can report this |
| primary | non-primary | the cursor - `mainScreen` carried no information |
| primary | primary | the primary |
| unavailable | anywhere | the cursor, else the primary |

The cursor is a good second signal here specifically because a fullscreen game
usually captures the pointer onto the display being played on.

The window is placed on the first frame and again on each closed → open
transition, since the process starts halfway through the hold and focus can move
before the wheel appears. Both happen at zero alpha, so neither is visible.

`screen_source=` in the `overlay_window_shown` log line reports which branch
won, because that is the only way to tell from outside.

### Losing the parent exits the process, without waiting for a frame

The same starvation applies to shutting down. The stdin reader therefore calls
`std::process::exit` directly when the pipe closes, rather than asking the
render loop to close the window: a child whose window is not currently being
drawn would never run the frame that handled the request, and would outlive the
menu app with a window still on screen. The overlay holds nothing that needs
flushing.

This is the backstop for a parent that crashes or is killed. In the normal case
the parent kills the child outright.

### Protocol

Newline-delimited JSON on the child's stdin, parent to child only:

```json
{"v":1,"kind":"roster","names":["Default","Gaming"],"active":0,"sectors_per_page":8}
{"v":1,"kind":"open","selected":1,"page":0}
{"v":1,"kind":"open","selected":3,"page":0}
```

A repeated `open` moves the highlight. There is no close message: a closed
wheel is a killed process, and closing the pipe is how the child learns the
menu app has exited.

The parent never writes the pipe from its own thread: a dedicated writer
thread owns the child's stdin, so a child wedged in window or GL setup can
stall its own queue but never the menu app's event loop.

### Floating over a fullscreen game

`winit` exposes no `NSWindowCollectionBehavior` and no window level high enough,
so an ordinary always-on-top window lands on the wrong Space and is invisible
over a fullscreen game. Building an `NSPanel` directly would need `unsafe`,
which this workspace forbids.

The window is therefore created by `eframe`, given a unique title, then found by
that title in `NSApp.windows()` and configured through AppKit's safe setters:

- `setLevel(NSPopUpMenuWindowLevel)` - 101, above ordinary windows.
- `setCollectionBehavior(CanJoinAllSpaces | FullScreenAuxiliary | Stationary | IgnoresCycle)`
  - `CanJoinAllSpaces` puts the wheel on whatever Space the game is on, and
  `FullScreenAuxiliary` lets it coexist with a fullscreen window rather than
  being hidden behind it.
- `setIgnoresMouseEvents(true)` and `setHasShadow(false)`.

The event loop is also built with `ActivationPolicy::Accessory` and
`with_activate_ignoring_other_apps(false)`, so no Dock icon appears and the
frontmost app keeps focus.

**Limitation:** a game that captures the display exclusively (`CGDisplayCapture`)
draws above every window level, and no overlay can appear over it. Native
fullscreen and borderless-windowed games - the common case on macOS, including
Game Porting Toolkit and Whisky - work.

## Diagnostics

The runtime logs `profile_picker=`, `profile_picker_open=`, and
`profile_picker_roster=` deltas alongside the binding fields, and carries the
same values in `BridgeStatus::profile_picker`.

The overlay process logs to stderr, which the menu app inherits:

- `event=overlay_window_configured` - the AppKit setters were applied.
- `event=overlay_window_shown` - the window's visibility, Space, level, frame,
  collection behaviour, `screen_source=`, and screen count, on the frame after it
  was shown. When a user reports that nothing appeared, this line separates the
  cases that look identical from outside: never shown, shown off-screen, shown
  too low, stranded on the wrong Space, or placed on the wrong display.
  `screen_source=` in particular says which of the display signals won.
- `event=profile_overlay_started` / `_stopped` / `_exited` / `_start_failed` -
  from the parent, covering the child's lifecycle. A started/stopped pair per
  hold is normal; anything left running between holds is not.
- `event=overlay_parent_closed` - the child exiting because the menu app went
  away without killing it.

## Settings

`~/Library/Application Support/Steam Controller Bridge/settings.json`, version 3:

| Field | Meaning |
| --- | --- |
| `profile_overlay_enabled` | Whether a hold on Quick Access opens the wheel. Defaults to `false`. |
| `profile_overlay_hold_ms` | The hold, either `2000` or `3000`. |

Version 1 and 2 files migrate forward with the wheel switched off, so an
existing Quick Access binding is never taken over without being asked.
