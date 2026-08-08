use super::{
    MainThreadMarker, NSApplication, NSEvent, NSPopUpMenuWindowLevel, NSScreen, NSWindow,
    NSWindowCollectionBehavior, OnceLock, Retained, OVERLAY_WINDOW_TITLE,
};

/// Sizes the window to cover the display the wheel should appear on.
///
/// `AppKit` keeps display frames and window placement in the same coordinate
/// space, unlike winit's primary-display-relative positioning.
///
/// Returns how the display was chosen, for the diagnostics.
pub(super) fn place_on_target_screen() -> Option<(ScreenSource, usize)> {
    let mtm = MainThreadMarker::new()?;
    let window = native_window()?;
    let target = target_screen(mtm)?;
    window.setFrame_display(target.screen.frame(), true);
    Some((target.source, target.screen_count))
}

/// Lifts the window above a fullscreen game, using only safe `AppKit` calls.
///
/// `winit` exposes neither `NSWindowCollectionBehavior` nor a window level high
/// enough, and building an `NSPanel` directly needs `unsafe`, which this UI
/// crate forbids. Creating the window through `winit` and then finding it
/// by title in `NSApp.windows()` reaches the same result through safe API.
/// Runs once; a failure leaves an overlay that still works on the current Space.
pub(super) fn configure_native_window() {
    static CONFIGURED: OnceLock<()> = OnceLock::new();
    if CONFIGURED.get().is_some() {
        return;
    }
    // The window may not exist yet on the very first frame.
    let Some(window) = native_window() else {
        return;
    };
    let _ = CONFIGURED.set(());
    window.setLevel(NSPopUpMenuWindowLevel);
    window.setCollectionBehavior(
        // `CanJoinAllSpaces` puts the wheel on whatever Space the game is on,
        // and `FullScreenAuxiliary` lets it coexist with a fullscreen window
        // instead of being hidden behind it.
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    window.setIgnoresMouseEvents(true);
    window.setHasShadow(false);
    // Ordered in later at zero alpha, so nothing is ever seen before the first
    // wheel opens. See `Presentation` for why it is not simply kept hidden.
    window.setAlphaValue(0.0);
    eprintln!("level=info event=overlay_window_configured");
}

/// Shows or hides the wheel without ordering the window out.
pub(super) fn set_wheel_alpha(visible: bool) {
    if let Some(window) = native_window() {
        window.setAlphaValue(if visible { 1.0 } else { 0.0 });
    }
}

/// How the display the wheel appears on was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScreenSource {
    /// `NSScreen::mainScreen`, which macOS reports as the screen holding the
    /// focused window.
    FocusedWindow,
    /// The screen the pointer is on.
    Cursor,
    /// First in the screen list, which is the one holding the menu bar.
    Primary,
}

impl ScreenSource {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::FocusedWindow => "focused_window",
            Self::Cursor => "cursor",
            Self::Primary => "primary",
        }
    }
}

pub(super) struct TargetScreen {
    /// Held rather than reduced to a rectangle so the geometry type never has
    /// to be named, which keeps `objc2-foundation` out of this crate's
    /// dependencies.
    pub(super) screen: Retained<NSScreen>,
    pub(super) source: ScreenSource,
    pub(super) screen_count: usize,
}

/// Picks the display the wheel should cover.
///
/// `NSScreen::mainScreen` is documented as the screen holding the window with
/// keyboard focus, which is what "the monitor I am looking at" means. But Apple
/// also documents it as falling back to the **menu-bar screen** when the calling
/// app has no key window, and this overlay never has one -- it is a background
/// accessory app that must never take focus. So a `mainScreen` answer of "the
/// primary" is ambiguous: it is indistinguishable from that fallback, and taking
/// it at face value is what would leave the wheel pinned to the primary display.
///
/// The rule is therefore to use whichever signal can actually discriminate.
/// `mainScreen` naming a **non**-primary display is real information, because
/// the fallback could never produce it. When it names the primary, the cursor is
/// asked instead, since that is unambiguous and, for a fullscreen game that has
/// captured the pointer, lands on the display being played on.
pub(super) fn target_screen(mtm: MainThreadMarker) -> Option<TargetScreen> {
    let screens = NSScreen::screens(mtm);
    let screen_count = screens.len();
    let frames: Vec<Rect> = screens.iter().map(|screen| rect_of(&screen)).collect();

    let main =
        NSScreen::mainScreen(mtm).and_then(|screen| index_of_frame(&frames, rect_of(&screen)));
    let cursor = NSEvent::mouseLocation();
    let cursor = screen_containing((cursor.x, cursor.y), &frames);

    let (index, source) = choose_screen(main, cursor);
    screens.iter().nth(index).map(|screen| TargetScreen {
        screen,
        source,
        screen_count,
    })
}

/// Chooses between the focused-window and cursor screens, by index.
///
/// Index 0 is the menu-bar screen, which is also what `NSScreen::mainScreen`
/// falls back to for an app with no key window -- so a `main` of `Some(0)`
/// carries no information and the cursor is preferred when it disagrees. Split
/// out as integers so the rule is testable without a display attached.
pub(super) fn choose_screen(main: Option<usize>, cursor: Option<usize>) -> (usize, ScreenSource) {
    match (main, cursor) {
        // Only a real key window can put `mainScreen` off the primary.
        (Some(index), _) if index != 0 => (index, ScreenSource::FocusedWindow),
        // `main` was the primary, so it may only be the fallback. The cursor is
        // the one signal left that can point somewhere else.
        (_, Some(index)) if index != 0 => (index, ScreenSource::Cursor),
        (Some(index), _) => (index, ScreenSource::FocusedWindow),
        (_, Some(index)) => (index, ScreenSource::Cursor),
        (None, None) => (0, ScreenSource::Primary),
    }
}

/// Finds the screen whose frame matches `target`.
///
/// Compared with a tolerance rather than exactly: both sides come from `AppKit`
/// and should be identical, but a float equality that silently fails would put
/// the wheel on the wrong display, which is the bug this is here to prevent.
pub(super) fn index_of_frame(frames: &[Rect], target: Rect) -> Option<usize> {
    const TOLERANCE: f64 = 0.5;
    frames.iter().position(|frame| {
        (frame.0 - target.0).abs() < TOLERANCE
            && (frame.1 - target.1).abs() < TOLERANCE
            && (frame.2 - target.2).abs() < TOLERANCE
            && (frame.3 - target.3).abs() < TOLERANCE
    })
}

/// A screen rectangle as `(x, y, width, height)` in `AppKit`'s global space.
pub(super) type Rect = (f64, f64, f64, f64);

pub(super) fn rect_of(screen: &NSScreen) -> Rect {
    let frame = screen.frame();
    (
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height,
    )
}

/// Finds which of `frames` contains `point`.
///
/// Screens share one global space in which the primary sits at the origin and
/// the others may extend into **negative** coordinates, so nothing here may
/// assume a non-negative origin. Edges are half-open, so a point on a shared
/// boundary belongs to exactly one screen.
pub(super) fn screen_containing(point: (f64, f64), frames: &[Rect]) -> Option<usize> {
    let (x, y) = point;
    frames
        .iter()
        .position(|&(origin_x, origin_y, width, height)| {
            x >= origin_x && x < origin_x + width && y >= origin_y && y < origin_y + height
        })
}

/// Finds the overlay's own window among the application's windows.
pub(super) fn native_window() -> Option<Retained<NSWindow>> {
    let mtm = MainThreadMarker::new()?;
    NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .find(|window| window.title().to_string() == OVERLAY_WINDOW_TITLE)
}

/// Records where the wheel actually landed.
///
/// If a user reports that nothing appears, this line distinguishes the cases
/// that look identical from the outside: a window that was never shown, one
/// sized or positioned off-screen, one at too low a level, and one stranded on
/// the wrong Space.
pub(super) fn log_native_window_state(screen: Option<(ScreenSource, usize)>) {
    let Some(window) = native_window() else {
        eprintln!("level=warn event=overlay_window_missing");
        return;
    };
    let frame = window.frame();
    let (source, screen_count) =
        screen.map_or(("none", 0), |(source, count)| (source.label(), count));
    eprintln!(
        "level=info event=overlay_window_shown visible={} on_active_space={} level={} \
         origin={},{} size={}x{} collection_behavior={:#x} screen_source={source} \
         screens={screen_count}",
        window.isVisible(),
        window.isOnActiveSpace(),
        window.level(),
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height,
        window.collectionBehavior().0,
    );
}
