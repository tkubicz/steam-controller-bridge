use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::legacy;
use crate::model::{
    BindingProfile, ControlBindings, PadBindings, PadConfig, PadSide, BINDINGS_VERSION,
    MAX_PAD_REGIONS, MAX_PAD_SPEED_PERCENT, MAX_PROFILES, MAX_PROFILE_NAME_CHARS,
    MAX_REGION_NAME_CHARS, MIN_PAD_SPEED_PERCENT,
};

fn valid_identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingStore {
    pub version: u32,
    pub profiles: Vec<BindingProfile>,
}

impl Default for BindingStore {
    fn default() -> Self {
        Self {
            version: BINDINGS_VERSION,
            profiles: vec![BindingProfile::default()],
        }
    }
}

impl BindingStore {
    /// Validates the complete persisted store.
    ///
    /// # Errors
    /// Returns a descriptive error for unsupported versions, invalid names or IDs,
    /// duplicate profiles, or an invalid profile count.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != BINDINGS_VERSION {
            return Err(format!("unsupported bindings version {}", self.version));
        }
        if self.profiles.is_empty() || self.profiles.len() > MAX_PROFILES {
            return Err(format!(
                "bindings must contain 1 to {MAX_PROFILES} profiles"
            ));
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for profile in &self.profiles {
            if !valid_identifier(&profile.id) {
                return Err(format!("invalid profile ID {:?}", profile.id));
            }
            let trimmed = profile.name.trim();
            if trimmed.is_empty()
                || trimmed.chars().count() > MAX_PROFILE_NAME_CHARS
                || trimmed != profile.name
            {
                return Err(format!("invalid profile name {:?}", profile.name));
            }
            if !ids.insert(profile.id.to_ascii_lowercase()) {
                return Err(format!("duplicate profile ID {:?}", profile.id));
            }
            if !names.insert(profile.name.to_lowercase()) {
                return Err(format!("duplicate profile name {:?}", profile.name));
            }
            for side in PadSide::ALL {
                validate_pad(&profile.name, side, profile.pads.get(side))?;
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn profile_by_id(&self, id: &str) -> Option<&BindingProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.id.eq_ignore_ascii_case(id))
    }

    #[must_use]
    pub fn profile_by_name(&self, name: &str) -> Option<&BindingProfile> {
        let folded = name.to_lowercase();
        self.profiles
            .iter()
            .find(|profile| profile.name.to_lowercase() == folded)
    }

    #[must_use]
    pub fn next_profile_id(&self) -> String {
        for suffix in 1_u32.. {
            let candidate = format!("profile-{suffix}");
            if self.profile_by_id(&candidate).is_none() {
                return candidate;
            }
        }
        unreachable!("an unbounded suffix space always has a free profile ID")
    }

    /// Creates an empty profile and returns its immutable generated ID.
    ///
    /// # Errors
    /// Returns an error when the profile limit or name constraints are violated.
    pub fn create_profile(&mut self, name: &str) -> Result<String, String> {
        if self.profiles.len() == MAX_PROFILES {
            return Err(format!("at most {MAX_PROFILES} profiles are supported"));
        }
        let name = self.available_name(name, None)?;
        let id = self.next_profile_id();
        self.profiles.push(BindingProfile {
            id: id.clone(),
            name,
            bindings: ControlBindings::default(),
            pads: PadBindings::default(),
        });
        Ok(id)
    }

    /// Duplicates a profile under a new immutable generated ID.
    ///
    /// # Errors
    /// Returns an error when the source is missing or the limit/name is invalid.
    pub fn duplicate_profile(&mut self, source_id: &str, name: &str) -> Result<String, String> {
        if self.profiles.len() == MAX_PROFILES {
            return Err(format!("at most {MAX_PROFILES} profiles are supported"));
        }
        let source = self
            .profile_by_id(source_id)
            .cloned()
            .ok_or_else(|| format!("profile {source_id:?} does not exist"))?;
        let name = self.available_name(name, None)?;
        let id = self.next_profile_id();
        self.profiles.push(BindingProfile {
            id: id.clone(),
            name,
            bindings: source.bindings,
            pads: source.pads,
        });
        Ok(id)
    }

    /// Renames a profile without changing its persisted ID.
    ///
    /// # Errors
    /// Returns an error when the profile is missing or the new name is invalid.
    pub fn rename_profile(&mut self, id: &str, name: &str) -> Result<(), String> {
        let name = self.available_name(name, Some(id))?;
        let profile = self
            .profiles
            .iter_mut()
            .find(|profile| profile.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| format!("profile {id:?} does not exist"))?;
        profile.name = name;
        Ok(())
    }

    /// Deletes a profile while preserving the required final profile.
    ///
    /// # Errors
    /// Returns an error when the profile is missing or is the final profile.
    pub fn delete_profile(&mut self, id: &str) -> Result<BindingProfile, String> {
        if self.profiles.len() == 1 {
            return Err("the final binding profile cannot be deleted".to_owned());
        }
        let index = self
            .profiles
            .iter()
            .position(|profile| profile.id.eq_ignore_ascii_case(id))
            .ok_or_else(|| format!("profile {id:?} does not exist"))?;
        Ok(self.profiles.remove(index))
    }

    fn available_name(&self, name: &str, excluding_id: Option<&str>) -> Result<String, String> {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_PROFILE_NAME_CHARS {
            return Err(format!(
                "profile names must contain 1 to {MAX_PROFILE_NAME_CHARS} characters"
            ));
        }
        let folded = trimmed.to_lowercase();
        if self.profiles.iter().any(|profile| {
            let excluded = excluding_id.is_some_and(|id| profile.id.eq_ignore_ascii_case(id));
            !excluded && profile.name.to_lowercase() == folded
        }) {
            return Err(format!("duplicate profile name {trimmed:?}"));
        }
        Ok(trimmed.to_owned())
    }
}

fn validate_pad(profile: &str, side: PadSide, pad: &PadConfig) -> Result<(), String> {
    let label = side.label();
    if !(MIN_PAD_SPEED_PERCENT..=MAX_PAD_SPEED_PERCENT).contains(&pad.speed_percent) {
        return Err(format!(
            "profile {profile:?} {label} speed must be between {MIN_PAD_SPEED_PERCENT}% and {MAX_PAD_SPEED_PERCENT}%"
        ));
    }
    if pad.regions.len() > MAX_PAD_REGIONS {
        return Err(format!(
            "profile {profile:?} {label} supports at most {MAX_PAD_REGIONS} regions"
        ));
    }
    let mut names = BTreeSet::new();
    for region in &pad.regions {
        let trimmed = region.name.trim();
        if trimmed.is_empty()
            || trimmed.chars().count() > MAX_REGION_NAME_CHARS
            || trimmed != region.name
        {
            return Err(format!("invalid {label} region name {:?}", region.name));
        }
        if !names.insert(region.name.to_lowercase()) {
            return Err(format!("duplicate {label} region name {:?}", region.name));
        }
        if !region.shape.is_valid() {
            return Err(format!(
                "{label} region {:?} needs a 1-360 degree sweep inside a 0-100% extent band",
                region.name
            ));
        }
    }
    Ok(())
}

/// Returns the standard per-user bindings file location.
///
/// # Errors
/// Returns an error if the current platform's path inputs are unavailable.
pub fn default_store_path() -> Result<PathBuf, String> {
    app_paths::current()
        .map(|paths| paths.bindings_file())
        .map_err(|error| format!("cannot locate the bindings directory: {error}"))
}

/// Loads and validates a binding store.
///
/// # Errors
/// Returns an error when the file cannot be read, decoded, or validated.
pub fn load_store(path: &Path) -> Result<BindingStore, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read '{}': {error}", path.display()))?;
    parse_store_at_path(path, &bytes)
}

/// Decodes and validates a binding store from an in-memory JSON document.
///
/// # Errors
/// Returns an error when the document cannot be decoded or validated.
pub fn parse_store(bytes: &[u8]) -> Result<BindingStore, String> {
    parse_store_with_migration(bytes).map(|(store, _)| store)
}

/// `deny_unknown_fields` stops an older document deserializing into the current
/// shape, so the version is probed before a decoder is chosen.
#[derive(Deserialize)]
struct VersionProbe {
    version: u32,
}

fn parse_store_with_migration(bytes: &[u8]) -> Result<(BindingStore, bool), String> {
    let probe: VersionProbe =
        serde_json::from_slice(bytes).map_err(|error| format!("invalid bindings JSON: {error}"))?;
    let store = match probe.version {
        version @ 1..=4 => legacy::parse_pre_region_store(bytes, version)?,
        BINDINGS_VERSION => serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid bindings JSON: {error}"))?,
        other => return Err(format!("unsupported bindings version {other}")),
    };
    store.validate()?;
    Ok((store, probe.version != BINDINGS_VERSION))
}

/// Loads a store or atomically creates the all-unbound default when missing.
///
/// # Errors
/// Returns an error for I/O, serialization, or validation failures.
pub fn load_or_create_store(path: &Path) -> Result<BindingStore, String> {
    match fs::read(path) {
        Ok(bytes) => parse_store_at_path(path, &bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let store = BindingStore::default();
            save_store(path, &store)?;
            Ok(store)
        }
        Err(error) => Err(format!("cannot read '{}': {error}", path.display())),
    }
}

/// Moves an unloadable store aside and writes a fresh default, returning the
/// path the original was kept at. Nothing is deleted, so this is undoable.
///
/// # Errors
/// Returns an error if the original cannot be moved or the default cannot be
/// written, in which case the original is left exactly where it was.
pub fn reset_store(path: &Path) -> Result<PathBuf, String> {
    let kept = kept_aside_path(path)?;
    fs::rename(path, &kept).map_err(|error| {
        format!(
            "cannot move '{}' to '{}': {error}",
            path.display(),
            kept.display()
        )
    })?;
    if let Err(error) = save_store(path, &BindingStore::default()) {
        // Leave neither a missing file nor an unusable default behind.
        let _ = fs::rename(&kept, path);
        return Err(error);
    }
    Ok(kept)
}

/// The first free `<stem>-invalid[-n].<ext>` beside the original.
fn kept_aside_path(path: &Path) -> Result<PathBuf, String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("bindings path '{}' has no parent", path.display()))?;
    let stem = path.file_stem().map_or_else(
        || "bindings".to_owned(),
        |stem| stem.to_string_lossy().into(),
    );
    let extension = path
        .extension()
        .map_or_else(|| "json".to_owned(), |ext| ext.to_string_lossy().into());
    for suffix in 0_u32.. {
        let name = if suffix == 0 {
            format!("{stem}-invalid.{extension}")
        } else {
            format!("{stem}-invalid-{suffix}.{extension}")
        };
        let candidate = directory.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    unreachable!("an unbounded suffix space always has a free name")
}

fn parse_store_at_path(path: &Path, bytes: &[u8]) -> Result<BindingStore, String> {
    let (store, migrated) = parse_store_with_migration(bytes)
        .map_err(|error| format!("{error} in '{}'", path.display()))?;
    if migrated {
        save_store(path, &store)?;
    }
    Ok(store)
}

/// Validates and atomically saves a binding store.
///
/// # Errors
/// Returns an error for invalid stores or failed directory, write, or rename operations.
pub fn save_store(path: &Path, store: &BindingStore) -> Result<(), String> {
    store.validate()?;
    let directory = path
        .parent()
        .ok_or_else(|| format!("bindings path '{}' has no parent", path.display()))?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temporary = path.with_extension(format!("json.{}.{nonce}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(store).map_err(|error| error.to_string())?;
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
