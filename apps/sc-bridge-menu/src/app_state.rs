use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bridge_runtime::{
    BridgeHandle, BridgeRuntime, OutputSelection, PendingOutputChange, PendingUpdateResume,
    PickerConfig, PickerEvent, PickerRoster, PuckDockAction, RuntimeConfig,
};
use desktop_bindings::{default_store_path, BindingStore};
use platform_capabilities::{CapabilityId, PlatformCapabilities};
use serde::{Deserialize, Serialize};

use crate::app_center_host::AppCenterHost;
use crate::bindings_recovery::load_store_or_recover;
use crate::overlay_host::OverlayHost;
#[cfg(feature = "updater")]
use crate::update_check::UpdateChecker;

pub(super) const PICKER_EVENT_MAILBOX_CAPACITY: usize = 32;

static SETTINGS_TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) struct AppState {
    pub(super) runtime: BridgeHandle,
    pub(super) settings: AppSettings,
    pub(super) virtual_hid_enabled: bool,
    pub(super) settings_path: PathBuf,
    pub(super) bindings_path: PathBuf,
    pub(super) binding_store: BindingStore,
    pub(super) bindings_file_fingerprint: BindingsFileFingerprint,
    pub(super) capabilities: Box<dyn PlatformCapabilities>,
    pub(super) permission_request_pending: Option<CapabilityId>,
    pub(super) shutting_down: bool,
    pub(super) overlay: OverlayHost,
    pub(super) picker_events: Arc<PickerEventMailbox>,
    pub(super) picker_roster_ids: Vec<String>,
    pub(super) picker_roster_revision: u64,
    pub(super) picker_roster_publishes: u64,
    pub(super) picker_roster_dirty: bool,
    pub(super) editor_children: Vec<Child>,
    pub(super) app_center_host: AppCenterHost,
    pub(super) app_center_recovery: AppCenterRecovery,
    pub(super) output_change: Option<(PendingOutputChange, OutputPreference)>,
    pub(super) output_change_problem: Option<String>,
    #[cfg(feature = "updater")]
    pub(super) update_checker: UpdateChecker,
    #[cfg(feature = "updater")]
    pub(super) last_update_available: Option<bool>,
}

pub(super) enum AppCenterRecovery {
    Idle,
    Waiting {
        request: PendingUpdateResume,
        error: Option<String>,
    },
    Failed(String),
}

impl AppCenterRecovery {
    pub(super) fn problem(&self) -> Option<&str> {
        match self {
            Self::Waiting {
                error: Some(error), ..
            }
            | Self::Failed(error) => Some(error),
            Self::Idle | Self::Waiting { error: None, .. } => None,
        }
    }
}

pub(super) const SETTINGS_VERSION: u32 = 4;
/// Hold durations the menu offers, in milliseconds.
pub(super) const OVERLAY_HOLD_CHOICES: [u64; 2] = [2_000, 3_000];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(super) enum OutputPreference {
    #[default]
    BridgeDevice,
    VirtualHid,
}

impl OutputPreference {
    pub(super) const fn when_virtual_hid_enabled(self, enabled: bool) -> Self {
        if enabled {
            self
        } else {
            Self::BridgeDevice
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct AppSettings {
    pub(super) version: u32,
    pub(super) idle_shutdown_minutes: Option<u64>,
    pub(super) power_off_on_puck: bool,
    #[serde(default)]
    pub(super) output: OutputPreference,
    #[serde(default = "default_binding_profile_id")]
    pub(super) active_binding_profile: String,
    /// Whether holding Quick Access opens the in-game profile wheel. Off by
    /// default: it takes Quick Access over, which existing users have bound.
    #[serde(default)]
    pub(super) profile_overlay_enabled: bool,
    #[serde(default = "default_overlay_hold_ms")]
    pub(super) profile_overlay_hold_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BindingsFileFingerprint {
    pub(super) length: u64,
    pub(super) modified: SystemTime,
}

pub(super) fn bindings_file_fingerprint(path: &Path) -> Result<BindingsFileFingerprint, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let modified = metadata.modified().map_err(|error| error.to_string())?;
    Ok(BindingsFileFingerprint {
        length: metadata.len(),
        modified,
    })
}

pub(super) fn default_binding_profile_id() -> String {
    desktop_bindings::DEFAULT_PROFILE_ID.to_owned()
}

/// Describes the profile store to the wheel: how many there are, and where the
/// active one sits. Names stay here, because only this side can resolve them.
pub(super) fn picker_roster(
    store: &BindingStore,
    active_profile_id: &str,
    revision: u64,
) -> PickerRoster {
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
pub(super) struct PickerEventMailbox {
    pub(super) events: Mutex<VecDeque<PickerEvent>>,
}

impl PickerEventMailbox {
    /// Publishes without blocking the runtime thread. Returns whether the UI
    /// needs a wake-up; one pending wake covers the whole bounded batch.
    pub(super) fn publish(&self, event: PickerEvent) -> bool {
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

    pub(super) fn pop(&self) -> Option<PickerEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

pub(super) fn resolve_picker_commit(
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

pub(super) const fn default_overlay_hold_ms() -> u64 {
    OVERLAY_HOLD_CHOICES[0]
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            idle_shutdown_minutes: Some(15),
            power_off_on_puck: false,
            output: OutputPreference::BridgeDevice,
            active_binding_profile: default_binding_profile_id(),
            profile_overlay_enabled: false,
            profile_overlay_hold_ms: default_overlay_hold_ms(),
        }
    }
}

impl AppSettings {
    /// The wheel's runtime configuration, or `None` when it is switched off.
    pub(super) fn picker_config(&self) -> Option<PickerConfig> {
        self.profile_overlay_enabled.then(|| {
            PickerConfig {
                hold: Duration::from_millis(self.profile_overlay_hold_ms),
                ..PickerConfig::default()
            }
            .sanitized()
        })
    }
}

impl AppState {
    pub(super) fn load(
        virtual_hid_enabled: bool,
        load_capabilities: impl FnOnce() -> Result<Box<dyn PlatformCapabilities>, String>,
        resolve_output: impl FnOnce(&Path, OutputPreference) -> Result<OutputSelection, String>,
        wake: impl Fn() + Send + 'static,
    ) -> Result<Option<Self>, String> {
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
        let capabilities = load_capabilities()?;
        let effective_output = settings
            .output
            .when_virtual_hid_enabled(virtual_hid_enabled);
        let output = resolve_output(&settings_path, effective_output)?;
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
        let picker_roster_ids = binding_store
            .profiles
            .iter()
            .map(|profile| profile.id.clone())
            .collect();
        let picker_events = Arc::new(PickerEventMailbox::default());
        let picker_sender = Arc::clone(&picker_events);
        let runtime = BridgeRuntime::spawn_with_picker(
            config,
            Box::new(move |event| {
                if picker_sender.publish(event) {
                    wake();
                }
            }),
        );
        Ok(Some(Self {
            runtime,
            settings,
            virtual_hid_enabled,
            settings_path,
            bindings_path,
            binding_store,
            bindings_file_fingerprint,
            capabilities,
            permission_request_pending: None,
            shutting_down: false,
            overlay: OverlayHost::new(),
            picker_events,
            picker_roster_ids,
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

pub(super) fn settings_path() -> Result<PathBuf, String> {
    app_paths::current()
        .map(|paths| paths.settings_file())
        .map_err(|error| format!("cannot locate the application settings directory: {error}"))
}

pub(super) fn load_settings(path: &Path) -> (AppSettings, Option<String>) {
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
            if matches!(settings.version, 1 | 2 | 3 | SETTINGS_VERSION)
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

/// Best-effort removal of temporaries left behind when an earlier save died
/// between write and rename; nothing else ever deletes them. May also sweep a
/// concurrent saver's in-flight temporary, which makes that save fail loudly
/// rather than letting two writers race over the same file.
fn remove_stale_settings_temporaries(directory: &Path, target: &Path) {
    let Some(target_name) = target.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let prefix = format!("{target_name}.");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let is_temporary = name.starts_with(&prefix)
            && Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"));
        if is_temporary {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub(super) fn save_settings(path: &Path, settings: &AppSettings) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("settings path '{}' has no parent", path.display()))?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    remove_stale_settings_temporaries(directory, path);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = SETTINGS_TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!(
        "json.{}.{nonce}.{sequence}.tmp",
        std::process::id()
    ));
    let bytes = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    if let Err(error) = fs::write(&temporary, bytes) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
