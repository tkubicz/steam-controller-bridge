use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use bridge_runtime::{
    new_firmware_install_receipt, FirmwareCapabilities, FirmwareInfo, FirmwareInstallSource,
    FirmwareInstallState, FirmwareTarget, FirmwareTargetId, FirmwareVersion,
};
use eframe::egui;
use release_updater::{
    firmware_target, flash_firmware, guided_replacement_supported, ApplicationRelease,
    ArtifactDescriptor, CatalogRefresh, FirmwareFlashError, FirmwareFlashProgress, FirmwareRelease,
    ReleaseManifestV1, APPLICATION_BUNDLE_ID, FIRMWARE_BOARD_ID, FIRMWARE_TARGET_ID, UF2_FAMILY_ID,
    XIAO_USB_MANUFACTURER, XIAO_USB_PRODUCT, XIAO_USB_PRODUCT_ID, XIAO_USB_VENDOR_ID,
};
use semver::Version;
use ui_theme::{ACCENT, ACCENT_SUBTLE, DANGER, MUTED_TEXT, PANEL, SUCCESS, SURFACE, TEXT};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

use crate::about_pages::AboutContent;
use crate::app_center_protocol::{
    AppCenterCommand, AppCenterPage, FirmwareDetails, FirmwareStatus, FirmwareTargetStatus,
    UpdateOperation,
};
use crate::cli::{AppCenterArgs, DemoMode};
use crate::macos::{open_path, reveal_path};
use crate::update_check::{running_version, update_context, UpdateContext};
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
    Catalog(Box<Result<CatalogRefresh, String>>),
    Application(Result<PathBuf, String>),
    FirmwareProgress(FirmwareFlashProgress),
    Firmware(Result<FirmwareInfo, FirmwareOperationError>),
    Quit(Result<(), String>),
}

impl WorkerEvent {
    fn status_placement(&self) -> StatusPlacement {
        match self {
            Self::FirmwareProgress(_) | Self::Firmware(_) => StatusPlacement::Firmware,
            Self::Catalog(_) | Self::Application(_) | Self::Quit(_) => StatusPlacement::Page,
        }
    }
}

#[derive(Debug)]
enum FirmwareOperationError {
    Preparation(String),
    Flash(FirmwareFlashError),
    Resume {
        firmware: Box<FirmwareInfo>,
        error: String,
    },
    FlashAndResume {
        flash: FirmwareFlashError,
        resume: String,
    },
    Panicked,
    PanickedAndResume(String),
}

impl FirmwareOperationError {
    fn verified_firmware(&self) -> Option<FirmwareInfo> {
        match self {
            Self::Resume { firmware, .. } => Some(**firmware),
            _ => None,
        }
    }

    fn message(self) -> String {
        match self {
            Self::Preparation(error) => error,
            Self::Flash(error) => flash_failure_message(&error),
            Self::Resume { error, .. } => format!(
                "Firmware was written and verified, but bridge recovery failed: {error}"
            ),
            Self::FlashAndResume { flash, resume } => {
                format!(
                    "{} Bridge recovery also failed: {resume} Restart the bridge manually.",
                    flash_failure_message(&flash)
                )
            }
            Self::Panicked => "Firmware installation stopped unexpectedly. The bridge recovery request completed; reopen Updates and retry.".to_owned(),
            Self::PanickedAndResume(resume) => format!(
                "Firmware installation stopped unexpectedly, and bridge recovery also failed: {resume} Restart the bridge manually."
            ),
        }
    }
}

fn flash_failure_message(error: &FirmwareFlashError) -> String {
    let message = error.to_string();
    if matches!(
        error,
        FirmwareFlashError::Io(_)
            | FirmwareFlashError::Discovery(_)
            | FirmwareFlashError::Timeout(_)
            | FirmwareFlashError::Revision { .. }
    ) {
        format!("{message} Check the cable, disconnect extra boards, and retry.")
    } else {
        message
    }
}

fn combine_flash_and_resume(
    flash: Result<FirmwareInfo, FirmwareFlashError>,
    resume: Result<(), String>,
) -> Result<FirmwareInfo, FirmwareOperationError> {
    match (flash, resume) {
        (Ok(firmware), Ok(())) => Ok(firmware),
        (Err(error), Ok(())) => Err(FirmwareOperationError::Flash(error)),
        (Ok(firmware), Err(error)) => Err(FirmwareOperationError::Resume {
            firmware: Box::new(firmware),
            error,
        }),
        (Err(flash), Err(resume)) => Err(FirmwareOperationError::FlashAndResume { flash, resume }),
    }
}

fn panic_and_resume_error(resume: Result<(), String>) -> FirmwareOperationError {
    match resume {
        Ok(()) => FirmwareOperationError::Panicked,
        Err(error) => FirmwareOperationError::PanickedAndResume(error),
    }
}

struct ActiveFirmwareSession(Arc<AtomicBool>);

impl ActiveFirmwareSession {
    fn begin(active: Arc<AtomicBool>) -> Self {
        active.store(true, Ordering::Release);
        Self(active)
    }
}

impl Drop for ActiveFirmwareSession {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct AppCenter {
    page: AppCenterPage,
    about: AboutContent,
    catalog: Option<ReleaseManifestV1>,
    release_notes: Vec<ReleaseNotes>,
    installed: Version,
    firmware: FirmwareDetails,
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
    updates: Option<Result<UpdateContext, String>>,
    status_tone: StatusTone,
    status_placement: StatusPlacement,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusTone {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusPlacement {
    Page,
    Firmware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogStatus {
    NotRequested,
    Checking,
    Loaded,
    Failed,
}

fn demo_firmware_details() -> FirmwareDetails {
    FirmwareInfo {
        target: FirmwareTarget::Reported(FirmwareTargetId::new(FIRMWARE_TARGET_ID).unwrap()),
        version: FirmwareVersion::Reported(3),
        capabilities: FirmwareCapabilities::ENTER_UF2_BOOTLOADER
            | FirmwareCapabilities::INSTALL_RECEIPT,
        install_state: new_firmware_install_receipt(FirmwareInstallSource::AppCenter).map_or(
            FirmwareInstallState::Pending,
            FirmwareInstallState::Recorded,
        ),
    }
    .into()
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

fn catalog_presentation(
    refresh: CatalogRefresh,
    local: bool,
) -> (ReleaseManifestV1, String, StatusTone, CatalogStatus) {
    match refresh {
        CatalogRefresh::Current(manifest) => (
            manifest,
            if local {
                "Signed local development catalog is current.".to_owned()
            } else {
                "Signed release information is current.".to_owned()
            },
            StatusTone::Success,
            CatalogStatus::Loaded,
        ),
        CatalogRefresh::Stale {
            manifest,
            refresh_error,
        } => (
            manifest,
            if local {
                format!(
                    "Cannot refresh the local development catalog; showing its last verified information. {refresh_error}"
                )
            } else {
                format!(
                    "Cannot check for a newer release; showing the last verified information. {refresh_error}"
                )
            },
            StatusTone::Error,
            CatalogStatus::Failed,
        ),
    }
}

impl AppCenter {
    fn new(
        ctx: &egui::Context,
        firmware: FirmwareDetails,
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
            Some(DemoMode::Available) => FirmwareDetails {
                target: FirmwareTargetStatus::Reported(FIRMWARE_TARGET_ID.to_owned()),
                version: FirmwareStatus::Reported(1),
                ..FirmwareDetails::default()
            },
            Some(DemoMode::Current) => demo_firmware_details(),
            None => firmware,
        };
        let updates = demo.is_none().then(update_context);
        let initial_status = if demo.is_some() {
            "Demo preview - updater networking, files, and hardware are disabled.".to_owned()
        } else {
            #[cfg(debug_assertions)]
            if updates
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .and_then(UpdateContext::local_root)
                .is_some()
            {
                "Open Updates to check the signed local development catalog.".to_owned()
            } else {
                "Open Updates to check the signed stable release.".to_owned()
            }
            #[cfg(not(debug_assertions))]
            {
                "Open Updates to check the signed stable release.".to_owned()
            }
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
            status: initial_status,
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
            updates,
            status_tone: StatusTone::Info,
            status_placement: StatusPlacement::Page,
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

    fn update_context(&self) -> Result<UpdateContext, String> {
        self.updates
            .as_ref()
            .expect("real update operations always have an update context")
            .clone()
    }

    #[cfg(debug_assertions)]
    fn local_update_root(&self) -> Option<&Path> {
        self.updates.as_ref()?.as_ref().ok()?.local_root()
    }

    fn uses_local_updates(&self) -> bool {
        self.updates
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .is_some_and(UpdateContext::is_local)
    }

    fn update_source_name(&self) -> &'static str {
        let local = self.uses_local_updates();
        #[cfg(debug_assertions)]
        if local {
            return "local development catalog";
        }
        #[cfg(not(debug_assertions))]
        let _ = local;
        "stable release"
    }

    fn application_current_message(&self) -> String {
        format!(
            "Application matches the signed {}.",
            self.update_source_name()
        )
    }

    fn application_newer_title(&self) -> &'static str {
        let local = self.uses_local_updates();
        #[cfg(debug_assertions)]
        if local {
            return "Newer than local catalog";
        }
        #[cfg(not(debug_assertions))]
        let _ = local;
        "Newer than stable"
    }

    fn application_newer_message(&self) -> String {
        format!(
            "This application is newer than the signed {}. No downgrade is offered.",
            self.update_source_name()
        )
    }

    fn firmware_current_message(&self) -> String {
        format!(
            "The connected board matches the signed {}.",
            self.update_source_name()
        )
    }

    fn firmware_newer_title(&self) -> String {
        format!(
            "Firmware is newer than the signed {}",
            self.update_source_name()
        )
    }

    fn check_catalog(&mut self, ctx: egui::Context) {
        let sender = self.sender.clone();
        self.cancel.store(false, Ordering::Release);
        let cancel = Arc::clone(&self.cancel);
        let updates = self.update_context();
        let started = spawn_worker(&mut self.worker, move || {
            let result = updates.and_then(|context| fetch_catalog(&context, &cancel));
            let _ = sender.send(WorkerEvent::Catalog(Box::new(result)));
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
            self.status_placement = StatusPlacement::Page;
            return;
        }
        self.activity = Activity::Busy { can_cancel: false };
        self.status_tone = StatusTone::Info;
        self.status_placement = StatusPlacement::Page;
        self.cancel.store(false, Ordering::Release);
        "Downloading and validating the application…".clone_into(&mut self.status);
        let sender = self.sender.clone();
        let cancel = Arc::clone(&self.cancel);
        let updates = self.update_context();
        let started = spawn_worker(&mut self.worker, move || {
            let result = updates
                .and_then(|context| download_and_stage_application(&context, &manifest, &cancel));
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
            self.firmware = demo_firmware_details();
            self.firmware.version = FirmwareStatus::Reported(manifest.firmware.revision);
            "Demo: firmware installation completed and verified.".clone_into(&mut self.status);
            self.status_tone = StatusTone::Success;
            self.status_placement = StatusPlacement::Firmware;
            return;
        }
        self.activity = Activity::Busy { can_cancel: true };
        self.status_tone = StatusTone::Info;
        self.status_placement = StatusPlacement::Firmware;
        self.cancel.store(false, Ordering::Release);
        "Preparing the verified firmware…".clone_into(&mut self.status);
        let sender = self.sender.clone();
        let host = self.host.clone();
        let cancel = Arc::clone(&self.cancel);
        let session_active = Arc::clone(&self.firmware_session_active);
        let updates = self.update_context();
        let started = spawn_worker(&mut self.worker, move || {
            let result = (|| -> Result<FirmwareInfo, FirmwareOperationError> {
                let context = updates.map_err(FirmwareOperationError::Preparation)?;
                let path = download_firmware(&context, &manifest, &cancel)
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
                let session = ActiveFirmwareSession::begin(session_active);
                let flash_result = catch_unwind(AssertUnwindSafe(|| {
                    flash_firmware(
                        &path,
                        &manifest.firmware,
                        Path::new("/Volumes"),
                        &cancel,
                        |progress| {
                            let _ = sender.send(WorkerEvent::FirmwareProgress(progress));
                            ctx.request_repaint();
                        },
                    )
                }));
                let resume_result = host.request(UpdateOperation::ResumeBridge);
                drop(session);
                match flash_result {
                    Ok(result) => combine_flash_and_resume(result, resume_result),
                    Err(_) => Err(panic_and_resume_error(resume_result)),
                }
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
            self.status_placement = StatusPlacement::Page;
            return;
        }
        self.activity = Activity::Busy { can_cancel: false };
        self.status_tone = StatusTone::Info;
        self.status_placement = StatusPlacement::Page;
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
            let status_placement = event.status_placement();
            match event {
                WorkerEvent::Catalog(result) => match *result {
                    Ok(refresh) => {
                        let (manifest, status, tone, catalog_status) =
                            catalog_presentation(refresh, self.uses_local_updates());
                        self.status = status;
                        self.release_notes = parse_release_notes(&manifest.release_notes);
                        self.catalog = Some(manifest);
                        self.activity = Activity::Idle;
                        self.status_tone = tone;
                        self.catalog_status = catalog_status;
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
                    if !firmware_progress_is_cancellable(&progress) {
                        self.firmware_write_started = true;
                        self.page = AppCenterPage::Updates;
                    }
                    self.activity = Activity::Busy {
                        can_cancel: firmware_progress_is_cancellable(&progress),
                    };
                    self.status = progress_text(
                        &progress,
                        self.catalog
                            .as_ref()
                            .and_then(|manifest| firmware_target(&manifest.firmware.target)),
                    );
                    self.status_tone = StatusTone::Info;
                }
                WorkerEvent::Firmware(Ok(firmware)) => {
                    "Firmware updated and verified. The bridge has restarted."
                        .clone_into(&mut self.status);
                    self.firmware = firmware.into();
                    self.activity = Activity::Idle;
                    self.status_tone = StatusTone::Success;
                }
                WorkerEvent::Firmware(Err(error)) => {
                    if let Some(firmware) = error.verified_firmware() {
                        self.firmware = firmware.into();
                    }
                    self.status = error.message();
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
            self.status_placement = status_placement;
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
            let panicked = worker.join().is_err();
            // Consuming the only worker is the common terminal path for
            // firmware success, failure, and panic. Navigation must unlock in
            // all three cases.
            self.firmware_write_started = false;
            if panicked {
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
        self.status_placement = StatusPlacement::Page;
        self.status = format!("Checking the signed {}...", self.update_source_name());
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
            self.status_placement = StatusPlacement::Page;
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
            self.status_placement = StatusPlacement::Page;
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
                            hero_badge(ui, &firmware_badge(&self.firmware), ACCENT);
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

    fn status_banner(&self, ui: &mut egui::Ui, placement: StatusPlacement) {
        if self.status_placement == placement {
            let _ = status_banner(ui, &self.status, self.status_tone, self.busy());
        }
    }
}

fn firmware_progress_is_cancellable(progress: &FirmwareFlashProgress) -> bool {
    !matches!(
        progress,
        FirmwareFlashProgress::Writing
            | FirmwareFlashProgress::WaitingForApplication
            | FirmwareFlashProgress::RecordingReceipt
            | FirmwareFlashProgress::VerifyingReceipt
    )
}

fn status_banner(ui: &mut egui::Ui, status: &str, tone: StatusTone, busy: bool) -> egui::Response {
    let (colour, fill) = match tone {
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
                if busy {
                    ui.spinner();
                } else {
                    ui.label(egui::RichText::new("●").size(11.0).color(colour));
                }
                ui.add(egui::Label::new(egui::RichText::new(status).color(TEXT)).wrap());
            });
        })
        .response
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
                self.status_placement = StatusPlacement::Firmware;
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
            revision: 3,
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

    fn verified_firmware_fixture() -> FirmwareInfo {
        FirmwareInfo {
            target: bridge_runtime::FirmwareTarget::Reported(
                bridge_runtime::FirmwareTargetId::new(FIRMWARE_TARGET_ID).unwrap(),
            ),
            version: bridge_runtime::FirmwareVersion::Reported(2),
            capabilities: FirmwareCapabilities::ENTER_UF2_BOOTLOADER
                | FirmwareCapabilities::INSTALL_RECEIPT,
            install_state: bridge_runtime::FirmwareInstallState::Recorded(
                bridge_runtime::FirmwareInstallReceipt {
                    installed_at: 1_786_456_920,
                    install_id: [0x42; 16],
                    source: bridge_runtime::FirmwareInstallSource::AppCenter,
                },
            ),
        }
    }

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
    fn worker_status_stays_with_the_component_that_started_it() {
        assert_eq!(
            WorkerEvent::FirmwareProgress(FirmwareFlashProgress::Writing).status_placement(),
            StatusPlacement::Firmware
        );
        assert_eq!(
            WorkerEvent::Application(Err("fixture".to_owned())).status_placement(),
            StatusPlacement::Page
        );
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
            Err(FirmwareFlashError::Discovery(
                "firmware copy failed".to_owned(),
            )),
            Err("runtime stopped".to_owned()),
        )
        .unwrap_err()
        .message();
        assert!(combined.contains("firmware copy failed"));
        assert!(combined.contains("runtime stopped"));
        assert!(combined.contains("Check the cable"));

        let verified = verified_firmware_fixture();
        assert_eq!(
            combine_flash_and_resume(Ok(verified), Ok(())).unwrap(),
            verified
        );
        let resume_error =
            combine_flash_and_resume(Ok(verified), Err("runtime stopped".to_owned())).unwrap_err();
        assert_eq!(resume_error.verified_firmware(), Some(verified));
        let resume_only = resume_error.message();
        assert!(resume_only.contains("runtime stopped"));
        assert!(!resume_only.contains("Check the cable"));

        let cancelled = combine_flash_and_resume(Err(FirmwareFlashError::Cancelled), Ok(()))
            .unwrap_err()
            .message();
        assert!(cancelled.contains("cancelled"));
        assert!(!cancelled.contains("Check the cable"));

        let cancelled_and_resume = combine_flash_and_resume(
            Err(FirmwareFlashError::Cancelled),
            Err("runtime stopped".to_owned()),
        )
        .unwrap_err()
        .message();
        assert!(cancelled_and_resume.contains("cancelled"));
        assert!(cancelled_and_resume.contains("runtime stopped"));
        assert!(!cancelled_and_resume.contains("Check the cable"));
    }

    #[test]
    fn a_panicking_flash_still_clears_the_session_and_reports_resume_failure() {
        let active = Arc::new(AtomicBool::new(false));
        let session = ActiveFirmwareSession::begin(Arc::clone(&active));
        let flash = catch_unwind(AssertUnwindSafe(|| panic!("fixture panic")));
        let error = match flash {
            Ok(()) => unreachable!(),
            Err(_) => panic_and_resume_error(Err("runtime stopped".to_owned())),
        };
        drop(session);

        assert!(!active.load(Ordering::Acquire));
        let message = error.message();
        assert!(message.contains("stopped unexpectedly"));
        assert!(message.contains("runtime stopped"));
    }

    #[test]
    fn long_status_errors_wrap_without_widening_the_window() {
        let context = egui::Context::default();
        let mut banner = egui::Rect::NOTHING;
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                ..egui::RawInput::default()
            },
            |ui| {
                banner = status_banner(
                    ui,
                    "A long updater failure must wrap inside the available content width instead of widening the page and clipping every card that follows it.",
                    StatusTone::Error,
                    false,
                )
                .rect;
            },
        );
        output.textures_delta.clear();

        assert!(
            banner.width() <= 640.0,
            "banner escaped the window: {banner:?}"
        );
        assert!(banner.height() > 40.0, "banner did not wrap: {banner:?}");
    }

    #[test]
    fn firmware_progress_cancellation_stops_before_the_write_phase() {
        for progress in [
            FirmwareFlashProgress::LookingForDevice,
            FirmwareFlashProgress::RequestingBootloader,
            FirmwareFlashProgress::WaitingForBootloader,
            FirmwareFlashProgress::ManualRecovery,
        ] {
            assert!(firmware_progress_is_cancellable(&progress));
        }
        for progress in [
            FirmwareFlashProgress::Writing,
            FirmwareFlashProgress::WaitingForApplication,
            FirmwareFlashProgress::RecordingReceipt,
            FirmwareFlashProgress::VerifyingReceipt,
        ] {
            assert!(!firmware_progress_is_cancellable(&progress));
        }
    }

    #[test]
    fn manual_recovery_points_to_the_physical_reset_button() {
        let guidance = progress_text(
            &FirmwareFlashProgress::ManualRecovery,
            firmware_target(FIRMWARE_TARGET_ID),
        );
        assert!(guidance.contains("reset button"));
        assert!(guidance.contains("USB-C"));
        assert!(!guidance.contains("RST"));
        assert!(!guidance.contains("GND"));
    }

    #[test]
    fn demo_release_notes_are_structured() {
        assert!(!parse_release_notes(&demo_manifest(DemoMode::Current).release_notes).is_empty());
    }

    #[test]
    fn hero_firmware_badges_are_explicit() {
        let supported = demo_firmware_details();
        assert_eq!(firmware_badge(&supported), "Firmware rev 3");
        let mut unsupported_format = supported.clone();
        unsupported_format.version = FirmwareStatus::UnsupportedFormat(2);
        assert_eq!(firmware_badge(&unsupported_format), "Firmware newer");
        let mut pending = supported;
        pending.version = FirmwareStatus::Pending;
        assert_eq!(firmware_badge(&pending), "Checking firmware");
        // A live check and a newer report never carry a parsed target, so the
        // version state must decide these badges on its own.
        assert_eq!(
            firmware_badge(&FirmwareDetails::default()),
            "Checking firmware"
        );
        assert_eq!(
            firmware_badge(&FirmwareDetails {
                version: FirmwareStatus::UnsupportedFormat(2),
                ..FirmwareDetails::default()
            }),
            "Firmware newer"
        );
        assert_eq!(
            firmware_badge(&FirmwareDetails {
                target: FirmwareTargetStatus::Reported("another-device".to_owned()),
                version: FirmwareStatus::Reported(3),
                ..FirmwareDetails::default()
            }),
            "Firmware unidentified"
        );
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

    #[test]
    fn stale_catalog_information_is_usable_but_warns_and_allows_retry() {
        let manifest = demo_manifest(DemoMode::Current);
        let (presented, message, tone, mut status) = catalog_presentation(
            CatalogRefresh::Stale {
                manifest: manifest.clone(),
                refresh_error: "offline".to_owned(),
            },
            false,
        );
        assert_eq!(presented, manifest);
        assert!(message.contains("last verified"));
        assert!(message.contains("offline"));
        assert_eq!(tone, StatusTone::Error);
        assert_eq!(status, CatalogStatus::Failed);
        assert!(status.retry_if_failed());

        let (_, local_message, _, _) = catalog_presentation(
            CatalogRefresh::Stale {
                manifest,
                refresh_error: "offline".to_owned(),
            },
            true,
        );
        assert!(local_message.contains("local development catalog"));
        assert!(local_message.contains("last verified"));
    }
}
