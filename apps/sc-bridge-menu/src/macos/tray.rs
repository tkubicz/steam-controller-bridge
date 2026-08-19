#[allow(
    clippy::wildcard_imports,
    reason = "tray construction and dispatch share the menu app's private item vocabulary"
)]
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HardwareStatusRow {
    Input,
    Output,
    Firmware,
    Controller,
    Battery,
    Haptics,
}

pub(super) fn hardware_status_rows(visibility: HardwareRowVisibility) -> Vec<HardwareStatusRow> {
    if !visibility.section {
        return Vec::new();
    }
    let mut rows = vec![HardwareStatusRow::Input, HardwareStatusRow::Output];
    if visibility.firmware {
        rows.push(HardwareStatusRow::Firmware);
    }
    rows.push(HardwareStatusRow::Controller);
    if visibility.controller_details {
        rows.extend([HardwareStatusRow::Battery, HardwareStatusRow::Haptics]);
    }
    rows
}

impl MenuApp {
    #[allow(clippy::too_many_lines)] // Native menu construction keeps item ownership and order together.
    pub(super) fn create_tray(&mut self) -> Result<(), String> {
        let bridge = MenuItem::new("Bridge: Starting", false, None);
        let status = MenuItem::new("Status: Looking for hardware", false, None);
        let input = MenuItem::new("Input: Discovering", false, None);
        let controller = MenuItem::new("Controller: Not connected", false, None);
        let output = MenuItem::new("Output: Discovering", false, None);
        let firmware = MenuItem::new("Firmware: Not available", false, None);
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
            FIRMWARE_UPDATES_LABEL,
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
        let output_bridge_device = CheckMenuItem::with_id(
            OUTPUT_BRIDGE_DEVICE_ID,
            "Bridge Device",
            true,
            self.settings.output == OutputPreference::BridgeDevice,
            None,
        );
        let output_virtual_hid = CheckMenuItem::with_id(
            OUTPUT_VIRTUAL_HID_ID,
            "Virtual Gamepad — Experimental",
            true,
            self.settings.output == OutputPreference::VirtualHid,
            None,
        );
        let output_submenu = Submenu::with_items(
            "Gamepad Output",
            true,
            &[
                &output_bridge_device as &dyn tray_icon::menu::IsMenuItem,
                &output_virtual_hid,
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
        let initial_status = self.runtime.status();
        let copy_error_visible = initial_status.last_error.is_some();
        let hardware_rows = MenuModel::from_status(&initial_status).hardware_rows;
        let mut root_items: Vec<&dyn tray_icon::menu::IsMenuItem> =
            vec![&bridge, &status, &run_toggle];
        if self.virtual_hid_enabled {
            root_items.push(&output_submenu);
        }
        root_items.push(&separators[0]);
        for row in hardware_status_rows(hardware_rows) {
            root_items.push(match row {
                HardwareStatusRow::Input => &input,
                HardwareStatusRow::Output => &output,
                HardwareStatusRow::Firmware => &firmware,
                HardwareStatusRow::Controller => &controller,
                HardwareStatusRow::Battery => &battery,
                HardwareStatusRow::Haptics => &haptics,
            });
        }
        if hardware_rows.section {
            root_items.push(&separators[1]);
        }
        root_items.push(&problem);
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
            output,
            firmware,
            battery,
            haptics,
            hardware_separator: separators[1].clone(),
            hardware_visibility: hardware_rows.into(),
            current_profile,
            automatic_shutdown,
            problem,
            run_toggle,
            copy_error,
            copy_error_visible,
            updates,
            idle_shutdown,
            puck_dock,
            output_bridge_device,
            output_virtual_hid,
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

    #[allow(
        clippy::too_many_lines,
        reason = "one pass keeps all menu items synchronized to the same runtime snapshot"
    )]
    pub(super) fn refresh_status(&mut self) {
        let status = self.runtime.status();
        let recovery_problem = self
            .app_center_recovery
            .problem()
            .map(str::to_owned)
            .or_else(|| self.output_change_problem.clone());
        if let Err(error) = self
            .app_center_host
            .update_firmware(status.output.capabilities.firmware, status.output.firmware)
        {
            eprintln!("cannot update app window firmware status: {error}");
        }
        #[cfg(feature = "updater")]
        {
            self.update_checker.poll();
            let available = self.update_checker.available(status.output.firmware);
            if self.last_update_available != Some(available) {
                self.last_update_available = Some(available);
                if let Some(items) = &self.items {
                    items.updates.set_text(if available {
                        UPDATE_AVAILABLE_LABEL
                    } else {
                        FIRMWARE_UPDATES_LABEL
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
                items.output.set_text(&model.output);
                items.firmware.set_text(&model.firmware);
                items.battery.set_text(&model.battery);
                items.haptics.set_text(&model.haptics);
                items.current_profile.set_text(&model.current_profile);
                items.automatic_shutdown.set_text(&model.automatic_shutdown);
                items.problem.set_text(&model.problem);
                items.run_toggle.set_text(model.run_action.label());
                items.run_toggle.set_enabled(model.run_enabled);
                if let Err(error) = items.sync_status_visibility(&model) {
                    eprintln!("cannot update hardware status visibility: {error}");
                }
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
        let output = self.runtime.status().output;
        match self
            .app_center_host
            .launch(page, output.capabilities.firmware, output.firmware)
        {
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
            OUTPUT_BRIDGE_DEVICE_ID => self.begin_output_change(OutputPreference::BridgeDevice),
            OUTPUT_VIRTUAL_HID_ID => self.begin_output_change(OutputPreference::VirtualHid),
            COPY_ERROR_ID => {
                let recovery_error = self.app_center_recovery.problem().map(str::to_owned);
                if let Some(error) = recovery_error
                    .or_else(|| self.output_change_problem.clone())
                    .or_else(|| self.runtime.status().last_error)
                {
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
            SETTINGS_ID => self.open_capability_settings(CapabilityId::InputMonitoring),
            ACCESSIBILITY_ID => self.open_capability_settings(CapabilityId::Accessibility),
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

    fn begin_output_change(&mut self, preference: OutputPreference) {
        if preference == OutputPreference::VirtualHid && !self.virtual_hid_enabled {
            self.output_change_problem = Some(format!(
                "Virtual HID is disabled; relaunch with --enable-virtual-hid or set \
                 {ENABLE_VIRTUAL_HID_ENV}=1"
            ));
            self.update_setting_checkmarks();
            return;
        }
        if preference == self.settings.output || self.output_change.is_some() {
            self.update_setting_checkmarks();
            return;
        }
        let selection = match preference.runtime_selection() {
            Ok(selection) => selection,
            Err(error) => {
                self.output_change_problem = Some(error);
                self.update_setting_checkmarks();
                return;
            }
        };
        match self.runtime.begin_set_output(selection) {
            Ok(request) => {
                self.output_change = Some((request, preference));
                self.output_change_problem = None;
                self.set_output_items_enabled(false);
            }
            Err(error) => {
                self.output_change_problem = Some(format!("Output change failed: {error}"));
                self.update_setting_checkmarks();
            }
        }
    }

    pub(super) fn poll_output_change(&mut self) {
        let Some((request, _)) = self.output_change.as_mut() else {
            return;
        };
        match request.poll() {
            OutputChangePoll::Pending => {}
            OutputChangePoll::TimedOut => {
                self.output_change_problem = Some(
                    "Output change is taking longer than expected; cleanup is still pending."
                        .to_owned(),
                );
            }
            OutputChangePoll::Complete(result) => {
                // The request and the preference it was raised for are stored
                // and taken together, so a completion cannot arrive without
                // knowing which selection it completed.
                if let Some((_, preference)) = self.output_change.take() {
                    match result {
                        Ok(()) => {
                            self.settings.output = preference;
                            self.output_change_problem = None;
                            if let Err(error) = save_settings(&self.settings_path, &self.settings) {
                                self.output_change_problem = Some(format!(
                                    "Output changed but settings could not be saved: {error}"
                                ));
                            }
                        }
                        Err(error) => {
                            self.output_change_problem =
                                Some(format!("Output change failed: {error}"));
                        }
                    }
                }
                self.set_output_items_enabled(true);
                self.update_setting_checkmarks();
            }
        }
    }

    fn set_output_items_enabled(&self, enabled: bool) {
        if let Some(items) = &self.items {
            items.output_bridge_device.set_enabled(enabled);
            items.output_virtual_hid.set_enabled(enabled);
        }
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
        let state = std::mem::replace(&mut self.app_center_recovery, AppCenterRecovery::Idle);
        match state {
            AppCenterRecovery::Waiting {
                mut request,
                mut error,
            } => match request.poll() {
                UpdateResumePoll::Pending => {
                    self.app_center_recovery = AppCenterRecovery::Waiting { request, error };
                }
                UpdateResumePoll::TimedOut => {
                    let message = "Updater suspension recovery is not responding. Quit remains deferred while the original recovery request is pending.".to_owned();
                    eprintln!("level=error event=app_center_recovery_delayed");
                    error = Some(message);
                    self.app_center_recovery = AppCenterRecovery::Waiting { request, error };
                }
                UpdateResumePoll::Complete(Ok(())) => {
                    self.app_center_host.complete_suspension_recovery();
                }
                UpdateResumePoll::Complete(Err(error)) => {
                    self.fail_app_center_recovery(&error.to_string());
                }
            },
            AppCenterRecovery::Failed(error) => {
                if !self.complete_terminated_app_center_recovery(&error) {
                    self.app_center_recovery = AppCenterRecovery::Failed(error);
                }
            }
            AppCenterRecovery::Idle => self.start_app_center_recovery(),
        }
    }

    fn start_app_center_recovery(&mut self) {
        match self.runtime.begin_resume_from_update() {
            Ok(request) => {
                self.app_center_recovery = AppCenterRecovery::Waiting {
                    request,
                    error: None,
                };
            }
            Err(error) => self.fail_app_center_recovery(&error.to_string()),
        }
    }

    fn fail_app_center_recovery(&mut self, error: &str) {
        if self.complete_terminated_app_center_recovery(error) {
            return;
        }
        let message = format!("Updater suspension recovery failed: {error}. Quit remains deferred because bridge ownership could not be proven safe.");
        eprintln!("level=error event=app_center_recovery_failed error={error:?}");
        self.app_center_recovery = AppCenterRecovery::Failed(message);
    }

    fn complete_terminated_app_center_recovery(&mut self, error: &str) -> bool {
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
            true
        } else {
            false
        }
    }
}

impl MenuItems {
    fn sync_status_visibility(&mut self, model: &MenuModel) -> Result<(), String> {
        if model.hardware_rows.section != self.hardware_visibility.section {
            return self.sync_hardware_section(model.hardware_rows);
        }
        if !model.hardware_rows.section {
            return Ok(());
        }
        sync_menu_item_visibility(
            &self.menu,
            &self.output,
            &self.firmware,
            model.hardware_rows.firmware,
            &mut self.hardware_visibility.optional.firmware,
        )?;
        if model.hardware_rows.controller_details {
            sync_menu_item_visibility(
                &self.menu,
                &self.controller,
                &self.battery,
                true,
                &mut self.hardware_visibility.optional.battery,
            )?;
            sync_menu_item_visibility(
                &self.menu,
                &self.battery,
                &self.haptics,
                true,
                &mut self.hardware_visibility.optional.haptics,
            )?;
        } else {
            sync_menu_item_visibility(
                &self.menu,
                &self.battery,
                &self.haptics,
                false,
                &mut self.hardware_visibility.optional.haptics,
            )?;
            sync_menu_item_visibility(
                &self.menu,
                &self.controller,
                &self.battery,
                false,
                &mut self.hardware_visibility.optional.battery,
            )?;
        }
        Ok(())
    }

    fn sync_hardware_section(&mut self, rows: HardwareRowVisibility) -> Result<(), String> {
        let target: HardwareItemVisibility = rows.into();
        if target.section {
            let position = self
                .menu
                .items()
                .iter()
                .position(|candidate| candidate.id() == self.problem.id())
                .ok_or_else(|| "Problem menu item is missing".to_owned())?;
            let items = self.hardware_items(target);
            self.menu
                .insert_items(&items, position)
                .map_err(|error| error.to_string())?;
        } else {
            let items = self.hardware_items(self.hardware_visibility);
            for item in items.into_iter().rev() {
                self.menu.remove(item).map_err(|error| error.to_string())?;
            }
        }
        self.hardware_visibility = target;
        Ok(())
    }

    fn hardware_items(
        &self,
        visibility: HardwareItemVisibility,
    ) -> Vec<&dyn tray_icon::menu::IsMenuItem> {
        if !visibility.section {
            return Vec::new();
        }
        let mut items: Vec<&dyn tray_icon::menu::IsMenuItem> = vec![&self.input, &self.output];
        if visibility.optional.firmware {
            items.push(&self.firmware);
        }
        items.push(&self.controller);
        if visibility.optional.battery {
            items.push(&self.battery);
        }
        if visibility.optional.haptics {
            items.push(&self.haptics);
        }
        items.push(&self.hardware_separator);
        items
    }
}

fn sync_menu_item_visibility(
    menu: &Menu,
    anchor: &MenuItem,
    item: &MenuItem,
    visible: bool,
    currently_visible: &mut bool,
) -> Result<(), String> {
    if visible == *currently_visible {
        return Ok(());
    }
    if visible {
        let position = menu
            .items()
            .iter()
            .position(|candidate| candidate.id() == anchor.id())
            .ok_or_else(|| format!("anchor for {} is missing", item.text()))?;
        menu.insert(item, position + 1)
            .map_err(|error| error.to_string())?;
    } else {
        menu.remove(item).map_err(|error| error.to_string())?;
    }
    *currently_visible = visible;
    Ok(())
}
