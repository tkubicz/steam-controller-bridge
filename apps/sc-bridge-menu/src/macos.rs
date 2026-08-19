use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bridge_runtime::{
    format_status_diagnostics, BridgeStatus, OutputChangePoll, PickerEvent, PuckDockAction,
    StatusLogRecord, StatusLogTracker, UpdateResumePoll,
};
use desktop_bindings::parse_store;
use desktop_input::DesktopSession;
use menu_shell::{activate_child_application, copy_text, open_path};
use objc2::{rc::Retained, MainThreadMarker};
use objc2_app_kit::{NSImage, NSStatusBarButton};
use platform_capabilities::{
    current_provider, CapabilityContext, CapabilityId, CapabilityRequestOutcome, CapabilityState,
    PlatformCapabilities,
};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, Path as SkiaPath, PathBuilder, Pixmap, Stroke, Transform,
};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use virtual_gamepad::ENABLE_VIRTUAL_HID_ENV;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::window::WindowId;

use crate::app_center_protocol::{
    AppCenterPage, UpdateOperation, UpdateRequest, UpdateResponse, UpdateResult,
};
use crate::app_state::{
    bindings_file_fingerprint, picker_roster, resolve_picker_commit, save_settings,
    AppCenterRecovery, AppState, IdleShutdownChoice, OutputPreference,
};
use crate::model::{
    hardware_status_rows, HardwareRowVisibility, HardwareStatusRow, MenuAction, MenuModel,
    ProfileOverlayHoldChoice, RunAction, TrayControlsModel, TrayState, WindowModel,
};

mod icons;
mod logging;
mod permissions;
mod profiles;
mod support;
mod system;
mod tray;

use icons::{template_icon, NativeTrayIcons};
use logging::{copy_diagnostics, StatusLogger};
#[cfg(test)]
use support::bundled_virtual_hid_helper_path_from;
use support::output_selection;
use system::{apply_capability_remedy, launch_bindings_editor};

#[cfg(test)]
mod tests;

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const LOG_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
const LOG_TRUNCATION_MARKER: &str = " log_truncated=true\n";
const PROFILES_MENU_LABEL: &str = "Profiles";
const EDIT_PROFILES_LABEL: &str = "Edit Profiles";
// `muda` treats a single ampersand as a mnemonic marker on every platform.
// Doubling it preserves one visible ampersand in the native macOS menu.
const FIRMWARE_UPDATES_LABEL: &str = "Firmware && Updates";
const UPDATE_AVAILABLE_LABEL: &str = "Update Available";
pub fn run(virtual_hid_enabled: bool) -> Result<(), String> {
    // A menu bar app has no windows and no Dock icon, and it must not take
    // focus when it starts. Winit otherwise runs as a regular foreground app
    // and calls `activateIgnoringOtherApps` at launch. In that state macOS
    // tears down the status item's menu the first time a submenu is opened
    // after launch, taking the whole menu with it; every later open is fine.
    // Declaring the app an accessory and leaving the frontmost app alone keeps
    // the menu up. Either one alone is enough, and both are correct here.
    let event_loop = EventLoop::builder()
        .with_activation_policy(ActivationPolicy::Accessory)
        .with_activate_ignoring_other_apps(false)
        .build()
        .map_err(|error| error.to_string())?;
    let Some(mut app) = MenuApp::new(event_loop.create_proxy(), virtual_hid_enabled)? else {
        // The user was shown an unreadable profile store and chose to quit so
        // they could repair it. That is a completed request, not a failure.
        return Ok(());
    };
    event_loop
        .run_app(&mut app)
        .map_err(|error| error.to_string())
}

struct MenuItems {
    menu: Menu,
    bridge: MenuItem,
    status: MenuItem,
    input: MenuItem,
    controller: MenuItem,
    output: MenuItem,
    firmware: MenuItem,
    battery: MenuItem,
    haptics: MenuItem,
    hardware_separator: PredefinedMenuItem,
    hardware_visibility: HardwareItemVisibility,
    current_profile: MenuItem,
    automatic_shutdown: MenuItem,
    problem: MenuItem,
    run_toggle: MenuItem,
    copy_error: MenuItem,
    copy_error_visible: bool,
    #[cfg(feature = "updater")]
    updates: MenuItem,
    idle_shutdown: Vec<(IdleShutdownChoice, CheckMenuItem)>,
    puck_dock: CheckMenuItem,
    output_bridge_device: CheckMenuItem,
    output_virtual_hid: CheckMenuItem,
    bindings_submenu: Submenu,
    binding_profiles: Vec<(String, CheckMenuItem)>,
    overlay_submenu: Submenu,
    overlay_enabled: CheckMenuItem,
    overlay_hold: Vec<(u64, CheckMenuItem)>,
}

#[derive(Clone, Copy)]
struct HardwareItemVisibility {
    section: bool,
    optional: OptionalHardwareItemVisibility,
}

#[derive(Clone, Copy)]
struct OptionalHardwareItemVisibility {
    firmware: bool,
    battery: bool,
    haptics: bool,
}

impl From<HardwareRowVisibility> for HardwareItemVisibility {
    fn from(rows: HardwareRowVisibility) -> Self {
        Self {
            section: rows.section,
            optional: OptionalHardwareItemVisibility {
                firmware: rows.section && rows.firmware,
                battery: rows.section && rows.controller_details,
                haptics: rows.section && rows.controller_details,
            },
        }
    }
}

fn binding_profile_menu_items(controls: &TrayControlsModel<'_>) -> Vec<(String, CheckMenuItem)> {
    controls
        .profiles
        .binding_profiles()
        .map(|profile| {
            (
                profile.id.to_owned(),
                CheckMenuItem::with_id(
                    MenuAction::id_for_binding_profile(profile.id),
                    profile.label,
                    true,
                    profile.selected,
                    None,
                ),
            )
        })
        .collect()
}

struct MenuApp {
    state: AppState,
    tray: Option<TrayIcon>,
    tray_icons: Option<NativeTrayIcons>,
    items: Option<MenuItems>,
    last_revision: u64,
    last_model: Option<MenuModel>,
    last_recovery_problem: Option<String>,
    next_poll: Instant,
    logger: StatusLogger,
}

impl MenuApp {
    /// Builds the menu app, or reports `None` when an unreadable profile store
    /// was presented to the user and they chose to quit rather than reset it.
    fn new(proxy: EventLoopProxy<()>, virtual_hid_enabled: bool) -> Result<Option<Self>, String> {
        let state = AppState::load(
            virtual_hid_enabled,
            || current_provider().map_err(|error| error.to_string()),
            |settings_path, preference| {
                output_selection(preference).map_err(|error| {
                    format!(
                        "cannot start with the saved gamepad output: {error}. Run the packaged \
                         application, or set \"output\" to \"bridge_device\" in {}",
                        settings_path.display()
                    )
                })
            },
            move || {
                let _ = proxy.send_event(());
            },
        )?;
        let Some(state) = state else {
            return Ok(None);
        };
        let logger = StatusLogger::new()?;
        Ok(Some(Self {
            state,
            tray: None,
            tray_icons: None,
            items: None,
            last_revision: u64::MAX,
            last_model: None,
            last_recovery_problem: None,
            next_poll: Instant::now(),
            logger,
        }))
    }
}

impl ApplicationHandler for MenuApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.tray.is_none() {
            if let Err(error) = self.create_tray() {
                eprintln!("cannot create menu-bar icon: {error}");
                let _ = self.shutdown();
                event_loop.exit();
                return;
            }
            self.request_permissions_in_order(false);
            self.sync_picker_roster();
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }

    /// The runtime thread woke us because the wheel moved.
    ///
    /// Going through the event loop rather than the status poll is what keeps
    /// the wheel responsive: a stick flick shows up in the next frame instead
    /// of up to `POLL_INTERVAL` later.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, (): ()) {
        self.drain_picker_events();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.recover_app_center_suspension();
        self.poll_output_change();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            self.handle_menu_event(event.id.as_ref(), event_loop);
        }
        // Also drained here so a wake-up that arrives between passes is never
        // left sitting in the channel.
        self.drain_picker_events();
        self.flush_overlay_diagnostics();
        if Instant::now() >= self.next_poll {
            if self.state.picker_roster_dirty {
                // A publish the runtime failed to acknowledge left the wheel
                // one generation behind; retry until it lands.
                self.sync_picker_roster();
            }
            self.state
                .editor_children
                .retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_)) | Err(_)));
            self.handle_update_requests(event_loop);
            self.reload_bindings_if_changed();
            self.observe_permission_grants();
            self.refresh_status();
            self.next_poll = Instant::now() + POLL_INTERVAL;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_poll));
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        let _ = self.shutdown();
    }
}
