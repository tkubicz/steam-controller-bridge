use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bridge_runtime::{
    format_status_diagnostics, BridgeHandle, BridgeRuntime, BridgeStatus, PickerConfig,
    PickerEvent, PickerRoster, PuckDockAction, RuntimeConfig, StatusLogRecord, StatusLogTracker,
};
use desktop_bindings::{
    default_store_path, input_monitoring_access, load_or_create_store, parse_store,
    preflight_accessibility_access, preflight_post_event_access, request_accessibility_access,
    request_input_monitoring_access, request_post_event_access, BindingStore, PermissionState,
};
use objc2::{rc::Retained, MainThreadMarker};
use objc2_app_kit::{NSImage, NSStatusBarButton};
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

use crate::model::{MenuModel, RunAction, TrayState};
use crate::overlay_host::OverlayHost;

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
const BINDING_PROFILE_PREFIX: &str = "binding-profile:";
const LOGS_ID: &str = "open-logs";
const QUIT_ID: &str = "quit";
const IDLE_NEVER_ID: &str = "idle-never";
const IDLE_5_ID: &str = "idle-5";
const IDLE_10_ID: &str = "idle-10";
const IDLE_15_ID: &str = "idle-15";
const IDLE_30_ID: &str = "idle-30";
const PUCK_DOCK_ID: &str = "puck-dock-power-off";
const OVERLAY_ENABLED_ID: &str = "profile-overlay-enabled";
const OVERLAY_HOLD_PREFIX: &str = "profile-overlay-hold:";
const PICKER_EVENT_MAILBOX_CAPACITY: usize = 32;
const SETTINGS_VERSION: u32 = 3;
/// Hold durations the menu offers, in milliseconds.
const OVERLAY_HOLD_CHOICES: [u64; 2] = [2_000, 3_000];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionStage {
    InputMonitoring,
    PostEvent,
    Accessibility,
    Ready,
}

const fn permission_stage(
    input_monitoring: bool,
    post_event: bool,
    accessibility: bool,
) -> PermissionStage {
    if !input_monitoring {
        PermissionStage::InputMonitoring
    } else if !post_event {
        PermissionStage::PostEvent
    } else if !accessibility {
        PermissionStage::Accessibility
    } else {
        PermissionStage::Ready
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AppSettings {
    version: u32,
    idle_shutdown_minutes: Option<u64>,
    power_off_on_puck: bool,
    #[serde(default = "default_binding_profile_id")]
    active_binding_profile: String,
    /// Whether holding Quick Access opens the in-game profile wheel. Off by
    /// default: it takes Quick Access over, which existing users have bound.
    #[serde(default)]
    profile_overlay_enabled: bool,
    #[serde(default = "default_overlay_hold_ms")]
    profile_overlay_hold_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BindingsFileFingerprint {
    length: u64,
    modified: SystemTime,
}

fn bindings_file_fingerprint(path: &Path) -> Result<BindingsFileFingerprint, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let modified = metadata.modified().map_err(|error| error.to_string())?;
    Ok(BindingsFileFingerprint {
        length: metadata.len(),
        modified,
    })
}

fn default_binding_profile_id() -> String {
    desktop_bindings::DEFAULT_PROFILE_ID.to_owned()
}

/// Describes the profile store to the wheel: how many there are, and where the
/// active one sits. Names stay here, because only this side can resolve them.
fn picker_roster(store: &BindingStore, active_profile_id: &str, revision: u64) -> PickerRoster {
    PickerRoster::with_revision(
        store.profiles.len(),
        store
            .profiles
            .iter()
            .position(|profile| profile.id.eq_ignore_ascii_case(active_profile_id)),
        revision,
    )
}

#[derive(Default)]
struct PickerEventMailbox {
    events: Mutex<VecDeque<PickerEvent>>,
}

impl PickerEventMailbox {
    /// Publishes without blocking the runtime thread. Returns whether the UI
    /// needs a wake-up; one pending wake covers the whole bounded batch.
    fn publish(&self, event: PickerEvent) -> bool {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let was_empty = events.is_empty();

        if matches!(event, PickerEvent::Selection { .. })
            && matches!(events.back(), Some(PickerEvent::Selection { .. }))
        {
            let _ = events.pop_back();
            events.push_back(event);
            return was_empty;
        }

        if matches!(event, PickerEvent::Opened { .. })
            && matches!(events.back(), Some(PickerEvent::Preparing))
        {
            let _ = events.pop_back();
        } else if matches!(
            event,
            PickerEvent::Commit { .. } | PickerEvent::Dismissed | PickerEvent::TriggerTapped
        ) {
            while matches!(
                events.back(),
                Some(
                    PickerEvent::Preparing
                        | PickerEvent::Opened { .. }
                        | PickerEvent::Selection { .. }
                )
            ) {
                let _ = events.pop_back();
            }
        }

        if events.len() == PICKER_EVENT_MAILBOX_CAPACITY {
            let _ = events.pop_front();
            eprintln!("level=warn event=profile_picker_event_mailbox_overflow action=drop_oldest");
        }
        events.push_back(event);
        was_empty
    }

    fn pop(&self) -> Option<PickerEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

fn resolve_picker_commit(
    profile_ids: &[String],
    current_revision: u64,
    event_revision: u64,
    index: usize,
) -> Option<&str> {
    if current_revision != event_revision {
        return None;
    }
    profile_ids.get(index).map(String::as_str)
}

const fn default_overlay_hold_ms() -> u64 {
    OVERLAY_HOLD_CHOICES[0]
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            idle_shutdown_minutes: Some(15),
            power_off_on_puck: false,
            active_binding_profile: default_binding_profile_id(),
            profile_overlay_enabled: false,
            profile_overlay_hold_ms: default_overlay_hold_ms(),
        }
    }
}

impl AppSettings {
    /// The wheel's runtime configuration, or `None` when it is switched off.
    fn picker_config(&self) -> Option<PickerConfig> {
        self.profile_overlay_enabled.then(|| {
            PickerConfig {
                hold: Duration::from_millis(self.profile_overlay_hold_ms),
                ..PickerConfig::default()
            }
            .sanitized()
        })
    }
}

fn settings_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set; cannot locate the application settings directory")?;
    Ok(home.join("Library/Application Support/Steam Controller Bridge/settings.json"))
}

fn load_settings(path: &Path) -> (AppSettings, Option<String>) {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (AppSettings::default(), None);
        }
        Err(error) => {
            return (
                AppSettings::default(),
                Some(format!("cannot read '{}': {error}", path.display())),
            );
        }
    };
    let parsed = serde_json::from_slice::<AppSettings>(&bytes).map_err(|error| error.to_string());
    match parsed {
        Ok(mut settings)
            if matches!(settings.version, 1 | 2 | SETTINGS_VERSION)
                && settings
                    .idle_shutdown_minutes
                    .is_none_or(|minutes| matches!(minutes, 5 | 10 | 15 | 30)) =>
        {
            settings.version = SETTINGS_VERSION;
            // A hand-edited hold falls back alone rather than dragging every
            // other setting down with it.
            if !OVERLAY_HOLD_CHOICES.contains(&settings.profile_overlay_hold_ms) {
                eprintln!(
                    "level=warn event=settings_invalid_overlay_hold value={} action=default",
                    settings.profile_overlay_hold_ms
                );
                settings.profile_overlay_hold_ms = default_overlay_hold_ms();
            }
            (settings, None)
        }
        Ok(settings) => (
            AppSettings::default(),
            Some(format!(
                "unsupported or invalid settings version/options: {settings:?}"
            )),
        ),
        Err(error) => (
            AppSettings::default(),
            Some(format!("invalid settings JSON: {error}")),
        ),
    }
}

fn save_settings(path: &Path, settings: &AppSettings) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("settings path '{}' has no parent", path.display()))?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

pub fn run() -> Result<(), String> {
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
    let mut app = MenuApp::new(event_loop.create_proxy())?;
    event_loop
        .run_app(&mut app)
        .map_err(|error| error.to_string())
}

struct MenuItems {
    bridge: MenuItem,
    status: MenuItem,
    input: MenuItem,
    controller: MenuItem,
    xiao: MenuItem,
    battery: MenuItem,
    haptics: MenuItem,
    bindings: MenuItem,
    automatic_shutdown: MenuItem,
    problem: MenuItem,
    run_toggle: MenuItem,
    copy_error: MenuItem,
    idle_shutdown: Vec<(Option<u64>, CheckMenuItem)>,
    puck_dock: CheckMenuItem,
    bindings_submenu: Submenu,
    binding_profiles: Vec<(String, CheckMenuItem)>,
    overlay_enabled: CheckMenuItem,
    overlay_hold: Vec<(u64, CheckMenuItem)>,
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
    next_poll: Instant,
    logger: StatusLogger,
    settings: AppSettings,
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
}

impl MenuApp {
    fn new(proxy: EventLoopProxy<()>) -> Result<Self, String> {
        let settings_path = settings_path()?;
        let (settings, warning) = load_settings(&settings_path);
        if let Some(warning) = warning {
            eprintln!("level=warn event=settings_load_failed message={warning:?} action=defaults");
        }
        let bindings_path = default_store_path()?;
        let binding_store = load_or_create_store(&bindings_path)?;
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
        let config = RuntimeConfig {
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
        Ok(Self {
            runtime,
            tray: None,
            tray_icons: None,
            items: None,
            last_revision: u64::MAX,
            last_model: None,
            next_poll: Instant::now(),
            logger: StatusLogger::new()?,
            settings,
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
        })
    }

    #[allow(clippy::too_many_lines)] // Native menu construction keeps item ownership and order together.
    fn create_tray(&mut self) -> Result<(), String> {
        let bridge = MenuItem::new("Bridge: Starting", false, None);
        let status = MenuItem::new("Status: Looking for hardware", false, None);
        let input = MenuItem::new("Input: Discovering", false, None);
        let controller = MenuItem::new("Controller: Not connected", false, None);
        let xiao = MenuItem::new("XIAO: Discovering", false, None);
        let battery = MenuItem::new("Battery: Unknown", false, None);
        let haptics = MenuItem::new("Haptics: Idle", false, None);
        let bindings = MenuItem::new("Bindings: Disabled", false, None);
        let automatic_shutdown = MenuItem::new("Auto shutdown: Idle 0:00 / 15:00", false, None);
        let problem = MenuItem::new("Problem: None", false, None);
        let run_toggle = MenuItem::with_id(RUN_TOGGLE_ID, "Start Bridge", false, None);
        let copy_error = MenuItem::with_id(COPY_ERROR_ID, "Copy Full Error", false, None);
        let copy = MenuItem::with_id(COPY_ID, "Copy Diagnostics", true, None);
        let settings = MenuItem::with_id(SETTINGS_ID, "Open Input Monitoring Settings", true, None);
        let accessibility =
            MenuItem::with_id(ACCESSIBILITY_ID, "Open Accessibility Settings", true, None);
        let enable_bindings =
            MenuItem::with_id(ENABLE_BINDINGS_ID, "Request Permissions…", true, None);
        let edit_bindings = MenuItem::with_id(EDIT_BINDINGS_ID, "Edit Bindings…", true, None);
        let logs = MenuItem::with_id(LOGS_ID, "Open Log Folder", true, None);
        let quit = MenuItem::with_id(QUIT_ID, "Quit", true, None);
        let idle_shutdown = vec![
            (
                None,
                CheckMenuItem::with_id(
                    IDLE_NEVER_ID,
                    "Never",
                    true,
                    self.settings.idle_shutdown_minutes.is_none(),
                    None,
                ),
            ),
            (
                Some(5),
                CheckMenuItem::with_id(
                    IDLE_5_ID,
                    "5 minutes",
                    true,
                    self.settings.idle_shutdown_minutes == Some(5),
                    None,
                ),
            ),
            (
                Some(10),
                CheckMenuItem::with_id(
                    IDLE_10_ID,
                    "10 minutes",
                    true,
                    self.settings.idle_shutdown_minutes == Some(10),
                    None,
                ),
            ),
            (
                Some(15),
                CheckMenuItem::with_id(
                    IDLE_15_ID,
                    "15 minutes",
                    true,
                    self.settings.idle_shutdown_minutes == Some(15),
                    None,
                ),
            ),
            (
                Some(30),
                CheckMenuItem::with_id(
                    IDLE_30_ID,
                    "30 minutes",
                    true,
                    self.settings.idle_shutdown_minutes == Some(30),
                    None,
                ),
            ),
        ];
        let idle_submenu = Submenu::with_items(
            "Idle Shutdown",
            true,
            &idle_shutdown
                .iter()
                .map(|(_, item)| item as &dyn tray_icon::menu::IsMenuItem)
                .collect::<Vec<_>>(),
        )
        .map_err(|error| error.to_string())?;
        let puck_dock = CheckMenuItem::with_id(
            PUCK_DOCK_ID,
            "Turn Off When Placed on Puck",
            true,
            self.settings.power_off_on_puck,
            None,
        );
        let overlay_enabled = CheckMenuItem::with_id(
            OVERLAY_ENABLED_ID,
            "Hold Quick Access for Profile Wheel",
            true,
            self.settings.profile_overlay_enabled,
            None,
        );
        let overlay_hold: Vec<(u64, CheckMenuItem)> = OVERLAY_HOLD_CHOICES
            .into_iter()
            .map(|milliseconds| {
                (
                    milliseconds,
                    CheckMenuItem::with_id(
                        format!("{OVERLAY_HOLD_PREFIX}{milliseconds}"),
                        format!("{} seconds", milliseconds / 1_000),
                        true,
                        self.settings.profile_overlay_hold_ms == milliseconds,
                        None,
                    ),
                )
            })
            .collect();
        let overlay_hold_submenu = Submenu::with_items(
            "Hold Duration",
            true,
            &overlay_hold
                .iter()
                .map(|(_, item)| item as &dyn tray_icon::menu::IsMenuItem)
                .collect::<Vec<_>>(),
        )
        .map_err(|error| error.to_string())?;
        let overlay_submenu = Submenu::with_items(
            "Profile Wheel",
            true,
            &[
                &overlay_enabled as &dyn tray_icon::menu::IsMenuItem,
                &overlay_hold_submenu,
            ],
        )
        .map_err(|error| error.to_string())?;
        let binding_profiles =
            binding_profile_menu_items(&self.binding_store, &self.settings.active_binding_profile);
        let bindings_submenu = Submenu::new("Bindings", true);
        for (_, item) in &binding_profiles {
            bindings_submenu
                .append(item)
                .map_err(|error| error.to_string())?;
        }
        bindings_submenu
            .append(&PredefinedMenuItem::separator())
            .map_err(|error| error.to_string())?;
        bindings_submenu
            .append(&edit_bindings)
            .map_err(|error| error.to_string())?;
        // Everything that asks macOS for a permission, or sends you to the
        // pane that grants it, lives together rather than being scattered
        // through the menu.
        let permissions_submenu = Submenu::with_items(
            "Permissions",
            true,
            &[
                &enable_bindings,
                &PredefinedMenuItem::separator(),
                &settings,
                &accessibility,
            ],
        )
        .map_err(|error| error.to_string())?;
        let separators: [PredefinedMenuItem; 6] =
            std::array::from_fn(|_| PredefinedMenuItem::separator());
        let menu = Menu::with_items(&[
            &bridge,
            &status,
            &separators[0],
            &controller,
            &input,
            &xiao,
            &battery,
            &haptics,
            &bindings,
            &separators[1],
            &problem,
            &copy_error,
            &separators[2],
            &run_toggle,
            &separators[3],
            &automatic_shutdown,
            &idle_submenu,
            &puck_dock,
            &separators[4],
            &bindings_submenu,
            &overlay_submenu,
            &permissions_submenu,
            &copy,
            &logs,
            &separators[5],
            &quit,
        ])
        .map_err(|error| error.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Steam Controller Bridge")
            .with_icon(template_icon(TrayState::Waiting)?)
            .with_icon_as_template(true)
            .build()
            .map_err(|error| error.to_string())?;
        let tray_icons = NativeTrayIcons::capture(&tray)?;
        self.items = Some(MenuItems {
            bridge,
            status,
            input,
            controller,
            xiao,
            battery,
            haptics,
            bindings,
            automatic_shutdown,
            problem,
            run_toggle,
            copy_error,
            idle_shutdown,
            puck_dock,
            bindings_submenu,
            binding_profiles,
            overlay_enabled,
            overlay_hold,
        });
        self.tray_icons = Some(tray_icons);
        self.tray = Some(tray);
        self.refresh_status();
        Ok(())
    }

    fn refresh_status(&mut self) {
        let status = self.runtime.status();
        if let Err(error) = self.logger.write_status(&status) {
            eprintln!("cannot write menu-app diagnostics: {error}");
        }
        self.sync_overlay_process(&status);
        if status.revision == self.last_revision {
            return;
        }
        let model = MenuModel::from_status(&status);
        let icon_changed = self
            .last_model
            .as_ref()
            .is_none_or(|previous| previous.tray_state != model.tray_state);
        if self.last_model.as_ref() != Some(&model) {
            if let Some(items) = &self.items {
                items.bridge.set_text(&model.bridge);
                items.status.set_text(&model.status);
                items.input.set_text(&model.input);
                items.controller.set_text(&model.controller);
                items.xiao.set_text(&model.xiao);
                items.battery.set_text(&model.battery);
                items.haptics.set_text(&model.haptics);
                items.bindings.set_text(&model.bindings);
                items.automatic_shutdown.set_text(&model.automatic_shutdown);
                items.problem.set_text(&model.problem);
                items.run_toggle.set_text(model.run_action.label());
                items.run_toggle.set_enabled(model.run_enabled);
                items.copy_error.set_enabled(model.has_error);
            }
            if let Some(tray) = &self.tray {
                if icon_changed {
                    if let Some(icons) = &self.tray_icons {
                        icons.install(model.tray_state);
                    }
                }
                let _ = tray.set_tooltip(Some(model.tray_state.tooltip()));
            }
            self.last_model = Some(model);
        }
        self.last_revision = status.revision;
    }

    #[allow(clippy::too_many_lines)] // One dispatch table; splitting it hides the menu's shape.
    fn handle_menu_event(&mut self, id: &str, event_loop: &ActiveEventLoop) {
        match id {
            RUN_TOGGLE_ID => {
                // One control: what it does depends on what the bridge is
                // doing, which is what its label already says.
                let starts = self
                    .last_model
                    .as_ref()
                    .is_none_or(|model| model.run_action == RunAction::Start);
                let result = if starts {
                    self.runtime.request_start()
                } else {
                    self.runtime.request_stop()
                };
                if let Err(error) = result {
                    let action = if starts { "start" } else { "stop" };
                    eprintln!("cannot {action} bridge: {error}");
                }
            }
            IDLE_NEVER_ID | IDLE_5_ID | IDLE_10_ID | IDLE_15_ID | IDLE_30_ID => {
                let minutes = match id {
                    IDLE_NEVER_ID => None,
                    IDLE_5_ID => Some(5),
                    IDLE_10_ID => Some(10),
                    IDLE_15_ID => Some(15),
                    IDLE_30_ID => Some(30),
                    _ => unreachable!(),
                };
                let timeout = minutes.map(|minutes| Duration::from_secs(minutes * 60));
                if let Err(error) = self.runtime.request_set_idle_shutdown_timeout(timeout) {
                    eprintln!("cannot update idle shutdown: {error}");
                } else {
                    self.settings.idle_shutdown_minutes = minutes;
                    self.update_setting_checkmarks();
                    if let Err(error) = save_settings(&self.settings_path, &self.settings) {
                        eprintln!("cannot save menu settings: {error}");
                    }
                }
            }
            PUCK_DOCK_ID => {
                self.settings.power_off_on_puck = !self.settings.power_off_on_puck;
                let action = if self.settings.power_off_on_puck {
                    PuckDockAction::PowerOff
                } else {
                    PuckDockAction::LeaveOn
                };
                if let Err(error) = self.runtime.request_set_puck_dock_action(action) {
                    self.settings.power_off_on_puck = !self.settings.power_off_on_puck;
                    eprintln!("cannot update Puck dock action: {error}");
                } else if let Err(error) = save_settings(&self.settings_path, &self.settings) {
                    eprintln!("cannot save menu settings: {error}");
                }
                self.update_setting_checkmarks();
            }
            COPY_ERROR_ID => {
                if let Some(error) = self.runtime.status().last_error {
                    if let Err(copy_error) = copy_text(&error) {
                        eprintln!("cannot copy full error: {copy_error}");
                    }
                }
            }
            COPY_ID => {
                if let Err(error) = copy_diagnostics(&self.runtime.status()) {
                    eprintln!("cannot copy diagnostics: {error}");
                }
            }
            SETTINGS_ID => open_privacy_pane(PrivacyPane::InputMonitoring),
            ACCESSIBILITY_ID => open_privacy_pane(PrivacyPane::Accessibility),
            ENABLE_BINDINGS_ID => {
                self.request_permissions_in_order(true);
            }
            EDIT_BINDINGS_ID => {
                if let Err(error) = launch_bindings_editor() {
                    eprintln!("cannot launch bindings editor: {error}");
                }
            }
            LOGS_ID => {
                if let Err(error) = open_path(&self.logger.directory.to_string_lossy()) {
                    eprintln!("cannot open log folder: {error}");
                }
            }
            QUIT_ID => {
                self.shutdown();
                event_loop.exit();
            }
            OVERLAY_ENABLED_ID => {
                self.settings.profile_overlay_enabled = !self.settings.profile_overlay_enabled;
                if !self.apply_picker_settings() {
                    self.settings.profile_overlay_enabled = !self.settings.profile_overlay_enabled;
                    self.update_setting_checkmarks();
                }
                self.sync_picker_roster();
            }
            _ if id.starts_with(BINDING_PROFILE_PREFIX) => {
                let profile_id = &id[BINDING_PROFILE_PREFIX.len()..];
                self.select_binding_profile(profile_id);
            }
            _ if id.starts_with(OVERLAY_HOLD_PREFIX) => {
                let Ok(milliseconds) = id[OVERLAY_HOLD_PREFIX.len()..].parse::<u64>() else {
                    return;
                };
                if !OVERLAY_HOLD_CHOICES.contains(&milliseconds) {
                    return;
                }
                let previous = self.settings.profile_overlay_hold_ms;
                self.settings.profile_overlay_hold_ms = milliseconds;
                if !self.apply_picker_settings() {
                    self.settings.profile_overlay_hold_ms = previous;
                    self.update_setting_checkmarks();
                }
            }
            _ => {}
        }
    }

    fn shutdown(&mut self) {
        if self.shutting_down {
            return;
        }
        self.shutting_down = true;
        self.overlay.stop();
        if let Err(error) = self.runtime.shutdown() {
            eprintln!("bridge shutdown failed: {error}");
        }
    }

    fn update_setting_checkmarks(&self) {
        if let Some(items) = &self.items {
            for (minutes, item) in &items.idle_shutdown {
                item.set_checked(*minutes == self.settings.idle_shutdown_minutes);
            }
            items.puck_dock.set_checked(self.settings.power_off_on_puck);
            for (profile_id, item) in &items.binding_profiles {
                item.set_checked(*profile_id == self.settings.active_binding_profile);
            }
            items
                .overlay_enabled
                .set_checked(self.settings.profile_overlay_enabled);
            for (milliseconds, item) in &items.overlay_hold {
                item.set_checked(*milliseconds == self.settings.profile_overlay_hold_ms);
            }
        }
    }

    /// Applies a change to the wheel's configuration, everywhere it is needed.
    ///
    /// Returns whether the runtime accepted it. Nothing is persisted on a
    /// refusal — the caller reverts its settings change, so the menu never
    /// claims a wheel the running bridge does not have.
    fn apply_picker_settings(&mut self) -> bool {
        let accepted = match self
            .runtime
            .request_set_picker_config(self.settings.picker_config())
        {
            Ok(()) => true,
            Err(error) => {
                eprintln!("cannot update the profile wheel: {error}");
                false
            }
        };
        if accepted {
            if let Err(error) = save_settings(&self.settings_path, &self.settings) {
                eprintln!("cannot save profile wheel settings: {error}");
            }
        }
        self.update_setting_checkmarks();
        if !self.settings.profile_overlay_enabled {
            self.overlay.stop();
        }
        accepted
    }

    /// Republishes the profile list after the store or the active profile moved.
    ///
    /// The runtime is told only how many there are; the overlay is told their
    /// names. Splitting it that way keeps profile names out of the runtime,
    /// which has no use for them.
    fn sync_picker_roster(&mut self) {
        // Finish every event the old generation already published before the
        // runtime switches away from it. The blocking acknowledgement then
        // guarantees any concurrently emitted old event is already in this
        // bounded mailbox, so the second drain resolves it against old ids.
        self.drain_picker_events();
        let previous_revision = self.picker_roster_revision;
        let revision = previous_revision.wrapping_add(1);
        let roster = picker_roster(
            &self.binding_store,
            &self.settings.active_binding_profile,
            revision,
        );
        if let Err(error) = self.runtime.set_picker_roster(roster) {
            eprintln!("cannot publish the profile wheel roster: {error}");
            return;
        }
        self.drain_picker_events();
        // A drained commit can synchronously select a profile and publish a
        // newer roster. That nested update supersedes this one.
        if self.picker_roster_revision != previous_revision {
            return;
        }
        // The snapshot a later `Commit { index }` resolves against, taken at
        // the same moment the runtime and the overlay learn this roster.
        self.picker_roster_ids = self
            .binding_store
            .profiles
            .iter()
            .map(|profile| profile.id.clone())
            .collect();
        self.picker_roster_revision = revision;
        let names = self
            .binding_store
            .profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect();
        // `picker_config` is already sanitized; the default is for the wheel
        // switched off, where the overlay still wants a plausible layout.
        let sectors = self
            .settings
            .picker_config()
            .unwrap_or_default()
            .sectors_per_page;
        self.overlay.set_roster(names, roster.active, sectors);
    }

    /// Handles everything the runtime's wheel reports.
    ///
    /// The overlay process is started here and torn down here, so no window and
    /// no process exist at rest. It is started halfway through the hold, which
    /// leaves it roughly a second to be ready -- several times what it needs --
    /// and means an ordinary Quick Access press never starts anything.
    fn handle_picker_event(&mut self, event: PickerEvent) {
        // A queued event can be drained after a quit has begun or after the
        // wheel was switched off; neither may resurrect the overlay process
        // the teardown just killed.
        if self.shutting_down {
            return;
        }
        if !self.settings.profile_overlay_enabled {
            self.overlay.stop();
            return;
        }
        match event {
            PickerEvent::Preparing => self.overlay.start(),
            PickerEvent::Opened {
                selected,
                page,
                roster_revision,
            } if roster_revision == self.picker_roster_revision => {
                // Idempotent, and the safety net for a `Preparing` that never
                // arrived because reports were sparse enough to skip past it.
                self.overlay.start();
                self.overlay.show(selected, page);
            }
            PickerEvent::Selection {
                selected,
                page,
                roster_revision,
            } if roster_revision == self.picker_roster_revision => {
                self.overlay.show(selected, page);
            }
            PickerEvent::Opened {
                roster_revision, ..
            }
            | PickerEvent::Selection {
                roster_revision, ..
            } => {
                eprintln!(
                    "level=warn event=stale_profile_wheel_visual_event event_revision={roster_revision} current_revision={}",
                    self.picker_roster_revision
                );
            }
            PickerEvent::Commit {
                index,
                roster_revision,
            } => {
                // Killing the process takes the window with it, which is both
                // instant and the only way to leave nothing behind.
                self.overlay.stop();
                // Resolved against the roster the wheel was actually showing,
                // not the live store: an external edit can reorder the store
                // between the publish and the press, and an index into the
                // wrong list would silently apply the wrong profile.
                let Some(profile_id) = resolve_picker_commit(
                    &self.picker_roster_ids,
                    self.picker_roster_revision,
                    roster_revision,
                    index,
                )
                .map(str::to_owned) else {
                    eprintln!(
                        "level=warn event=profile_wheel_commit_unknown index={index} event_revision={roster_revision} current_revision={}",
                        self.picker_roster_revision
                    );
                    return;
                };
                // The same path the tray submenu uses, so the checkmark, the
                // settings file, and the permission chain all stay in step.
                self.select_binding_profile(&profile_id);
            }
            // Either way no wheel is coming, so the overlay goes away. A tap
            // normally has nothing to stop, being far shorter than the half
            // hold that starts one, and the runtime has already replayed its
            // press to the desktop bindings.
            PickerEvent::Dismissed | PickerEvent::TriggerTapped => self.overlay.stop(),
        }
    }

    fn drain_picker_events(&mut self) {
        while let Some(event) = self.picker_events.pop() {
            self.handle_picker_event(event);
        }
    }

    /// Tears the overlay down when it can no longer be wanted.
    ///
    /// Starting is driven entirely by the wheel's own events, so this never
    /// starts anything: it is the backstop for a controller that vanishes or a
    /// feature switched off while the wheel is up, either of which would
    /// otherwise strand a window on screen.
    fn sync_overlay_process(&mut self, status: &BridgeStatus) {
        let wanted = self.settings.profile_overlay_enabled && status.controller.connected;
        if !wanted && self.overlay.is_running() {
            self.overlay.stop();
        }
    }

    fn select_binding_profile(&mut self, profile_id: &str) {
        if self
            .settings
            .active_binding_profile
            .eq_ignore_ascii_case(profile_id)
        {
            return;
        }
        let Some(profile) = self.binding_store.profile_by_id(profile_id).cloned() else {
            return;
        };
        if let Err(error) = self
            .runtime
            .request_set_binding_profile(Some(profile.clone()))
        {
            eprintln!("cannot switch binding profile: {error}");
            return;
        }
        self.settings.active_binding_profile = profile.id;
        self.update_setting_checkmarks();
        if let Err(error) = save_settings(&self.settings_path, &self.settings) {
            eprintln!("cannot save active binding profile: {error}");
        }
        // The wheel highlights whichever profile is in use, so it has to learn
        // about a switch however that switch was made.
        self.sync_picker_roster();
        self.request_permissions_in_order(false);
    }

    fn reload_bindings_if_changed(&mut self) {
        let fingerprint = match bindings_file_fingerprint(&self.bindings_path) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                eprintln!("cannot inspect binding profiles: {error}");
                return;
            }
        };
        if fingerprint == self.bindings_file_fingerprint {
            return;
        }
        let bytes = match fs::read(&self.bindings_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("cannot reload binding profiles: {error}");
                return;
            }
        };
        let store = match parse_store(&bytes) {
            Ok(store) => store,
            Err(error) => {
                eprintln!(
                    "level=warn event=binding_profiles_reload_failed error={error:?} action=keep_previous"
                );
                return;
            }
        };
        let profile = store
            .profile_by_id(&self.settings.active_binding_profile)
            .or_else(|| store.profiles.first())
            .cloned();
        if let Some(profile) = profile {
            let current = self
                .binding_store
                .profile_by_id(&self.settings.active_binding_profile);
            if current != Some(&profile) {
                if let Err(error) = self
                    .runtime
                    .request_set_binding_profile(Some(profile.clone()))
                {
                    eprintln!("cannot apply reloaded binding profile: {error}");
                    return;
                }
                self.request_permissions_in_order(false);
            }
            self.settings.active_binding_profile.clone_from(&profile.id);
            if let Err(error) = save_settings(&self.settings_path, &self.settings) {
                eprintln!("cannot save active binding profile: {error}");
            }
        }
        self.binding_store = store;
        self.bindings_file_fingerprint = fingerprint;
        if let Err(error) = self.rebuild_bindings_submenu() {
            eprintln!("cannot rebuild Bindings menu: {error}");
        }
        // Profiles may have been added, renamed, or deleted, so the wheel needs
        // both the new count and the new names.
        self.sync_picker_roster();
    }

    fn rebuild_bindings_submenu(&mut self) -> Result<(), String> {
        let Some(items) = self.items.as_mut() else {
            return Ok(());
        };
        while items.bindings_submenu.remove_at(0).is_some() {}
        items.binding_profiles =
            binding_profile_menu_items(&self.binding_store, &self.settings.active_binding_profile);
        for (_, item) in &items.binding_profiles {
            items
                .bindings_submenu
                .append(item)
                .map_err(|error| error.to_string())?;
        }
        // The permission items live in their own submenu, so this one only
        // carries the profiles and the editor.
        let separator = PredefinedMenuItem::separator();
        let edit = MenuItem::with_id(EDIT_BINDINGS_ID, "Edit Bindings…", true, None);
        for item in [&separator as &dyn tray_icon::menu::IsMenuItem, &edit] {
            items
                .bindings_submenu
                .append(item)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Walks the permission chain, asking macOS for whatever is missing.
    ///
    /// `interactive` marks the run as something the user just asked for. Only
    /// then may this open a System Settings pane: macOS shows no dialog for a
    /// permission it has already recorded a refusal for, so the pane is the
    /// only way forward -- but opening it on every launch would be obnoxious.
    fn request_permissions_in_order(&mut self, interactive: bool) {
        // Ask macOS directly rather than inferring the grant from a controller
        // having opened: the two are different questions, and inferring it left
        // this doing nothing at all whenever no controller was attached.
        let input_monitoring = input_monitoring_access() == PermissionState::Granted;
        // Do not even query later TCC services until the preceding service is
        // granted. macOS can otherwise register simultaneous requests as
        // denied without presenting the original Input Monitoring prompt.
        let mut post_event = input_monitoring && preflight_post_event_access();
        let mut accessibility = post_event && preflight_accessibility_access();
        let mut input_monitoring_granted = input_monitoring;

        loop {
            match permission_stage(input_monitoring_granted, post_event, accessibility) {
                PermissionStage::InputMonitoring => {
                    // An undecided permission produces macOS's dialog. A
                    // refusal it already recorded produces nothing, so the
                    // only way forward is the settings pane.
                    let undecided = input_monitoring_access() == PermissionState::Undecided;
                    let granted = undecided && request_input_monitoring_access();
                    self.permission_request_pending =
                        (!granted).then_some(PermissionStage::InputMonitoring);
                    eprintln!(
                        "level=info event=input_monitoring_permission_requested \
                         granted={granted} undecided={undecided} api=IOHIDRequestAccess"
                    );
                    if !granted {
                        if interactive && !undecided {
                            open_privacy_pane(PrivacyPane::InputMonitoring);
                        }
                        return;
                    }
                    input_monitoring_granted = true;
                }
                PermissionStage::PostEvent => {
                    let granted = request_post_event_access();
                    self.permission_request_pending =
                        (!granted).then_some(PermissionStage::PostEvent);
                    eprintln!(
                        "level=info event=post_event_permission_requested granted={granted} \
                         api=CGRequestPostEventAccess"
                    );
                    if !granted {
                        if interactive {
                            open_privacy_pane(PrivacyPane::Accessibility);
                        }
                        return;
                    }
                    post_event = true;
                    accessibility = preflight_accessibility_access();
                }
                PermissionStage::Accessibility => {
                    let granted = request_accessibility_access();
                    self.permission_request_pending =
                        (!granted).then_some(PermissionStage::Accessibility);
                    eprintln!(
                        "level=info event=accessibility_permission_requested granted={granted} \
                         api=AXIsProcessTrustedWithOptions"
                    );
                    if !granted {
                        if interactive {
                            open_privacy_pane(PrivacyPane::Accessibility);
                        }
                        return;
                    }
                    accessibility = true;
                }
                PermissionStage::Ready => {
                    self.permission_request_pending = None;
                    self.activate_desktop_bindings_after_permission();
                    return;
                }
            }
        }
    }

    fn observe_permission_grants(&mut self) {
        match self.permission_request_pending {
            Some(PermissionStage::InputMonitoring)
                if input_monitoring_access() == PermissionState::Granted =>
            {
                self.permission_request_pending = None;
                eprintln!("level=info event=input_monitoring_permission_granted");
                self.request_permissions_in_order(false);
            }
            Some(PermissionStage::PostEvent) if preflight_post_event_access() => {
                self.permission_request_pending = None;
                eprintln!("level=info event=post_event_permission_granted");
                self.request_permissions_in_order(false);
            }
            Some(PermissionStage::Accessibility) if preflight_accessibility_access() => {
                self.permission_request_pending = None;
                eprintln!("level=info event=accessibility_permission_granted");
                self.activate_desktop_bindings_after_permission();
            }
            _ => {}
        }
    }

    fn activate_desktop_bindings_after_permission(&self) {
        if let Err(error) = self.runtime.request_enable_desktop_bindings() {
            eprintln!("cannot activate desktop bindings after Accessibility grant: {error}");
        }
    }
}

struct NativeTrayIcons {
    button: Retained<NSStatusBarButton>,
    off: Retained<NSImage>,
    waiting: Retained<NSImage>,
    ready: Retained<NSImage>,
    error: Retained<NSImage>,
}

impl NativeTrayIcons {
    fn capture(tray: &TrayIcon) -> Result<Self, String> {
        let mtm =
            MainThreadMarker::new().ok_or("menu-bar icons must be created on the main thread")?;
        let status_item = tray
            .ns_status_item()
            .ok_or("tray-icon did not create a native macOS status item")?;
        let button = status_item
            .button(mtm)
            .ok_or("the native macOS status item has no button")?;
        let waiting = button
            .image()
            .ok_or("tray-icon did not install the initial menu-bar image")?;
        let render = |state| {
            tray.set_icon_with_as_template(Some(template_icon(state)?), true)
                .map_err(|error| error.to_string())?;
            button
                .image()
                .ok_or_else(|| "tray-icon did not install a menu-bar status image".to_owned())
        };
        let off = render(TrayState::Off)?;
        let ready = render(TrayState::Ready)?;
        let error = render(TrayState::Error)?;

        let icons = Self {
            button,
            off,
            waiting,
            ready,
            error,
        };
        icons.install(TrayState::Waiting);
        Ok(icons)
    }

    fn install(&self, state: TrayState) {
        let image = match state {
            TrayState::Off => &self.off,
            TrayState::Waiting => &self.waiting,
            TrayState::Ready => &self.ready,
            TrayState::Error => &self.error,
        };
        self.button.setImage(Some(image));
    }
}

impl ApplicationHandler for MenuApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.tray.is_none() {
            if let Err(error) = self.create_tray() {
                eprintln!("cannot create menu-bar icon: {error}");
                self.shutdown();
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
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            self.handle_menu_event(event.id.as_ref(), event_loop);
        }
        // Also drained here so a wake-up that arrives between passes is never
        // left sitting in the channel.
        self.drain_picker_events();
        if Instant::now() >= self.next_poll {
            self.reload_bindings_if_changed();
            self.observe_permission_grants();
            self.refresh_status();
            self.next_poll = Instant::now() + POLL_INTERVAL;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_poll));
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.shutdown();
    }
}

const ICON_LOGICAL_WIDTH: u32 = 24;
const ICON_LOGICAL_HEIGHT: u32 = 18;
const ICON_RENDER_SCALE: u32 = 4;
const ICON_RENDER_SCALE_F32: f32 = 4.0;
const ICON_WIDTH: u32 = ICON_LOGICAL_WIDTH * ICON_RENDER_SCALE;
const ICON_HEIGHT: u32 = ICON_LOGICAL_HEIGHT * ICON_RENDER_SCALE;

fn template_icon(state: TrayState) -> Result<Icon, String> {
    Icon::from_rgba(template_icon_rgba(state), ICON_WIDTH, ICON_HEIGHT)
        .map_err(|error| error.to_string())
}

fn template_icon_rgba(state: TrayState) -> Vec<u8> {
    let mut pixmap =
        Pixmap::new(ICON_WIDTH, ICON_HEIGHT).expect("the fixed menu icon dimensions are valid");
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = true;
    let transform = Transform::from_scale(ICON_RENDER_SCALE_F32, ICON_RENDER_SCALE_F32);

    stroke_icon_path(&mut pixmap, &controller_outline(), &paint, 1.4, transform);
    stroke_icon_path(&mut pixmap, &d_pad(), &paint, 1.3, transform);
    fill_icon_circle(&mut pixmap, &paint, 12.2, 7.1, 0.72, transform);
    fill_icon_circle(&mut pixmap, &paint, 14.2, 8.7, 0.72, transform);

    match state {
        TrayState::Off => {
            stroke_icon_path(&mut pixmap, &off_badge(), &paint, 1.5, transform);
        }
        TrayState::Waiting => {
            for x in [19.3, 21.2, 23.1] {
                fill_icon_circle(&mut pixmap, &paint, x, 9.0, 0.53, transform);
            }
        }
        TrayState::Ready => {
            stroke_icon_path(&mut pixmap, &ready_badge(), &paint, 1.55, transform);
        }
        TrayState::Error => {
            stroke_icon_path(&mut pixmap, &error_badge(), &paint, 1.55, transform);
            fill_icon_circle(&mut pixmap, &paint, 21.2, 12.8, 0.68, transform);
        }
    }

    pixmap.take()
}

fn controller_outline() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(5.5, 2.4);
    path.cubic_to(3.7, 2.4, 2.3, 3.6, 1.9, 5.3);
    path.line_to(0.65, 11.6);
    path.cubic_to(0.2, 13.8, 1.3, 15.9, 3.05, 16.45);
    path.cubic_to(4.5, 16.9, 5.5, 15.8, 6.25, 14.35);
    path.line_to(7.25, 12.5);
    path.cubic_to(7.55, 11.9, 7.95, 11.7, 8.55, 11.7);
    path.line_to(9.35, 11.7);
    path.cubic_to(9.95, 11.7, 10.35, 11.9, 10.65, 12.5);
    path.line_to(11.65, 14.35);
    path.cubic_to(12.4, 15.8, 13.4, 16.9, 14.85, 16.45);
    path.cubic_to(16.6, 15.9, 17.7, 13.8, 17.25, 11.6);
    path.line_to(16.0, 5.3);
    path.cubic_to(15.6, 3.6, 14.2, 2.4, 12.4, 2.4);
    path.close();
    path.finish()
        .expect("the static controller outline is a valid path")
}

fn d_pad() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(5.15, 6.4);
    path.line_to(5.15, 9.6);
    path.move_to(3.55, 8.0);
    path.line_to(6.75, 8.0);
    path.finish().expect("the static d-pad is a valid path")
}

fn off_badge() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(19.3, 7.1);
    path.line_to(22.9, 10.9);
    path.move_to(22.9, 7.1);
    path.line_to(19.3, 10.9);
    path.finish().expect("the static off badge is a valid path")
}

fn ready_badge() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(19.1, 9.1);
    path.line_to(20.7, 10.7);
    path.line_to(23.1, 6.8);
    path.finish()
        .expect("the static ready badge is a valid path")
}

fn error_badge() -> SkiaPath {
    let mut path = PathBuilder::new();
    path.move_to(21.2, 5.5);
    path.line_to(21.2, 10.2);
    path.finish()
        .expect("the static error badge is a valid path")
}

fn stroke_icon_path(
    pixmap: &mut Pixmap,
    path: &SkiaPath,
    paint: &Paint<'_>,
    width: f32,
    transform: Transform,
) {
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(path, paint, &stroke, transform, None);
}

fn fill_icon_circle(
    pixmap: &mut Pixmap,
    paint: &Paint<'_>,
    x: f32,
    y: f32,
    radius: f32,
    transform: Transform,
) {
    let path =
        PathBuilder::from_circle(x, y, radius).expect("the static icon circle is a valid path");
    pixmap.fill_path(&path, paint, FillRule::Winding, transform, None);
}

struct StatusLogger {
    directory: PathBuf,
    path: PathBuf,
    started: Instant,
    tracker: StatusLogTracker,
    pending_batch: Option<String>,
}

impl StatusLogger {
    fn new() -> Result<Self, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is not set; cannot locate the user log directory")?;
        let directory = home.join("Library/Logs/Steam Controller Bridge");
        fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "cannot create log directory '{}': {error}",
                directory.display()
            )
        })?;
        let path = directory.join("sc-bridge.log");
        Ok(Self {
            directory,
            path,
            started: Instant::now(),
            tracker: StatusLogTracker::default(),
            pending_batch: None,
        })
    }

    fn write_status(&mut self, status: &BridgeStatus) -> Result<(), String> {
        self.flush_pending()?;
        let records = self.tracker.observe(self.started.elapsed(), status);
        if records.is_empty() {
            return Ok(());
        }
        self.write_records(&records, unix_timestamp())
    }

    #[cfg(test)]
    fn write_status_at(
        &mut self,
        status: &BridgeStatus,
        elapsed: Duration,
        timestamp: u64,
    ) -> Result<(), String> {
        self.flush_pending()?;
        let records = self.tracker.observe(elapsed, status);
        if records.is_empty() {
            return Ok(());
        }
        self.write_records(&records, timestamp)
    }

    fn write_records(&mut self, records: &[StatusLogRecord], timestamp: u64) -> Result<(), String> {
        let mut batch = String::new();
        for record in records {
            let _ = writeln!(batch, "timestamp={timestamp} {record}");
        }
        let batch = bounded_log_batch(batch);
        if let Err(error) = write_log_batch(&self.path, &batch) {
            self.pending_batch = Some(batch);
            return Err(error);
        }
        Ok(())
    }

    fn flush_pending(&mut self) -> Result<(), String> {
        let Some(batch) = self.pending_batch.take() else {
            return Ok(());
        };
        if let Err(error) = write_log_batch(&self.path, &batch) {
            self.pending_batch = Some(batch);
            return Err(error);
        }
        Ok(())
    }
}

fn write_log_batch(path: &Path, batch: &str) -> Result<(), String> {
    rotate_log(path, batch.len() as u64)?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    log.write_all(batch.as_bytes())
        .map_err(|error| error.to_string())
}

fn bounded_log_batch(mut batch: String) -> String {
    // Writing a log line must not be able to panic. A limit that does not fit
    // usize cannot be exceeded by an in-memory batch anyway, so saturating to
    // usize::MAX simply means "never truncate" on such a platform.
    let limit = usize::try_from(LOG_LIMIT_BYTES).unwrap_or(usize::MAX);
    if batch.len() <= limit {
        return batch;
    }
    let mut end = limit.saturating_sub(LOG_TRUNCATION_MARKER.len());
    while !batch.is_char_boundary(end) {
        end -= 1;
    }
    batch.truncate(end);
    batch.push_str(LOG_TRUNCATION_MARKER);
    batch
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn rotate_log(path: &Path, incoming_bytes: u64) -> Result<(), String> {
    let Ok(metadata) = path.metadata() else {
        return Ok(());
    };
    if metadata.len() == 0 || metadata.len().saturating_add(incoming_bytes) <= LOG_LIMIT_BYTES {
        return Ok(());
    }
    let rotated = path.with_extension("log.1");
    if rotated.exists() {
        fs::remove_file(&rotated).map_err(|error| error.to_string())?;
    }
    fs::rename(path, rotated).map_err(|error| error.to_string())
}

fn diagnostics_text(status: &BridgeStatus) -> String {
    format_status_diagnostics(status)
}

fn copy_diagnostics(status: &BridgeStatus) -> Result<(), String> {
    copy_text(&diagnostics_text(status))
}

fn copy_text(value: &str) -> Result<(), String> {
    let mut process = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    process
        .stdin
        .take()
        .ok_or("pbcopy stdin is unavailable")?
        .write_all(value.as_bytes())
        .map_err(|error| error.to_string())?;
    let exit = process.wait().map_err(|error| error.to_string())?;
    if exit.success() {
        Ok(())
    } else {
        Err(format!("pbcopy exited with {exit}"))
    }
}

/// The System Settings panes a user has to visit when macOS has already
/// recorded a refusal, since nothing can re-prompt after that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrivacyPane {
    InputMonitoring,
    Accessibility,
}

const fn privacy_pane_url(pane: PrivacyPane) -> &'static str {
    match pane {
        PrivacyPane::InputMonitoring => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
        PrivacyPane::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
    }
}

fn open_privacy_pane(pane: PrivacyPane) {
    eprintln!("level=info event=privacy_pane_opened pane={pane:?}");
    if let Err(error) = open_path(privacy_pane_url(pane)) {
        eprintln!("cannot open {pane:?} settings: {error}");
    }
}

fn launch_bindings_editor() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Command::new(executable)
        .arg("--bindings-editor")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn open_path(path: &str) -> Result<(), String> {
    let status = Command::new("/usr/bin/open")
        .arg(path)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_permission_sends_the_user_to_the_pane_that_grants_it() {
        // macOS shows no dialog once it has recorded a refusal, so the pane is
        // the only remaining route, and each permission has its own.
        assert_ne!(
            privacy_pane_url(PrivacyPane::InputMonitoring),
            privacy_pane_url(PrivacyPane::Accessibility),
        );
        for pane in [PrivacyPane::InputMonitoring, PrivacyPane::Accessibility] {
            let url = privacy_pane_url(pane);
            assert!(
                url.starts_with("x-apple.systempreferences:"),
                "{pane:?} must open System Settings, got {url}",
            );
        }
    }

    #[test]
    fn permission_requests_never_skip_input_monitoring_or_post_event() {
        assert_eq!(
            permission_stage(false, false, false),
            PermissionStage::InputMonitoring
        );
        assert_eq!(
            permission_stage(false, true, true),
            PermissionStage::InputMonitoring
        );
        assert_eq!(
            permission_stage(true, false, false),
            PermissionStage::PostEvent
        );
        assert_eq!(
            permission_stage(true, false, true),
            PermissionStage::PostEvent
        );
        assert_eq!(
            permission_stage(true, true, false),
            PermissionStage::Accessibility
        );
        assert_eq!(permission_stage(true, true, true), PermissionStage::Ready);
    }

    fn temporary_settings_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "steam-controller-bridge-{name}-{}-settings.json",
            std::process::id()
        ))
    }

    fn temporary_log_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "steam-controller-bridge-{name}-{}-sc-bridge.log",
            std::process::id()
        ))
    }

    fn test_logger(path: PathBuf) -> StatusLogger {
        StatusLogger {
            directory: path.parent().unwrap().to_path_buf(),
            path,
            started: Instant::now(),
            tracker: StatusLogTracker::default(),
            pending_batch: None,
        }
    }

    #[test]
    fn menu_settings_round_trip_and_invalid_data_falls_back() {
        let path = temporary_settings_path("round-trip");
        let settings = AppSettings {
            version: SETTINGS_VERSION,
            idle_shutdown_minutes: None,
            power_off_on_puck: true,
            active_binding_profile: "gaming".to_owned(),
            profile_overlay_enabled: true,
            profile_overlay_hold_ms: 3_000,
        };
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path), (settings, None));

        fs::write(&path, b"not json").unwrap();
        let (fallback, warning) = load_settings(&path);
        assert_eq!(fallback, AppSettings::default());
        assert!(warning.is_some());

        save_settings(
            &path,
            &AppSettings {
                version: SETTINGS_VERSION + 1,
                idle_shutdown_minutes: Some(1),
                power_off_on_puck: true,
                active_binding_profile: "default".to_owned(),
                ..AppSettings::default()
            },
        )
        .unwrap();
        let (fallback, warning) = load_settings(&path);
        assert_eq!(fallback, AppSettings::default());
        assert!(warning.is_some());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn version_one_settings_migrate_without_losing_shutdown_choices() {
        let path = temporary_settings_path("migration");
        fs::write(
            &path,
            br#"{"version":1,"idle_shutdown_minutes":30,"power_off_on_puck":true}"#,
        )
        .unwrap();
        let (settings, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(settings.version, SETTINGS_VERSION);
        assert_eq!(settings.idle_shutdown_minutes, Some(30));
        assert!(settings.power_off_on_puck);
        assert_eq!(settings.active_binding_profile, "default");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn version_two_settings_migrate_with_the_wheel_switched_off() {
        let path = temporary_settings_path("overlay-migration");
        fs::write(
            &path,
            br#"{"version":2,"idle_shutdown_minutes":10,"power_off_on_puck":false,"active_binding_profile":"gaming"}"#,
        )
        .unwrap();
        let (settings, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(settings.version, SETTINGS_VERSION);
        // Existing choices survive, and the new feature stays off so it cannot
        // take Quick Access away from a binding the user already relies on.
        assert_eq!(settings.idle_shutdown_minutes, Some(10));
        assert_eq!(settings.active_binding_profile, "gaming");
        assert!(!settings.profile_overlay_enabled);
        assert_eq!(settings.profile_overlay_hold_ms, OVERLAY_HOLD_CHOICES[0]);
        assert!(settings.picker_config().is_none());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_hold_duration_the_menu_cannot_offer_falls_back_alone() {
        // A hand-edited hold must not take idle shutdown, the active profile,
        // and the enablement down with it; only the bad field resets.
        let path = temporary_settings_path("overlay-bad-hold");
        fs::write(
            &path,
            br#"{"version":3,"idle_shutdown_minutes":null,"power_off_on_puck":false,"active_binding_profile":"default","profile_overlay_enabled":true,"profile_overlay_hold_ms":45000}"#,
        )
        .unwrap();
        let (settings, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(settings.profile_overlay_hold_ms, default_overlay_hold_ms());
        assert!(settings.profile_overlay_enabled);
        assert_eq!(settings.idle_shutdown_minutes, None);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn an_enabled_wheel_configures_the_chosen_hold() {
        let settings = AppSettings {
            profile_overlay_enabled: true,
            profile_overlay_hold_ms: 3_000,
            ..AppSettings::default()
        };
        let config = settings.picker_config().expect("the wheel is enabled");
        assert_eq!(config.hold, Duration::from_secs(3));
        assert_eq!(
            config.sectors_per_page,
            PickerConfig::default().sectors_per_page
        );
    }

    #[test]
    fn the_roster_reports_the_active_profiles_position() {
        let mut store = BindingStore::default();
        store.create_profile("Gaming").unwrap();
        store.create_profile("Couch").unwrap();
        assert_eq!(store.profiles.len(), 3);

        let second = store.profiles[1].id.clone();
        let roster = picker_roster(&store, &second, 7);
        assert_eq!(roster.len, 3);
        assert_eq!(roster.active, Some(1));
        assert_eq!(roster.revision, 7);
        assert!(roster.is_openable());

        // A profile that no longer exists must not point the wheel somewhere
        // arbitrary; it opens on the first sector instead.
        let roster = picker_roster(&store, "deleted-profile", 8);
        assert_eq!(roster.len, 3);
        assert_eq!(roster.active, None);
        assert_eq!(roster.revision, 8);
    }

    #[test]
    fn a_single_profile_store_cannot_open_the_wheel() {
        let store = BindingStore::default();
        assert_eq!(store.profiles.len(), 1);
        assert!(!picker_roster(&store, &store.profiles[0].id, 0).is_openable());
    }

    #[test]
    fn picker_event_mailbox_coalesces_visual_updates_and_bounds_backlog() {
        let mailbox = PickerEventMailbox::default();
        assert!(mailbox.publish(PickerEvent::Preparing));
        assert!(!mailbox.publish(PickerEvent::Opened {
            selected: 0,
            page: 0,
            roster_revision: 4,
        }));
        assert_eq!(mailbox.len(), 1, "Opened replaces its pending preparation");

        assert!(!mailbox.publish(PickerEvent::Selection {
            selected: 1,
            page: 0,
            roster_revision: 4,
        }));
        assert!(!mailbox.publish(PickerEvent::Selection {
            selected: 2,
            page: 0,
            roster_revision: 4,
        }));
        assert_eq!(mailbox.len(), 2, "only the latest selection is useful");
        assert!(!mailbox.publish(PickerEvent::Commit {
            index: 2,
            roster_revision: 4,
        }));
        assert_eq!(
            mailbox.pop(),
            Some(PickerEvent::Commit {
                index: 2,
                roster_revision: 4,
            }),
            "a terminal event supersedes every pending visual update"
        );
        assert!(mailbox.pop().is_none());

        for _ in 0..=PICKER_EVENT_MAILBOX_CAPACITY {
            let _ = mailbox.publish(PickerEvent::Dismissed);
        }
        assert_eq!(mailbox.len(), PICKER_EVENT_MAILBOX_CAPACITY);
    }

    #[test]
    fn picker_commits_only_resolve_against_the_roster_the_wheel_used() {
        let ids = vec!["default".to_owned(), "gaming".to_owned()];
        assert_eq!(resolve_picker_commit(&ids, 7, 7, 1), Some("gaming"));
        assert_eq!(resolve_picker_commit(&ids, 8, 7, 1), None);
        assert_eq!(resolve_picker_commit(&ids, 7, 7, 2), None);
    }

    #[test]
    fn diagnostics_include_hardware_and_safety_state() {
        let text = diagnostics_text(&BridgeStatus::default());
        assert!(text.contains("source:"));
        assert!(text.contains("xiao:"));
        assert!(text.contains("lizard:"));
        assert!(text.contains("haptics:"));
        assert!(text.contains("automatic_shutdown:"));
        assert!(text.contains("output_diagnostics:"));
    }

    #[test]
    fn menu_logger_writes_periodic_snapshots_without_a_revision_change() {
        let path = temporary_log_path("periodic");
        let _ = fs::remove_file(&path);
        let mut logger = test_logger(path.clone());
        let status = BridgeStatus::default();
        logger
            .write_status_at(&status, Duration::ZERO, 100)
            .unwrap();
        logger
            .write_status_at(&status, bridge_runtime::STATUS_SNAPSHOT_INTERVAL, 400)
            .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("event=status_snapshot").count(), 2);
        assert!(text.contains("reason=startup"));
        assert!(text.contains("reason=periodic"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rotation_keeps_an_error_change_and_snapshot_in_the_same_file() {
        let path = temporary_log_path("rotation");
        let rotated = path.with_extension("log.1");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&rotated);
        let mut logger = test_logger(path.clone());
        let initial = BridgeStatus::default();
        logger
            .write_status_at(&initial, Duration::ZERO, 100)
            .unwrap();
        fs::write(
            &path,
            vec![b'x'; usize::try_from(LOG_LIMIT_BYTES - 16).unwrap()],
        )
        .unwrap();

        let mut failed = initial;
        failed.revision = 1;
        failed.last_error = Some("controller failed".to_owned());
        logger
            .write_status_at(&failed, Duration::from_secs(1), 101)
            .unwrap();

        let active = fs::read_to_string(&path).unwrap();
        assert!(active.contains("event=status_change"));
        assert!(active.contains("event=status_snapshot reason=error"));
        assert!(rotated.metadata().unwrap().len() >= LOG_LIMIT_BYTES - 16);
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(rotated);
    }

    #[test]
    fn failed_writes_are_retried_without_losing_the_record() {
        let directory = std::env::temp_dir().join(format!(
            "steam-controller-bridge-retry-{}",
            std::process::id()
        ));
        let path = directory.join("sc-bridge.log");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&directory);
        let mut logger = test_logger(path.clone());
        let status = BridgeStatus::default();

        assert!(logger
            .write_status_at(&status, Duration::ZERO, 100)
            .is_err());
        fs::create_dir_all(&directory).unwrap();
        logger
            .write_status_at(&status, Duration::from_secs(1), 101)
            .unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("event=status_snapshot").count(), 1);
        assert!(text.contains("timestamp=100"));
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(directory);
    }

    #[test]
    fn oversized_error_batches_are_explicitly_truncated_to_the_log_limit() {
        let path = temporary_log_path("oversized");
        let rotated = path.with_extension("log.1");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&rotated);
        let mut logger = test_logger(path.clone());
        let initial = BridgeStatus::default();
        logger
            .write_status_at(&initial, Duration::ZERO, 100)
            .unwrap();

        let mut failed = initial;
        failed.revision = 1;
        failed.last_error = Some("x".repeat(usize::try_from(LOG_LIMIT_BYTES).unwrap() + 1_024));
        logger
            .write_status_at(&failed, Duration::from_secs(1), 101)
            .unwrap();

        let active = fs::read(&path).unwrap();
        assert_eq!(active.len(), usize::try_from(LOG_LIMIT_BYTES).unwrap());
        assert!(active.ends_with(LOG_TRUNCATION_MARKER.as_bytes()));
        assert!(rotated.metadata().unwrap().len() < LOG_LIMIT_BYTES);
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(rotated);
    }

    /// Copy Diagnostics is what the troubleshooting guide tells users to paste
    /// into a public issue, so no whole serial may reach it. On Bluetooth that
    /// value is the controller's MAC address.
    #[test]
    fn diagnostics_never_expose_a_whole_device_serial() {
        let text = diagnostics_text(&BridgeStatus {
            source: bridge_runtime::ControllerSourceStatus {
                identity: Some(steam_controller_device::HidDeviceInfo {
                    id: "controller-source".to_owned(),
                    path: "controller-source".to_owned(),
                    vendor_id: 0x28de,
                    product_id: 0x1303,
                    usage_page: 0xff00,
                    usage: 1,
                    interface_number: -1,
                    serial_number: Some("a1b2c3d4e5f6".to_owned()),
                    manufacturer: Some("Valve Corporation".to_owned()),
                    product: Some("Steam Ctrl (BT)".to_owned()),
                    transport: "Bluetooth".to_owned(),
                }),
                transport: Some(steam_controller_device::ControllerTransport::Bluetooth),
                connected: true,
                active: true,
            },
            xiao: bridge_runtime::XiaoStatus {
                path: Some("/dev/cu.usbmodem11201".to_owned()),
                usb_serial: Some("5E6EF905E5468F85".to_owned()),
                handshake_complete: true,
            },
            ..BridgeStatus::default()
        });
        assert!(!text.contains("a1b2c3d4e5f6"));
        assert!(text.contains("****e5f6"));
        // The XIAO's MCU serial is a stable hardware identifier too.
        assert!(!text.contains("5E6EF905E5468F85"));
        assert!(text.contains("****8F85"));
        // Transport, product, and port still have to be diagnosable.
        assert!(text.contains("Steam Ctrl (BT)"));
        assert!(text.contains("/dev/cu.usbmodem11201"));
    }

    #[test]
    fn template_icons_are_valid_and_distinct_for_every_state() {
        let states = [
            TrayState::Off,
            TrayState::Waiting,
            TrayState::Ready,
            TrayState::Error,
        ];
        let images: Vec<_> = states
            .iter()
            .map(|state| template_icon_rgba(*state))
            .collect();
        for (state, pixels) in states.iter().zip(&images) {
            assert!(template_icon(*state).is_ok());
            assert_eq!(
                pixels.len(),
                usize::try_from(ICON_WIDTH * ICON_HEIGHT * 4).unwrap()
            );
            assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
            assert!(
                pixels
                    .chunks_exact(4)
                    .any(|pixel| pixel[3] > 0 && pixel[3] < 255),
                "{state:?} should retain anti-aliased edges"
            );
            let occupied_rows: Vec<_> = pixels
                .chunks_exact(usize::try_from(ICON_WIDTH * 4).unwrap())
                .enumerate()
                .filter_map(|(row, pixels)| {
                    pixels
                        .chunks_exact(4)
                        .any(|pixel| pixel[3] > 8)
                        .then_some(row)
                })
                .collect();
            assert!(
                occupied_rows.last().unwrap() - occupied_rows.first().unwrap()
                    >= usize::try_from(14 * ICON_RENDER_SCALE).unwrap(),
                "{state:?} should fill the native menu-bar height"
            );
        }
        for left in 0..images.len() {
            for right in left + 1..images.len() {
                assert_ne!(images[left], images[right]);
            }
        }
    }
}
