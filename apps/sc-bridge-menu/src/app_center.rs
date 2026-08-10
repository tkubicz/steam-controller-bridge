use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use eframe::egui;
use release_updater::{
    flash_firmware, guided_replacement_supported, ApplicationRelease, ArtifactDescriptor,
    FirmwareFlashProgress, FirmwareRelease, ReleaseManifestV1, APPLICATION_BUNDLE_ID,
    FIRMWARE_BOARD_ID, FIRMWARE_TARGET_ID, UF2_FAMILY_ID, XIAO_USB_MANUFACTURER, XIAO_USB_PRODUCT,
    XIAO_USB_PRODUCT_ID, XIAO_USB_VENDOR_ID,
};
use semver::Version;
use ui_theme::{ACCENT, ACCENT_SUBTLE, DANGER, MUTED_TEXT, PANEL, SUCCESS, SURFACE, TEXT};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

use crate::about_pages::AboutContent;
use crate::app_center_protocol::{
    AppCenterCommand, AppCenterPage, FirmwareStatus, UpdateOperation,
};
use crate::cli::{AppCenterArgs, DemoMode};
use crate::macos::{open_path, reveal_path};
use crate::update_check::running_version;
use crate::window_ui::{
    activate_window, configure_window_style, hero_transition, load_texture, parse_release_notes,
    ReleaseNotes,
};

const WINDOW_TITLE: &str = "Steam Controller Bridge";
const APP_ICON: &[u8] = include_bytes!("../../../packaging/macos/AppIcon.png");
const WINDOW_SIZE: [f32; 2] = [760.0, 680.0];
const MIN_WINDOW_SIZE: [f32; 2] = [640.0, 540.0];

mod host_client;
mod release_actions;
mod view;

use self::host_client::HostClient;
use self::release_actions::{
    download_and_stage_application, download_firmware, fetch_catalog, progress_text,
};
use self::view::{firmware_badge, hero_badge};

pub fn run(arguments: AppCenterArgs) -> Result<(), String> {
    let demo = arguments.demo;
    let page = arguments.page();
    let firmware = arguments.firmware;
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
            Ok(Box::new(AppCenter::new(
                &creation.egui_ctx,
                firmware,
                demo,
                page,
            )))
        }),
    )
    .map_err(|error| error.to_string())
}

enum WorkerEvent {
    Catalog(Box<Result<ReleaseManifestV1, String>>),
    Application(Result<PathBuf, String>),
    FirmwareProgress(FirmwareFlashProgress),
    Firmware(Result<(), FirmwareOperationError>),
    Quit(Result<(), String>),
}

#[derive(Debug, PartialEq, Eq)]
enum FirmwareOperationError {
    Preparation(String),
    Flash(String),
    Resume(String),
    FlashAndResume { flash: String, resume: String },
}

impl FirmwareOperationError {
    fn message(self) -> String {
        match self {
            Self::Preparation(error) => error,
            Self::Flash(error) => {
                format!("{error} Check the cable, disconnect extra boards, and retry.")
            }
            Self::Resume(error) => format!(
                "Firmware was written and verified, but bridge recovery failed: {error}"
            ),
            Self::FlashAndResume { flash, resume } => format!(
                "{flash} Bridge recovery also failed: {resume} Check the cable, disconnect extra boards, and retry; then restart the bridge manually."
            ),
        }
    }
}

fn combine_flash_and_resume(
    flash: Result<(), String>,
    resume: Result<(), String>,
) -> Result<(), FirmwareOperationError> {
    match (flash, resume) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(FirmwareOperationError::Flash(error)),
        (Ok(()), Err(error)) => Err(FirmwareOperationError::Resume(error)),
        (Err(flash), Err(resume)) => Err(FirmwareOperationError::FlashAndResume { flash, resume }),
    }
}

struct AppCenter {
    page: AppCenterPage,
    about: AboutContent,
    catalog: Option<ReleaseManifestV1>,
    release_notes: Vec<ReleaseNotes>,
    installed: Version,
    firmware: FirmwareStatus,
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
    host_commands: mpsc::Receiver<AppCenterCommand>,
    catalog_status: CatalogStatus,
    firmware_write_started: bool,
    firmware_session_active: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    close_when_idle: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Activity {
    Idle,
    Busy { can_cancel: bool },
    Closing,
}

fn must_defer_close(activity: Activity, firmware_session_active: bool) -> bool {
    firmware_session_active || matches!(activity, Activity::Busy { .. })
}

fn operation_available(activity: Activity, worker_running: bool) -> bool {
    activity == Activity::Idle && !worker_running
}

fn spawn_worker(
    slot: &mut Option<thread::JoinHandle<()>>,
    task: impl FnOnce() + Send + 'static,
) -> bool {
    if slot.is_some() {
        return false;
    }
    *slot = Some(thread::spawn(task));
    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusTone {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogStatus {
    NotRequested,
    Checking,
    Loaded,
    Failed,
}

impl CatalogStatus {
    fn begin_if_needed(&mut self, page: AppCenterPage) -> bool {
        if page != AppCenterPage::Updates || *self != Self::NotRequested {
            return false;
        }
        *self = Self::Checking;
        true
    }

    fn retry_if_failed(&mut self) -> bool {
        if *self != Self::Failed {
            return false;
        }
        *self = Self::NotRequested;
        true
    }
}

impl AppCenter {
    fn new(
        ctx: &egui::Context,
        firmware: FirmwareStatus,
        demo: Option<DemoMode>,
        page: AppCenterPage,
    ) -> Self {
        let (sender, events) = mpsc::channel();
        let (host, host_commands) = HostClient::new(ctx.clone());
        let catalog = demo.map(demo_manifest);
        let release_notes = catalog.as_ref().map_or_else(Vec::new, |manifest| {
            parse_release_notes(&manifest.release_notes)
        });
        let running = running_version();
        let installed = match demo {
            Some(DemoMode::Available) => running.clone(),
            Some(DemoMode::Current) => catalog.as_ref().map_or_else(
                || running.clone(),
                |manifest| manifest.application_version.clone(),
            ),
            None => running,
        };
        let firmware = match demo {
            Some(DemoMode::Available) => FirmwareStatus::Reported(1),
            Some(DemoMode::Current) => FirmwareStatus::Reported(2),
            None => firmware,
        };
        // A window that opens on Updates starts its check in `ensure_catalog`
        // on the first frame, so opening and navigating share one path.
        Self {
            page,
            about: AboutContent::new(ctx, &installed),
            catalog,
            release_notes,
            installed,
            firmware,
            status: if demo.is_some() {
                "Demo preview — updater networking, files, and hardware are disabled.".to_owned()
            } else {
                "Open Updates to check the signed stable release.".to_owned()
            },
            activity: Activity::Idle,
            staged_application: None,
            events,
            sender,
            host,
            cancel: Arc::new(AtomicBool::new(false)),
            replacement_supported: demo.is_some()
                || guided_replacement_supported(env!("CARGO_PKG_VERSION")),
            app_icon: load_texture(ctx, "app-center-app-icon", APP_ICON),
            demo,
            status_tone: StatusTone::Info,
            host_commands,
            catalog_status: if demo.is_some() {
                CatalogStatus::Loaded
            } else {
                CatalogStatus::NotRequested
            },
            firmware_write_started: false,
            firmware_session_active: Arc::new(AtomicBool::new(false)),
            worker: None,
            close_when_idle: false,
        }
    }

    fn check_catalog(&mut self, ctx: egui::Context) {
        let sender = self.sender.clone();
        self.cancel.store(false, Ordering::Release);
        let cancel = Arc::clone(&self.cancel);
        let started = spawn_worker(&mut self.worker, move || {
            let _ = sender.send(WorkerEvent::Catalog(Box::new(fetch_catalog(&cancel))));
            ctx.request_repaint();
        });
        debug_assert!(started, "catalog worker slot must be empty");
    }

    fn download_application(&mut self, manifest: ReleaseManifestV1, ctx: egui::Context) {
        if !self.operation_available() {
            return;
        }
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
        self.cancel.store(false, Ordering::Release);
        "Downloading and validating the application…".clone_into(&mut self.status);
        let sender = self.sender.clone();
        let cancel = Arc::clone(&self.cancel);
        let started = spawn_worker(&mut self.worker, move || {
            let result = download_and_stage_application(&manifest, &cancel);
            let _ = sender.send(WorkerEvent::Application(result));
            ctx.request_repaint();
        });
        debug_assert!(started, "application worker slot must be empty");
    }

    fn install_firmware(&mut self, manifest: ReleaseManifestV1, ctx: egui::Context) {
        if !self.operation_available() {
            return;
        }
        if self.demo.is_some() {
            self.firmware = FirmwareStatus::Reported(manifest.firmware.revision);
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
        let session_active = Arc::clone(&self.firmware_session_active);
        let started = spawn_worker(&mut self.worker, move || {
            let result = (|| -> Result<(), FirmwareOperationError> {
                let path = download_firmware(&manifest, &cancel)
                    .map_err(FirmwareOperationError::Preparation)?;
                if cancel.load(Ordering::Acquire) {
                    return Err(FirmwareOperationError::Preparation(
                        "firmware update cancelled".to_owned(),
                    ));
                }
                if let Err(error) = host.request(UpdateOperation::SuspendBridge) {
                    return Err(FirmwareOperationError::Preparation(
                        match host.request(UpdateOperation::ResumeBridge) {
                            Ok(()) => error,
                            Err(resume) => {
                                format!("{error} Bridge recovery also failed: {resume}")
                            }
                        },
                    ));
                }
                session_active.store(true, Ordering::Release);
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
                let resume_result = host.request(UpdateOperation::ResumeBridge);
                session_active.store(false, Ordering::Release);
                combine_flash_and_resume(flash_result, resume_result)
            })();
            let _ = sender.send(WorkerEvent::Firmware(result));
            ctx.request_repaint();
        });
        debug_assert!(started, "firmware worker slot must be empty");
    }

    fn quit_for_replacement(&mut self, ctx: egui::Context) {
        if !self.operation_available() {
            return;
        }
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
        let started = spawn_worker(&mut self.worker, move || {
            let result = host.request(UpdateOperation::QuitForReplacement);
            let _ = sender.send(WorkerEvent::Quit(result));
            ctx.request_repaint();
        });
        debug_assert!(started, "quit worker slot must be empty");
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            let terminal = !matches!(&event, WorkerEvent::FirmwareProgress(_));
            match event {
                WorkerEvent::Catalog(result) => match *result {
                    Ok(manifest) => {
                        "Signed release information is current.".clone_into(&mut self.status);
                        self.release_notes = parse_release_notes(&manifest.release_notes);
                        self.catalog = Some(manifest);
                        self.activity = Activity::Idle;
                        self.status_tone = StatusTone::Success;
                        self.catalog_status = CatalogStatus::Loaded;
                    }
                    Err(error) => {
                        self.status = error;
                        self.activity = Activity::Idle;
                        self.status_tone = StatusTone::Error;
                        self.catalog_status = CatalogStatus::Failed;
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
                    if matches!(
                        progress,
                        FirmwareFlashProgress::Writing
                            | FirmwareFlashProgress::WaitingForApplication
                            | FirmwareFlashProgress::Verifying
                    ) {
                        self.firmware_write_started = true;
                        self.page = AppCenterPage::Updates;
                    }
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
                    self.firmware = self
                        .catalog
                        .as_ref()
                        .map_or(FirmwareStatus::Unreported, |item| {
                            FirmwareStatus::Reported(item.firmware.revision)
                        });
                    self.activity = Activity::Idle;
                    self.status_tone = StatusTone::Success;
                    self.firmware_write_started = false;
                }
                WorkerEvent::Firmware(Err(error)) => {
                    self.status = error.message();
                    self.activity = Activity::Idle;
                    self.status_tone = StatusTone::Error;
                    self.firmware_write_started = false;
                }
                WorkerEvent::Application(Err(error)) | WorkerEvent::Quit(Err(error)) => {
                    self.status = error;
                    self.activity = Activity::Idle;
                    self.status_tone = StatusTone::Error;
                }
                WorkerEvent::Quit(Ok(())) => self.activity = Activity::Closing,
            }
            if terminal {
                self.join_worker();
                if self.close_when_idle {
                    self.activity = Activity::Closing;
                }
            }
        }
    }

    fn join_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                "The background operation stopped unexpectedly.".clone_into(&mut self.status);
                self.status_tone = StatusTone::Error;
                self.activity = Activity::Idle;
            }
        }
    }

    fn detect_worker_panic(&mut self) {
        if self
            .worker
            .as_ref()
            .is_some_and(thread::JoinHandle::is_finished)
        {
            self.join_worker();
            if self.close_when_idle {
                self.activity = Activity::Closing;
            }
        }
    }

    fn drain_host_commands(&mut self) {
        while let Ok(command) = self.host_commands.try_recv() {
            match command {
                AppCenterCommand::Close => self.request_close(),
                AppCenterCommand::Navigate { page, firmware } => {
                    self.firmware = firmware;
                    if self.firmware_write_started {
                        continue;
                    }
                    self.navigate_to(page);
                }
                AppCenterCommand::FirmwareVersion { firmware } => {
                    self.firmware = firmware;
                }
                AppCenterCommand::UpdateResponse(_) => {
                    // Responses are routed to the worker waiting in HostClient.
                }
            }
        }
    }

    fn request_close(&mut self) {
        self.close_when_idle = true;
        self.cancel.store(true, Ordering::Release);
        if !must_defer_close(
            self.activity,
            self.firmware_session_active.load(Ordering::Acquire),
        ) {
            self.activity = Activity::Closing;
        }
    }

    fn ensure_catalog(&mut self, ctx: egui::Context) {
        if self.demo.is_some() || !self.catalog_status.begin_if_needed(self.page) {
            return;
        }
        self.activity = Activity::Busy { can_cancel: false };
        self.status_tone = StatusTone::Info;
        "Checking the signed stable release…".clone_into(&mut self.status);
        self.check_catalog(ctx);
    }

    fn busy(&self) -> bool {
        self.activity != Activity::Idle
    }

    fn operation_available(&self) -> bool {
        operation_available(self.activity, self.worker.is_some())
    }

    fn navigate_to(&mut self, page: AppCenterPage) {
        self.page = page;
        if page == AppCenterPage::Updates && self.catalog.is_none() && !self.busy() {
            self.catalog_status.retry_if_failed();
        }
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
        if reveal_path(path)
            .and_then(|()| open_path("/Applications"))
            .is_err()
        {
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
                            egui::RichText::new("Steam Controller Bridge")
                                .size(28.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            egui::RichText::new("Controller translation, profiles, and updates")
                                .size(15.0)
                                .color(MUTED_TEXT),
                        );
                        ui.add_space(7.0);
                        ui.horizontal_wrapped(|ui| {
                            hero_badge(ui, &format!("App {}", self.installed), ACCENT);
                            hero_badge(ui, &firmware_badge(self.firmware), ACCENT);
                            if let Some(mode) = self.demo {
                                hero_badge(ui, &format!("Demo · {}", mode.label()), MUTED_TEXT);
                            }
                        });
                    });
                });
            });
    }

    fn navigation(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(!self.firmware_write_started, |ui| {
            let mut page = self.page;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut page, AppCenterPage::About, "About");
                ui.selectable_value(&mut page, AppCenterPage::Changelog, "Changelog");
                ui.selectable_value(&mut page, AppCenterPage::Updates, "Updates");
            });
            if page != self.page {
                self.navigate_to(page);
            }
        });
        if self.firmware_write_started {
            ui.label(
                egui::RichText::new("Firmware verification must finish before leaving Updates.")
                    .size(12.0)
                    .color(MUTED_TEXT),
            );
        }
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

impl eframe::App for AppCenter {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.detect_worker_panic();
        self.drain_host_commands();
        let close_requested = ui.ctx().input(|input| input.viewport().close_requested());
        if close_requested
            && must_defer_close(
                self.activity,
                self.firmware_session_active.load(Ordering::Acquire),
            )
        {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_close();
            if self.firmware_session_active.load(Ordering::Acquire) {
                self.page = AppCenterPage::Updates;
                "Firmware verification is still in progress. Keep the board connected."
                    .clone_into(&mut self.status);
            } else {
                "Finishing the current operation before closing…".clone_into(&mut self.status);
            }
            self.status_tone = StatusTone::Info;
        }
        if self.activity == Activity::Closing {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::Frame::new().fill(PANEL).show(ui, |ui| {
            ui.set_min_size(ui.available_size());

            egui::Panel::bottom("app-center-footer")
                .exact_size(36.0)
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(PANEL))
                .show(ui, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new("Copyright © Lynxware · MIT licensed")
                                .size(12.0)
                                .color(MUTED_TEXT),
                        );
                    });
                });

            egui::Panel::top("app-center-hero")
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(PANEL))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    self.hero(ui);
                    hero_transition(ui);
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        self.navigation(ui);
                    });
                    ui.add_space(4.0);
                    ui.separator();
                    self.ensure_catalog(ui.ctx().clone());
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
                            match self.page {
                                AppCenterPage::About => self.about.about_page(ui),
                                AppCenterPage::Changelog => self.about.changelog_page(ui),
                                AppCenterPage::Updates => self.updates_page(ui),
                            }
                            ui.add_space(16.0);
                        });
                });
        });
    }
}

impl Drop for AppCenter {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.join_worker();
    }
}

impl DemoMode {
    fn label(self) -> &'static str {
        match self {
            Self::Available => "update available",
            Self::Current => "up to date",
        }
    }
}

fn demo_manifest(_mode: DemoMode) -> ReleaseManifestV1 {
    let running = running_version();
    let application_version = Version::new(running.major, running.minor + 1, 0);
    let release_tag = format!("v{application_version}");
    ReleaseManifestV1 {
        schema_version: 1,
        release_tag: release_tag.clone(),
        application_version: application_version.clone(),
        minimum_macos: Version::new(13, 0, 0),
        release_notes: format!(
            r"## [{application_version}](https://github.com/tkubicz/steam-controller-bridge/releases/tag/{release_tag})

### Features

* **updater:** preview signed application and XIAO firmware updates
* **menu:** make update status and recovery actions easier to understand

### Bug Fixes

* **firmware:** verify the exact revision after reconnecting
* **menu:** keep release notes readable at every window size"
        ),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_waits_for_owned_work_but_not_the_final_close_command() {
        assert!(must_defer_close(
            Activity::Busy { can_cancel: false },
            false
        ));
        assert!(must_defer_close(Activity::Idle, true));
        assert!(!must_defer_close(Activity::Idle, false));
        assert!(!must_defer_close(Activity::Closing, false));
    }

    #[test]
    fn operations_require_both_an_idle_activity_and_an_empty_worker_slot() {
        assert!(operation_available(Activity::Idle, false));
        assert!(!operation_available(Activity::Idle, true));
        assert!(!operation_available(
            Activity::Busy { can_cancel: false },
            false
        ));
        assert!(!operation_available(Activity::Closing, false));
    }

    #[test]
    fn a_second_worker_cannot_replace_the_owned_handle() {
        let (release, wait) = mpsc::channel();
        let mut slot = None;
        assert!(spawn_worker(&mut slot, move || {
            let _ = wait.recv();
        }));
        let first_thread = slot.as_ref().unwrap().thread().id();
        assert!(!spawn_worker(&mut slot, || panic!("must not run")));
        assert_eq!(slot.as_ref().unwrap().thread().id(), first_thread);
        release.send(()).unwrap();
        slot.take().unwrap().join().unwrap();
    }

    #[test]
    fn flash_and_resume_failures_are_both_reported_with_specific_guidance() {
        let combined = combine_flash_and_resume(
            Err("firmware copy failed".to_owned()),
            Err("runtime stopped".to_owned()),
        )
        .unwrap_err()
        .message();
        assert!(combined.contains("firmware copy failed"));
        assert!(combined.contains("runtime stopped"));
        assert!(combined.contains("Check the cable"));

        let resume_only = combine_flash_and_resume(Ok(()), Err("runtime stopped".to_owned()))
            .unwrap_err()
            .message();
        assert!(resume_only.contains("runtime stopped"));
        assert!(!resume_only.contains("Check the cable"));
    }

    #[test]
    fn demo_release_notes_are_structured() {
        assert!(!parse_release_notes(&demo_manifest(DemoMode::Current).release_notes).is_empty());
    }

    #[test]
    fn hero_firmware_badges_are_explicit() {
        assert_eq!(
            firmware_badge(FirmwareStatus::Reported(3)),
            "Firmware rev 3"
        );
        assert_eq!(
            firmware_badge(FirmwareStatus::UnsupportedFormat(2)),
            "Firmware newer"
        );
        assert_eq!(firmware_badge(FirmwareStatus::Pending), "Checking firmware");
    }

    #[test]
    fn catalog_state_retries_only_after_an_explicit_failed_action() {
        let mut status = CatalogStatus::NotRequested;
        assert!(!status.begin_if_needed(AppCenterPage::About));
        assert!(status.begin_if_needed(AppCenterPage::Updates));
        assert_eq!(status, CatalogStatus::Checking);
        assert!(!status.retry_if_failed());

        status = CatalogStatus::Failed;
        assert!(status.retry_if_failed());
        assert_eq!(status, CatalogStatus::NotRequested);
        assert!(status.begin_if_needed(AppCenterPage::Updates));
    }
}
