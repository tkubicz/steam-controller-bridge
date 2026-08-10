use std::sync::atomic::Ordering;

use eframe::egui;
use release_updater::{classify_firmware_release, FirmwareReleaseState, ReleaseManifestV1};
use ui_theme::{ACCENT, BORDER, DANGER, MUTED_TEXT, ON_ACCENT, SUCCESS, SURFACE, TEXT};

use super::{Activity, AppCenter, CatalogStatus, StatusTone};
use crate::app_center_protocol::FirmwareStatus;
use crate::window_ui::{full_width_card, render_release_notes};

impl AppCenter {
    pub(super) fn updates_page(&mut self, ui: &mut egui::Ui) {
        self.status_banner(ui);
        if self.catalog_status == CatalogStatus::Failed && !self.busy() {
            ui.add_space(10.0);
            if secondary_button(ui, "Retry Check", true).clicked() {
                self.catalog_status.retry_if_failed();
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
        if matches!(self.activity, Activity::Busy { can_cancel: true }) {
            ui.add_space(14.0);
            if secondary_button(ui, "Cancel Before Writing", true).clicked() {
                self.cancel.store(true, Ordering::Release);
                "Cancelling safely…".clone_into(&mut self.status);
                self.status_tone = StatusTone::Info;
            }
        }
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
                    if secondary_button(ui, "Show New App and Applications", true).clicked() {
                        self.reveal_application();
                    }
                    ui.add_space(6.0);
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
                    ui.add_space(10.0);
                    if primary_button(
                        ui,
                        "Quit Bridge for Replacement",
                        self.replacement_supported,
                    )
                    .clicked()
                    {
                        self.quit_for_replacement(ui.ctx().clone());
                    }
                }
                std::cmp::Ordering::Less => {
                    status_callout(
                        ui,
                        ACCENT,
                        &format!("Version {} is ready", manifest.application_version),
                        "Download and verify the application before replacing the installed version.",
                    );
                    ui.add_space(12.0);
                    if primary_button(ui, "Download Application Update", !self.busy()).clicked() {
                        self.download_application(manifest.clone(), ui.ctx().clone());
                    }
                }
                std::cmp::Ordering::Equal => {
                    status_callout(
                        ui,
                        SUCCESS,
                        "Application is up to date",
                        "You are running the latest signed stable release.",
                    );
                }
                std::cmp::Ordering::Greater => {
                    status_callout(
                        ui,
                        MUTED_TEXT,
                        "Newer than stable",
                        "This application is newer than the latest stable release. No downgrade is offered.",
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
            let release_state =
                classify_firmware_release(self.firmware.into(), manifest.firmware.revision);
            let (badge, badge_colour) = match release_state {
                FirmwareReleaseState::Pending => ("Checking firmware", MUTED_TEXT),
                FirmwareReleaseState::UpdateAvailable => ("Update available", ACCENT),
                FirmwareReleaseState::Current => ("Up to date", SUCCESS),
                FirmwareReleaseState::Newer => ("Newer firmware", MUTED_TEXT),
            };
            card_header(
                ui,
                "FIRMWARE",
                &format!("XIAO firmware revision {}", manifest.firmware.revision),
                &format!(
                    "Connected revision {} · non-Sense XIAO nRF52840",
                    firmware_description(self.firmware)
                ),
                badge,
                badge_colour,
            );
            ui.add_space(14.0);
            if app_pending {
                status_callout(
                    ui,
                    ACCENT,
                    "Application update required first",
                    "Replace and relaunch the application before installing this firmware.",
                );
                ui.add_space(12.0);
                let _ = primary_button(ui, "Update Application First", false);
            } else if app_incompatible {
                status_callout(
                    ui,
                    DANGER,
                    "Newer application required",
                    "The installed application cannot communicate with this firmware revision.",
                );
            } else if release_state == FirmwareReleaseState::Pending {
                status_callout(
                    ui,
                    MUTED_TEXT,
                    "Waiting for firmware information",
                    "Reconnect the board if its firmware revision does not appear shortly.",
                );
            } else if release_state == FirmwareReleaseState::Newer {
                status_callout(
                    ui,
                    MUTED_TEXT,
                    "Firmware is newer than stable",
                    "Downgrading the connected board is disabled.",
                );
            } else if release_state == FirmwareReleaseState::Current {
                status_callout(
                    ui,
                    SUCCESS,
                    "Firmware is up to date",
                    "The connected board reports the latest signed revision.",
                );
                ui.add_space(12.0);
                if secondary_button(ui, "Reinstall Firmware", !self.busy()).clicked() {
                    self.install_firmware(manifest.clone(), ui.ctx().clone());
                }
            } else {
                status_callout(
                    ui,
                    ACCENT,
                    &format!("Revision {} is ready", manifest.firmware.revision),
                    "The board and firmware are verified before anything is written.",
                );
                ui.add_space(12.0);
                if primary_button(ui, "Install Firmware Update", !self.busy()).clicked() {
                    self.install_firmware(manifest.clone(), ui.ctx().clone());
                }
            }
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("The bridge pauses only while flashing. If automatic bootloader entry fails, double-tap RESET. Success requires the exact signed revision to reconnect.")
                    .size(13.0)
                    .color(MUTED_TEXT),
            );
        });
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

pub(super) fn firmware_badge(status: FirmwareStatus) -> String {
    match status {
        FirmwareStatus::Reported(revision) => format!("Firmware rev {revision}"),
        FirmwareStatus::UnsupportedFormat(_) => "Firmware newer".to_owned(),
        FirmwareStatus::Pending => "Checking firmware".to_owned(),
        FirmwareStatus::Malformed | FirmwareStatus::Unreported => {
            "Firmware update needed".to_owned()
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

fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).strong().color(ON_ACCENT))
            .fill(ACCENT)
            .stroke(egui::Stroke::NONE)
            .corner_radius(8)
            .min_size(egui::vec2(180.0, 36.0)),
    )
}

fn secondary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).strong().color(TEXT))
            .fill(SURFACE)
            .stroke(egui::Stroke::new(1.0, BORDER))
            .corner_radius(8)
            .min_size(egui::vec2(164.0, 36.0)),
    )
}
