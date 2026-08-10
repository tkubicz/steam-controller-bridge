use std::io::{BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use eframe::egui;
use release_updater::{
    ensure_release_artifact, flash_firmware, guided_replacement_supported, installed_macos_version,
    refresh_catalog_if_due, stage_application, ApplicationRelease, ArtifactDescriptor,
    FirmwareFlashProgress, FirmwareRelease, LatestReleaseClient, ReleaseCache, ReleaseManifestV1,
    APPLICATION_BUNDLE_ID, FIRMWARE_BOARD_ID, FIRMWARE_TARGET_ID, UF2_FAMILY_ID,
    XIAO_USB_MANUFACTURER, XIAO_USB_PRODUCT, XIAO_USB_PRODUCT_ID, XIAO_USB_VENDOR_ID,
};
use semver::Version;
use ui_theme::{
    ACCENT, ACCENT_SUBTLE, BORDER, DANGER, MUTED_TEXT, ON_ACCENT, PANEL, SUCCESS, SURFACE, TEXT,
};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

use crate::update_check::{update_context, CHECK_INTERVAL};
use crate::update_protocol::{
    encode, read, UpdateRequest, UpdateResponse, UPDATE_CENTER_DEMO_ARGUMENT,
};
use crate::window_ui::{
    activate_window, configure_window_style, full_width_card, hero_transition, load_texture,
    parse_release_notes, render_inline, ReleaseNotes,
};

const WINDOW_TITLE: &str = "Steam Controller Bridge Update Center";
const APP_ICON: &[u8] = include_bytes!("../../../packaging/macos/AppIcon.png");
const WINDOW_SIZE: [f32; 2] = [760.0, 680.0];
const MIN_WINDOW_SIZE: [f32; 2] = [640.0, 540.0];

pub fn run() -> Result<(), String> {
    let demo = demo_mode();
    let firmware = std::env::args()
        .skip_while(|argument| argument != "--firmware-version")
        .nth(1)
        .unwrap_or_else(|| "unknown".to_owned());
    let icon = eframe::icon_data::from_png_bytes(APP_ICON).map_err(|error| error.to_string())?;
    let options = eframe::NativeOptions {
        event_loop_builder: Some(Box::new(|builder| {
            builder
                .with_activation_policy(ActivationPolicy::Accessory)
                .with_activate_ignoring_other_apps(true);
        })),
        viewport: egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(MIN_WINDOW_SIZE)
            .with_icon(icon),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |creation| {
            configure_window_style(&creation.egui_ctx);
            activate_window();
            Ok(Box::new(UpdateCenter::new(
                creation.egui_ctx.clone(),
                firmware,
                demo,
            )))
        }),
    )
    .map_err(|error| error.to_string())
}

#[derive(Clone)]
struct HostClient {
    inner: Arc<Mutex<HostConnection>>,
}

struct HostConnection {
    input: BufReader<std::io::Stdin>,
    output: std::io::Stdout,
}

impl HostClient {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HostConnection {
                input: BufReader::new(std::io::stdin()),
                output: std::io::stdout(),
            })),
        }
    }

    fn request(&self, request: UpdateRequest) -> Result<UpdateResponse, String> {
        let mut connection = self.inner.lock().map_err(|_| "Update Center IPC failed")?;
        let encoded = encode(request)?;
        connection
            .output
            .write_all(&encoded)
            .and_then(|()| connection.output.flush())
            .map_err(|error| error.to_string())?;
        let response: UpdateResponse =
            read(&mut connection.input)?.ok_or("Update Center host response is unavailable")?;
        if let UpdateResponse::Error { message } = &response {
            return Err(message.clone());
        }
        Ok(response)
    }
}

enum WorkerEvent {
    Catalog(Box<Result<ReleaseManifestV1, String>>),
    Application(Result<PathBuf, String>),
    FirmwareProgress(FirmwareFlashProgress),
    Firmware(Result<(), String>),
    Quit(Result<(), String>),
}

struct UpdateCenter {
    catalog: Option<ReleaseManifestV1>,
    release_notes: Vec<ReleaseNotes>,
    installed: Version,
    firmware_version: String,
    status: String,
    activity: Activity,
    staged_application: Option<PathBuf>,
    events: mpsc::Receiver<WorkerEvent>,
    sender: mpsc::Sender<WorkerEvent>,
    host: HostClient,
    cancel: Arc<AtomicBool>,
    replacement_supported: bool,
    app_icon: egui::TextureHandle,
    demo: Option<DemoMode>,
    status_tone: StatusTone,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Activity {
    Idle,
    Busy { can_cancel: bool },
    Closing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusTone {
    Info,
    Success,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DemoMode {
    Available,
    Current,
}

impl UpdateCenter {
    fn new(ctx: egui::Context, firmware_version: String, demo: Option<DemoMode>) -> Self {
        let (sender, events) = mpsc::channel();
        let catalog = demo.map(demo_manifest);
        let release_notes = catalog.as_ref().map_or_else(Vec::new, |manifest| {
            parse_release_notes(&manifest.release_notes)
        });
        let installed = match demo {
            Some(DemoMode::Available) => Version::new(1, 5, 0),
            Some(DemoMode::Current) => Version::new(1, 6, 0),
            None => Version::parse(env!("CARGO_PKG_VERSION")).expect("package version"),
        };
        let firmware_version = match demo {
            Some(DemoMode::Available) => "1".to_owned(),
            Some(DemoMode::Current) => "2".to_owned(),
            None => firmware_version,
        };
        let center = Self {
            catalog,
            release_notes,
            installed,
            firmware_version,
            status: if demo.is_some() {
                "Demo preview — no network, files, or hardware will be accessed.".to_owned()
            } else {
                "Checking the signed stable release…".to_owned()
            },
            activity: if demo.is_some() {
                Activity::Idle
            } else {
                Activity::Busy { can_cancel: false }
            },
            staged_application: None,
            events,
            sender,
            host: HostClient::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            replacement_supported: demo.is_some()
                || guided_replacement_supported(env!("CARGO_PKG_VERSION")),
            app_icon: load_texture(&ctx, "update-center-app-icon", APP_ICON),
            demo,
            status_tone: StatusTone::Info,
        };
        if demo.is_none() {
            center.check_catalog(ctx);
        }
        center
    }

    fn check_catalog(&self, ctx: egui::Context) {
        let sender = self.sender.clone();
        thread::spawn(move || {
            let _ = sender.send(WorkerEvent::Catalog(Box::new(fetch_catalog())));
            ctx.request_repaint();
        });
    }

    fn download_application(&mut self, manifest: ReleaseManifestV1, ctx: egui::Context) {
        if self.demo.is_some() {
            self.staged_application = Some(PathBuf::from(
                "/Applications/Steam Controller Bridge.app (demo)",
            ));
            "Demo: the verified application is ready for replacement.".clone_into(&mut self.status);
            self.status_tone = StatusTone::Success;
            return;
        }
        self.activity = Activity::Busy { can_cancel: false };
        self.status_tone = StatusTone::Info;
        "Downloading and validating the application…".clone_into(&mut self.status);
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = download_and_stage_application(&manifest);
            let _ = sender.send(WorkerEvent::Application(result));
            ctx.request_repaint();
        });
    }

    fn install_firmware(&mut self, manifest: ReleaseManifestV1, ctx: egui::Context) {
        if self.demo.is_some() {
            self.firmware_version = manifest.firmware.revision.to_string();
            "Demo: firmware installation completed and verified.".clone_into(&mut self.status);
            self.status_tone = StatusTone::Success;
            return;
        }
        self.activity = Activity::Busy { can_cancel: true };
        self.status_tone = StatusTone::Info;
        self.cancel.store(false, Ordering::Release);
        "Preparing the verified firmware…".clone_into(&mut self.status);
        let sender = self.sender.clone();
        let host = self.host.clone();
        let cancel = Arc::clone(&self.cancel);
        thread::spawn(move || {
            let result = (|| {
                let path = download_firmware(&manifest)?;
                host.request(UpdateRequest::SuspendBridge)?;
                let flash_result = flash_firmware(
                    &path,
                    &manifest.firmware,
                    Path::new("/Volumes"),
                    &cancel,
                    |progress| {
                        let _ = sender.send(WorkerEvent::FirmwareProgress(progress));
                        ctx.request_repaint();
                    },
                )
                .map_err(|error| error.to_string());
                let resume_result = host.request(UpdateRequest::ResumeBridge).map(|_| ());
                flash_result.and(resume_result)
            })();
            let _ = sender.send(WorkerEvent::Firmware(result));
            ctx.request_repaint();
        });
    }

    fn quit_for_replacement(&mut self, ctx: egui::Context) {
        if self.demo.is_some() {
            "Demo: the bridge would now quit safely for replacement.".clone_into(&mut self.status);
            self.status_tone = StatusTone::Success;
            return;
        }
        self.activity = Activity::Busy { can_cancel: false };
        self.status_tone = StatusTone::Info;
        "Waiting for the bridge to release hardware safely…".clone_into(&mut self.status);
        let sender = self.sender.clone();
        let host = self.host.clone();
        thread::spawn(move || {
            let result = host.request(UpdateRequest::QuitForReplacement).map(|_| ());
            let _ = sender.send(WorkerEvent::Quit(result));
            ctx.request_repaint();
        });
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                WorkerEvent::Catalog(result) => match *result {
                    Ok(manifest) => {
                        "Signed release information is current.".clone_into(&mut self.status);
                        self.release_notes = parse_release_notes(&manifest.release_notes);
                        self.catalog = Some(manifest);
                        self.activity = Activity::Idle;
                        self.status_tone = StatusTone::Success;
                    }
                    Err(error) => {
                        self.status = error;
                        self.activity = Activity::Idle;
                        self.status_tone = StatusTone::Error;
                    }
                },
                WorkerEvent::Application(Ok(path)) => {
                    "The new application is verified and ready in Finder."
                        .clone_into(&mut self.status);
                    self.staged_application = Some(path);
                    self.activity = Activity::Idle;
                    self.status_tone = StatusTone::Success;
                }
                WorkerEvent::FirmwareProgress(progress) => {
                    self.activity = Activity::Busy {
                        can_cancel: !matches!(
                            progress,
                            FirmwareFlashProgress::Writing
                                | FirmwareFlashProgress::WaitingForApplication
                                | FirmwareFlashProgress::Verifying
                        ),
                    };
                    progress_text(&progress).clone_into(&mut self.status);
                    self.status_tone = StatusTone::Info;
                }
                WorkerEvent::Firmware(Ok(())) => {
                    "Firmware updated and verified. The bridge has restarted."
                        .clone_into(&mut self.status);
                    self.firmware_version = self.catalog.as_ref().map_or_else(
                        || "unknown".to_owned(),
                        |item| item.firmware.revision.to_string(),
                    );
                    self.activity = Activity::Idle;
                    self.status_tone = StatusTone::Success;
                }
                WorkerEvent::Firmware(Err(error)) => {
                    self.status =
                        format!("{error} Check the cable, disconnect extra boards, and retry.");
                    self.activity = Activity::Idle;
                    self.status_tone = StatusTone::Error;
                }
                WorkerEvent::Application(Err(error)) | WorkerEvent::Quit(Err(error)) => {
                    self.status = error;
                    self.activity = Activity::Idle;
                    self.status_tone = StatusTone::Error;
                }
                WorkerEvent::Quit(Ok(())) => self.activity = Activity::Closing,
            }
        }
    }

    fn busy(&self) -> bool {
        self.activity != Activity::Idle
    }

    fn reveal_application(&mut self) {
        if self.demo.is_some() {
            "Demo: Finder would reveal the verified app and Applications."
                .clone_into(&mut self.status);
            self.status_tone = StatusTone::Success;
            return;
        }
        let Some(path) = self.staged_application.as_ref() else {
            return;
        };
        let selected = Command::new("/usr/bin/open").arg("-R").arg(path).spawn();
        let applications = Command::new("/usr/bin/open").arg("/Applications").spawn();
        if selected.is_err() || applications.is_err() {
            "Finder could not be opened; the verified app remains cached."
                .clone_into(&mut self.status);
            self.status_tone = StatusTone::Error;
        }
    }

    fn hero(&self, ui: &mut egui::Ui) {
        let content_width = (ui.available_width() - 56.0).max(0.0);
        egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(egui::Margin::symmetric(28, 24))
            .show(ui, |ui| {
                ui.set_min_width(content_width);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(&self.app_icon)
                            .fit_to_exact_size(egui::vec2(84.0, 84.0))
                            .corner_radius(20),
                    );
                    ui.add_space(14.0);
                    ui.vertical(|ui| {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Update Center")
                                .size(28.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            egui::RichText::new("Application and XIAO firmware")
                                .size(15.0)
                                .color(MUTED_TEXT),
                        );
                        ui.add_space(7.0);
                        egui::Frame::new()
                            .fill(ACCENT_SUBTLE)
                            .corner_radius(7)
                            .inner_margin(egui::Margin::symmetric(10, 5))
                            .show(ui, |ui| {
                                let label = if let Some(mode) = self.demo {
                                    format!("Demo preview · {}", mode.label())
                                } else if let Some(manifest) = &self.catalog {
                                    format!("Signed stable release {}", manifest.release_tag)
                                } else {
                                    "Checking signed release…".to_owned()
                                };
                                ui.label(egui::RichText::new(label).size(13.0).color(ACCENT));
                            });
                    });
                });
            });
    }

    fn status_banner(&self, ui: &mut egui::Ui) {
        let (colour, fill) = match self.status_tone {
            StatusTone::Info => (ACCENT, ACCENT_SUBTLE),
            StatusTone::Success => (SUCCESS, SUCCESS.gamma_multiply(0.16)),
            StatusTone::Error => (DANGER, DANGER.gamma_multiply(0.14)),
        };
        egui::Frame::new()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, colour.gamma_multiply(0.55)))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(14, 11))
            .show(ui, |ui| {
                ui.set_width((ui.available_width() - 2.0).max(0.0));
                ui.horizontal(|ui| {
                    if self.busy() {
                        ui.spinner();
                    } else {
                        ui.label(egui::RichText::new("●").size(11.0).color(colour));
                    }
                    ui.label(egui::RichText::new(&self.status).color(TEXT));
                });
            });
    }
}

impl eframe::App for UpdateCenter {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();
        if self.activity == Activity::Closing {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::Frame::new().fill(PANEL).show(ui, |ui| {
            ui.set_min_size(ui.available_size());

            egui::Panel::top("update-center-hero")
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(PANEL))
                .show(ui, |ui| {
                    // Match About exactly: the hero and gradient touch, with
                    // no panel-coloured gap between the two surfaces.
                    ui.spacing_mut().item_spacing.y = 0.0;
                    self.hero(ui);
                    hero_transition(ui);
                });

            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(PANEL)
                        .inner_margin(egui::Margin::symmetric(24, 8)),
                )
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.status_banner(ui);
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
                            ui.add_space(16.0);
                        });
                });
        });
    }
}

impl UpdateCenter {
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
            let target_revision = manifest.firmware.revision.to_string();
            let current_firmware = self.firmware_version == target_revision;
            let newer_firmware = self
                .firmware_version
                .parse::<u16>()
                .is_ok_and(|revision| revision > manifest.firmware.revision)
                || self.firmware_version == "newer";
            let (badge, badge_colour) = if newer_firmware {
                ("Newer firmware", MUTED_TEXT)
            } else if current_firmware {
                ("Up to date", SUCCESS)
            } else {
                ("Update available", ACCENT)
            };
            card_header(
                ui,
                "FIRMWARE",
                &format!("XIAO firmware revision {}", manifest.firmware.revision),
                &format!(
                    "Connected revision {} · non-Sense XIAO nRF52840",
                    self.firmware_version
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
            } else if newer_firmware {
                status_callout(
                    ui,
                    MUTED_TEXT,
                    "Firmware is newer than stable",
                    "Downgrading the connected board is disabled.",
                );
            } else if current_firmware {
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
                    "The Update Center will verify the board and firmware before writing.",
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
            for (release_index, release) in self.release_notes.iter().enumerate() {
                if release_index > 0 {
                    ui.add_space(12.0);
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    render_inline(ui, &release.title, 17.0, true);
                });
                for section in &release.sections {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(section.title.to_uppercase())
                            .size(11.0)
                            .strong()
                            .color(ACCENT),
                    );
                    for note in &section.notes {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.label(egui::RichText::new("•").color(ACCENT));
                            render_inline(ui, note, 14.0, false);
                        });
                    }
                }
            }
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

impl DemoMode {
    fn label(self) -> &'static str {
        match self {
            Self::Available => "update available",
            Self::Current => "up to date",
        }
    }
}

fn demo_mode() -> Option<DemoMode> {
    let arguments: Vec<_> = std::env::args().collect();
    arguments.iter().enumerate().find_map(|(index, argument)| {
        if argument == UPDATE_CENTER_DEMO_ARGUMENT {
            return Some(match arguments.get(index + 1).map(String::as_str) {
                Some("current") => DemoMode::Current,
                _ => DemoMode::Available,
            });
        }
        argument
            .strip_prefix("--update-center-demo=")
            .map(|state| match state {
                "current" => DemoMode::Current,
                _ => DemoMode::Available,
            })
    })
}

fn demo_manifest(_mode: DemoMode) -> ReleaseManifestV1 {
    let application_version = Version::new(1, 6, 0);
    ReleaseManifestV1 {
        schema_version: 1,
        release_tag: "v1.6.0".to_owned(),
        application_version: application_version.clone(),
        minimum_macos: Version::new(13, 0, 0),
        release_notes: r"## [1.6.0](https://github.com/tkubicz/steam-controller-bridge/releases/tag/v1.6.0) (2026-08-10)

### Features

* **updater:** add signed application and XIAO firmware updates ([#42](https://github.com/tkubicz/steam-controller-bridge/pull/42))
* **menu:** make update status and recovery actions easier to understand

### Bug Fixes

* **firmware:** verify the exact revision after reconnecting
* **menu:** keep release notes readable at every window size"
            .to_owned(),
        application: ApplicationRelease {
            bundle_identifier: APPLICATION_BUNDLE_ID.to_owned(),
            version: application_version.clone(),
            artifact: ArtifactDescriptor {
                name: "steam-controller-bridge-macos.zip".to_owned(),
                size: 12 * 1024 * 1024,
                sha256: "11".repeat(32),
            },
        },
        firmware: FirmwareRelease {
            target: FIRMWARE_TARGET_ID.to_owned(),
            revision: 2,
            minimum_application_version: application_version,
            protocol_version: 1,
            device_info_format: 1,
            board_id: FIRMWARE_BOARD_ID.to_owned(),
            uf2_family_id: UF2_FAMILY_ID,
            usb_vendor_id: XIAO_USB_VENDOR_ID,
            usb_product_id: XIAO_USB_PRODUCT_ID,
            usb_manufacturer: XIAO_USB_MANUFACTURER.to_owned(),
            usb_product: XIAO_USB_PRODUCT.to_owned(),
            artifact: ArtifactDescriptor {
                name: "steam-controller-bridge-xiao-nrf52840.uf2".to_owned(),
                size: 256 * 1024,
                sha256: "22".repeat(32),
            },
        },
    }
}

fn cache() -> Result<ReleaseCache, String> {
    ReleaseCache::for_current_user().map_err(|error| error.to_string())
}

fn fetch_catalog() -> Result<ReleaseManifestV1, String> {
    let (keys, cache) = update_context()?;
    refresh_catalog_if_due(&LatestReleaseClient, &cache, &keys, CHECK_INTERVAL)
}

fn download_and_stage_application(manifest: &ReleaseManifestV1) -> Result<PathBuf, String> {
    if installed_macos_version()? < manifest.minimum_macos {
        return Err(format!(
            "This release requires macOS {} or newer.",
            manifest.minimum_macos
        ));
    }
    let cache = cache()?;
    let artifact = &manifest.application.artifact;
    let path = ensure_release_artifact(
        &LatestReleaseClient,
        &cache,
        &manifest.release_tag,
        artifact,
    )?;
    let staged = stage_application(
        &path,
        &manifest.application,
        &cache.root().join("staged-app"),
    )?;
    Ok(staged.bundle_path)
}

fn download_firmware(manifest: &ReleaseManifestV1) -> Result<PathBuf, String> {
    let cache = cache()?;
    let artifact = &manifest.firmware.artifact;
    ensure_release_artifact(
        &LatestReleaseClient,
        &cache,
        &manifest.release_tag,
        artifact,
    )
}

fn progress_text(progress: &FirmwareFlashProgress) -> &'static str {
    match progress {
        FirmwareFlashProgress::LookingForDevice => "Looking for one compatible XIAO…",
        FirmwareFlashProgress::EnteringBootloader => "Entering the UF2 bootloader…",
        FirmwareFlashProgress::WaitingForBootloader => {
            "Waiting for bootloader. Double-tap RESET if needed…"
        }
        FirmwareFlashProgress::Writing => "Writing firmware. Do not unplug the board…",
        FirmwareFlashProgress::WaitingForApplication => {
            "Waiting for the flashed device to reconnect…"
        }
        FirmwareFlashProgress::Verifying => "Verifying the reported firmware revision…",
    }
}
