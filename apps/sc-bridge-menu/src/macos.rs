use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::bindings_recovery::load_store_or_recover;
use bridge_runtime::{
    format_status_diagnostics, BridgeHandle, BridgeRuntime, BridgeStatus, OutputChangePoll,
    OutputSelection, PendingOutputChange, PendingUpdateResume, PickerConfig, PickerEvent,
    PickerRoster, PuckDockAction, RuntimeConfig, StatusLogRecord, StatusLogTracker,
    UpdateResumePoll, VirtualHidConfig,
};
use desktop_bindings::{default_store_path, parse_store, BindingStore};
use desktop_input::{
    input_monitoring_access, preflight_accessibility_access, preflight_post_event_access,
    request_accessibility_access, request_input_monitoring_access, request_post_event_access,
    PermissionState,
};
use macos_virtual_hid::ENABLE_VIRTUAL_HID_ENV;
use objc2::{rc::Retained, MainThreadMarker};
use objc2_app_kit::{
    NSApplicationActivationOptions, NSImage, NSRunningApplication, NSStatusBarButton,
};
use serde::{Deserialize, Serialize};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, Path as SkiaPath, PathBuilder, Pixmap, Stroke, Transform,
};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::window::WindowId;

use crate::app_center_host::AppCenterHost;
use crate::app_center_protocol::{
    AppCenterPage, UpdateOperation, UpdateRequest, UpdateResponse, UpdateResult,
};
use crate::model::{HardwareRowVisibility, MenuModel, RunAction, TrayState};
use crate::overlay_host::OverlayHost;
#[cfg(feature = "updater")]
use crate::update_check::UpdateChecker;

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
use support::{
    bindings_file_fingerprint, load_settings, permission_stage, picker_roster,
    resolve_picker_commit, save_settings, settings_path, AppSettings, BindingsFileFingerprint,
    OutputPreference, PermissionStage, PickerEventMailbox, OVERLAY_HOLD_CHOICES,
};
pub(crate) use system::open_path;
#[cfg(feature = "updater")]
pub(crate) use system::reveal_path;
use system::{
    activate_child_application, copy_text, launch_bindings_editor, open_privacy_pane, PrivacyPane,
};

#[cfg(test)]
mod tests;

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const LOG_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
const LOG_TRUNCATION_MARKER: &str = " log_truncated=true\n";
const RUN_TOGGLE_ID: &str = "run-toggle";
const COPY_ERROR_ID: &str = "copy-error";
const COPY_ID: &str = "copy-diagnostics";
const SETTINGS_ID: &str = "input-monitoring";
const ACCESSIBILITY_ID: &str = "accessibility";
const ENABLE_BINDINGS_ID: &str = "enable-bindings";
const EDIT_BINDINGS_ID: &str = "edit-bindings";
const PROFILES_MENU_LABEL: &str = "Profiles";
const EDIT_PROFILES_LABEL: &str = "Edit Profiles";
const BINDING_PROFILE_PREFIX: &str = "binding-profile:";
const LOGS_ID: &str = "open-logs";
const ABOUT_ID: &str = "about";
const UPDATES_ID: &str = "updates";
// `muda` treats a single ampersand as a mnemonic marker on every platform.
// Doubling it preserves one visible ampersand in the native macOS menu.
const FIRMWARE_UPDATES_LABEL: &str = "Firmware && Updates";
#[cfg(any(feature = "updater", test))]
const UPDATE_AVAILABLE_LABEL: &str = "Update Available";

const fn app_center_available() -> bool {
    cfg!(feature = "updater")
}

fn app_center_page_for_menu(id: &str) -> Option<AppCenterPage> {
    if !app_center_available() {
        return None;
    }
    match id {
        ABOUT_ID => Some(AppCenterPage::About),
        UPDATES_ID => Some(AppCenterPage::Updates),
        _ => None,
    }
}
const QUIT_ID: &str = "quit";
const IDLE_NEVER_ID: &str = "idle-never";
const IDLE_5_ID: &str = "idle-5";
const IDLE_10_ID: &str = "idle-10";
const IDLE_15_ID: &str = "idle-15";
const IDLE_30_ID: &str = "idle-30";
const PUCK_DOCK_ID: &str = "puck-dock-power-off";
const OUTPUT_BRIDGE_DEVICE_ID: &str = "output-bridge-device";
const OUTPUT_VIRTUAL_HID_ID: &str = "output-virtual-hid";
const OVERLAY_ENABLED_ID: &str = "profile-overlay-enabled";
const OVERLAY_HOLD_PREFIX: &str = "profile-overlay-hold:";
const PICKER_EVENT_MAILBOX_CAPACITY: usize = 32;
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
    #[cfg_attr(
        not(feature = "updater"),
        expect(dead_code, reason = "only the updater feature rewrites the label")
    )]
    updates: MenuItem,
    idle_shutdown: Vec<(Option<u64>, CheckMenuItem)>,
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

fn binding_profile_menu_items(
    store: &BindingStore,
    active_profile_id: &str,
) -> Vec<(String, CheckMenuItem)> {
    store
        .profiles
        .iter()
        .map(|profile| {
            (
                profile.id.clone(),
                CheckMenuItem::with_id(
                    format!("{BINDING_PROFILE_PREFIX}{}", profile.id),
                    &profile.name,
                    true,
                    profile.id.eq_ignore_ascii_case(active_profile_id),
                    None,
                ),
            )
        })
        .collect()
}

struct MenuApp {
    runtime: BridgeHandle,
    tray: Option<TrayIcon>,
    tray_icons: Option<NativeTrayIcons>,
    items: Option<MenuItems>,
    last_revision: u64,
    last_model: Option<MenuModel>,
    last_recovery_problem: Option<String>,
    next_poll: Instant,
    logger: StatusLogger,
    settings: AppSettings,
    /// Product-surface gate only; macOS still enforces the helper entitlement.
    virtual_hid_enabled: bool,
    settings_path: PathBuf,
    bindings_path: PathBuf,
    binding_store: BindingStore,
    bindings_file_fingerprint: BindingsFileFingerprint,
    permission_request_pending: Option<PermissionStage>,
    shutting_down: bool,
    overlay: OverlayHost,
    /// Bounded/coalesced wheel events from the runtime thread.
    picker_events: Arc<PickerEventMailbox>,
    /// Profile ids in the order last published to the wheel, so a
    /// `Commit { index }` resolves against what the wheel showed even if the
    /// store has changed since.
    picker_roster_ids: Vec<String>,
    picker_roster_revision: u64,
    /// Monotonic count of roster publish attempts. A revision is spent even by
    /// a publish whose acknowledgement failed, because the runtime may still
    /// apply it late - reusing the number would let events from that stale
    /// generation resolve against a newer id list.
    picker_roster_publishes: u64,
    /// The last roster publish failed; retried on the next status poll so a
    /// transient runtime stall cannot leave the wheel dead until the next
    /// unrelated store change.
    picker_roster_dirty: bool,
    /// Spawned bindings editors, reaped on the status poll once they exit.
    editor_children: Vec<std::process::Child>,
    /// About, Changelog, and Updates share one child native event loop. The
    /// host also owns the updater's safety-ordered bridge lifecycle requests.
    app_center_host: AppCenterHost,
    app_center_recovery: AppCenterRecovery,
    output_change: Option<(PendingOutputChange, OutputPreference)>,
    output_change_problem: Option<String>,
    #[cfg(feature = "updater")]
    update_checker: UpdateChecker,
    #[cfg(feature = "updater")]
    last_update_available: Option<bool>,
}

enum AppCenterRecovery {
    Idle,
    Waiting {
        request: PendingUpdateResume,
        error: Option<String>,
    },
    Failed(String),
}

impl AppCenterRecovery {
    fn problem(&self) -> Option<&str> {
        match self {
            Self::Waiting {
                error: Some(error), ..
            }
            | Self::Failed(error) => Some(error),
            Self::Idle | Self::Waiting { error: None, .. } => None,
        }
    }
}

impl MenuApp {
    /// Builds the menu app, or reports `None` when an unreadable profile store
    /// was presented to the user and they chose to quit rather than reset it.
    fn new(proxy: EventLoopProxy<()>, virtual_hid_enabled: bool) -> Result<Option<Self>, String> {
        let settings_path = settings_path()?;
        let (settings, warning) = load_settings(&settings_path);
        if let Some(warning) = warning {
            eprintln!("level=warn event=settings_load_failed message={warning:?} action=defaults");
        }
        let bindings_path = default_store_path()?;
        let Some(binding_store) = load_store_or_recover(&bindings_path)? else {
            return Ok(None);
        };
        let active_profile = binding_store
            .profile_by_id(&settings.active_binding_profile)
            .or_else(|| binding_store.profiles.first())
            .cloned();
        let mut settings = settings;
        if let Some(profile) = &active_profile {
            settings.active_binding_profile.clone_from(&profile.id);
        }
        save_settings(&settings_path, &settings)?;
        let bindings_file_fingerprint = bindings_file_fingerprint(&bindings_path)?;
        // The dormant gate never resolves or launches the helper. Once enabled,
        // a bad packaged path still names the saved value that unsticks launch.
        let effective_output = settings
            .output
            .when_virtual_hid_enabled(virtual_hid_enabled);
        let output = effective_output.runtime_selection().map_err(|error| {
            format!(
                "cannot start with the saved gamepad output: {error}. Run the packaged \
                 application, or set \"output\" to \"bridge_device\" in {}",
                settings_path.display()
            )
        })?;
        let config = RuntimeConfig {
            output,
            idle_shutdown_timeout: settings
                .idle_shutdown_minutes
                .map(|minutes| Duration::from_secs(minutes * 60)),
            puck_dock_action: if settings.power_off_on_puck {
                PuckDockAction::PowerOff
            } else {
                PuckDockAction::LeaveOn
            },
            binding_profile: active_profile,
            profile_picker: settings.picker_config(),
            picker_roster: picker_roster(&binding_store, &settings.active_binding_profile, 0),
            ..RuntimeConfig::default()
        };
        let profile_ids = binding_store
            .profiles
            .iter()
            .map(|profile| profile.id.clone())
            .collect();
        // The runtime thread hands wheel events over and wakes the event loop
        // immediately. Polling for them would add up to POLL_INTERVAL of lag
        // between letting go of Quick Access and the wheel appearing.
        let picker_events = Arc::new(PickerEventMailbox::default());
        let picker_sender = Arc::clone(&picker_events);
        let runtime = BridgeRuntime::spawn_with_picker(
            config,
            Box::new(move |event| {
                if picker_sender.publish(event) {
                    let _ = proxy.send_event(());
                }
            }),
        );
        Ok(Some(Self {
            runtime,
            tray: None,
            tray_icons: None,
            items: None,
            last_revision: u64::MAX,
            last_model: None,
            last_recovery_problem: None,
            next_poll: Instant::now(),
            logger: StatusLogger::new()?,
            settings,
            virtual_hid_enabled,
            settings_path,
            bindings_path,
            binding_store,
            bindings_file_fingerprint,
            permission_request_pending: None,
            shutting_down: false,
            overlay: OverlayHost::new(),
            picker_events,
            // Matches the roster just handed to the runtime in `config`.
            picker_roster_ids: profile_ids,
            picker_roster_revision: 0,
            picker_roster_publishes: 0,
            picker_roster_dirty: false,
            editor_children: Vec::new(),
            app_center_host: AppCenterHost::new(),
            app_center_recovery: AppCenterRecovery::Idle,
            output_change: None,
            output_change_problem: None,
            #[cfg(feature = "updater")]
            update_checker: UpdateChecker::new(),
            #[cfg(feature = "updater")]
            last_update_available: None,
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
            if self.picker_roster_dirty {
                // A publish the runtime failed to acknowledge left the wheel
                // one generation behind; retry until it lands.
                self.sync_picker_roster();
            }
            self.editor_children
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
