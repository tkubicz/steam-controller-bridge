use std::fmt::Write as _;
use std::sync::atomic::Ordering;

use chrono::{DateTime, Local};
use eframe::egui;
use release_updater::{
    classify_firmware_release, firmware_target, FirmwareReleaseState, ReleaseManifestV1,
};
use ui_theme::{ActionButton, ACCENT, BORDER, DANGER, MUTED_TEXT, SUCCESS, SURFACE, TEXT};

use super::{Activity, AppCenter, StatusPlacement, StatusTone};
use crate::app_center_protocol::{
    FirmwareDetails, FirmwareInstallStatus, FirmwareReceiptSource, FirmwareStatus,
    FirmwareTargetStatus,
};
use crate::window_ui::{full_width_card, render_release_notes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FirmwareAction {
    None,
    Disabled(&'static str),
    Install(&'static str),
}

impl AppCenter {
    pub(super) fn updates_page(&mut self, ui: &mut egui::Ui) {
        #[cfg(debug_assertions)]
        if let Some(root) = self.local_update_root() {
            status_callout(
                ui,
                ACCENT,
                "Local development updates",
                &format!(
                    "Signed metadata and artifacts are loaded from {}. Production releases never use this source.",
                    root.display()
                ),
            );
            ui.add_space(10.0);
        }
        self.status_banner(ui, StatusPlacement::Page);
        if self.catalog_status.shows_check_again() {
            ui.add_space(10.0);
            let checking = self.catalog_status == super::CatalogStatus::Checking;
            if update_refresh_button(ui, "Check Again", self.operation_available(), checking)
                .clicked()
            {
                self.check_again(ui.ctx().clone());
            }
        }
        ui.add_space(14.0);
        let catalog = self.catalog.take();
        if let Some(manifest) = catalog.as_ref() {
            self.application_card(ui, manifest);
            ui.add_space(14.0);
            self.firmware_card(ui, manifest);
        }
        self.catalog = catalog;
    }

    fn application_card(&mut self, ui: &mut egui::Ui, manifest: &ReleaseManifestV1) {
        full_width_card(ui, 20, |ui| {
            let installed = &self.installed;
            let ordering = installed.cmp(&manifest.application_version);
            let (badge, badge_colour) = match ordering {
                std::cmp::Ordering::Less => ("Update available", ACCENT),
                std::cmp::Ordering::Equal => ("Up to date", SUCCESS),
                std::cmp::Ordering::Greater => ("Newer build", MUTED_TEXT),
            };
            card_header(
                ui,
                "APPLICATION",
                &format!("Steam Controller Bridge {}", manifest.application_version),
                &format!(
                    "Installed {installed} · Requires macOS {}+",
                    manifest.minimum_macos
                ),
                badge,
                badge_colour,
            );
            ui.add_space(14.0);
            match installed.cmp(&manifest.application_version) {
                std::cmp::Ordering::Less if self.staged_application.is_some() => {
                    status_callout(
                        ui,
                        SUCCESS,
                        "Application verified",
                        "The new version is staged and ready to replace the installed app.",
                    );
                    ui.add_space(12.0);
                    if self.replacement_supported {
                        ui.label(
                            egui::RichText::new("Quit the bridge, drag the revealed app into Applications, choose Replace, then launch it. Right-click Open may be required for this ad-hoc-signed build.")
                                .color(MUTED_TEXT),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("This source or metadata-mismatched build cannot use guided replacement. The verified archive remains available for manual installation.")
                                .color(MUTED_TEXT),
                        );
                    }
                    self.application_operation_status(ui);
                    if self.operation_available() {
                        ui.add_space(12.0);
                        if update_action_button(ui, "Show New App and Applications", true).clicked()
                        {
                            self.reveal_application();
                        }
                        ui.add_space(6.0);
                        if update_action_button(
                            ui,
                            "Quit Bridge for Replacement",
                            self.replacement_supported,
                        )
                        .clicked()
                        {
                            self.quit_for_replacement(ui.ctx().clone());
                        }
                    }
                }
                std::cmp::Ordering::Less => {
                    status_callout(
                        ui,
                        ACCENT,
                        &format!("Version {} is ready", manifest.application_version),
                        "Download and verify the application before replacing the installed version.",
                    );
                    self.application_operation_status(ui);
                    if self.operation_available() {
                        ui.add_space(12.0);
                        if update_action_button(ui, "Download Application Update", true).clicked() {
                            self.download_application(manifest.clone(), ui.ctx().clone());
                        }
                    }
                }
                std::cmp::Ordering::Equal => {
                    status_callout(
                        ui,
                        SUCCESS,
                        "Application is up to date",
                        &self.application_current_message(),
                    );
                }
                std::cmp::Ordering::Greater => {
                    status_callout(
                        ui,
                        MUTED_TEXT,
                        self.application_newer_title(),
                        &self.application_newer_message(),
                    );
                }
            }
            self.release_notes(ui, manifest, ordering == std::cmp::Ordering::Less);
        });
    }

    fn firmware_card(&mut self, ui: &mut egui::Ui, manifest: &ReleaseManifestV1) {
        full_width_card(ui, 20, |ui| {
            let installed = &self.installed;
            let app_pending = installed < &manifest.application_version;
            let app_incompatible = installed < &manifest.firmware.minimum_application_version;
            let target_matches = firmware_target_matches(&self.firmware, &manifest.firmware.target);
            let release_state = firmware_release_state(
                &self.firmware,
                &manifest.firmware.target,
                manifest.firmware.revision,
            );
            let (badge, badge_colour) = match release_state {
                Some(FirmwareReleaseState::Pending) => ("Checking firmware", MUTED_TEXT),
                Some(FirmwareReleaseState::UpdateAvailable) => ("Update available", ACCENT),
                Some(FirmwareReleaseState::Current) => ("Up to date", SUCCESS),
                Some(FirmwareReleaseState::Newer) => ("Newer firmware", MUTED_TEXT),
                None => ("Unidentified firmware", MUTED_TEXT),
            };
            let target_name = firmware_target(&manifest.firmware.target)
                .ok()
                .flatten()
                .map_or(manifest.firmware.target.as_str(), |target| {
                    target.display_name.as_str()
                });
            let target_association = match &self.firmware.target {
                FirmwareTargetStatus::Reported(_) if target_matches => "target verified",
                FirmwareTargetStatus::Reported(_) => "different firmware target",
                FirmwareTargetStatus::Unreported => "target not reported",
                FirmwareTargetStatus::Malformed => "target report invalid",
            };
            card_header(
                ui,
                "FIRMWARE",
                &format!(
                    "{target_name} firmware revision {}",
                    manifest.firmware.revision
                ),
                &format!(
                    "Connected revision {} · {target_association}",
                    firmware_description(self.firmware.version),
                ),
                badge,
                badge_colour,
            );
            ui.add_space(14.0);
            let show_reinstall = !app_pending
                && !app_incompatible
                && release_state == Some(FirmwareReleaseState::Current);
            if target_matches {
                firmware_receipt_callout(ui, self.firmware.version, self.firmware.install);
                ui.add_space(12.0);
            }
            let action = self.firmware_release_context(
                ui,
                manifest,
                release_state,
                app_pending,
                app_incompatible,
            );
            self.firmware_operation_status(ui);
            self.firmware_action(ui, manifest, action);
            self.firmware_installation_footer(
                ui,
                manifest,
                show_reinstall && self.operation_available(),
            );
        });
    }

    fn firmware_release_context(
        &self,
        ui: &mut egui::Ui,
        manifest: &ReleaseManifestV1,
        release_state: Option<FirmwareReleaseState>,
        app_pending: bool,
        app_incompatible: bool,
    ) -> FirmwareAction {
        if app_pending {
            status_callout(
                ui,
                ACCENT,
                "Application update required first",
                "Replace and relaunch the application before installing this firmware.",
            );
            return FirmwareAction::Disabled("Update Application First");
        }
        if app_incompatible {
            status_callout(
                ui,
                DANGER,
                "Newer application required",
                "The installed application cannot communicate with this firmware revision.",
            );
            return FirmwareAction::None;
        }
        match release_state {
            None => self.unidentified_firmware_context(ui),
            Some(FirmwareReleaseState::Pending) => {
                status_callout(
                    ui,
                    MUTED_TEXT,
                    "Firmware information unavailable",
                    "A factory XIAO does not report bridge firmware. Install the signed firmware, or wait and reconnect if a bridge-enabled board is still starting.",
                );
                FirmwareAction::Install("Install or Recover XIAO Firmware")
            }
            Some(FirmwareReleaseState::Newer) => {
                status_callout(
                    ui,
                    MUTED_TEXT,
                    &self.firmware_newer_title(),
                    "Downgrading the connected board is disabled.",
                );
                FirmwareAction::None
            }
            Some(FirmwareReleaseState::Current) => {
                status_callout(
                    ui,
                    SUCCESS,
                    "Firmware is up to date",
                    &self.firmware_current_message(),
                );
                FirmwareAction::None
            }
            Some(FirmwareReleaseState::UpdateAvailable) => {
                status_callout(
                    ui,
                    ACCENT,
                    &format!("Revision {} is ready", manifest.firmware.revision),
                    "The board and firmware are verified before anything is written.",
                );
                FirmwareAction::Install("Install Firmware Update")
            }
        }
    }

    fn unidentified_firmware_context(&self, ui: &mut egui::Ui) -> FirmwareAction {
        let (title, identity) = match &self.firmware.target {
            FirmwareTargetStatus::Unreported => (
                "Firmware target unidentified",
                "This bridge device did not report a firmware target.",
            ),
            FirmwareTargetStatus::Malformed => (
                "Firmware target invalid",
                "This bridge device reported a malformed or duplicate firmware target.",
            ),
            FirmwareTargetStatus::Reported(_) => (
                "Different firmware target",
                "This bridge device reports a target that is not this XIAO release target.",
            ),
        };
        status_callout(
            ui,
            MUTED_TEXT,
            title,
            &format!(
                "{identity} Normal bridging remains available, but automatic recommendations and bootloader commands are disabled."
            ),
        );
        FirmwareAction::Install("Install or Recover XIAO Firmware")
    }

    fn firmware_action(
        &mut self,
        ui: &mut egui::Ui,
        manifest: &ReleaseManifestV1,
        action: FirmwareAction,
    ) {
        if action == FirmwareAction::None || !self.operation_available() {
            return;
        }
        ui.add_space(12.0);
        match action {
            FirmwareAction::None => {}
            FirmwareAction::Disabled(label) => {
                let _ = update_action_button(ui, label, false);
            }
            FirmwareAction::Install(label) => {
                if update_action_button(ui, label, true).clicked() {
                    self.install_firmware(manifest.clone(), ui.ctx().clone());
                }
            }
        }
    }

    fn firmware_installation_footer(
        &mut self,
        ui: &mut egui::Ui,
        manifest: &ReleaseManifestV1,
        show_reinstall: bool,
    ) {
        ui.add_space(12.0);
        if show_reinstall {
            if firmware_reinstall_action(ui, self.operation_available()).clicked() {
                self.install_firmware(manifest.clone(), ui.ctx().clone());
            }
            ui.add_space(12.0);
        }
        ui.separator();
        ui.add_space(6.0);
        let target = firmware_target(&manifest.firmware.target).ok().flatten();
        let target_name = target.map_or("the selected firmware target", |target| {
            target.display_name.as_str()
        });
        ui.label(
            egui::RichText::new(format!(
                "During installation, the temporary {target_name} UF2 drive disconnects automatically. macOS may show a harmless \"Disk Not Ejected Properly\" notification even when verification succeeds."
            ))
                .size(13.0)
                .color(MUTED_TEXT),
        );
        ui.add_space(6.0);
        let automatic_entry = target.map_or_else(
            || "Automatic UF2 entry requires matching target identity and an advertised bootloader capability.".to_owned(),
            |target| format!(
                "Automatic UF2 entry is used only when the connected firmware reports the matching {} target and advertises bootloader capability.",
                target.display_name
            ),
        );
        ui.label(
            egui::RichText::new(format!(
                "{automatic_entry} For first installation or recovery, quickly press the tiny reset button beside the USB-C connector twice when prompted. Success requires the exact signed revision and a newly committed installation receipt."
            ))
                .size(13.0)
                .color(MUTED_TEXT),
        );
    }

    fn firmware_operation_status(&mut self, ui: &mut egui::Ui) {
        if self.status_placement != StatusPlacement::Firmware {
            return;
        }
        ui.add_space(12.0);
        self.status_banner(ui, StatusPlacement::Firmware);
        if matches!(self.activity, Activity::Busy { can_cancel: true }) {
            ui.add_space(10.0);
            if update_action_button(ui, "Cancel Before Writing", true).clicked() {
                self.cancel.store(true, Ordering::Release);
                if self.demo.is_some() {
                    self.activity = Activity::Idle;
                    self.firmware_write_started = false;
                    "Demo: firmware preparation cancelled safely.".clone_into(&mut self.status);
                } else {
                    "Cancelling safely…".clone_into(&mut self.status);
                }
                self.status_tone = StatusTone::Info;
            }
        }
    }

    fn application_operation_status(&mut self, ui: &mut egui::Ui) {
        if self.status_placement != StatusPlacement::Application {
            return;
        }
        ui.add_space(12.0);
        self.status_banner(ui, StatusPlacement::Application);
    }

    fn release_notes(
        &self,
        ui: &mut egui::Ui,
        manifest: &ReleaseManifestV1,
        update_available: bool,
    ) {
        if self.release_notes.is_empty() {
            return;
        }
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(4.0);
        egui::CollapsingHeader::new(
            egui::RichText::new(format!("What’s new in {}", manifest.application_version))
                .strong()
                .color(TEXT),
        )
        .id_salt(("update-release-notes", manifest.release_tag.as_str()))
        .default_open(update_available)
        .show(ui, |ui| {
            ui.add_space(6.0);
            render_release_notes(ui, &self.release_notes);
        });
    }
}

fn firmware_receipt_callout(
    ui: &mut egui::Ui,
    version: FirmwareStatus,
    install: FirmwareInstallStatus,
) {
    let colour = match install {
        FirmwareInstallStatus::Unsupported => MUTED_TEXT,
        FirmwareInstallStatus::Pending | FirmwareInstallStatus::Invalid => DANGER,
        FirmwareInstallStatus::Recorded(_) => SUCCESS,
    };
    let (title, body) = firmware_receipt_copy(version, install);
    status_callout(ui, colour, &title, &body);
}

fn firmware_receipt_copy(
    version: FirmwareStatus,
    install: FirmwareInstallStatus,
) -> (String, String) {
    match install {
        // `Unsupported` is also the pre-handshake and no-device default, so
        // only a reported revision justifies a claim about the firmware.
        FirmwareInstallStatus::Unsupported => (
            "Installation date unavailable".to_owned(),
            match version {
                FirmwareStatus::Reported(1) => {
                    "Firmware revision 1 does not support installation receipts.".to_owned()
                }
                FirmwareStatus::Reported(revision) => format!(
                    "Firmware revision {revision} did not report installation receipt support."
                ),
                _ => {
                    "The connected board has not reported installation receipt support.".to_owned()
                }
            },
        ),
        FirmwareInstallStatus::Pending => (
            "Installation verification pending".to_owned(),
            "The firmware is running, but its new installation receipt has not been recorded yet."
                .to_owned(),
        ),
        FirmwareInstallStatus::Invalid => (
            "Installation receipt is invalid".to_owned(),
            "The receipt marker is corrupted. Reinstall firmware to restore verified installation metadata."
                .to_owned(),
        ),
        FirmwareInstallStatus::Recorded(receipt) => {
            let timestamp = format_install_time(receipt.installed_at);
            let title = match receipt.source {
                FirmwareReceiptSource::AppCenter => format!("Installed and verified {timestamp}"),
                FirmwareReceiptSource::FirstObserved => {
                    format!("First observed after flashing {timestamp}")
                }
            };
            (
                title,
                format!("Installation ID {}", format_install_id(receipt.install_id)),
            )
        }
    }
}

fn format_install_time(installed_at: u64) -> String {
    i64::try_from(installed_at)
        .ok()
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        .map_or_else(
            || "at an unavailable date".to_owned(),
            |utc| {
                utc.with_timezone(&Local)
                    .format("%b %-d, %Y at %H:%M")
                    .to_string()
            },
        )
}

fn format_install_id(install_id: [u8; 16]) -> String {
    let mut id = String::with_capacity(32);
    for byte in install_id {
        write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    id
}

fn card_header(
    ui: &mut egui::Ui,
    eyebrow: &str,
    title: &str,
    subtitle: &str,
    badge: &str,
    badge_colour: egui::Color32,
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(eyebrow)
                    .size(11.0)
                    .strong()
                    .color(ACCENT),
            );
            ui.label(egui::RichText::new(title).size(20.0).strong().color(TEXT));
            ui.label(egui::RichText::new(subtitle).size(13.0).color(MUTED_TEXT));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            egui::Frame::new()
                .fill(badge_colour.gamma_multiply(0.16))
                .corner_radius(7)
                .inner_margin(egui::Margin::symmetric(9, 5))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(badge)
                            .size(12.0)
                            .strong()
                            .color(badge_colour),
                    );
                });
        });
    });
}

pub(super) fn hero_badge(ui: &mut egui::Ui, label: &str, colour: egui::Color32) {
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.16))
        .corner_radius(7)
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).size(13.0).color(colour));
        });
}

fn firmware_description(status: FirmwareStatus) -> String {
    match status {
        FirmwareStatus::Reported(revision) => revision.to_string(),
        FirmwareStatus::Pending => "checking".to_owned(),
        FirmwareStatus::UnsupportedFormat(format) => format!("newer format {format}"),
        FirmwareStatus::Malformed => "invalid report".to_owned(),
        FirmwareStatus::Unreported => "not reported".to_owned(),
    }
}

fn firmware_target_matches(firmware: &FirmwareDetails, release_target: &str) -> bool {
    firmware
        .target_id()
        .is_some_and(|target| target.as_str() == release_target)
}

/// `None` marks firmware whose reported identity is outside this release's
/// target. A pending report is not an identity verdict yet - a factory board
/// or a still-starting bridge keeps its dedicated checking guidance instead of
/// reading as a foreign target.
fn firmware_release_state(
    firmware: &FirmwareDetails,
    release_target: &str,
    release_revision: u16,
) -> Option<FirmwareReleaseState> {
    (firmware.version == FirmwareStatus::Pending
        || firmware_target_matches(firmware, release_target))
    .then(|| classify_firmware_release(firmware.version.into(), release_revision))
}

pub(super) fn firmware_badge(firmware: &FirmwareDetails) -> String {
    // Pending and newer-format reports carry no parseable identity, so the
    // version state must win before the target check declares them
    // unidentified.
    let target_is_supported = firmware.target_id().is_some_and(|identifier| {
        firmware_target(identifier.as_str())
            .ok()
            .flatten()
            .is_some()
    });
    match firmware.version {
        FirmwareStatus::Pending => "Checking firmware".to_owned(),
        FirmwareStatus::UnsupportedFormat(_) => "Firmware newer".to_owned(),
        FirmwareStatus::Reported(revision) if target_is_supported => {
            format!("Firmware rev {revision}")
        }
        FirmwareStatus::Reported(_) | FirmwareStatus::Malformed | FirmwareStatus::Unreported => {
            "Firmware unidentified".to_owned()
        }
    }
}

fn status_callout(ui: &mut egui::Ui, colour: egui::Color32, title: &str, body: &str) {
    let inner_width = (ui.available_width() - 28.0 - 2.0).max(0.0);
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, colour.gamma_multiply(0.42)))
        .corner_radius(9)
        .inner_margin(egui::Margin::symmetric(14, 11))
        .show(ui, |ui| {
            ui.set_width(inner_width);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("●").size(11.0).color(colour));
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(title).strong().color(TEXT));
                    ui.label(egui::RichText::new(body).size(13.0).color(MUTED_TEXT));
                });
            });
        });
}

fn update_action_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    show_update_action_button(ui, ActionButton::new(label).enabled(enabled))
}

fn update_refresh_button(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    spinning: bool,
) -> egui::Response {
    show_update_action_button(
        ui,
        ActionButton::new(label)
            .enabled(enabled)
            .refresh_icon(spinning),
    )
}

fn show_update_action_button(ui: &mut egui::Ui, button: ActionButton<'_>) -> egui::Response {
    let width = ui.available_width().min(320.0);
    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
        button.min_size(egui::vec2(width, 38.0)).show(ui)
    })
    .inner
}

fn firmware_reinstall_action(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let inner_width = (ui.available_width() - 28.0 - 2.0).max(0.0);
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(9)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(inner_width);
            ui.label(
                egui::RichText::new("Reinstall the current firmware")
                    .strong()
                    .color(TEXT),
            );
            ui.label(
                egui::RichText::new("Use this to verify the updater or restore the current signed image. A successful reinstall creates a new installation ID and date.")
                    .size(13.0)
                    .color(MUTED_TEXT),
            );
            ui.add_space(4.0);
            update_action_button(ui, "Reinstall Firmware", enabled)
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_center_protocol::FirmwareReceiptStatus;

    #[test]
    fn receipt_copy_distinguishes_all_installation_states() {
        let legacy = FirmwareStatus::Reported(1);
        let current = FirmwareStatus::Reported(2);
        assert_eq!(
            firmware_receipt_copy(legacy, FirmwareInstallStatus::Unsupported).0,
            "Installation date unavailable"
        );
        assert_eq!(
            firmware_receipt_copy(current, FirmwareInstallStatus::Pending).0,
            "Installation verification pending"
        );
        assert_eq!(
            firmware_receipt_copy(current, FirmwareInstallStatus::Invalid).0,
            "Installation receipt is invalid"
        );

        let receipt = FirmwareReceiptStatus {
            installed_at: 1_786_456_920,
            install_id: [0xa5; 16],
            source: FirmwareReceiptSource::AppCenter,
        };
        let (app_center_title, installation_id) =
            firmware_receipt_copy(current, FirmwareInstallStatus::Recorded(receipt));
        assert!(app_center_title.starts_with("Installed and verified "));
        assert_eq!(
            installation_id,
            "Installation ID a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5"
        );

        let (first_observed_title, _) = firmware_receipt_copy(
            current,
            FirmwareInstallStatus::Recorded(FirmwareReceiptStatus {
                source: FirmwareReceiptSource::FirstObserved,
                ..receipt
            }),
        );
        assert!(first_observed_title.starts_with("First observed after flashing "));
    }

    #[test]
    fn receipt_copy_does_not_invent_a_firmware_revision() {
        let (_, reported_body) = firmware_receipt_copy(
            FirmwareStatus::Reported(1),
            FirmwareInstallStatus::Unsupported,
        );
        assert!(reported_body.contains("revision 1"));
        let (_, newer_body) = firmware_receipt_copy(
            FirmwareStatus::Reported(2),
            FirmwareInstallStatus::Unsupported,
        );
        assert!(newer_body.contains("revision 2"));
        assert!(!newer_body.contains("revision 1"));
        for version in [
            FirmwareStatus::Pending,
            FirmwareStatus::Unreported,
            FirmwareStatus::Malformed,
            FirmwareStatus::UnsupportedFormat(2),
        ] {
            let (title, body) = firmware_receipt_copy(version, FirmwareInstallStatus::Unsupported);
            assert_eq!(title, "Installation date unavailable");
            assert!(!body.contains("revision 1"), "misleading copy: {body}");
        }
    }

    #[test]
    fn release_state_requires_the_exact_target_except_while_the_report_is_pending() {
        let target = release_updater::firmware_targets().unwrap()[0].id.as_str();
        let pending = FirmwareDetails::default();
        assert_eq!(
            firmware_release_state(&pending, target, 3),
            Some(FirmwareReleaseState::Pending)
        );

        let matching = FirmwareDetails {
            target: FirmwareTargetStatus::Reported(target.to_owned()),
            version: FirmwareStatus::Reported(2),
            ..FirmwareDetails::default()
        };
        assert_eq!(
            firmware_release_state(&matching, target, 3),
            Some(FirmwareReleaseState::UpdateAvailable)
        );

        let targetless = FirmwareDetails {
            version: FirmwareStatus::Reported(2),
            ..FirmwareDetails::default()
        };
        assert_eq!(firmware_release_state(&targetless, target, 3), None);

        let different = FirmwareDetails {
            target: FirmwareTargetStatus::Reported("community-nrf52840".to_owned()),
            version: FirmwareStatus::Reported(2),
            ..FirmwareDetails::default()
        };
        assert_eq!(firmware_release_state(&different, target, 3), None);
    }

    #[test]
    fn uf2_disconnect_notice_explains_the_expected_macos_warning() {
        let target = &release_updater::firmware_targets().unwrap()[0];
        let notice = format!(
            "During installation, the temporary {} UF2 drive disconnects automatically. macOS may show a harmless \"Disk Not Ejected Properly\" notification even when verification succeeds.",
            target.display_name
        );
        assert!(notice.contains("Disk Not Ejected Properly"));
        assert!(notice.contains("verification succeeds"));
    }
}
