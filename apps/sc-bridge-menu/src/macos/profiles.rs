#[allow(
    clippy::wildcard_imports,
    reason = "profile synchronization operates on the menu app's private state"
)]
use super::*;

impl MenuApp {
    pub(super) fn update_setting_checkmarks(&self) {
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
    pub(super) fn apply_picker_settings(&mut self) -> bool {
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
    pub(super) fn sync_picker_roster(&mut self) {
        // Finish every event the old generation already published before the
        // runtime switches away from it. The blocking acknowledgement then
        // guarantees any concurrently emitted old event is already in this
        // bounded mailbox, so the second drain resolves it against old ids.
        self.drain_picker_events();
        let previous_revision = self.picker_roster_revision;
        // Spent regardless of outcome: a publish whose acknowledgement timed
        // out may still be applied by the runtime later, and that stale
        // generation must never share a number with a successful one.
        let revision = self.picker_roster_publishes.wrapping_add(1);
        self.picker_roster_publishes = revision;
        let roster = picker_roster(
            &self.binding_store,
            &self.settings.active_binding_profile,
            revision,
        );
        if let Err(error) = self.runtime.set_picker_roster(roster) {
            eprintln!("cannot publish the profile wheel roster: {error}");
            self.picker_roster_dirty = true;
            return;
        }
        self.picker_roster_dirty = false;
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
    pub(super) fn handle_picker_event(&mut self, event: PickerEvent) {
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

    pub(super) fn drain_picker_events(&mut self) {
        while let Some(event) = self.picker_events.pop() {
            self.handle_picker_event(event);
        }
    }

    pub(super) fn flush_overlay_diagnostics(&mut self) {
        let diagnostics = self.overlay.drain_diagnostics();
        if let Err(error) = self.logger.write_diagnostics(&diagnostics) {
            eprintln!("cannot write overlay diagnostics: {error}");
        }
    }

    /// Tears the overlay down when it can no longer be wanted.
    ///
    /// Starting is driven entirely by the wheel's own events, so this never
    /// starts anything: it is the backstop for a controller that vanishes or a
    /// feature switched off while the wheel is up, either of which would
    /// otherwise strand a window on screen.
    pub(super) fn sync_overlay_process(&mut self, status: &BridgeStatus) {
        let wanted = self.settings.profile_overlay_enabled && status.controller.connected;
        if !wanted && self.overlay.is_running() {
            self.overlay.stop();
        }
    }

    pub(super) fn select_binding_profile(&mut self, profile_id: &str) {
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

    pub(super) fn reload_bindings_if_changed(&mut self) {
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
            eprintln!("cannot rebuild Profiles menu: {error}");
        }
        // Profiles may have been added, renamed, or deleted, so the wheel needs
        // both the new count and the new names.
        self.sync_picker_roster();
    }

    pub(super) fn rebuild_bindings_submenu(&mut self) -> Result<(), String> {
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
        let edit = MenuItem::with_id(EDIT_BINDINGS_ID, EDIT_PROFILES_LABEL, true, None);
        for item in [&separator as &dyn tray_icon::menu::IsMenuItem, &edit] {
            items
                .bindings_submenu
                .append(item)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
