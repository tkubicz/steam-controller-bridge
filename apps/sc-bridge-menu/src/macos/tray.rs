#[allow(
    clippy::wildcard_imports,
    reason = "tray construction and dispatch share the menu app's private item vocabulary"
)]
use super::*;

impl MenuApp {
    #[allow(clippy::too_many_lines)] // Native menu construction keeps item ownership and order together.
    pub(super) fn create_tray(&mut self) -> Result<(), String> {
        let bridge = MenuItem::new("Bridge: Starting", false, None);
        let status = MenuItem::new("Status: Looking for hardware", false, None);
        let input = MenuItem::new("Input: Discovering", false, None);
        let controller = MenuItem::new("Controller: Not connected", false, None);
        let xiao = MenuItem::new("XIAO: Discovering", false, None);
        let firmware = MenuItem::new("Firmware: Unknown", false, None);
        let battery = MenuItem::new("Battery: Unknown", false, None);
        let haptics = MenuItem::new("Haptics: Idle", false, None);
        let current_profile = MenuItem::new("Current Profile: None · Disabled", false, None);
        let automatic_shutdown = MenuItem::new("Auto shutdown: Idle 0:00 / 15:00", false, None);
        let problem = MenuItem::new("Problem: None", false, None);
        let run_toggle = MenuItem::with_id(RUN_TOGGLE_ID, "Start Bridge", false, None);
        let copy_error = MenuItem::with_id(COPY_ERROR_ID, "Copy Full Error", true, None);
        let copy = MenuItem::with_id(COPY_ID, "Copy Diagnostics", true, None);
        let settings = MenuItem::with_id(SETTINGS_ID, "Open Input Monitoring Settings", true, None);
        let accessibility =
            MenuItem::with_id(ACCESSIBILITY_ID, "Open Accessibility Settings", true, None);
        let enable_bindings =
            MenuItem::with_id(ENABLE_BINDINGS_ID, "Request Permissions…", true, None);
        let edit_profiles = MenuItem::with_id(EDIT_BINDINGS_ID, EDIT_PROFILES_LABEL, true, None);
        let logs = MenuItem::with_id(LOGS_ID, "Open Log Folder", true, None);
        let updates = MenuItem::with_id(
            UPDATES_ID,
            "Check for Updates…",
            app_center_available(),
            None,
        );
        let about = MenuItem::with_id(ABOUT_ID, "About", app_center_available(), None);
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
        let shutdown_submenu = Submenu::with_items(
            "Shutdown Settings",
            true,
            &[
                &idle_submenu as &dyn tray_icon::menu::IsMenuItem,
                &puck_dock,
            ],
        )
        .map_err(|error| error.to_string())?;
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
        let bindings_submenu = Submenu::new(PROFILES_MENU_LABEL, true);
        for (_, item) in &binding_profiles {
            bindings_submenu
                .append(item)
                .map_err(|error| error.to_string())?;
        }
        bindings_submenu
            .append(&PredefinedMenuItem::separator())
            .map_err(|error| error.to_string())?;
        bindings_submenu
            .append(&edit_profiles)
            .map_err(|error| error.to_string())?;
        bindings_submenu
            .append(&PredefinedMenuItem::separator())
            .map_err(|error| error.to_string())?;
        bindings_submenu
            .append(&overlay_submenu)
            .map_err(|error| error.to_string())?;
        let troubleshooting_submenu = Submenu::with_items(
            "Troubleshooting",
            true,
            &[
                &enable_bindings,
                &PredefinedMenuItem::separator(),
                &settings,
                &accessibility,
                &PredefinedMenuItem::separator(),
                &copy,
                &logs,
            ],
        )
        .map_err(|error| error.to_string())?;
        let separators: [PredefinedMenuItem; 6] =
            std::array::from_fn(|_| PredefinedMenuItem::separator());
        let copy_error_visible = self.runtime.status().last_error.is_some();
        let mut root_items: Vec<&dyn tray_icon::menu::IsMenuItem> = vec![
            &bridge,
            &status,
            &run_toggle,
            &separators[0],
            &controller,
            &input,
            &xiao,
            &firmware,
            &battery,
            &haptics,
            &separators[1],
            &problem,
        ];
        if copy_error_visible {
            root_items.push(&copy_error);
        }
        root_items.extend([
            &troubleshooting_submenu as &dyn tray_icon::menu::IsMenuItem,
            &separators[2],
            &automatic_shutdown,
            &shutdown_submenu,
            &separators[3],
            &current_profile,
            &bindings_submenu,
            &separators[4],
            &updates,
            &about,
            &separators[5],
            &quit,
        ]);
        let menu = Menu::with_items(&root_items).map_err(|error| error.to_string())?;
        let menu_handle = menu.clone();
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Steam Controller Bridge")
            .with_icon(template_icon(TrayState::Waiting)?)
            .with_icon_as_template(true)
            .build()
            .map_err(|error| error.to_string())?;
        let tray_icons = NativeTrayIcons::capture(&tray)?;
        self.items = Some(MenuItems {
            menu: menu_handle,
            bridge,
            status,
            input,
            controller,
            xiao,
            firmware,
            battery,
            haptics,
            current_profile,
            automatic_shutdown,
            problem,
            run_toggle,
            copy_error,
            copy_error_visible,
            updates,
            idle_shutdown,
            puck_dock,
            bindings_submenu,
            binding_profiles,
            overlay_submenu,
            overlay_enabled,
            overlay_hold,
        });
        self.tray_icons = Some(tray_icons);
        self.tray = Some(tray);
        self.refresh_status();
        Ok(())
    }

    pub(super) fn refresh_status(&mut self) {
        let status = self.runtime.status();
        let recovery_problem = self.app_center_recovery.problem().map(str::to_owned);
        if let Err(error) = self.app_center_host.update_firmware(status.xiao.firmware) {
            eprintln!("cannot update app window firmware status: {error}");
        }
        #[cfg(feature = "updater")]
        {
            self.update_checker.poll();
            let available = self.update_checker.available(status.xiao.firmware);
            if self.last_update_available != Some(available) {
                self.last_update_available = Some(available);
                if let Some(items) = &self.items {
                    items.updates.set_text(if available {
                        "Updates Available…"
                    } else {
                        "Check for Updates…"
                    });
                }
            }
        }
        if let Err(error) = self.logger.write_status(&status) {
            eprintln!("cannot write menu-app diagnostics: {error}");
        }
        self.sync_overlay_process(&status);
        if status.revision == self.last_revision && recovery_problem == self.last_recovery_problem {
            return;
        }
        let mut model = MenuModel::from_status(&status);
        if let Some(error) = recovery_problem.as_deref() {
            model.apply_external_error(error);
        }
        let icon_changed = self
            .last_model
            .as_ref()
            .is_none_or(|previous| previous.tray_state != model.tray_state);
        if self.last_model.as_ref() != Some(&model) {
            if let Some(items) = self.items.as_mut() {
                items.bridge.set_text(&model.bridge);
                items.status.set_text(&model.status);
                items.input.set_text(&model.input);
                items.controller.set_text(&model.controller);
                items.xiao.set_text(&model.xiao);
                items.firmware.set_text(&model.firmware);
                items.battery.set_text(&model.battery);
                items.haptics.set_text(&model.haptics);
                items.current_profile.set_text(&model.current_profile);
                items.automatic_shutdown.set_text(&model.automatic_shutdown);
                items.problem.set_text(&model.problem);
                items.run_toggle.set_text(model.run_action.label());
                items.run_toggle.set_enabled(model.run_enabled);
                if items.copy_error_visible != model.has_error {
                    let result: Result<(), String> = if model.has_error {
                        let problem_position = items
                            .menu
                            .items()
                            .iter()
                            .position(|item| item.id() == items.problem.id());
                        problem_position
                            .ok_or_else(|| "Problem menu item is missing".to_owned())
                            .and_then(|position| {
                                items
                                    .menu
                                    .insert(&items.copy_error, position + 1)
                                    .map_err(|error| error.to_string())
                            })
                    } else {
                        items
                            .menu
                            .remove(&items.copy_error)
                            .map_err(|error| error.to_string())
                    };
                    match result {
                        Ok(()) => items.copy_error_visible = model.has_error,
                        Err(error) => {
                            eprintln!("cannot update Copy Full Error visibility: {error}");
                        }
                    }
                }
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
        self.last_recovery_problem = recovery_problem;
    }

    pub(super) fn show_app_center(&mut self, page: AppCenterPage) {
        let firmware = self.runtime.status().xiao.firmware;
        match self.app_center_host.launch(page, firmware) {
            Ok(reused) => {
                if reused
                    && self
                        .app_center_host
                        .child()
                        .is_some_and(|child| !activate_child_application(child))
                {
                    eprintln!("level=warn event=app_window_focus_deferred");
                }
            }
            Err(error) => eprintln!("cannot open Steam Controller Bridge window: {error}"),
        }
    }

    #[allow(clippy::too_many_lines)] // One dispatch table; splitting it hides the menu's shape.
    pub(super) fn handle_menu_event(&mut self, id: &str, event_loop: &ActiveEventLoop) {
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
                let recovery_error = self.app_center_recovery.problem().map(str::to_owned);
                if let Some(error) = recovery_error.or_else(|| self.runtime.status().last_error) {
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
            EDIT_BINDINGS_ID => match launch_bindings_editor() {
                Ok(child) => self.editor_children.push(child),
                Err(error) => eprintln!("cannot launch bindings editor: {error}"),
            },
            LOGS_ID => {
                if let Err(error) = open_path(&self.logger.directory) {
                    eprintln!("cannot open log folder: {error}");
                }
            }
            ABOUT_ID | UPDATES_ID => {
                if let Some(page) = app_center_page_for_menu(id) {
                    self.show_app_center(page);
                }
            }
            QUIT_ID => {
                if self.shutdown() {
                    event_loop.exit();
                }
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

    pub(super) fn shutdown(&mut self) -> bool {
        if self.shutting_down {
            return true;
        }
        if self.app_center_host.firmware_session_active() {
            eprintln!("level=warn event=quit_deferred reason=firmware_update_active");
            if let Some(child) = self.app_center_host.child() {
                let _ = activate_child_application(child);
            }
            return false;
        }
        self.shutting_down = true;
        self.overlay.stop();
        let _ = self.app_center_host.stop();
        self.flush_overlay_diagnostics();
        if let Err(error) = self.runtime.shutdown() {
            eprintln!("bridge shutdown failed: {error}");
        }
        true
    }

    pub(super) fn handle_update_requests(&mut self, event_loop: &ActiveEventLoop) {
        for session_request in self.app_center_host.drain() {
            let UpdateRequest { id, operation } = session_request.request;
            let result = match operation {
                UpdateOperation::SuspendBridge => match self.runtime.suspend_for_update() {
                    Ok(()) => match self
                        .app_center_host
                        .claim_suspension(session_request.generation)
                    {
                        Ok(()) => UpdateResult::Suspended,
                        Err(error) => {
                            let _ = self.runtime.resume_from_update();
                            UpdateResult::Error { message: error }
                        }
                    },
                    Err(error) => UpdateResult::Error {
                        message: format!("Bridge could not release its devices: {error}"),
                    },
                },
                UpdateOperation::ResumeBridge => match self.runtime.resume_from_update() {
                    Ok(()) => match self
                        .app_center_host
                        .release_suspension(session_request.generation)
                    {
                        Ok(()) => UpdateResult::Resumed,
                        Err(error) => UpdateResult::Error { message: error },
                    },
                    Err(error) => UpdateResult::Error {
                        message: format!("Bridge could not resume: {error}"),
                    },
                },
                UpdateOperation::QuitForReplacement => UpdateResult::Quitting,
            };
            let response = UpdateResponse { id, result };
            if let Err(error) = self
                .app_center_host
                .respond(session_request.generation, &response)
            {
                eprintln!("cannot answer app window: {error}");
            }
            if operation == UpdateOperation::QuitForReplacement && self.shutdown() {
                event_loop.exit();
                return;
            }
        }
    }

    pub(super) fn recover_app_center_suspension(&mut self) {
        self.app_center_host.reap();
        if !self.app_center_host.suspension_recovery_needed() {
            self.app_center_recovery = AppCenterRecovery::Idle;
            return;
        }
        let now = Instant::now();
        let state = std::mem::replace(&mut self.app_center_recovery, AppCenterRecovery::Idle);
        match state {
            AppCenterRecovery::Waiting(request) => match request.poll() {
                UpdateResumePoll::Pending => {
                    self.app_center_recovery = AppCenterRecovery::Waiting(request);
                }
                UpdateResumePoll::Complete(Ok(())) => {
                    self.app_center_host.complete_suspension_recovery();
                }
                UpdateResumePoll::Complete(Err(error)) => {
                    self.defer_or_complete_app_center_recovery(now, &error);
                }
            },
            AppCenterRecovery::Backoff { retry_at, error } if now < retry_at => {
                self.app_center_recovery = AppCenterRecovery::Backoff { retry_at, error };
            }
            AppCenterRecovery::Idle | AppCenterRecovery::Backoff { .. } => {
                self.start_app_center_recovery(now);
            }
        }
    }

    fn start_app_center_recovery(&mut self, now: Instant) {
        match self.runtime.begin_resume_from_update() {
            Ok(request) => {
                self.app_center_recovery = AppCenterRecovery::Waiting(request);
            }
            Err(error) => self.defer_or_complete_app_center_recovery(now, &error),
        }
    }

    fn defer_or_complete_app_center_recovery(
        &mut self,
        now: Instant,
        error: &bridge_runtime::RuntimeError,
    ) {
        if self.runtime.is_terminated() {
            let join = self.runtime.join();
            eprintln!(
                "level=error event=app_center_recovery_abandoned reason=runtime_terminated error={error:?} join={join:?}"
            );
            // A joined runtime cannot still own HID or serial handles, so
            // retaining the updater's quit interlock would only strand the
            // menu process after its worker has already ended.
            self.app_center_host.complete_suspension_recovery();
            self.app_center_recovery = AppCenterRecovery::Idle;
        } else {
            let message = format!(
                "Updater suspension recovery is not responding: {error}. Quit remains deferred while recovery retries."
            );
            eprintln!("level=error event=app_center_recovery_deferred error={error:?}");
            self.app_center_recovery = AppCenterRecovery::Backoff {
                retry_at: now + APP_CENTER_RECOVERY_RETRY_DELAY,
                error: message,
            };
        }
    }
}
