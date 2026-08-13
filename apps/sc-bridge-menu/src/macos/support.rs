use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static SETTINGS_TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) const SETTINGS_VERSION: u32 = 4;
/// Hold durations the menu offers, in milliseconds.
pub(super) const OVERLAY_HOLD_CHOICES: [u64; 2] = [2_000, 3_000];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PermissionStage {
    InputMonitoring,
    PostEvent,
    Accessibility,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(super) enum OutputPreference {
    #[default]
    BridgeDevice,
    VirtualHid,
}

impl OutputPreference {
    pub(super) fn runtime_selection(self) -> Result<OutputSelection, String> {
        match self {
            Self::BridgeDevice => Ok(OutputSelection::Serial),
            Self::VirtualHid => bundled_virtual_hid_helper_path()
                .map(VirtualHidConfig::new)
                .map(OutputSelection::VirtualHid),
        }
    }
}

pub(super) const fn permission_stage(
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

pub(super) fn settings_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set; cannot locate the application settings directory")?;
    Ok(home.join("Library/Application Support/Steam Controller Bridge/settings.json"))
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

pub(super) fn bundled_virtual_hid_helper_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate menu executable: {error}"))?;
    bundled_virtual_hid_helper_path_from(&executable)
}

pub(super) fn bundled_virtual_hid_helper_path_from(executable: &Path) -> Result<PathBuf, String> {
    let macos = executable
        .parent()
        .ok_or("menu executable has no parent directory")?;
    let contents = macos
        .parent()
        .ok_or("menu executable is not inside an app Contents directory")?;
    if macos.file_name().and_then(|name| name.to_str()) != Some("MacOS") {
        return Err("menu executable is not in Contents/MacOS".to_owned());
    }
    Ok(contents
        .join("Helpers/Steam Controller Bridge Virtual HID Helper.app")
        .join("Contents/MacOS/sc-virtual-hid-helper"))
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
