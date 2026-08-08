//! The in-game profile wheel, drawn by a second process of this binary.
//!
//! It renders what the runtime decided and nothing else: no keyboard, no
//! mouse, no focus. That is what lets the window be click-through and
//! non-activating, so it can sit over a game without the game noticing.

use std::io::BufRead;
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSEvent, NSPopUpMenuWindowLevel, NSScreen, NSWindow, NSWindowCollectionBehavior,
};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

use crate::overlay_protocol::{OverlayEnvelope, OverlayMessage, OVERLAY_WINDOW_TITLE};
use ui_theme::{ACCENT, MUTED_TEXT, ON_ACCENT, SURFACE, SURFACE_RAISED, TEXT};

/// Matches the bindings editor so the two windows read as one product.
const SCRIM: egui::Color32 = egui::Color32::from_rgba_premultiplied(0, 0, 0, 130);

const WHEEL_RADIUS: f32 = 210.0;
const HUB_RADIUS: f32 = 84.0;
const LABEL_RADIUS_FRACTION: f32 = 0.72;
/// Angular gap between wedges, in radians, so the sectors read as separate.
const WEDGE_GAP: f32 = 0.03;
const ARC_STEPS: usize = 16;

/// Placeholder until the window is placed on a real display. Never shown: the
/// window is only ordered in once `place` has resized it.
const INITIAL_SIZE: [f32; 2] = [1280.0, 800.0];

#[derive(Debug, Clone, Default)]
struct OverlayState {
    names: Vec<String>,
    active: Option<usize>,
    sectors_per_page: usize,
    /// `Some((selected, page))` while the wheel is up.
    open: Option<(usize, usize)>,
}

impl OverlayState {
    fn apply(&mut self, message: OverlayMessage) {
        match message {
            OverlayMessage::Roster {
                names,
                active,
                sectors_per_page,
            } => {
                self.names = names;
                self.active = active;
                self.sectors_per_page = sectors_per_page.max(1);
            }
            OverlayMessage::Open { selected, page } => {
                self.open = Some((selected, page));
            }
        }
    }

    /// The profile indices on the given page, and where the selection sits.
    fn page_entries(&self, page: usize) -> (&[String], usize) {
        let per_page = self.sectors_per_page.max(1);
        let start = (page * per_page).min(self.names.len());
        let end = (start + per_page).min(self.names.len());
        (&self.names[start..end], start)
    }

    /// Delegated to the picker so the two sides can never disagree on how a
    /// roster splits into pages.
    fn page_count(&self) -> usize {
        profile_picker::page_count(self.names.len(), self.sectors_per_page)
    }
}

/// Runs the overlay process until the parent's pipe closes.
///
/// # Errors
/// Returns an error if the window cannot be created.
pub fn run() -> Result<(), String> {
    let state = Arc::new(Mutex::new(OverlayState::default()));
    let options = eframe::NativeOptions {
        // Without this the overlay becomes a regular foreground app: a Dock
        // icon appears and the game loses focus the moment the wheel opens.
        event_loop_builder: Some(Box::new(|builder| {
            builder
                .with_activation_policy(ActivationPolicy::Accessory)
                .with_activate_ignoring_other_apps(false);
        })),
        viewport: egui::ViewportBuilder::default()
            .with_title(OVERLAY_WINDOW_TITLE)
            .with_inner_size(INITIAL_SIZE)
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_has_shadow(false)
            .with_taskbar(false)
            // The wheel is driven by the controller, so the window must never
            // take focus or swallow a click meant for the game.
            .with_active(false)
            .with_mouse_passthrough(true)
            // Stays hidden until the runtime says the wheel opened. Starting up
            // early is what keeps the first hold from paying window-creation cost.
            .with_visible(false),
        ..eframe::NativeOptions::default()
    };
    let reader_state = Arc::clone(&state);
    eframe::run_native(
        OVERLAY_WINDOW_TITLE,
        options,
        Box::new(move |creation| {
            ui_theme::configure_ui(&creation.egui_ctx);
            spawn_stdin_reader(reader_state, creation.egui_ctx.clone());
            Ok(Box::new(ProfileOverlay {
                state,
                presentation: Presentation::default(),
                placed: false,
                screen: None,
                log_native_window: false,
            }))
        }),
    )
    .map_err(|error| error.to_string())
}

/// Reads the parent's commands on a background thread and wakes the UI.
fn spawn_stdin_reader(state: Arc<Mutex<OverlayState>>, ctx: egui::Context) {
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match OverlayEnvelope::from_line(&line) {
                Ok(envelope) => {
                    lock(&state).apply(envelope.message);
                    ctx.request_repaint();
                }
                Err(error) => {
                    // A message this build cannot read is survivable; the next
                    // one may well be fine, so keep the wheel alive.
                    eprintln!("level=warn event=overlay_message_rejected error={error:?}");
                }
            }
        }
        // The pipe closed, so the menu app is gone and this process has nothing
        // left to display. Exit from this thread rather than asking the render
        // loop to close: a window that is not currently being drawn would never
        // run the frame that handles the request, and the overlay would outlive
        // its parent with a window still on screen. There is nothing to flush.
        eprintln!("level=info event=overlay_parent_closed action=exit");
        std::process::exit(0);
    });
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the window must be told this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct PresentationChange {
    /// Order the window in. Happens exactly once in the process's life.
    order_in: bool,
    /// The wheel's new visibility, when it changed.
    wheel: Option<bool>,
}

/// Decides how the window presents the wheel, without touching one.
///
/// The rule this encodes, and the reason it is a type of its own rather than a
/// pair of bools inline: **the window is ordered in once and never ordered
/// out.** A window that macOS has ordered out stops receiving redraws, and the
/// overlay's only route to the main thread is a redraw -- so a hidden window
/// can never be told to come back, and the wheel opens exactly once per process
/// and never again. Visibility is therefore expressed as alpha on a window that
/// stays ordered in.
#[derive(Debug, Default, PartialEq, Eq)]
struct Presentation {
    ordered_in: bool,
    wheel_visible: bool,
}

impl Presentation {
    /// `sized` gates the first order-in so the window is never shown at the
    /// placeholder size it was created with.
    fn update(&mut self, open: bool, sized: bool) -> PresentationChange {
        let mut change = PresentationChange::default();
        if !self.ordered_in && sized {
            self.ordered_in = true;
            change.order_in = true;
        }
        if open != self.wheel_visible {
            self.wheel_visible = open;
            change.wheel = Some(open);
        }
        change
    }
}

struct ProfileOverlay {
    state: Arc<Mutex<OverlayState>>,
    presentation: Presentation,
    /// Whether the window has been sized to a display yet. Gates the order-in,
    /// so the window is never shown at the placeholder size it was created with.
    placed: bool,
    /// How the current display was chosen, for the diagnostics.
    screen: Option<(ScreenSource, usize)>,
    /// Report the native window once, on the frame after the wheel is shown.
    log_native_window: bool,
}

impl ProfileOverlay {
    /// Places the window on the display the wheel should appear on. Returns
    /// whether the window existed to be placed.
    fn place(&mut self) -> bool {
        if let Some(screen) = place_on_target_screen() {
            self.screen = Some(screen);
            return true;
        }
        false
    }
}

impl eframe::App for ProfileOverlay {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Fully transparent: everything visible is painted below.
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Everything about where this window is and whether it is on screen now
        // goes through AppKit, so no viewport commands are sent from here.
        configure_native_window();
        if !self.placed {
            // The window does not exist on the very first frame, so keep trying
            // until it does.
            self.placed = self.place();
        }

        let state = lock(&self.state).clone();
        let open = state.open.is_some();
        let change = self.presentation.update(open, self.placed);
        if let Some(wheel) = change.wheel {
            if wheel {
                // Re-pick the display: the process starts halfway through the
                // hold, and the focused window can move between then and now.
                self.place();
            }
            set_wheel_alpha(wheel);
            self.log_native_window = wheel;
            if wheel {
                // The diagnostics below run a frame late, and only stdin
                // traffic causes frames — so ask for one, or a wheel that is
                // opened and immediately committed logs nothing.
                ui.ctx().request_repaint();
            }
        }
        if change.order_in {
            // Ordered in through AppKit rather than `ViewportCommand::Visible`,
            // which winit implements as `makeKeyAndOrderFront`. That asks to
            // become the key window, which a background accessory app cannot do
            // over another app's fullscreen Space -- so the wheel never appeared
            // over a fullscreen game. `orderFrontRegardless` is the call meant
            // for showing a window from an app that is not active, and it is
            // what the collection behaviour above is waiting for.
            //
            // Alpha is already zero, so this orders in a window showing nothing.
            if let Some(window) = native_window() {
                window.orderFrontRegardless();
            }
        } else if self.log_native_window && open {
            // Logged a frame late, once the alpha change has been applied.
            // "The wheel never appeared" is this feature's whole failure mode,
            // and these are the numbers that say whether the window is where it
            // should be, so they belong in the diagnostics.
            self.log_native_window = false;
            log_native_window_state(self.screen);
        }
        if !open {
            // Painting nothing clears the previous wheel out of the surface, so
            // a stale frame cannot survive behind the zeroed alpha.
            return;
        }
        let screen = ui.max_rect();
        paint_wheel(ui.painter(), screen, &state);
    }
}

/// Sizes the window to cover the display the wheel should appear on.
///
/// Placement goes through `AppKit` rather than `winit` because winit positions
/// windows relative to the **primary** display, so an overlay could only ever
/// land there, while the size came from whichever display winit associated with
/// the window -- one display's position with another's size. `NSScreen::frame`
/// and `NSWindow::setFrame_display` share a coordinate space and unit, so this
/// needs no conversion and cannot reproduce that mismatch.
///
/// Returns how the display was chosen, for the diagnostics.
fn place_on_target_screen() -> Option<(ScreenSource, usize)> {
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
fn configure_native_window() {
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
fn set_wheel_alpha(visible: bool) {
    if let Some(window) = native_window() {
        window.setAlphaValue(if visible { 1.0 } else { 0.0 });
    }
}

/// How the display the wheel appears on was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScreenSource {
    /// `NSScreen::mainScreen`, which macOS reports as the screen holding the
    /// focused window.
    FocusedWindow,
    /// The screen the pointer is on.
    Cursor,
    /// First in the screen list, which is the one holding the menu bar.
    Primary,
}

impl ScreenSource {
    const fn label(self) -> &'static str {
        match self {
            Self::FocusedWindow => "focused_window",
            Self::Cursor => "cursor",
            Self::Primary => "primary",
        }
    }
}

struct TargetScreen {
    /// Held rather than reduced to a rectangle so the geometry type never has
    /// to be named, which keeps `objc2-foundation` out of this crate's
    /// dependencies.
    screen: Retained<NSScreen>,
    source: ScreenSource,
    screen_count: usize,
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
fn target_screen(mtm: MainThreadMarker) -> Option<TargetScreen> {
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
fn choose_screen(main: Option<usize>, cursor: Option<usize>) -> (usize, ScreenSource) {
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
fn index_of_frame(frames: &[Rect], target: Rect) -> Option<usize> {
    const TOLERANCE: f64 = 0.5;
    frames.iter().position(|frame| {
        (frame.0 - target.0).abs() < TOLERANCE
            && (frame.1 - target.1).abs() < TOLERANCE
            && (frame.2 - target.2).abs() < TOLERANCE
            && (frame.3 - target.3).abs() < TOLERANCE
    })
}

/// A screen rectangle as `(x, y, width, height)` in `AppKit`'s global space.
type Rect = (f64, f64, f64, f64);

fn rect_of(screen: &NSScreen) -> Rect {
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
fn screen_containing(point: (f64, f64), frames: &[Rect]) -> Option<usize> {
    let (x, y) = point;
    frames
        .iter()
        .position(|&(origin_x, origin_y, width, height)| {
            x >= origin_x && x < origin_x + width && y >= origin_y && y < origin_y + height
        })
}

/// Finds the overlay's own window among the application's windows.
fn native_window() -> Option<Retained<NSWindow>> {
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
fn log_native_window_state(screen: Option<(ScreenSource, usize)>) {
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

fn paint_wheel(painter: &egui::Painter, rect: egui::Rect, state: &OverlayState) {
    let Some((selected, page)) = state.open else {
        return;
    };
    let (entries, offset) = state.page_entries(page);
    if entries.is_empty() {
        // A roster update can outrun the picker's page clamp by one report;
        // painting nothing beats dimming the game under a wheel-less scrim.
        return;
    }
    painter.rect_filled(rect, 0.0, SCRIM);
    let center = rect.center();

    let sectors = entries.len();
    let selected = selected.min(sectors - 1);
    for (index, name) in entries.iter().enumerate() {
        let chosen = index == selected;
        paint_wedge(painter, center, index, sectors, chosen);
        paint_label(
            painter,
            center,
            index,
            sectors,
            name,
            chosen,
            state.active == Some(offset + index),
        );
    }

    // The hub covers the wedges' shared apex, turning the pie into a ring and
    // leaving room to name what is about to be applied.
    painter.circle_filled(center, HUB_RADIUS, SURFACE);
    painter.text(
        center - egui::vec2(0.0, 14.0),
        egui::Align2::CENTER_CENTER,
        entries[selected].as_str(),
        egui::FontId::proportional(17.0),
        TEXT,
    );
    painter.text(
        center + egui::vec2(0.0, 10.0),
        egui::Align2::CENTER_CENTER,
        "A apply",
        egui::FontId::proportional(12.0),
        MUTED_TEXT,
    );
    painter.text(
        center + egui::vec2(0.0, 26.0),
        egui::Align2::CENTER_CENTER,
        "B cancel",
        egui::FontId::proportional(12.0),
        MUTED_TEXT,
    );

    let pages = state.page_count();
    if pages > 1 {
        painter.text(
            center + egui::vec2(0.0, WHEEL_RADIUS + 28.0),
            egui::Align2::CENTER_CENTER,
            format!("L1 / R1   page {} of {pages}", page + 1),
            egui::FontId::proportional(13.0),
            MUTED_TEXT,
        );
    }
}

fn paint_wedge(
    painter: &egui::Painter,
    center: egui::Pos2,
    index: usize,
    sectors: usize,
    selected: bool,
) {
    let radius = if selected {
        WHEEL_RADIUS + 10.0
    } else {
        WHEEL_RADIUS
    };
    let fill = if selected { ACCENT } else { SURFACE_RAISED };
    if sectors <= 1 {
        // A lone entry — the short last page of a roster like 9-of-8 — owns
        // the whole wheel. Its "wedge" swept nearly a full turn, which is not
        // convex and mistessellates; a disc is the same shape drawn honestly.
        painter.circle_filled(center, radius, fill);
        return;
    }
    let arc = std::f32::consts::TAU / sectors_as_f32(sectors);
    let middle = arc * sectors_as_f32(index);
    let (start, end) = (
        middle - arc / 2.0 + WEDGE_GAP,
        middle + arc / 2.0 - WEDGE_GAP,
    );

    // A pie wedge shares the centre, so it stays convex for any sector count of
    // two or more and tessellates correctly. The hub hides the apex afterwards.
    let mut points = Vec::with_capacity(ARC_STEPS + 2);
    points.push(center);
    for step in 0..=ARC_STEPS {
        let t = sectors_as_f32(step) / sectors_as_f32(ARC_STEPS);
        points.push(point_on_wheel(center, start + (end - start) * t, radius));
    }
    painter.add(egui::Shape::convex_polygon(
        points,
        fill,
        egui::Stroke::NONE,
    ));
}

fn paint_label(
    painter: &egui::Painter,
    center: egui::Pos2,
    index: usize,
    sectors: usize,
    name: &str,
    selected: bool,
    active: bool,
) {
    let arc = std::f32::consts::TAU / sectors_as_f32(sectors);
    let position = point_on_wheel(
        center,
        arc * sectors_as_f32(index),
        WHEEL_RADIUS * LABEL_RADIUS_FRACTION,
    );
    painter.text(
        position,
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(14.0),
        if selected { ON_ACCENT } else { TEXT },
    );
    if active {
        // A dot marks the profile that is already in use, so the user can see
        // what a cancel would leave them with.
        painter.circle_filled(
            position + egui::vec2(0.0, 15.0),
            3.0,
            if selected { ON_ACCENT } else { ACCENT },
        );
    }
}

/// Sector zero sits at twelve o'clock and they run clockwise, matching
/// `profile_picker::sector_for`. Screen y grows downwards, hence the negation.
fn point_on_wheel(center: egui::Pos2, angle: f32, radius: f32) -> egui::Pos2 {
    center + egui::vec2(angle.sin() * radius, -angle.cos() * radius)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "sector and step counts are small enough to be exact in f32"
)]
fn sectors_as_f32(value: usize) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster(count: usize, sectors_per_page: usize) -> OverlayState {
        let mut state = OverlayState::default();
        state.apply(OverlayMessage::Roster {
            names: (0..count).map(|index| format!("Profile {index}")).collect(),
            active: Some(0),
            sectors_per_page,
        });
        state
    }

    /// A primary display with a second one placed to its left, which puts the
    /// second at a negative origin -- the case that breaks any code assuming
    /// screen coordinates start at zero.
    const LEFT_OF_PRIMARY: [Rect; 2] = [(0.0, 0.0, 2560.0, 1440.0), (-1920.0, 0.0, 1920.0, 1080.0)];

    #[test]
    fn a_point_resolves_to_the_screen_it_falls_in() {
        assert_eq!(screen_containing((100.0, 100.0), &LEFT_OF_PRIMARY), Some(0));
        assert_eq!(
            screen_containing((-100.0, 100.0), &LEFT_OF_PRIMARY),
            Some(1),
            "a display left of the primary has a negative origin"
        );
        assert_eq!(
            screen_containing((-1920.0, 0.0), &LEFT_OF_PRIMARY),
            Some(1),
            "the far corner of that display is still inside it"
        );
    }

    #[test]
    fn a_shared_edge_belongs_to_exactly_one_screen() {
        // x = 0 is the primary's left edge and one past the second's right
        // edge. Half-open ranges keep it from matching both.
        assert_eq!(screen_containing((0.0, 500.0), &LEFT_OF_PRIMARY), Some(0));
        assert_eq!(
            screen_containing((-0.001, 500.0), &LEFT_OF_PRIMARY),
            Some(1)
        );
    }

    #[test]
    fn a_point_outside_every_screen_matches_nothing() {
        // The cursor can sit in a gap between displays that are not aligned.
        assert_eq!(screen_containing((0.0, 2000.0), &LEFT_OF_PRIMARY), None);
        assert_eq!(screen_containing((-5000.0, 0.0), &LEFT_OF_PRIMARY), None);
        assert_eq!(
            screen_containing((2560.0, 0.0), &LEFT_OF_PRIMARY),
            None,
            "just past the primary's right edge"
        );
    }

    #[test]
    fn an_empty_screen_list_matches_nothing() {
        assert_eq!(screen_containing((0.0, 0.0), &[]), None);
    }

    #[test]
    fn a_non_primary_focused_window_wins_because_only_a_real_one_reports_it() {
        assert_eq!(
            choose_screen(Some(2), Some(1)),
            (2, ScreenSource::FocusedWindow)
        );
        assert_eq!(
            choose_screen(Some(3), None),
            (3, ScreenSource::FocusedWindow)
        );
    }

    #[test]
    fn a_primary_focused_window_yields_to_the_cursor() {
        // `mainScreen` returning the primary is indistinguishable from its
        // no-key-window fallback, so it must not outvote a cursor that is
        // somewhere else. Trusting it here is exactly the bug being fixed.
        assert_eq!(choose_screen(Some(0), Some(4)), (4, ScreenSource::Cursor));
        assert_eq!(choose_screen(None, Some(1)), (1, ScreenSource::Cursor));
    }

    #[test]
    fn the_primary_is_used_when_nothing_points_anywhere_else() {
        assert_eq!(
            choose_screen(Some(0), Some(0)),
            (0, ScreenSource::FocusedWindow)
        );
        assert_eq!(
            choose_screen(Some(0), None),
            (0, ScreenSource::FocusedWindow)
        );
        assert_eq!(choose_screen(None, Some(0)), (0, ScreenSource::Cursor));
        assert_eq!(choose_screen(None, None), (0, ScreenSource::Primary));
    }

    #[test]
    fn a_screen_frame_resolves_to_its_index() {
        assert_eq!(
            index_of_frame(&LEFT_OF_PRIMARY, LEFT_OF_PRIMARY[1]),
            Some(1)
        );
        // AppKit hands back the same numbers on both sides, but a sub-pixel
        // difference must not silently fall through to the wrong display.
        assert_eq!(
            index_of_frame(&LEFT_OF_PRIMARY, (-1920.2, 0.1, 1920.0, 1080.0)),
            Some(1)
        );
        assert_eq!(
            index_of_frame(&LEFT_OF_PRIMARY, (500.0, 0.0, 800.0, 600.0)),
            None
        );
        assert_eq!(index_of_frame(&[], LEFT_OF_PRIMARY[0]), None);
    }

    #[test]
    fn every_screen_source_has_a_distinct_label() {
        let labels: Vec<_> = [
            ScreenSource::FocusedWindow,
            ScreenSource::Cursor,
            ScreenSource::Primary,
        ]
        .into_iter()
        .map(ScreenSource::label)
        .collect();
        // The log line is the only way to tell which branch won on real
        // hardware, so the labels have to be distinguishable.
        assert_eq!(labels, ["focused_window", "cursor", "primary"]);
    }

    #[test]
    fn the_wheel_can_be_shown_again_after_it_has_been_hidden() {
        // Regression: the overlay used to hide by ordering the window out.
        // macOS then stopped delivering redraws, the overlay's only route to
        // the main thread, so the second open could never be acted on and the
        // wheel opened exactly once per process.
        let mut presentation = Presentation::default();
        assert_eq!(
            presentation.update(false, false),
            PresentationChange::default(),
            "nothing happens before the window has been sized"
        );
        assert_eq!(
            presentation.update(false, true),
            PresentationChange {
                order_in: true,
                wheel: None
            }
        );

        for round in 0..3 {
            assert_eq!(
                presentation.update(true, true),
                PresentationChange {
                    order_in: false,
                    wheel: Some(true)
                },
                "round {round} must be able to show the wheel"
            );
            assert_eq!(
                presentation.update(false, true),
                PresentationChange {
                    order_in: false,
                    wheel: Some(false)
                },
                "round {round} must be able to hide the wheel"
            );
        }
    }

    #[test]
    fn the_window_is_ordered_in_exactly_once() {
        let mut presentation = Presentation::default();
        let mut order_ins = 0;
        for open in [false, true, false, true, true, false, false, true] {
            if presentation.update(open, true).order_in {
                order_ins += 1;
            }
        }
        // More than one would mean the window had been ordered out in between,
        // which is the state the wheel cannot recover from.
        assert_eq!(order_ins, 1);
    }

    #[test]
    fn an_unchanged_wheel_state_asks_for_no_work() {
        let mut presentation = Presentation::default();
        presentation.update(false, true);
        presentation.update(true, true);
        assert_eq!(
            presentation.update(true, true),
            PresentationChange::default(),
            "a repeated frame must not re-apply the alpha"
        );
    }

    #[test]
    fn open_drives_visibility_and_moves_the_highlight() {
        // There is no close message: a closed wheel is a killed process.
        let mut state = roster(4, 8);
        assert!(state.open.is_none());
        state.apply(OverlayMessage::Open {
            selected: 2,
            page: 0,
        });
        assert_eq!(state.open, Some((2, 0)));
        state.apply(OverlayMessage::Open {
            selected: 3,
            page: 0,
        });
        assert_eq!(state.open, Some((3, 0)));
    }

    #[test]
    fn a_roster_update_while_open_keeps_the_wheel_up() {
        let mut state = roster(4, 8);
        state.apply(OverlayMessage::Open {
            selected: 1,
            page: 0,
        });
        state.apply(OverlayMessage::Roster {
            names: vec!["Solo".to_owned()],
            active: None,
            sectors_per_page: 8,
        });
        // Only the runtime closes the wheel; the overlay must not decide that
        // for itself or the two would disagree about what is on screen.
        assert_eq!(state.open, Some((1, 0)));
    }

    #[test]
    fn pages_slice_the_roster_without_gaps_or_overlap() {
        let state = roster(11, 8);
        assert_eq!(state.page_count(), 2);
        let (first, first_offset) = state.page_entries(0);
        let (second, second_offset) = state.page_entries(1);
        assert_eq!(first.len(), 8);
        assert_eq!(first_offset, 0);
        assert_eq!(second.len(), 3);
        assert_eq!(second_offset, 8);
        assert_eq!(second[0], "Profile 8");
    }

    #[test]
    fn a_page_past_the_end_yields_nothing_rather_than_panicking() {
        let state = roster(3, 8);
        let (entries, offset) = state.page_entries(9);
        assert!(entries.is_empty());
        assert_eq!(offset, 3);
    }

    #[test]
    fn an_empty_roster_reports_one_page_and_no_entries() {
        let state = roster(0, 8);
        assert_eq!(state.page_count(), 1);
        assert!(state.page_entries(0).0.is_empty());
    }

    #[test]
    fn a_zero_sector_roster_cannot_divide_by_zero() {
        let mut state = OverlayState::default();
        state.apply(OverlayMessage::Roster {
            names: vec!["One".to_owned(), "Two".to_owned()],
            active: None,
            sectors_per_page: 0,
        });
        assert_eq!(state.sectors_per_page, 1);
        assert_eq!(state.page_count(), 2);
        assert_eq!(state.page_entries(1).0, ["Two".to_owned()]);
    }

    #[test]
    fn sector_zero_is_up_and_the_wheel_runs_clockwise() {
        let center = egui::pos2(100.0, 100.0);
        let up = point_on_wheel(center, 0.0, 10.0);
        assert!((up.x - 100.0).abs() < 1.0e-4);
        assert!((up.y - 90.0).abs() < 1.0e-4, "sector zero must point up");

        let right = point_on_wheel(center, std::f32::consts::FRAC_PI_2, 10.0);
        assert!(
            (right.x - 110.0).abs() < 1.0e-4,
            "a quarter turn must point right"
        );
        assert!((right.y - 100.0).abs() < 1.0e-4);
    }
}
