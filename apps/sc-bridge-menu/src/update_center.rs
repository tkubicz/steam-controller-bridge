use std::io::{BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use eframe::egui;
use release_updater::{
    embedded_trusted_keys, ensure_release_artifact, flash_firmware, guided_replacement_supported,
    installed_macos_version, refresh_catalog_if_due, stage_application, FirmwareFlashProgress,
    LatestReleaseClient, ReleaseCache, ReleaseManifestV1,
};
use semver::Version;
use ui_theme::{BORDER, MUTED_TEXT, PANEL, SURFACE_RAISED, TEXT};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

use crate::update_protocol::{encode, read, UpdateRequest, UpdateResponse};

const WINDOW_TITLE: &str = "Steam Controller Bridge Update Center";
const APP_ICON: &[u8] = include_bytes!("../../../packaging/macos/AppIcon.png");

pub fn run() -> Result<(), String> {
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
            .with_inner_size([760.0, 650.0])
            .with_min_inner_size([640.0, 520.0])
            .with_icon(icon),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |creation| {
            ui_theme::configure_ui(&creation.egui_ctx);
            Ok(Box::new(UpdateCenter::new(
                creation.egui_ctx.clone(),
                firmware,
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
    firmware_version: String,
    status: String,
    activity: Activity,
    staged_application: Option<PathBuf>,
    events: mpsc::Receiver<WorkerEvent>,
    sender: mpsc::Sender<WorkerEvent>,
    host: HostClient,
    cancel: Arc<AtomicBool>,
    replacement_supported: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Activity {
    Idle,
    Busy { can_cancel: bool },
    Closing,
}

impl UpdateCenter {
    fn new(ctx: egui::Context, firmware_version: String) -> Self {
        let (sender, events) = mpsc::channel();
        let center = Self {
            catalog: None,
            firmware_version,
            status: "Checking the signed stable release…".to_owned(),
            activity: Activity::Busy { can_cancel: false },
            staged_application: None,
            events,
            sender,
            host: HostClient::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            replacement_supported: guided_replacement_supported(),
        };
        center.check_catalog(ctx);
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
        self.activity = Activity::Busy { can_cancel: false };
        "Downloading and validating the application…".clone_into(&mut self.status);
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = download_and_stage_application(&manifest);
            let _ = sender.send(WorkerEvent::Application(result));
            ctx.request_repaint();
        });
    }

    fn install_firmware(&mut self, manifest: ReleaseManifestV1, ctx: egui::Context) {
        self.activity = Activity::Busy { can_cancel: true };
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
        self.activity = Activity::Busy { can_cancel: false };
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
                        self.catalog = Some(manifest);
                        self.activity = Activity::Idle;
                    }
                    Err(error) => {
                        self.status = error;
                        self.activity = Activity::Idle;
                    }
                },
                WorkerEvent::Application(Ok(path)) => {
                    "The new application is verified and ready in Finder."
                        .clone_into(&mut self.status);
                    self.staged_application = Some(path);
                    self.activity = Activity::Idle;
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
                }
                WorkerEvent::Firmware(Ok(())) => {
                    "Firmware updated and verified. The bridge has restarted."
                        .clone_into(&mut self.status);
                    self.firmware_version = self.catalog.as_ref().map_or_else(
                        || "unknown".to_owned(),
                        |item| item.firmware.revision.to_string(),
                    );
                    self.activity = Activity::Idle;
                }
                WorkerEvent::Firmware(Err(error)) => {
                    self.status =
                        format!("{error} Check the cable, disconnect extra boards, and retry.");
                    self.activity = Activity::Idle;
                }
                WorkerEvent::Application(Err(error)) | WorkerEvent::Quit(Err(error)) => {
                    self.status = error;
                    self.activity = Activity::Idle;
                }
                WorkerEvent::Quit(Ok(())) => self.activity = Activity::Closing,
            }
        }
    }

    fn busy(&self) -> bool {
        self.activity != Activity::Idle
    }

    fn reveal_application(&mut self) {
        let Some(path) = self.staged_application.as_ref() else {
            return;
        };
        let selected = Command::new("/usr/bin/open").arg("-R").arg(path).spawn();
        let applications = Command::new("/usr/bin/open").arg("/Applications").spawn();
        if selected.is_err() || applications.is_err() {
            "Finder could not be opened; the verified app remains cached."
                .clone_into(&mut self.status);
        }
    }
}

impl eframe::App for UpdateCenter {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();
        if self.activity == Activity::Closing {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::Frame::new()
            .fill(PANEL)
            .inner_margin(24.0)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                ui.heading(egui::RichText::new("Update Center").size(28.0).color(TEXT));
                ui.label(egui::RichText::new(&self.status).color(MUTED_TEXT));
                ui.add_space(18.0);
                let catalog = self.catalog.take();
                if let Some(manifest) = catalog.as_ref() {
                    self.application_card(ui, manifest);
                    ui.add_space(14.0);
                    self.firmware_card(ui, manifest);
                }
                self.catalog = catalog;
                if self.busy() {
                    ui.add_space(16.0);
                    ui.spinner();
                }
                if matches!(self.activity, Activity::Busy { can_cancel: true })
                    && ui.button("Cancel before writing").clicked()
                {
                    self.cancel.store(true, Ordering::Release);
                    "Cancelling safely…".clone_into(&mut self.status);
                }
            });
    }
}

impl UpdateCenter {
    fn application_card(&mut self, ui: &mut egui::Ui, manifest: &ReleaseManifestV1) {
        card(ui, |ui| {
            let installed = Version::parse(env!("CARGO_PKG_VERSION")).expect("package version");
            ui.heading(format!("Application {}", manifest.application_version));
            ui.label(format!(
                "Installed: {installed} · Requires macOS {}+",
                manifest.minimum_macos
            ));
            ui.add_space(8.0);
            ui.label(&manifest.release_notes);
            ui.add_space(10.0);
            match installed.cmp(&manifest.application_version) {
                std::cmp::Ordering::Less if self.staged_application.is_some() => {
                    let path = self.staged_application.as_ref().expect("checked above");
                    ui.label(format!("Verified: {}", path.display()));
                    if ui.button("Show New App and Applications").clicked() {
                        self.reveal_application();
                    }
                    if self.replacement_supported {
                        ui.label("Quit, drag the new app into Applications, choose Replace, then launch it. Right-click Open may be required for this ad-hoc-signed build.");
                    } else {
                        ui.label("This source or metadata-mismatched build is unsupported for guided replacement. The verified archive remains available for manual installation.");
                    }
                    if ui
                        .add_enabled(
                            self.replacement_supported,
                            egui::Button::new("Quit Bridge for Replacement"),
                        )
                        .clicked()
                    {
                        self.quit_for_replacement(ui.ctx().clone());
                    }
                }
                std::cmp::Ordering::Less => {
                    if ui
                        .add_enabled(
                            !self.busy(),
                            egui::Button::new("Download Application Update"),
                        )
                        .clicked()
                    {
                        self.download_application(manifest.clone(), ui.ctx().clone());
                    }
                }
                std::cmp::Ordering::Equal => {
                    ui.label("The application is up to date.");
                }
                std::cmp::Ordering::Greater => {
                    ui.label("This application is newer than the latest stable release; no downgrade is offered.");
                }
            }
        });
    }

    fn firmware_card(&mut self, ui: &mut egui::Ui, manifest: &ReleaseManifestV1) {
        card(ui, |ui| {
            let installed = Version::parse(env!("CARGO_PKG_VERSION")).expect("package version");
            let app_pending = installed < manifest.application_version;
            let newer_firmware = self
                .firmware_version
                .parse::<u16>()
                .is_ok_and(|revision| revision > manifest.firmware.revision)
                || self.firmware_version == "newer";
            ui.heading(format!("XIAO firmware rev {}", manifest.firmware.revision));
            ui.label(format!(
                "Connected: {} · Target: non-Sense XIAO nRF52840",
                self.firmware_version
            ));
            if app_pending {
                ui.label("Update and relaunch the application before installing firmware.");
            } else if installed < manifest.firmware.minimum_application_version {
                ui.label("This firmware requires a newer application.");
            } else if newer_firmware {
                ui.label("The connected firmware is newer; downgrade is disabled.");
            } else if ui
                .add_enabled(
                    !self.busy(),
                    egui::Button::new(
                        if self.firmware_version == manifest.firmware.revision.to_string() {
                            "Reinstall Firmware"
                        } else {
                            "Install/Update Firmware"
                        },
                    ),
                )
                .clicked()
            {
                self.install_firmware(manifest.clone(), ui.ctx().clone());
            }
            ui.label("The bridge will pause only for flashing. If automatic bootloader entry fails, double-tap RESET; verification must report the exact signed revision.");
        });
    }
}

fn card<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(12)
        .inner_margin(18.0)
        .show(ui, contents)
        .inner
}

fn cache() -> Result<ReleaseCache, String> {
    ReleaseCache::for_current_user().map_err(|error| error.to_string())
}

fn fetch_catalog() -> Result<ReleaseManifestV1, String> {
    let keys = embedded_trusted_keys().map_err(|error| error.to_string())?;
    if keys.is_empty() {
        return Err("Secure updates are unavailable in this source build: no release public key is embedded.".to_owned());
    }
    let cache = cache()?;
    refresh_catalog_if_due(
        &LatestReleaseClient,
        &cache,
        &keys,
        Duration::from_hours(24),
    )
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
