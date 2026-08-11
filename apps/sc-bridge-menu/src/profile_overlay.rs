//! The in-game profile wheel, drawn by a second process of this binary.
//!
//! It renders what the runtime decided and nothing else: no keyboard, no
//! mouse, no focus. That is what lets the window be click-through and
//! non-activating, so it can sit over a game without the game noticing.

use std::io::BufReader;
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSEvent, NSScreen, NSScreenSaverWindowLevel, NSWindow,
    NSWindowCollectionBehavior,
};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

use crate::line_protocol::read_bounded_line;
use crate::overlay_protocol::{
    OverlayEnvelope, OverlayMessage, MAX_OVERLAY_LINE_BYTES, OVERLAY_WINDOW_TITLE,
};
use ui_theme::{ACCENT, MUTED_TEXT, ON_ACCENT, SURFACE, SURFACE_RAISED, TEXT};

mod wheel;
mod window;

use wheel::paint_wheel;
use window::{
    configure_native_window, log_native_window_state, native_window, place_on_target_screen,
    set_wheel_alpha, ScreenSource,
};

#[cfg(test)]
mod tests;

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
        let mut input = BufReader::new(std::io::stdin());
        loop {
            let line = match read_bounded_line(&mut input, MAX_OVERLAY_LINE_BYTES) {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(error) => {
                    eprintln!("level=warn event=overlay_message_rejected error={error:?}");
                    break;
                }
            };
            let line = String::from_utf8_lossy(&line);
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
                // traffic causes frames - so ask for one, or a wheel that is
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
